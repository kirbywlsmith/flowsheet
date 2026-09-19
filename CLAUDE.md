# CLAUDE.md

## Project

Open-source process simulation engine in Rust. Solves steady-state material balances over a flowsheet — a directed graph
of units (mixers, splitters, tanks) connected by streams. Aim to eventually expand to also support dynamic behaviour in
future.

## Goals

The primary goals are:

- Learn Rust
- Learn low-level systems concepts

The user has no prior exposure to Rust, so explain idioms and borrow-checker reasoning. The user has some minimal
experience with C and C++. The user is most experienced with C# / .NET, so C# comparisons would be appreciated.

Some secondary goals include:

- Learn process simulation domain concepts (the user is not a process or chemical engineer)
- Create something useful

Speed of delivery is explicitly not a goal.

[TODO.md](TODO.md) is the roadmap — a flat, sequential checklist. Remove items as they are completed.

## How to help

- **User writes the code.** Explain, review, and sketch small snippets. Don't implement features unless asked.
- Prefer one clear recommendation over a survey of options.
- Small edits (removing finished TODO items, config files) are fine to just do.
- Remember to update this CLAUDE.md file when it makes sense to do so.

## Review tagging

Tag every review point so it can be triaged at a glance:

- **[bug]** — actually broken, fix it
- **[style]** — works, but not idiomatic; optional
- **[learn]** — nothing to change, context worth knowing

Never mix severities in one unlabelled list.

## Design decisions already made

- Flows are **absolute mass per species (t/h)**, never fractions — one source of truth, mixing is a vector add.
- Temperature in **Kelvin** (energy balance needs absolute), pressure in **kPa**.
- `SpeciesId` / `UnitId` / `StreamId` are new types with **private fields**, so an invalid id can't be constructed.
- Indexing (`registry[id]`, `stream[id]`) **panics** — a bad id is a bug in our code, not user input.
  `Result` is reserved for real user input (JSON loading).
- The **id space is guarded on the builder side too**, by `assert_id_space` in `lib.rs`. All three ids are `u16`
  newtypes built by casting a `Vec` index, so `add_unit`, `add_stream` and `SpeciesRegistry::insert` each wrapped the
  65,537th entry's id round to 0 and aliased the first one. `serial` already rejected an oversized *document*; this is
  the same mistake made through the builder. `assert!` and not `debug_assert!`, because release is exactly where
  someone generates 70,000 units, and a panic rather than a `Result` for the reason on the line above.
  `MAX_IDS` and the helper live at the **crate root rather than in `flowsheet.rs`**, because `species` sits below
  `flowsheet` in the layering — `flowsheet` uses `species`, so putting the constant in `flowsheet` and importing it
  back down would invert that. One helper, so the three panic messages match `LoadError::TooMany` word for word and
  cannot drift. `insert`'s check sits in its `None` arm: a duplicate hands back an existing id and allocates none, so
  a full registry can still answer for what it already holds.
- Testing that guard is cheap only because of what the fixtures avoid: `add_unit("", Mixer)` allocates nothing
  (`String::new` does not, and neither does a `Box` of a ZST), and an empty `SpeciesRegistry` makes `Stream::zeros`
  a zero-length vector, so filling 65,536 slots is one vector growth. The species test pushes **straight into the
  private vector** instead of calling `insert`, because `insert` runs the linear-scan `find` first and filling it
  through the public path would be O(n²) — about two billion comparisons. All three run in 0.02s together.
- Species registry uses a **linear scan**, not a HashMap. It holds 3–20 entries; two collections would desync.
- Unit ops were an **enum first, `Box<dyn UnitOp>` now** — the comparison was the point. The enum version is in
  git history at `1612492` if a bench wants the baseline back.
- `UnitOp` is a **public trait**, and it had to be: `Flowsheet::add_unit` is public and cannot name a private type in
  its signature, so `inlet_arity` / `outlet_arity` / `evaluate` and `Arity` all stopped being `pub(crate)`. That is the
  real price of the change, and also the payoff — the set of ops is now open to downstream crates.
- `add_unit` takes **`impl Into<Box<dyn UnitOp>>`**, not `Box<dyn UnitOp>`, so call sites stay `add_unit("mixer",
  Mixer)` and the box is an implementation detail. `impl<T: UnitOp + 'static> From<T> for Box<dyn UnitOp>` supplies one
  direction; the reflexive `impl From<T> for T` lets an already-boxed op (what `serial` builds when loading) through
  unchanged. Same shape as std's `impl<E: Error> From<E> for Box<dyn Error>`.
- Saving is a **`serial::ToDocument` supertrait on `UnitOp`**, not a `match` in `serial.rs`, because
  `Box<dyn UnitOp>` has erased the concrete type and only the op still knows what it is. The alternative, downcasting
  through `Any`, would put a closed list of types back in `serial.rs` and give up exactly the open set the trait object
  was adopted for. The `impl` blocks still live in `serial.rs`, so the wire format remains one module; only the trait's
  declaration leaks into `unit.rs`. It returns a **`&'static str` tag plus a `serde_json::Value`**, not an enum
  variant — one per type, and the same string the constructor is registered under, so the two sides cannot drift.
- Loading is a **`serial::OpRegistry`**, a `BTreeMap<&'static str, Box<dyn Fn(Spec) -> Result<Box<dyn UnitOp>>>>`.
  A document names its op with a string and something must own the name-to-constructor table; making that something a
  *value* rather than a `match` is what makes the wire format as open as the solver. `OpRegistry::builtin()` holds the
  twelve shipped ops, `register` adds or replaces one, and `serial::Flowsheet::into_domain(&ops)` is the real entry
  point — `TryFrom` stays, delegating to `builtin()`. Two costs, both paid deliberately: `serde_json` stops being a
  dev-dependency of the library (`Value` is now in its public API), and `deny_unknown_fields` no longer fires during
  parsing. It still fires, one step later, when `Spec::parse` deserialises into the op's own spec struct — so a stray
  `fraction` on a `mixer` is a `LoadError::BadOp` naming the field, not a `serde_json` parse error. `tests/downstream_op.rs`
  is the proof: a `Bleed` op defined in an integration test (a genuinely separate crate) solves, saves under its own
  tag, and loads back with its parameter intact — and `OpRegistry::builtin()` rejects it by name instead of silently
  handing back a `Tank`, which was the only outcome available before.
- `serial::State`'s fields are declared **alphabetically** (`flows`, `pressure`, `temperature`). A stream's state
  serialises straight from the struct, in declaration order; a feed's state is nested inside a spec that
  `ToDocument::spec` turns into a `Value` first, and that pass rewrites every struct as a map, which sorts. Declaring
  them in the order the map would produce anyway is what keeps the two paths writing the same bytes. The alternative
  was `serde_json`'s `preserve_order` feature — an `indexmap` dependency, workspace-wide, to preserve a cosmetic
  ordering.
- `UnitOp` requires **`Send + Sync`**. The enum had them automatically; a bare `Box<dyn UnitOp>` has neither, and
  without them `Flowsheet` and `ValidFlowsheet` stop being `Sync` — which would quietly cost the parallelism the
  wave-emitting topological sort exists to enable. The price is that an op may not hold an `Rc` or a `Cell`.
- `Arity`'s **fields stay `pub(crate)`** even though the type had to go public. `exactly` / `at_least` / `permits`
  are the whole public surface, so a downstream op cannot hand back an unsatisfiable `Arity { min: 2, max: Some(1) }`.
- `Box<dyn UnitOp>` is **not `Clone`**, and nothing needed it to be, so the old `#[derive(Clone)]` on the enum was
  dropped rather than replaced with a `clone_box` method or the `dyn-clone` crate. The concrete op structs still derive
  it. `Debug` is a supertrait because `Flowsheet` derives `Debug` and a derive cannot reach through the box otherwise.
- The **flotation cell is the first op that changes composition**. `Flotation { recovery: Vec<f64> }` recovers each
  species to the concentrate at its own rate, which is what a `Splitter` structurally cannot do - a splitter sends the
  same composition to both outlets, so no arrangement of splitters concentrates anything. Outlets are positional:
  concentrate first, tails second.
- `recovery` is a **dense `Vec<f64>` in `SpeciesId` order**, the same shape as `Stream::flows`, and the validation
  lives in the `unit::recover` free function rather than in a constructor - matching `split`/`Splitter`, and for the
  same reason: the field is public and the species count is not known until an inlet arrives.
- On the wire, `recovery` is a **species-name map**, like flows, so a document does not depend on species order.
  Two asymmetries with flows, both deliberate: an omitted species recovers **zero** (it leaves in the tails), and a
  zero recovery is **not dropped on save** - a zero flow says nothing, but a zero recovery is a statement that the
  species does not float, and it belongs on the page next to the ones that do.
- `demo::balance` is the **circuit's scalar solution worked out by hand**: each species travels the loop
  independently, so `M = F / (1 - f(1 - r))` is the whole answer. The balance tests assert against that formula
  rather than against pasted numbers, so changing `f` or the recovery vector does not invalidate them. Convergence
  now varies by species too - the rate is `f(1 - r)`, and the slowest species sets it.
- The **conversion reactor is the first op that transforms species**. `ConversionReactor { reactions: Vec<Reaction>,
  energy }`, tag `conversion_reactor`, one inlet and one outlet. `Reaction { stoichiometry, limiting, conversion }` is
  plain data defined in `unit.rs` beside `unit::react`, not in the op's file, so `unit.rs` never imports from its own
  child. Coefficients are **molar and signed**; extent is
  `conversion * n_limiting / |nu_limiting|` in Mmol/h, and t/h over g/mol needs no factor. The limiting species is
  written `in * (1 - conversion)`, not through the extent, which does not cancel exactly and lands a ULP either side
  of zero at conversion 1.
- Several reactions run **in declared order**, Aspen `RStoic`'s series mode: `evaluate` folds `react` over the list,
  so each conversion is a fraction of the limiting reactant *at that point* and two reactions competing for one
  reactant cannot over-consume it. That also made "no species goes negative" free - `react` already checks it - and
  the only addition is a `reaction {i}: ` prefix on the error. An adiabatic reactor solves temperature **once, after
  the last reaction**, because enthalpy is a state function; `tests/several_reactions.rs` proves it against two
  single-reaction reactors in series. Simultaneous mode (every conversion against the inlet) was left out on purpose:
  it can over-consume a shared reactant and needs its own validation. An empty list panics in `evaluate` and is a
  `LoadError::BadValue` of 0 at the boundary.
- The reactor's energy is a **required `energy: ReactorEnergy` spec, `isothermal` or `adiabatic`**, with no
  heat-of-reaction parameter anywhere: with absolute enthalpies, `H_out - H_in` *is* the heat of reaction. Isothermal
  keeps the inlet temperature; adiabatic runs `react` and then hands `H_in` to `unit::solve_temperature`, with the
  same empty-inlet guard as `heat`. Required rather than defaulting to isothermal, because a silent default is how a
  combustion chamber ends up at 25 °C. `ReactorEnergy` derives `serde` directly (plain data, like `Phase`), written
  `snake_case` to match the op tags. `tests/reactor_energy.rs` holds the two proofs: an n-butane -> isobutane ->
  n-butane cycle over temperature-dependent Shomate fits returns to its feed temperature, and adiabatic combustion
  lands on a temperature solved by hand in molar units - where Mmol/h times kJ/mol is **GJ/h**, a factor of 1000 the
  library never sees because it works per kg.
- Every **reaction participant must carry an `enthalpy_of_formation`**, or a missing value would read as zero and
  invent a heat of reaction. `unit::formation_enthalpies` returns the first offender and is shared, like
  `mass_closure`, by `react`'s panic and `LoadError::MissingFormationEnthalpy` - a variant of its own, because
  `BadValue` needs an `f64` and here there is no value. A species with a zero coefficient is not a participant.
- **`report::duty(fs, unit)`** is a unit's outlet enthalpy flow minus its inlet's, computed from the streams rather
  than stored by the op, so `UnitOp` did not change and it works for every unit: a heater's duty comes back, an
  isothermal reactor's heat of reaction appears, and mixers, splitters and adiabatic reactors give zero to rounding.
- `report::table` prints **unit duties after the footer**, a blank line and then `unit`, `type`, `duty (MJ/h)` for
  every unit with inlets and outlets whose duty is not noise. After the footer rather than between the rows and it, so
  every existing layout test - which pins the rows and then the footer by line number - held unchanged. Feeds and
  products are skipped because `duty` on them is a stream's enthalpy, not heat. A plain mixing circuit prints no
  section at all, not even a header, so its output is byte-identical to before. **Noise** is `|duty| <= 1e-6 *
  sum(|H| + C*T)` over the unit's ports: relative because a mixer on a recycle sees its tear inlet move once more
  after it ran, by up to the solver tolerance, and `C*T` as well as `|H|` because a stream at 25 °C with no formation
  enthalpy carries no enthalpy at all - measured, the Wegstein stiff recycle printed its mixer at `0.0` without it.
  `write_row` takes the set of left-aligned columns now, since this table has two text columns, and the duty is
  computed through a private `unit_duty(fs, &Unit)` because `UnitId`'s field is private and `table` walks units.
  `burner.json` pins a heater and an isothermal reactor, and checks the reactor's duty against Kirchhoff's law by hand.
- Mass closure is **two layers**. First, `unit::mass_closure` rejects `|sum(nu_i M_i)| / sum(|nu_i|) > 0.01` g/mol -
  divided by `sum(|nu_i|)` because rounding error is bounded by the rounding step times that sum and does not grow
  with molar mass. A tolerance relative to `sum(|nu_i| M_i)` accepted triolein + 2.5 H2 -> tristearin, off by a whole
  hydrogen molecule, because the molecules are heavy. The helper does the comparison and returns `Result<(), f64>`,
  so `react`'s panic and the loader's `LoadError::BadValue` share one tolerance; it is written `residual <= TOL`
  because a `NaN` coefficient or an all-zero stoichiometry makes the residual `NaN`. Second, `react` scales every
  product's mass change by `k = reactant mass / product mass`, computed from **coefficients only** - computing it from
  the extent is `0 / 0` whenever the extent is zero. So a rounded equation still conserves total mass exactly, and
  "every op conserves mass" stays true on totals. The 0.01 assumes two-decimal masses; the demo's one-decimal
  CuFeS2 passes in practice because the error is averaged, not by guarantee.
- A reactant driven negative is an **`EvalError`**, after clamping round-off: an outlet in
  `[-1e-12 * in[i], 0)` becomes 0.0. The window is scaled by **that species' own inlet flow**, not the stream total,
  so a reactant absent from the inlet has no window and consuming any of it is an error rather than a silent clamp
  that creates mass. `tests/conversion_reactor.rs` runs the reactor in a recycle under Wegstein too, because Wegstein
  extrapolates tear flows with no floor and the extent couples the species - the transient this error could turn
  into a failed solve. It does not on that loop.
- `UnitOp::conserves_species()` is a **default trait method returning `true`**; the reactor overrides it. If any unit
  returns `false`, `report::imbalance` compares total mass instead of per species. A default method is not a breaking
  change (a C# 8 default interface method), so `tests/downstream_op.rs`'s `Bleed` compiled unchanged. **Cost,
  accepted:** the switch is flowsheet-wide, so one reactor hides a species mis-wiring anywhere else in the plant.
  Without every reaction's extent stored somewhere, total mass is the only balance a reacting flowsheet can check.
- On the wire the reactions are **a nested array**: `{ "type": "conversion_reactor", "energy": "adiabatic",
  "reactions": [{ "stoichiometry": {..}, "limiting": "CH4", "conversion": 0.9 }] }`. The stoichiometry is a
  species-name map like `recovery`, but **zero coefficients are dropped on save** - an equation lists its
  participants, whereas a zero recovery is a statement. The singular `reaction` of the first version was replaced
  outright, not kept as an alias, so there is one way to write a reactor; a test pins that the old key is rejected.
  Load errors name the reaction in the field, `reactions[1].conversion`, which `BadValue`'s `String` field already
  allowed.
- **The energy balance solves temperature and nothing else.** Pressure is declared and propagated, not solved (the
  entry below), and there is no phase change.
- **Pressure is a per-op `pressure_drop`, kPa, never negative**, on `Mixer`, `Splitter`, `SplitterN`, `Flotation`,
  `Heater` and `ConversionReactor`; `Feed` sets a pressure outright, `Product` has no outlet, and `Tank` is a
  placeholder. Each op takes its drop off every outlet through `unit::drop_pressure`, and `mix` starts from the
  **lowest pressure among the inlets that carry flow** - lowest because the pipes meet at one pressure and the others
  would flow backwards at anything higher; flowing only because a recycle's first pass hands the mixer an empty
  placeholder at whatever pressure it was built with, and letting that set the outlet held the loop at 101.325 kPa
  forever in the first version. `drop_pressure` likewise leaves an empty outlet alone, the same guard as `heat`
  ignoring its duty. A drop that would take a flowing outlet to 0 kPa or below is an `EvalError` (the `EvalError`
  entry: user input the numerics cannot answer), a negative drop is a panic with `LoadError::BadValue` at the
  boundary, and a stream pressure must now be **strictly positive** on load, because the residual divides by it.
  On the wire `pressure_drop` is **optional, default 0, and left off on save when 0** (`serial::is_zero`, the same
  rule as Shomate's zero terms), so every existing document saves byte for byte; `Mixer` gained a `MixerSpec` and
  stopped being a unit struct, which is why every call site reads `Mixer::default()`. **A recycle that loses
  pressure has no steady state**: the recycle re-enters the mixer below the feed, the mixer takes the recycle's
  pressure, and the loop falls by its total drop every pass. So the solver's convergence check includes
  `Stream::pressure_residual` - without it the flows settle in ~17 passes and the solve reports success at whatever
  pressure the loop had reached - and `tests/pressure.rs` pins that such a loop fails on the pass the drop reaches
  0 kPa (pass 50 for 500 kPa in and 10 off), naming the mixer, rather than converging. A pump in the loop is what
  fixes that (the entries below). Wegstein still copies pressure from the result rather than extrapolating
  it: on a converging loop pressure settles in one pass, and on a diverging one there is nothing to extrapolate
  towards. The report table has no pressure column yet; adding one moves every layout test.
- **`Pump` and `Compressor` are sized by a `pressure_rise` in kPa**, never an outlet pressure or a ratio, though
  Aspen offers all three. One way to write it, the mirror of `pressure_drop`, and it keeps the loop honest: a pump
  that puts back less than the loop loses still has no steady state and fails on the pass the shortfall reaches
  0 kPa (`tests/pumped_recycle.rs` pins pass 99 for half of a 10 kPa drop, against pass 50 for the bare loop),
  whereas a discharge-pressure spec would silently hold any loop at its set point. A pump that puts back *more*
  is harmless - the recycle comes round above the feed and the mixer takes the lower of its inlets. `efficiency`
  is optional, default 1, and left off on save when 1 (`serial::is_one`, the same rule as a zero drop), so the
  common `{ "type": "pump", "pressure_rise": 20 }` reads as it should. The two ops share one `PumpSpec` because
  they read the same two fields the same way; the tag alone tells them apart. Both pass an empty inlet through
  untouched, pressure included, like `heat` and `drop_pressure`: a pump on the recycle branch sits downstream of
  the tear and sees nothing on pass 1.
- A pump needs a volume, so **`Species` gained `density: Option<f64>`**, kg/m³, held constant, optional on the wire
  and `None` for anything nothing pumps - the same allowance as `enthalpy_of_formation`, and for the same reason: a
  flotation plant with no pump should not have to look up chalcopyrite's. The difference is where a missing one
  is caught. A reaction names its participants in the document, so the loader can demand their formation
  enthalpies; which species reach a pump is a property of the solved flows, not of the document, so
  `unit::pump_work` reports a flowing species with no density as an **`EvalError` naming it**, and a species with
  no density that never flows through the pump is fine. The loader only checks that a density that *is* given is
  positive. The phase checks sit on the same side of the line for the same reason: a pump rejects a flowing gas
  and a compressor a flowing solid or liquid, both `EvalError`s that say which machine the species wanted.
- **Only the friction reaches the stream from a pump.** The library's enthalpy is `h(T)` with no pressure term, so
  of the shaft work `W = V·ΔP/η` the `V·ΔP` that became pressure has nowhere to land and `unit::pump` hands
  `W(1 - η)` to `heat`. That is the right temperature - a perfectly efficient pump warms nothing - and it costs
  the accounting: `report::duty` on a pump is its loss, not its power, and the duty table says so (the
  `pumped_recycle.json` fixture prints 20.1 MJ/h for a pump whose shaft takes 40.1). `unit::pump_work` is the
  power. Two alternatives were rejected. Putting all of `W` into temperature closes the books but has an ideal
  pump heating its fluid, which is what efficiency is supposed to rule out. Adding `P/ρ` to `Species::enthalpy`
  is physically right for a liquid but means every `drop_pressure` changes enthalpy at constant temperature, so
  every op that drops pressure would have to re-solve its temperature to hold `h`, and a heater's duty would stop
  coming back as its duty. The compressor has no such gap: an ideal gas's enthalpy depends on temperature alone,
  so its whole shaft work is enthalpy and its duty row *is* its power.
- **There is no `enthalpy_of_vaporisation` field.** With both phases carrying an `enthalpy_of_formation`, the gap
  between them at 298.15 K *is* the latent heat there - liquid water is -285.83 kJ/mol and steam -241.83, 44.0
  apart - and each phase's Shomate fit carries it to any other temperature. A stored value would be a second,
  rounded source of the same number, the argument that keeps Shomate's `F` off the wire. `species::latent_heat`
  is that difference per mole; it lives in `species.rs`, not `thermo.rs`, because it takes two `Species` and
  `thermo` sits below `species` in the layering. It panics on a phase without a formation enthalpy, the reactor's
  rule for the same reason - a missing value reads as zero and invents a latent heat. Tested against the steam
  table at 373.15 K: 40.86 kJ/mol from the fits against 40.65, and the 0.5% is steam's cp held at its 25 °C value
  across 75 K because NIST's gas fit starts at 500 K, so the tolerance is 1% and the test says why.
- **Vapour pressure is an Antoine fit on `Species`, in kPa and K.** `thermo::Antoine { a, b, c }` with
  `vapour_pressure(T)` and its closed-form inverse `boiling_point(P)`, no iteration. Written in the crate's units
  on the wire rather than converted in code, so that one system of units holds everywhere; the price is that
  NIST's `A`, published in bar, needs `+2` on the way in, and the README says so in one line. The field is
  optional and `None` for anything nothing evaporates, the same allowance as `density`, and it belongs on the
  **liquid** entry of a name declared in two phases - vapour pressure is what the condensed phase exerts, and the
  flash will pair `H2O(l)` with `H2O(g)` by name and read it from that side. The loader accepts it on any phase
  (a solid has a sublimation pressure) and checks only that `b` is positive, the one coefficient whose sign the
  physics fixes: negative would have vapour pressure fall with temperature. Nothing checks the fitted range, as
  with Shomate. `demo::WATER_VAPOUR_PRESSURE` is NIST's 344-373 K fit, kept beside the Shomate constants though
  the circuit never uses it, because it is what the steam-table tests are written against.
- **The two fits are checked against each other through Clausius-Clapeyron**: `R T² d(ln Psat)/dT` from Antoine
  against `latent_heat` from the enthalpies, at 373.15 K, within 2%. Antoine's slope gives 41.3 kJ/mol and the
  enthalpies 40.9; the 1% between them is the ideal-gas vapour and negligible liquid volume the law assumes. Two
  independent property sets and one law between them, so a fit with its `A` still in bar or a formation enthalpy
  off by a phase lands far outside the tolerance. It is the "measured against the absolute basis" the TODO item
  asked for, and the `GAS_CONSTANT` the compressor introduced is what makes it a one-line test.
- **The isothermal flash is `Flash { temperature, pressure }`**, tag `flash`, one inlet and two outlets, **vapour
  first**. Both specs are required and set outright, like a feed's state - there is no `pressure_drop`, because a
  drum is specified by the pressure it runs at and that pressure decides the split. The work is two free functions
  in `unit.rs`, as for every op: `rachford_rice(amounts, k) -> f64` and `flash(registry, inlet, T, P)`.
- **A feed that does not split is `Ok`, not an `EvalError`.** `rachford_rice` checks the bubble point
  (`sum n K <= sum n`, all liquid, exactly `0.0`) and the dew point (`sum n / K <= sum n`, all vapour, exactly
  `1.0`) before it looks for a root, so a subcooled or superheated feed returns one full outlet and one empty one,
  as Aspen's `Flash2` does. Between the two it is Newton from 0.5 held inside a bracket, bisecting whenever a step
  would leave it; the function falls monotonically, so there is one root. Its tolerance is **relative to
  `min(V, 1 - V)`**, 1e-12, because each outlet's flows scale with its own share - an absolute 1e-12 gets a drum
  that barely boils (V = 5e-9, tested) right to three digits. It takes amounts on any scale, not mole fractions,
  since the equation is linear in them.
- **Species are paired by name**: a name declared as both a `Liquid` and a `Gas` is one substance, its two inlet
  flows pooled and split by `K = Psat(T) / P` (Raoult) read from the liquid entry's Antoine fit. Everything else goes
  where its phase belongs, and the three cases are deliberately different. A gas with no liquid entry never
  condenses: `K = inf`, whose Rachford-Rice term is the limit `n / V`, so it takes part - it is what the steam's
  partial pressure shares the vapour with. A liquid with no gas entry and no Antoine never evaporates: `K = 0`,
  and it takes part too, diluting the water the way Raoult says. A **solid takes no part**: it leaves in the liquid
  as bottoms but is its own phase and dissolves in nothing, so it dilutes nothing - a test pins the vapour
  bit-for-bit equal with and without sand. The `0 * inf` that an absent inert would make in the bubble check is
  why both checks skip zero amounts.
- Two flowing cases are **`EvalError`s**: a two-phase pair whose liquid has no `vapour_pressure` (its split is
  unknown), and a liquid with a `vapour_pressure` but no gas entry (its vapour has nowhere to go - which is every
  shipped organic, since only water ships in both phases). Errors rather than load checks for the pump's reason:
  which species reach a flash is a property of the solved flows. An absent one is fine either way.
- **Mass is split, not moles.** Each substance's pooled mass goes to the two outlets by its molar vapour share
  `V K / (1 + V (K - 1))`, clamped to `[0, 1]` against a ULP; the moles Rachford-Rice sees are that mass over the
  liquid entry's molar mass. So mass is conserved exactly even if a document gives the two phases different molar
  masses. The flash **returns `false` from `conserves_species`** - `H2O(l)` becoming `H2O(g)` is two species ids -
  which puts the whole flowsheet on the total-mass check, the reactor's cost again.
- No temperature is solved, so the drum's heat is whatever `report::duty` finds, and `tests/flash.rs` pins it at
  exactly the evaporated moles times `species::latent_heat`. Humid nitrogen has a closed form - the liquid is pure
  water, so the vapour carries `n_N2 Psat / (P - Psat)` of steam - and a liquid recycle round the drum leaves the
  products exactly there at any fraction. That loop takes 18 direct-substitution passes at 0.3 and 176 at 0.9.
- **Cost, accepted: a flash hides a pressure-losing loop.** Setting its outlet pressure outright means a recycle
  through a drum settles whatever the pipes upstream drop, the way a discharge-pressure pump spec would have. It is
  what the equipment does - a drum runs at its pressure - but `tests/pressure.rs`'s "a lossy loop fails" no longer
  holds for a loop with a drum in it.
- **The shipped species library is `crates/flowsheet/data/species.json`**, compiled in with `include_str!` and parsed
  once behind a `LazyLock` in `library.rs` (an embedded resource and a `Lazy<T>`). A bad file is a panic, not a
  `Result`: it is part of the crate, and the tests parse it on every run. The file's record is its own struct with a
  `source` field rather than `Species` with `source` flattened beside it, because `flatten` and `deny_unknown_fields`
  cannot share a struct - the wall `serial::Op` hit - and a typo in the shipped file must be an error.
- **A document's species entry is complete or a reference, decided by the two required properties.** Giving both
  `molar_mass` and `shomate` makes it complete, and it is taken exactly as written with nothing merged in; leaving
  either out makes it a reference to the shipped species of that name and phase, which supplies every property the
  entry does not give. The rule is by completeness rather than "always merge" because merging would quietly give
  every existing document the shipped formation enthalpy, density and vapour pressure of any species it happens to
  share a name with, and it would stop saving byte for byte. The price is two things a reference cannot say: "the
  shipped entry without its density" (a property can be replaced, not removed) and "my own cp with the shipped
  optional properties" (giving cp makes it complete). Either is written out in full. `serial::Species` is the
  mirror type that makes this possible, every property optional; `Species::resolve` does the merge, and
  `LoadError::MissingProperty` names the species, its phase and the field when a reference points at nothing shipped.
  The loader's checks run on the resolved species, so a shipped entry meets the same checks as an inline one.
- **Saving writes every property out, never the reference.** A saved document pins the numbers it was solved with,
  so a later edit to the library cannot change a result someone already has. The cost is that a document written as
  references does not round-trip byte for byte; every fixture is written in full, so none moved.
- **Every entry is copied from its NIST WebBook page, not remembered**, and its `source` names the page, the fit's
  range and the original reference. Where NIST publishes no Shomate fit - every organic here - the entry says what
  stands in for one. Liquids carry a constant cp at 298.15 K, from the NIST fluid tables where the species is in them
  (they also supply the density) and from the condensed-phase page otherwise, which is why ethanol and acetone ship
  without a density. Ethane and propane carry three terms fitted by this crate exactly through NIST's tabulated gas cp
  at 298.15, 400 and 500 K, and say not to trust them far outside that. Steam uses NIST's 500-1700 K fit
  extrapolated down, which is within 0.03% at 298.15 K. Elements carry a formation enthalpy of exactly 0, NIST's
  `H`. Hematite, calcite and graphite were planned and dropped: NIST has no fit for any of them.
- **The library is checked against physics, not only against itself.** Beyond the structural tests (every entry
  sourced, no duplicate name and phase, every entry passes the loader's checks, cp positive at 298, 350 and 400 K),
  every vapour-pressure fit must boil within 1 K of NIST's normal boiling point, a list that sits in the test and
  that a new fit must join; that is what catches an Antoine `A` left in bar, which moves water from 373 K to 587 K.
  The shipped water phases must give the steam-table latent heat. And the README's table of shipped species is
  rendered from the library by a test and compared between two comment markers, so the two cannot drift; the test
  prints the table to paste in when they do. `demo`'s water and quartz constants are pinned equal to the library's.
- **The compressor is isentropic with an efficiency over the ideal gas**, and needed no new property data:
  `Shomate::entropy` is `∫cp/T dT` from the same five coefficients, a difference only - NIST's `G` would make it
  absolute and is still not stored, because both ends of an isentropic path carry the same offset. Pressure enters
  once, as the `nR ln(P2/P1)` an ideal gas's sensible entropy has to climb to hold total entropy fixed, which is
  what `thermo::GAS_CONSTANT` is for. `Stream::entropy` is MJ/(h·K) and `Stream::molar_flow` is Mmol/h, so
  Mmol/h times J/(mol·K) lands in the same unit with no factor. The Newton solve for the isentropic temperature
  shares its loop with `solve_temperature` through a private `solve_for_temperature` that takes the property and
  its slope as closures - enthalpy and heat capacity for one, entropy and heat capacity over temperature for the
  other, both positive wherever cp is, so both have one root. The seed is the constant-cp closed form
  `T1 (P2/P1)^(nR/C)`, exact when cp is constant and a step or two off otherwise; a nitrogen Shomate fit in
  `unit.rs`'s tests is what proves Newton does the rest. Benched against the saved baseline, `solve/tears`
  moved between -9% and +2% (improved or unchanged at every size), so the closures monomorphise away and sharing
  the loop cost `mix` nothing.
- `UnitOp::evaluate` takes **`&SpeciesRegistry`** as a plain second parameter. A `Stream` is a bare vector of flows and
  enthalpy needs each species' heat capacity. Not a context struct - there is one thing to pass - and not a registry
  reference inside `Stream`, whose lifetime would infect `Flowsheet`, `ValidFlowsheet`, `serial` and every test.
  `&SpeciesRegistry` is `Sync`, so the `Send + Sync` bound survives.
- Heat capacity is **Shomate, per mole, five coefficients `A`-`E`** (`thermo::Shomate`). `F`, `G` and `H` are **not
  stored**, and a document that pastes them is rejected by `deny_unknown_fields`. NIST's `H` *is* the standard enthalpy
  of formation, which lives on `Species` instead; `F - H` only makes the antiderivative vanish at
  `thermo::REFERENCE_K`, which `Shomate::enthalpy` already does exactly by subtracting it there, so storing `F` would
  be a second, rounded source of the same number. `G` is entropy. Constant cp is `B = C = D = E = 0` (`Shomate::constant`), so there
  is no `Thermo` enum, and zero terms are dropped on save for the same reason zero flows are.
- Enthalpy is **absolute once `Species::enthalpy_of_formation` is set**: `h(T) = ΔHf + ∫cp dT` from 298.15 K, in
  kJ/mol, per phase. The field is **`Option<f64>`, and optional on the wire** - the one exception to "don't default a
  property" below, because a species no reaction touches has the same offset on both sides of every balance and it
  cancels, and a flotation plant should not have to look up chalcopyrite's. `None` reads as `0.0`, and `0.0 + x` is
  `x` bit for bit, so every document and test without it gives identical numbers. The price is that a *missing*
  value reads as zero too, which is why every reaction participant is required to carry one (see the reactor entries).
  With it set, temperatures agree to 1e-9 rather than bit for bit: Newton subtracts enthalpy flows of millions of
  MJ/h to find a few degrees, which costs about three digits.
- `shomate` is **required** on the wire, unless the species is shipped (the library entry above), in which case the
  library's is a sourced value rather than a default. A defaulted temperature is a guess at state; a defaulted cp would fabricate a
  property and give plausible, wrong temperatures. Loading checks cp > 0 at 298.15 K - one point, not the whole fit.
- Units are picked so **no conversion factor appears**: `Shomate` is per mole, `Species::heat_capacity` / `enthalpy`
  divide by `molar_mass` to get per kg, and `Stream::heat_capacity` / `enthalpy` are MJ/(h·K) and MJ/h, because t/h
  times kJ/kg is MJ/h.
- Demo heat capacities: water and quartz are **NIST-JANAF Shomate fits**. Chalcopyrite has no published fit, so it is a
  **Neumann-Kopp estimate** - Cu + Fe + 2 S at 298.15 K, 95.0 J/(mol·K) - held constant.
- `Mixer`, `Heater` and an adiabatic `ConversionReactor` are the **only ops with a temperature solve**. `Splitter`, `SplitterN` and `Flotation`
  partition flows at constant temperature, which conserves enthalpy exactly. `unit::mix` sums inlet enthalpy and hands it to
  `unit::solve_temperature`: Newton on `H(T) - target`, whose slope is the heat capacity flow and therefore positive,
  so there is exactly one root. It is seeded with the heat-capacity-weighted mean temperature, exact for constant cp,
  and takes 2-3 steps from there. Its 1e-9 K tolerance sits far inside the solver's, or the outer loop would converge
  on inner-solve noise. All-empty inlets keep the first inlet's temperature and pressure.
- `Heater { duty }` is **duty-specified, MJ/h, negative cools** - no separate `Cooler`, and no outlet-temperature
  spec yet. `unit::heat` hands `H_in + duty` to `solve_temperature` starting from the inlet temperature, so Newton's
  first step is `T + Q/C`, exact for constant cp. An **empty inlet passes through with the duty ignored**, because a
  recycle's first pass feeds a heater downstream of a tear exactly that. The price of a duty spec: a large negative
  duty can ask for a temperature below 0 K, which is now a `SolveError::Evaluation` rather than a panic - see the
  `EvalError` entry below. There is no `LoadError` for a non-finite duty because JSON cannot write one; `serde_json`
  rejects `NaN`, `Infinity` and `1e400`, and a test pins that. `tests/heated_recycle.rs` asserts
  `T_product = T_feed + Q / C_feed` by hand, independent of the recycle fraction.
- **`UnitOp::evaluate` returns `Result<Vec<Stream>, EvalError>`.** The alternative was clamping and letting an early
  pass be wrong on its way to being right; clamping loses, because it cannot tell a transient apart from a genuine
  contradiction. A duty that over-cools the *converged* flow would clamp, converge, and report success at the clamp
  temperature - a wrong number and exit 0, which is worse than the panic it replaced. (This entry once added that the
  flash would force the same answer, because Rachford-Rice on a single-phase feed has no root. It does not: the
  bubble- and dew-point checks answer 0 or 1 before the root-finder runs - see the flash entry.)
  Six of the eight ops became `Ok(..)` and nothing else; the real changes are `solve_temperature`, `mix`, `heat`,
  `evaluate_unit` and the wave loop in `solve_with`. Benched against `master`, every group moved between -4.7% and
  +4.2% in both directions and mostly not significantly: no cost, the same finding as the `solve_with` closure.
- `EvalError` is **opaque, wrapping a `Box<dyn Error + Send + Sync>`**, not an enum of failure reasons. Same argument
  that made saving a `ToDocument` supertrait rather than a `match` in `serial.rs`: the op set is open, and a
  library-owned enum puts the closed list back. A downstream op keeps its own error type and `Error::source` hands it
  back to downcast. `Send + Sync` on the inner box or the error would not cross a thread and `SolveError` would stop
  being `Send`.
- The **line between a panic and an `EvalError`** is whose mistake it is. `EvalError`: user input the numerics cannot
  answer - a duty no positive temperature absorbs, Newton not converging because a Shomate fit extrapolates cp
  negative, a reactant driven negative, an adiabatic endothermic reaction no positive temperature can pay for, a flowing
  substance a flash cannot place (a two-phase pair with no vapour pressure, or a volatile liquid with no gas entry). Still a panic: a split fraction outside
  `0.0..=1.0`, empty or all-zero `split_n` ratios, a `recovery` or `stoichiometry` of the wrong length, a conversion
  outside `0.0..=1.0`, a limiting species that is not a reactant, a stoichiometry whose mass does not close, a reaction
  participant without an enthalpy of formation, a
  non-finite duty, and `solve_temperature` on a stream with no heat capacity. Every one of those is either caught by a `LoadError` at the JSON boundary or guarded by the
  caller, so reaching it is a bug in the crate.
- `SolveError::Evaluation` carries **both `UnitId` and `name`**: the id so a caller can look the unit up, the name
  because this is the one `SolveError` a *user* caused and "unit 41" is no help in a 300-cell circuit.
  `SolveError::Untearable` keeps ids alone because it is about the shape of the graph, which no single name explains.
  The message is `unit 'cooler' failed on pass 1: no positive temperature holds an enthalpy flow of -1e9 MJ/h
  (Newton reached -1.93e7 K)` - the user-facing half first, the iterate as a parenthetical diagnostic.
  `tests/fixtures/impossible_duty.json` is that case end to end: a legal document, a correctly wired flowsheet, and a
  contradiction that only shows up once a stream reaches the unit.
- The solver writes `if let Err(source) = flowsheet.evaluate_unit(unit)` rather than `.map_err(|source| ..)?`,
  because the closure would want `flowsheet` shared while the `&mut` borrow for `evaluate_unit` is still live in the
  same expression. (C# has no equivalent problem, which is exactly why it is worth a note.)
- Threading `Result` through does **not** fix the early-pass transient it was partly motivated by, and on the
  `tests/heated_recycle.rs` shape there is nothing to fix: all the feed passes through the heater and out to the
  product, so the boundary balance pins the heater outlet at `T_feed + Q / C_feed` whatever the recycle fraction, and
  a duty the converged loop can absorb is one every pass can absorb too.
- **A cooler on a recycle-only branch has no transient either**, which this file used to claim it did. Feed ->
  mixer -> splitter, recycle -> cooler -> mixer, constant cp: the tear is the cooler outlet, so pass 1 hands the
  cooler `f * F` at `T_feed`, and its outlet is `T_feed - q / f` (with `q = |Q| / C_feed`). The converged boundary
  balance puts the mixer outlet at `T_feed - q` and the cooler, on `f / (1 - f)` of the feed, at
  `T_feed - q - q (1 - f) / f` - which is `T_feed - q / f` again. Measured: a duty that cools the converged recycle to
  1 K solves from the all-zero tear at f = 0.3, 0.5 and 0.9, and so does the same loop with the demo's
  temperature-dependent Shomate feed. A transient that crosses 0 K would need a recycle whose composition, and so its
  cp per tonne, drifts between passes; none has been found.
- **Tear streams are not seeded from the feeds, deliberately** - it was built, measured, and thrown away. The seed was
  every empty tear set to the feeds' sum at their adiabatic mix temperature (a tear that already held flow was left
  alone, so a warm start survived). On flows alone it saved one pass at f = 0.9 (119 to 118, 75 to 74, heated 171 to
  170), nothing at f = 0.3, and three on `blend.json` (30 to 27, where a recycle of one half makes the loop carry
  exactly the feed). Direct substitution shrinks the error by a constant factor, so a seed saves
  `ln(|x0 - x*| / |seed - x*|) / ln(1 / factor)` passes, and a feed-sized seed is only much nearer a recycle that is many
  times the feed - about one pass at most, at every fraction measured. Temperature is where it lost: the seed starts the recycle
  at feed temperature, whereas a zero tear carries no enthalpy and pass 1 lands on the right temperature (the entry
  above). On the cooled loop it cost **18 to 24 passes at f = 0.3, 30 to 39 at 0.5 and 176 to 210 at 0.9**, the colder
  the recycle the worse. A seed that helps would have to guess temperature as well as flow, and there is nothing to
  guess it from.
- `Stream::set_temperature` is **public** because `mix` lives in `unit.rs` and cannot reach the private field; the
  alternative was copying the flows into a new stream on every Newton step. `+=` on `Stream` stays **flows-only** by
  design - combining temperatures needs the registry, and that is `mix`.
- The solver stops on **max(flow residual, temperature residual)** per tear stream. One tolerance serves both because
  both are relative, and a relative temperature residual only means anything in Kelvin. `solver::nan_max` exists
  because `f64::max` drops a `NaN` and returns the number (C#'s `Math.Max` returns the `NaN`), which would let a
  garbage pass report itself converged.
- **Wegstein extrapolates temperature too**, with its own `q`, the same as one more species. It went in once a
  measurement showed temperature setting the pass count: on the heated loop in `tests/heated_recycle.rs` at 90%
  recycle, flows-only Wegstein settled the flows by pass 22 and then waited on temperature until pass 142. With
  temperature extrapolated it takes 22, the same as the loop with no duty. A non-positive extrapolated temperature
  falls back to the pass's result, because `solve_temperature` panics at or below 0 K. Pressure is still copied from
  the result. Isothermal loops are unaffected: the tear temperature never changes between passes, so `dx = 0` and
  `q = 0`.
- An adiabatic circuit with **one feed sits at the feed temperature everywhere**. With several, the recycle enters and
  leaves the mixer at the same temperature and cancels, so every internal stream sits at the feeds' own adiabatic mix.
  `tests/energy_balance.rs` asserts against that, the energy counterpart of `demo::balance`. The demo circuit is all at
  25 °C, so its temperature column is uniform and none of its pass counts moved.
- Outlet order is positional: the Nth `add_stream` from a unit matches the Nth stream from `UnitOp::evaluate`. Wiring a
  splitter backwards still balances mass, so it fails silently.
- Flowsheets are built loosely then validated: `Flowsheet::validate` consumes `self` and hands back a `ValidFlowsheet`,
  which is the only thing `Solver::solve` accepts. Illegal-states-unrepresentable does not work here — a recycle forces
  a unit to be referenced before its upstream exists, and arity is only decidable once construction stops.
  `ValidFlowsheet` has `Deref` but deliberately no `DerefMut`, so the topology cannot change behind the validation.
- Kahn's topological sort emits **waves** (`Vec<Vec<UnitId>>`), not a flat order, so parallelism is free later.
- `tear_streams` tears **back edges, one per cyclic component per round, repeating until nothing cyclic is left**.
  A component holding one loop costs one round; interlocking loops cost a round each, because no single stream lies
  on all of them. Before this, one tear per component was all it ever returned, and a component of interlocking loops
  came back short and unorderable - `SolveError::Untearable` on a legal flowsheet. Two details make the back edge come
  out as the recycle: `Flowsheet::edges` carries the `StreamId` alongside the downstream `UnitId`, so a back edge can
  name the stream to cut; and `Tarjan` tracks **`on_path` separately from `on_stack`**, because Tarjan's stack holds
  the whole component being built, so `on_stack` alone cannot tell a back edge from a cross edge into a sibling
  branch. The traversal also starts at **sources before other units**, since entering the demo circuit anywhere but
  the feed finds a different edge of the same loop.
- The tear set is **not minimal**, deliberately. `tests/interlocking_loops.rs` is a circuit whose three loops need
  two tears and get three, and it says so. A minimal set is minimum feedback arc set, which is NP-hard; ordering at
  all is what the heuristic promises, and an extra tear costs one more stream in the unknown vector.
- Moving the demo's tear from S1 (mixer outlet) to S4 (the recycle) **cost one pass** at every direct-substitution
  setting: 11 to 12 and 74 to 75 at 1e-6, 118 to 119 at 1e-9 for `f = 0.9`. The residual is now measured one unit
  further round the loop, and the tight low-recycle case (17) and every Wegstein count did not move. The benched cost
  of tearing itself is +3% to +6% on flowsheets small enough for setup to dominate, and nothing at 512 units and
  above - it runs once per solve, not once per pass.
- Float equality: never `==`. `approx` (dev-dependency) in tests; `Stream::max_flow_residual` for the solver.
- JSON is a **separate wire format** in `serial.rs`, not serde attributes on the domain types. `serial::Flowsheet`
  etc. reuse the domain names and are told apart by module path, the Rust convention over a `Doc`/`Dto` suffix. The
  rule for what gets `#[derive(Deserialize)]` directly: types whose privacy encodes an invariant (`SpeciesId`,
  `Stream`, `Flowsheet`) get a mirror type, and so does any type the document can write in a shape the domain type
  cannot hold - `Species`, since the library entry below, because a document may leave out its required properties
  and point at a shipped species instead. Plain data with no such gap (`Phase`, `Shomate`, `Antoine`) does not.
- Loading **replays the builder** (`insert` / `add_unit` / `add_stream`) rather than constructing a `Flowsheet`
  directly, so every invariant those methods maintain still holds. `TryFrom` returns a plain `Flowsheet`; the caller
  still calls `validate`. `LoadError` fails fast — unlike `check`, which collects — because load errors are typos.
- Every domain constructor that **panics** on bad input has a matching `LoadError` at the JSON boundary
  (split fractions, split ratios, negative flows, non-positive temperature).
- Document flows (and `recovery`, `stoichiometry`, `limiting`) are keyed by **`SpeciesRegistry::key`**, so files don't
  depend on species order. The key is the **bare name when only one phase uses it, `"H2O(g)"` when two do**. Saving
  writes the bare form wherever it can, so every single-phase document saves byte for byte as before; loading accepts a
  suffixed key always and a bare one only when it is unambiguous (`LoadError::AmbiguousSpecies`, listing the keys that
  would work). The suffix is exactly `(s)`, `(l)` or `(g)`, so `Fe(OH)3` and `Quartz(150um)` stay ordinary names, and
  a name that *ends* in one is rejected on both boundaries (`LoadError::ReservedSpeciesName`,
  `FlowsheetError::ReservedSpeciesName`) - otherwise a liquid named `H2O(g)` beside a gaseous `H2O` would key the same.
  That rule is what lets `resolve` try the bare name first and the suffix second without the two readings competing.
  `DuplicateSpecies` narrowed to the same name *and* phase, because `insert` would silently hand back the first id.
  The loader's `BTreeMap<String, SpeciesId>` went with it: `Spec::species(key)` resolves by linear scan against the
  registry, so `Spec` lost its public `species_ids` field and there is one source of truth for a key. Accepting both
  forms means one species has two keys, so a map writing `"H2O"` *and* `"H2O(l)"` is `LoadError::DuplicateSpeciesKey`
  rather than the later value silently overwriting the earlier; all three maps go through one `species_vector` helper
  so none can forget the check. One qualifier
  only - the screen item still has to decide whether a key carries a list.
- Units carry a **user-supplied `name`**. Uniqueness is checked in `Flowsheet::check`, not `add_unit`, so the builder
  stays infallible and `ValidFlowsheet` is what guarantees a saveable document.
- `serde_json` needs the **`float_roundtrip` feature**. Its default parser is off by up to 1 ULP, which silently
  perturbs flows on every save/load cycle and breaks byte-identical round-trip tests.
- Every document type carries **`deny_unknown_fields`** — the format is hand-edited, so a typo must be an error, not
  a silently dropped key. `serial::Unit` nests `op` rather than flattening it, so `Unit` keeps the attribute; `serial::Op`
  cannot, because it flattens the op's parameters and serde forbids the two together. The per-op spec structs
  (`SplitterSpec`, `NoSpec` for the parameterless ops) still carry it, and `Spec::parse` is where it fires — so the
  check moved from parse time to load time and nothing else changed. The JSON shape is unchanged by all of this.
- `unit.rs` keeps the trait, `Arity`, `Unit` and the `mix`/`solve_temperature`/`split`/`split_n`/`recover` free functions; each operation
  gets **its own file under `unit/`**, re-exported flat (`pub use feed::Feed;`) so every call site still writes
  `unit::Mixer`. The submodules stay private - they are a file-layout detail and buy nothing else. Sibling-file module
  style (`unit.rs` next to `unit/`), not `unit/mod.rs`.
- The **table formatter lives in the library** (`report.rs`), not in `main.rs`. A stream's
  endpoints are only recoverable from the `inlets`/`outlets` of the units that list it, and those
  are `pub(crate)` — `main.rs` is a separate crate and cannot see them. Keeping the formatter in
  the library avoids widening `Unit`'s public surface just to compute a display column, and it
  moved into the `flowsheet` library crate unchanged at the workspace split, as predicted.
- Stream labels are `{from}.{to}`, deduped with a **single running counter**: the first occurrence
  stays bare, repeats become `#2`, `#3`. No counting pre-pass is needed, because only occurrences
  after the first are ever suffixed.
- `report::imbalance` answers **whether the balance closes**: sources are units with no inlets,
  sinks are units with no outlets, so the material that entered is the sum of every source's
  outlets and the material that left is the sum of every sink's inlets. The footer prints it
  beside the residual (`converged in 17 iterations, residual 6.4e-10, imbalance 1.9e-10`). It is
  **per species, not per total** - a mis-wiring that moved gangue into the copper column leaves
  both totals untouched - unless any unit's `UnitOp::conserves_species()` is `false`, which switches the whole
  flowsheet to total mass (see the conversion reactor entry) - and normalised by the larger of the two totals, `NaN`-propagating,
  which is `Stream::max_flow_residual`'s convention: the two numbers sit side by side, so they
  have to mean the same thing. That is what promoted `solver::nan_max` to `pub(crate)`.
- The TODO item said the demo circuit's `S2 + S5 == S0` **exactly**; it does not, and cannot.
  A solve stops when the tear stream stops moving, so the tear still holds the error the next
  pass would have removed, and that error is exactly what shows up at the plant's doors:
  1.9e-10 against a residual of 5.4e-10, the same quantity seen from the other end. The
  assertion is therefore against `SolverConfig::default().tolerance`, not against zero, and the
  acyclic case gets its own test at 1e-15 to keep rounding and iteration error told apart.
  Because the footer's imbalance is now a computed number rather than a supplied one, the
  table-layout test pins the rows exactly and the footer by prefix, the same split
  `crates/flowsheet-cli/tests/cli.rs` already used for the residual.
- Nothing in the library can produce a flowsheet whose ends disagree - every op conserves mass -
  so the `report::tests::leaking` fixture **writes the two streams by hand** and never solves.
  `add_stream` takes the stream, so the numbers go in at construction and no `IndexMut` or
  registry clone is needed.
- `main` deliberately does **not** return `Result`. Rust prints a returned error with `Debug`, not
  `Display`, so every message these error types carefully write would surface as a struct literal.
  Instead `main` calls `run()`, prints `{e}` to stderr, and exits 1.
- Benches are **one criterion target**, two groups: `solve/units` over an acyclic chain of tanks and `solve/tears`
  over `k` independent recycle loops. The fixtures live in `benches/solve.rs` rather than `demo` or `test_support` —
  `test_support` is `#[cfg(test)]` and a bench is compiled without it, and a chain of 4096 tanks is not a circuit
  anyone needs demonstrated. `iter_batched_ref`, never `iter`: `solve` writes into the flowsheet, so a reused one is
  already converged and the second iteration would silently measure a single no-op pass. The `units` group uses
  `BatchSize::LargeInput` because `SmallInput` holds a tenth of the iteration count in memory at once and a
  4096-unit flowsheet is about a megabyte.
- `criterion` is pulled with **`default-features = false, features = ["cargo_bench_support"]`**, which drops
  `plotters` and `rayon`. It still adds ~15 crates (clap, regex, walkdir, ciborium) to the dev graph, and
  `cargo clippy --all-targets` compiles all of them on every run. CI does not run `cargo bench` — a shared
  runner has no stable baseline to compare against, and `--all-targets` already proves the benches build.
- The **enum-vs-trait-object bench** was run once and deleted, so its numbers live here instead. On the demo
  circuit, 64 passes: enum 22.9 µs, `Box<dyn UnitOp>` 23.3 µs — +1.8%, with overlapping confidence intervals. On a
  bare `inlet_arity()` over 1024 ops: enum 0.11 ns/call, dyn 1.36 ns/call. The vtable hop is ~1.4 ns against a ~60 ns
  `evaluate` that allocates a `Vec<Stream>`, so dispatch is ~2% of a unit evaluation and invisible end to end. The
  12× on the bare call is a ceiling, not a per-call instruction cost: the enum loop auto-vectorises (>1 element per
  cycle), and a `dyn` call is an optimisation barrier, so it cannot vectorise at all. Re-measure only if `evaluate`
  ever stops allocating.
- The **energy balance bench** (criterion, `master` at `5900c95` against `energy-balance`, same machine):
  `solve/units` did not move at any size from 8 to 4096 tanks, so passing `&SpeciesRegistry` to `evaluate` is free.
  `solve/tears` regressed **+9% to +21%**, about +17% typical: `tears/1` went from 6.16 to 6.60 µs and `tears/16` from
  81.3 to 95.9 µs, roughly 25-50 ns per mixer evaluation. None of it is allocation. It is the arithmetic of summing
  enthalpy and heat capacity over every inlet and taking one Newton step - which `mix` does even when every stream
  is at 25 °C and the seed is already exact.
- The CLI has **no `--output` flag**. `--json` writes a whole document to stdout and a test proves it reloads as a
  legal input, so `> solved.json` is the supported way to save one — the shell already owns that job. Column
  include/ignore flags were dropped from the original clap item for the same reason they were never missed: nothing
  needs them yet.
- The CLI's solver flags are **`--tolerance`, `--max-iterations`, `--method direct|wegstein`**. The two numeric
  defaults are `default_value_t = SolverConfig::default().tolerance` / `.max_iterations`, read off the library so
  they cannot drift - the price is that `--help` prints `0.000000001` rather than `1e-9`, since clap formats a
  `default_value_t` with `Display`. `--method` needs its own **fieldless `Method` enum** in `main.rs`, because
  `ValueEnum` derives a name per variant and `ConvergenceMethod::Wegstein` carries `q_min`/`q_max`; `From<Method>`
  supplies the clamps (-5.0 / 0.0, the same pair `tests/recycle_convergence.rs` uses) and they stay off the command
  line until something needs them. `tests/fixtures/stiff_recycle.json` is `recycle.json` with the splitter fraction
  at 0.9 and **nothing else changed**, so it is the 119-pass case from that test as a document: it fails with
  `no convergence after 100 passes` under the defaults, and solves in 11 under `--method wegstein`, 119 under
  `--max-iterations 200`, and 75 under `--tolerance 1e-6`.
- Solve progress is **`Solver::solve_with(fs, impl FnMut(SolveEvent))`**, and `solve` is `solve_with(fs, |_| {})`. A
  generic closure rather than `&mut dyn FnMut`, a trait or a channel: the no-op closure monomorphises away, and a
  caller wanting a channel or several subscribers can forward from the closure. Benched against `194d1e6`: every
  group moved between -9% and +6%, in both directions, so no cost. `FnMut` rather than `Fn + Sync`, which means the
  rayon item must emit `UnitEvaluated` from the serial side of a wave, not from inside it. `SolveEvent` is
  `#[non_exhaustive]`, so a new variant is not a breaking change for downstream `match`es.
- The CLI shows an **indicatif spinner on stderr**: pass number and residual. indicatif hides it when stderr is not a
  terminal, so pipes and `tests/cli.rs` never see it. The demo circuit solves before the first redraw, so in practice
  it only appears on large flowsheets.
- The **rayon item is gated on a unit op with an inner solve**, which is why it sits behind the unit-op items in
  TODO.md rather than first. A unit evaluation costs ~90 ns (total solve time over unit evaluations, release, this
  machine), so a solve is `units × passes × 90 ns`: the demo circuit is 23 µs, and the smallest *converging* flowsheet
  that reaches 100 ms is ~18,000 units at 81 passes - 3,000 independent flotation loops at recycle 0.9 with the slowest
  species recovering 0.12, which is the most a single tear can cost inside the default 100-pass cap. Spawning and
  joining a rayon wave costs a microsecond or two against the 9 µs of work in a hundred of today's ops, so on this
  workload the bench can only measure overhead - matching the earlier finding that the allocator dominated and that
  making allocation cheaper made parallelism worse, not better. Conversion reactions and pressure do not move it:
  both are straight arithmetic over the flows vector, ~1× a mixer. What moves it is an op that iterates on every
  evaluation - a flash is ~20× a mixer ideal and ~200× with a cubic EOS, and a 40-stage distillation column ~5,000×,
  enough for 100 ms on its own inside a recycle. There is no realistic mass-and-energy-only flowsheet that takes
  100 ms; a plant-scale flotation circuit of 300 cells is single-digit milliseconds. So "the demo feels substantial"
  and "parallelism pays" are one blocker, not two.
- Three limits found while measuring that: `Flowsheet::components` recursed, so a 16,000-unit single chain overflowed
  the stack; cascading tears in series costs O(N) passes, and 128 flotation stages in series took 239, past the
  default 100; and `add_unit` / `add_stream` cast with `as u16`, so the 65,536th unit or stream wrapped silently
  rather than failing. Independent loops dodge the first two - short paths, one tear each, and a pass count that does
  not grow with the loop count. The third is `assert_id_space`, near the top; the first is the entry below.
- **Tarjan runs on an explicit stack**, `Vec<(UnitId, slice::Iter)>` - a unit and the edges it has yet to look at,
  the frame a recursive call would have kept. The iterator rather than a `(UnitId, usize)` edge index because it
  borrows `edges` and not the frame, so `rest.next()` hands back an edge and ends the frame's borrow, leaving `call`
  free to push. The one line that moves is the lowlink inheritance: a recursive traversal runs it right after its call
  returns, and here it runs when the child's frame is popped, against the new top. `visit` split into `enter` and
  `finish` either side of that loop, and `edges` / `torn` stopped being fields, because `run` is the only reader.
  The tests are a 50,000-unit chain and a 50,000-unit ring run on a **256 KiB thread**: a test thread's default 2 MiB
  and the debug/release difference in frame size would otherwise decide whether the old code overflowed. A stack
  overflow **aborts the process** (`STATUS_STACK_OVERFLOW`, 0xc00000fd) rather than panicking, so it cannot be a
  `should_panic` test, and before the fix it took the whole test binary down with it.
  Benched against `a04caee`: `solve/units` improved 4-6% at 8, 512 and 4096 and did not move at 64; `solve/tears`
  moved between +0.1% and +5.8%, significant only at `tears/1` (6.9 to 7.3 µs). The likeliest cause there is the
  `call` vector - one more heap allocation per Tarjan run, on a flowsheet small enough for setup to dominate - but
  that is not measured. Nothing at 512 units or 16 tears and above regressed.

## Layout

Cargo workspace. The root `Cargo.toml` is a **virtual manifest** — `[workspace]`, no `[package]` — so `cargo test`
and `cargo clippy` at the root cover every member.

```
crates/flowsheet/       the library: domain types, solver, serial, report. Depends on serde only.
  src/unit.rs           the `UnitOp` trait, `Arity`, `Unit`, and the free functions ops are built from.
  src/unit/             one file per operation, re-exported flat from `unit.rs`.
  src/thermo.rs         Shomate heat capacity and enthalpy, per mole. `Species` converts to per kg.
  src/library.rs        the shipped species library, loaded once from data/species.json.
  data/species.json     the shipped species, each with the source of its numbers.
  benches/solve.rs      criterion, `harness = false`. Solve time vs unit count and vs tear count.
crates/flowsheet-cli/   the `flowsheet` binary: clap parsing, file IO, progress, error printing.
```

- The split exists so **`clap` stays out of the library's dependency graph**. Cargo has no per-target dependencies,
  so in a single crate the binary's deps are also the library's, for everyone downstream.
- `serde_json` **is a dependency** of `flowsheet`, not just of the CLI. It was a dev-dependency until `serial::Op`
  started carrying an op's parameters as a `serde_json::Map`, which puts `Value` in the library's public API. Turning
  a *whole document* into bytes is still the caller's job — the library never writes a file.
- **The library gets the plain name**, the CLI package is `flowsheet-cli`. `-core` earns its keep only when a facade
  crate re-exports it, and there is no facade here; the library is what people `use`, so `_core` would be noise on
  every import. Same shape as `wasmtime` / `wasmtime-cli`. The cost is that `cargo install flowsheet` fetches the
  library and installs no binary — the quickstart has to say `cargo install flowsheet-cli`.
- The CLI's `[[bin]]` sets **`doc = false`**. A `[[bin]]` name is a filename rather than a Rust identifier, which is
  how package `flowsheet-cli` can produce a binary called plain `flowsheet` — but that makes the bin target and the
  library share a name, so both want to write `target/doc/flowsheet/index.html` and one silently clobbers the other
  (rust-lang/cargo#6313). Note `RUSTDOCFLAGS="-D warnings"` does *not* catch this: it is a Cargo warning, not a
  rustdoc lint.
- The root manifest sets **`resolver = "3"`** explicitly. A virtual manifest has no `edition` to infer it from and
  silently defaults to resolver 1 otherwise.
- Dependency versions are declared once in **`[workspace.dependencies]`** and inherited with
  `serde_json = { workspace = true }`. That is what keeps the `float_roundtrip` feature from drifting between the two
  crates that parse JSON. (Closest C# analogue: central package management in `Directory.Packages.props`.)
- Package metadata follows the same rule: `repository` and `rust-version` live once in
  **`[workspace.package]`** and are inherited with `repository.workspace = true`. `description` stays per-crate, because
  the two crates are different things. **`edition` deliberately stays per-crate too** - the note about `resolver`
  above turns on a virtual manifest having no edition of its own to infer from, and moving it would undercut that.
- `rust-version` is **1.85**, which edition 2024 requires; clap and indicatif ask the same and serde asks for less.
  It is a promise to people who *use* the crates, so it excludes dev-dependencies: `criterion` needs 1.86, and so
  therefore do `cargo test` and `cargo bench` here, but nothing downstream sees that.
- `cargo publish -p flowsheet-cli --dry-run` **cannot pass before the library is published**. Packaging rewrites the
  path dependency into a registry one and then resolves it, so it fails with `no matching package named flowsheet`
  until `flowsheet 0.1.0` is really on crates.io. Not a defect, and not fixable by ordering the fields differently -
  it is why the library publishes first.
- The **headline types are re-exported at the crate root** (`pub use` in `lib.rs`), so callers write
  `flowsheet::Flowsheet` instead of `flowsheet::flowsheet::Flowsheet` — the crate and its main module share a name and
  the stutter otherwise shows up at every import. The modules stay `pub`, so the long paths still work. Two things
  are deliberately *not* re-exported: `serial`'s types, because `serial::Flowsheet` vs `flowsheet::Flowsheet` is
  exactly the module-path distinction the wire format relies on and flattening them would collide; and free functions
  like `unit::mix` and `report::table`, which read better carrying their module.
- **`.gitattributes` pins `eol=lf`.** `core.autocrlf=true` is set on the dev machine, and `tests/cli.rs` compares a
  fixture byte-for-byte against `serde_json` output, which always writes LF. Without the pin that test passes on CI
  (Linux) and fails on Windows.

## Testing

- Unit tests in-file (`#[cfg(test)] mod tests`) — can see private items.
- `crates/flowsheet/tests/` for end-to-end flowsheet solves; `crates/flowsheet-cli/tests/` for running the
  binary over a document in its own `tests/fixtures/`. Cargo sets an integration test's working directory to the
  *package* root, so the relative fixture paths in `cli.rs` resolve inside `crates/flowsheet-cli/`.
- `recycle.json` is the demo circuit saved as a document; `cli.rs` asserts it byte-for-byte against
  `serial::Flowsheet::from(&demo::build_flowsheet())`, so it cannot drift from the demo.
- `blend.json` is the **README's worked example**, pasted verbatim in hand-written form, and `cli.rs` pins the
  table rows the README prints. It replaced the demo circuit as the README example because flotation read as too
  mineral-processing-specific; the price is that the README example is no longer the flowsheet the rest of the suite
  reasons about. Edit the README's JSON or table and this fixture and test move with it.
- Every completed TODO item ships with tests. Demo and example code is illustrative and exempt.

## Commands

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
```

Plus, when touching the solver or a unit op:

```bash
cargo bench -p flowsheet
```

Not part of CI, and not a gate on removing a TODO item — `--all-targets` above already compiles it.

All four must be clean before removing a TODO item. These are exactly what
`.github/workflows/rust.yml` runs, on both Linux and Windows — the matrix is not redundant, because the byte-exact
document test in `flowsheet-cli` is line-ending sensitive and only fails on Windows.

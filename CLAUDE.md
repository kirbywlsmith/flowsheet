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
  nine shipped ops, `register` adds or replaces one, and `serial::Flowsheet::into_domain(&ops)` is the real entry
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
- The **conversion reactor is the first op that transforms species**. `ConversionReactor { reaction: Reaction }`,
  tag `conversion_reactor`, one inlet and one outlet. `Reaction { stoichiometry, limiting, conversion }` is plain data
  defined in `unit.rs` beside `unit::react`, not in the op's file, so `unit.rs` never imports from its own child and
  the later several-reactions item can reuse it. Coefficients are **molar and signed**; extent is
  `conversion * n_limiting / |nu_limiting|` in Mmol/h, and t/h over g/mol needs no factor. The limiting species is
  written `in * (1 - conversion)`, not through the extent, which does not cancel exactly and lands a ULP either side
  of zero at conversion 1. **Isothermal**: the outlet takes the inlet temperature, so any reaction with a heat of
  reaction breaks the energy balance silently until the reactor energy balance item lands.
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
- On the wire the reaction is **nested and singular**: `{ "type": "conversion_reactor", "reaction": { "stoichiometry":
  {..}, "limiting": "CH4", "conversion": 0.9 } }`. The stoichiometry is a species-name map like `recovery`, but
  **zero coefficients are dropped on save** - an equation lists its participants, whereas a zero recovery is a
  statement. The several-reactions item will rename `reaction` to `reactions` and break the format; acceptable only
  because nothing is published.
- **The energy balance solves temperature and nothing else.** `pressure` is still a carried label (see TODO.md), and
  there is no phase change.
- `UnitOp::evaluate` takes **`&SpeciesRegistry`** as a plain second parameter. A `Stream` is a bare vector of flows and
  enthalpy needs each species' heat capacity. Not a context struct - there is one thing to pass - and not a registry
  reference inside `Stream`, whose lifetime would infect `Flowsheet`, `ValidFlowsheet`, `serial` and every test.
  `&SpeciesRegistry` is `Sync`, so the `Send + Sync` bound survives.
- Heat capacity is **Shomate, per mole, five coefficients `A`-`E`** (`thermo::Shomate`). NIST's `F` and `H` only fix the
  absolute zero of enthalpy, and every enthalpy is taken relative to `thermo::REFERENCE_K` (298.15 K), so they cancel;
  `G` is entropy. Reactions need an absolute reference and will bring `F` and `H` back; until then a document that
  pastes them is rejected by `deny_unknown_fields`. Constant cp is `B = C = D = E = 0` (`Shomate::constant`), so there
  is no `Thermo` enum, and zero terms are dropped on save for the same reason zero flows are.
- `shomate` is **required** on the wire. A defaulted temperature is a guess at state; a defaulted cp would fabricate a
  property and give plausible, wrong temperatures. Loading checks cp > 0 at 298.15 K - one point, not the whole fit.
- Units are picked so **no conversion factor appears**: `Shomate` is per mole, `Species::heat_capacity` / `enthalpy`
  divide by `molar_mass` to get per kg, and `Stream::heat_capacity` / `enthalpy` are MJ/(h·K) and MJ/h, because t/h
  times kJ/kg is MJ/h.
- Demo heat capacities: water and quartz are **NIST-JANAF Shomate fits**. Chalcopyrite has no published fit, so it is a
  **Neumann-Kopp estimate** - Cu + Fe + 2 S at 298.15 K, 95.0 J/(mol·K) - held constant.
- `Mixer` and `Heater` are the **only ops with a temperature solve**. `Splitter`, `SplitterN` and `Flotation`
  partition flows at constant temperature, which conserves enthalpy exactly. `unit::mix` sums inlet enthalpy and hands it to
  `unit::solve_temperature`: Newton on `H(T) - target`, whose slope is the heat capacity flow and therefore positive,
  so there is exactly one root. It is seeded with the heat-capacity-weighted mean temperature, exact for constant cp,
  and takes 2-3 steps from there. Its 1e-9 K tolerance sits far inside the solver's, or the outer loop would converge
  on inner-solve noise. All-empty inlets keep the first inlet's temperature; the outlet takes the first inlet's
  pressure.
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
  temperature - a wrong number and exit 0, which is worse than the panic it replaced. The flash forces the same
  answer anyway: Rachford-Rice on a single-phase composition has no root, so there is no value to clamp *to*.
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
  negative, a reactant driven negative, later a flash with no two-phase split. Still a panic: a split fraction outside
  `0.0..=1.0`, empty or all-zero `split_n` ratios, a `recovery` or `stoichiometry` of the wrong length, a conversion
  outside `0.0..=1.0`, a limiting species that is not a reactant, a stoichiometry whose mass does not close, a
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
  a duty the converged loop can absorb is one every pass can absorb too. A heater on a recycle-only branch would not
  have that property. Seeding tear streams from feeds instead of zeros is the general mitigation, and it is not an
  item yet.
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
  `Stream`, `Flowsheet`) get a mirror type; all-public-field data with no constructor (`Species`, `Phase`) does not.
- Loading **replays the builder** (`insert` / `add_unit` / `add_stream`) rather than constructing a `Flowsheet`
  directly, so every invariant those methods maintain still holds. `TryFrom` returns a plain `Flowsheet`; the caller
  still calls `validate`. `LoadError` fails fast — unlike `check`, which collects — because load errors are typos.
- Every domain constructor that **panics** on bad input has a matching `LoadError` at the JSON boundary
  (split fractions, split ratios, negative flows, non-positive temperature).
- Document flows are a **species-name map**, so files don't depend on species order. Consequence: names must be unique
  across phases, so `H2O` liquid + `H2O` gas is currently rejected on **both** boundaries: `LoadError::DuplicateSpecies`
  on the way in, and `FlowsheetError::DuplicateSpeciesName` in `check` on the way out - otherwise saving would silently
  drop one of the two flows. Revisit with a composite key (`"H2O(g)"`) when phase change lands -
  the energy balance shipped without it, with every species staying in the phase it was registered with.
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
  Benched against `9d35a92`: `solve/units` improved 4-6% at 8, 512 and 4096 and did not move at 64; `solve/tears`
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

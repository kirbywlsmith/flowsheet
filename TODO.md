# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `crates/flowsheet-cli/src/main.rs`.

Each item is meant to ship on its own: all four commands in CLAUDE.md clean, tests included. Where an item leaves
something out deliberately, the item that picks it up is named - nothing below is a reduction in what the library is
eventually for.

- [ ] Conversion reactor, isothermal, one reaction. `Reaction { stoichiometry: Vec<f64>, limiting: SpeciesId,
  conversion: f64 }` with one inlet and one outlet - structurally `Flotation`'s sibling: a dense `Vec<f64>` in
  `SpeciesId` order validated by a `unit::react` free function, composition changed by transforming species rather
  than partitioning them. Coefficients are **molar** and signed (negative consumed, positive produced), because that
  is how a reaction is written; `react` converts with `molar_mass` and rejects a stoichiometry whose mass balance does
  not close (`sum of nu_i * M_i ~ 0`), which is the one way a stoichiometry can be wrong that a recovery vector
  cannot. Extent is `conversion * n_limiting / |nu_limiting|`. On the wire, a species-name map like `recovery`, but
  zero coefficients are **dropped** on save rather than kept: a zero recovery says a species does not float, while a
  reaction equation simply lists its participants. The outlet leaves at the inlet temperature, which means an
  exothermic reaction silently breaks the energy balance until the next item lands - say so in the doc comment, and
  make the test assert the extent by hand the way `demo::balance` does.
- [ ] Heat of reaction, and an absolute enthalpy basis. Every enthalpy today is relative to `thermo::REFERENCE_K` and
  the zero point cancels, which stops being true the moment a species is destroyed: bring Shomate's `F` and `H` back
  (`deny_unknown_fields` currently rejects them) plus a standard enthalpy of formation per species, and the reactor's
  energy balance falls out of `H_out - H_in` with no separate heat-of-reaction parameter. Then the reactor gets a
  spec: isothermal reports the duty it needed, adiabatic hands `H_in` to `unit::solve_temperature` the way `heat`
  does. Reference-state consistency is the thing to test - an inert species' enthalpy must not move, and a cycle of
  reactions must sum to zero.
- [ ] Several reactions in one reactor. `Vec<Reaction>`, extents applied in declared order, with the limiting
  reactant's moles re-read between reactions so that two reactions competing for one reactant are well defined rather
  than over-consuming it. Aspen's `RStoic` is the shape. Mostly a loop plus a validation that no species goes
  negative.
- [ ] Equilibrium reactor. Gibbs free energy minimisation over the declared species, which needs entropy and so
  brings Shomate's `G` back alongside a standard entropy of formation. This is the first op with a genuine inner
  iteration (constrained minimisation under element balances - Lagrange multipliers, or RAND), so it is also the first
  real test of whether the outer solver's tolerance and an inner one's stay properly separated; `solve_temperature`
  set that precedent at 1e-9 K.
- [ ] Pressure, specified and propagated. Nothing reads it and nothing solves it - `Stream::add_assign` says so, and
  the field survives a whole solve untouched. Aspen Plus and HYSYS specify it at this level too: each op declares a
  pressure drop, `evaluate` writes the outlet pressure, and a mixer takes the minimum inlet pressure rather than the
  first. `Pump` and `Compressor` add head and a duty. Solving pressure *properly* makes flows and pressures
  simultaneous unknowns - that is the hydraulic network problem, and it waits for the dynamic item.
- [ ] Composite species key on the wire, e.g. `"H2O(g)"`. `SpeciesRegistry::find` already keys on name *and* phase,
  but the JSON flows map is name-only, so the same name in two phases is rejected on both boundaries
  (`LoadError::DuplicateSpecies` going in, `FlowsheetError::DuplicateSpeciesName` in `check` coming out). Parse and
  format the phase suffix, drop both errors, and keep the bare name valid for the single-phase case so every existing
  document still loads byte-for-byte. Small, standalone, and a hard prerequisite for both phase change and a
  size-classified screen.
- [ ] Latent heat. Enthalpy of vaporisation per species plus a vapour pressure correlation (Antoine is the usual
  three-coefficient fit), measured against the absolute basis the heat-of-reaction item established. No flash yet -
  this is the property data and the `thermo` functions over it, testable on their own against steam-table values.
- [ ] Isothermal flash, `T` and `P` specified. K-values from vapour pressure over system pressure (Raoult to start),
  vapour fraction from a Rachford-Rice solve, outlets vapour first then liquid. Needs the composite key, the latent
  heat and the propagated pressure above. This is also the op that finally makes the rayon item measurable: CLAUDE.md
  puts a flash at ~20x a mixer ideal and ~200x with a cubic EOS, against the ~90 ns unit evaluation that currently
  makes parallelism pure overhead.
- [ ] Adiabatic flash, `P` and `H` specified. Newton on temperature around the isothermal flash, so it is an outer
  loop over an inner loop inside one `evaluate` - the same structure as `mix` over `solve_temperature`, one level
  deeper. A flash drum on a recycle is the test that proves the three tolerances compose.
- [ ] Heat exchanger. Two inlets and two outlets with one duty shared between them, specified as a UA plus an
  arrangement (counter- or co-current LMTD), an outlet temperature, or a duty - the first op whose two sides are
  coupled only through energy and not through flows, which is a new shape for `evaluate`. Declares a pressure drop per
  side, so it waits on the pressure item.
- [ ] Screen, and size classes. A partition curve over size fractions is structurally `Flotation` with a different
  name - one inlet, two outlets, a dense per-species vector - so the op is not the work. The work is that a size
  fraction has to become a species (`"Quartz(150um)"`), which is the composite key item generalised from phase to an
  arbitrary qualifier. Decide whether a key carries one qualifier or a list before writing the op.
- [ ] Design constraints. Vary a unit parameter until a stream quantity hits a target - Aspen's Design Spec, a secant
  or Newton loop wrapped around `Solver::solve` rather than inside it. `solve_with` and `SolveEvent` already provide
  the progress hook; what is new is that a flowsheet parameter becomes an unknown, which means naming a parameter
  generically across an open set of ops.
- [ ] Scenario runs and Monte Carlo over feed uncertainty. Many independent solves of one flowsheet with perturbed
  feeds, summarised by percentile rather than printed one by one. Embarrassingly parallel at the *solve* level, which
  is a far better rayon target than a wave of units and should be benched as such.
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off. Gated
  on an op with an inner solve (the flash above) - see CLAUDE.md for why the measurement is meaningless before that,
  and for the finding that making allocation cheaper made parallelism worse rather than better.
- [ ] README, then MIT/Apache-2.0 dual licence and publish to crates.io. The README covers what it is, quickstart,
  JSON format, one worked example.
    - Show `flowsheet --json circuit.json > solved.json` in the quickstart. There is deliberately no `--output` flag:
      the shell already does it, and `--json` output is tested to reload as a legal input. Saying so keeps the
      omission reading as a decision rather than a gap.
    - Both manifests need `description`, `license` and `repository` first, and `flowsheet-cli`'s path dependency needs
      a `version` next to the `path`. Cargo ignores that field for local builds — the path always wins inside a
      workspace — and only writes it into the uploaded manifest, so it looks redundant right up until
      `cargo publish` refuses the crate without it. `cargo publish -p <crate> --dry-run` lists every missing piece.
- [ ] Dynamic simulation — inventory/holdup, fixed-step integrator. Two things wait here because neither has a
  steady-state meaning: a kinetic reactor, which needs a residence time, and the hydraulic network solve that makes
  pressures and flows simultaneous unknowns.
- [ ] C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3? Ratatui? indicatif

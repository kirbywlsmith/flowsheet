# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `crates/flowsheet-cli/src/main.rs`.

Each item is meant to ship on its own: all four commands in CLAUDE.md clean, tests included. Where an item leaves
something out deliberately, the item that picks it up is named - nothing below is a reduction in what the library is
eventually for.

Packaging comes first, ahead of the domain work, because it is the only part of this list that changes whether any of
the rest is ever seen. Then the small correctness gaps, which are cheap and self-contained. Then the process
engineering, ordered so that no large item gates a small one.

- [ ] README: what it is, quickstart, the JSON format, one worked example. The worked example is the demo circuit,
  because `crates/flowsheet-cli/tests/cli.rs` already pins its table output byte-for-byte, so a README that pastes
  that table cannot drift silently.
    - Write the input document in its *hand-written* form, not the saved form. `serial::Stream::state` is
      `#[serde(default)]`, so a stream is `{"from": "feed", "to": "mixer"}` and nothing else - the six `"flows": {}`
      blocks in `tests/fixtures/recycle.json` are save output, not input. Nobody will work that out from the fixture.
    - Show `flowsheet --json circuit.json > solved.json` in the quickstart. There is deliberately no `--output` flag:
      the shell already does it, and `--json` output is tested to reload as a legal input. Saying so keeps the
      omission reading as a decision rather than a gap.
    - `cargo install flowsheet-cli`, not `flowsheet` - the library owns the plain name and installs no binary.
    - Say plainly what is *not* modelled yet: no phase change, no reactions, pressure carried but never solved. A
      stated scope reads as engineering judgement; an unstated one reads as an unfinished project.
- [ ] Publish 0.1.0 to crates.io, the library first and the CLI second. That order is forced, not preferred:
  packaging rewrites `flowsheet-cli`'s path dependency into a registry one, so until `flowsheet 0.1.0` actually
  exists on crates.io, `cargo publish -p flowsheet-cli --dry-run` fails at resolution with `no matching package
  named flowsheet`. The library's own dry-run is clean and is the only one that can be run beforehand; the CLI's is
  a post-publish check, not a gate. Irreversible in a way nothing else here is - a published version can be yanked
  but never replaced. Afterwards add the docs.rs and crates.io badges to the README, and set the repository
  description and topics on GitHub. None of that is checkable by the four commands in CLAUDE.md, which is exactly
  why it needs to be written down as an item.
- [ ] Conversion reactor, isothermal, one reaction. `Reaction { stoichiometry: Vec<f64>, limiting: SpeciesId,
  conversion: f64 }` with one inlet and one outlet - structurally `Flotation`'s sibling: a dense `Vec<f64>` in
  `SpeciesId` order validated by a `unit::react` free function, composition changed by transforming species rather
  than partitioning them. Coefficients are **molar** and signed (negative consumed, positive produced), because that
  is how a reaction is written; `react` converts with `molar_mass` and rejects a stoichiometry whose mass balance does
  not close (`sum of nu_i * M_i ~ 0`), which is the one way a stoichiometry can be wrong that a recovery vector
  cannot. Extent is `conversion * n_limiting / |nu_limiting|`. On the wire, a species-name map like `recovery`, but
  zero coefficients are **dropped** on save rather than kept: a zero recovery says a species does not float, while a
  reaction equation simply lists its participants. The outlet leaves at the inlet temperature, which means an
  exothermic reaction silently breaks the energy balance until the reactor energy balance item lands - say so in the
  doc comment, and make the test assert the extent by hand the way `demo::balance` does.
- [ ] An absolute enthalpy basis. Every enthalpy today is relative to `thermo::REFERENCE_K` and the zero point
  cancels, which stops being true the moment a species is destroyed: bring Shomate's `F` and `H` back
  (`deny_unknown_fields` currently rejects them) and add a standard enthalpy of formation per species. No unit
  operation changes here - this is the property data and the `thermo`/`Species` functions over it, which is what
  makes it testable on its own. Reference-state consistency is the thing to prove: an inert species' enthalpy must
  not move, and every existing energy balance test must give an identical answer, because a basis shift that cancels
  out is the claim.
- [ ] The reactor's energy balance, on the basis above. `H_out - H_in` *is* the heat of reaction once enthalpies are
  absolute, so there is no separate heat-of-reaction parameter; what the reactor gains instead is a spec - isothermal
  reports the duty it needed, adiabatic hands `H_in` to `unit::solve_temperature` the way `heat` already does. Two
  tests earn their keep. A cycle of reactions whose enthalpies sum to zero proves the basis is consistent; an
  exothermic reaction raising its own outlet temperature proves the reactor reads it.
- [ ] Several reactions in one reactor. `Vec<Reaction>`, extents applied in declared order, with the limiting
  reactant's moles re-read between reactions so that two reactions competing for one reactant are well defined rather
  than over-consuming it. Aspen's `RStoic` is the shape. Mostly a loop plus a validation that no species goes
  negative.
- [ ] Composite species key on the wire, e.g. `"H2O(g)"`. `SpeciesRegistry::find` already keys on name *and* phase,
  but the JSON flows map is name-only, so the same name in two phases is rejected on both boundaries
  (`LoadError::DuplicateSpecies` going in, `FlowsheetError::DuplicateSpeciesName` in `check` coming out). Parse and
  format the phase suffix, drop both errors, and keep the bare name valid for the single-phase case so every existing
  document still loads byte-for-byte. Small, standalone, and a hard prerequisite for both phase change and a
  size-classified screen.
- [ ] Pressure, declared per op and propagated. Nothing reads it and nothing solves it - `Stream::add_assign` says so,
  and the field survives a whole solve untouched. Aspen Plus and HYSYS specify it at this level too: each op declares
  a pressure drop, `evaluate` writes the outlet pressure, and a mixer takes the minimum inlet pressure rather than
  the first. Solving pressure *properly* makes flows and pressures simultaneous unknowns - that is the hydraulic
  network problem, and it waits for the dynamic item.
- [ ] `Pump` and `Compressor`: add head, and account for the duty it costs. One inlet and one outlet each, sitting on
  the pressure field the item above made meaningful. A pump on an incompressible stream is `V * dP / efficiency` and
  is nearly free once pressure propagates. A compressor needs a compression path - isentropic with an efficiency is
  the usual shape - and the ideal-gas case falls out of the Shomate `cp` already stored, so neither of them waits on
  new property data.
- [ ] Latent heat. Enthalpy of vaporisation per species plus a vapour pressure correlation (Antoine is the usual
  three-coefficient fit), measured against the absolute basis established above. No flash yet - this is the property
  data and the `thermo` functions over it, testable on their own against steam-table values.
- [ ] Isothermal flash, `T` and `P` specified. K-values from vapour pressure over system pressure (Raoult to start),
  vapour fraction from a Rachford-Rice solve, outlets vapour first then liquid. Needs the composite key, the latent
  heat and the propagated pressure above. This is also the op that finally makes the rayon item measurable: CLAUDE.md
  puts a flash at ~20x a mixer ideal and ~200x with a cubic EOS, against the ~90 ns unit evaluation that currently
  makes parallelism pure overhead.
- [ ] Adiabatic flash, `P` and `H` specified. Newton on temperature around the isothermal flash, so it is an outer
  loop over an inner loop inside one `evaluate` - the same structure as `mix` over `solve_temperature`, one level
  deeper. A flash drum on a recycle is the test that proves the three tolerances compose.
- [ ] Equilibrium reactor. Gibbs free energy minimisation over the declared species, which needs entropy and so
  brings Shomate's `G` back alongside a standard entropy of formation. The largest item on this list, and
  deliberately behind the flashes rather than in front of them: constrained minimisation under element balances
  (Lagrange multipliers, or RAND) is a hard inner solve, and by this point the adiabatic flash has already
  settled how an inner iteration nests inside `evaluate` and how its tolerance sits against the outer solver's -
  `solve_temperature` set that precedent at 1e-9 K. Nothing after it depends on it, so it can slip without blocking
  anything.
- [ ] Heat exchanger. Two inlets and two outlets with one duty shared between them, specified as a UA plus an
  arrangement (counter- or co-current LMTD), an outlet temperature, or a duty - the first op whose two sides are
  coupled only through energy and not through flows, which is a new shape for `evaluate`. Declares a pressure drop
  per side, so it waits on the pressure item.
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
- [ ] Dynamic simulation — inventory/holdup, fixed-step integrator. Two things wait here because neither has a
  steady-state meaning: a kinetic reactor, which needs a residence time, and the hydraulic network solve that makes
  pressures and flows simultaneous unknowns.
- [ ] C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3? Ratatui? indicatif

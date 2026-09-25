# TODO

Rough sequential order.

Each item is meant to ship on its own: fmt, clippy, tests and docs clean (what CI runs), tests included. Where an item leaves
something out deliberately, the item that picks it up is named - nothing below is a reduction in what the library is
eventually for.

The process engineering is ordered so that no large item gates a small one.

- [ ] Non-ideal vapour-liquid equilibrium. Raoult's law is wrong for almost every mixture anyone flashes - ethanol
  and water form an azeotrope it cannot see. Add an activity-coefficient model for the liquid, Wilson or NRTL, with
  binary interaction parameters declared per species pair in the document, so `K_i = gamma_i * Psat_i / P`. The
  Rachford-Rice loop is unchanged; `gamma` now depends on the liquid composition, so the flash gains an inner
  fixed-point iteration on `x`. Test against the published ethanol-water azeotrope. A cubic equation of state for
  the vapour side is left out on purpose: it is a second model of the same size, and at atmospheric pressure the
  vapour is near enough ideal that the liquid side is where the error lives.
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
  per side, the same `pressure_drop` every other op carries.
- [ ] Distillation column. A stack of `N` equilibrium stages with a condenser and reboiler, specified by stage count,
  feed stage, reflux ratio and distillate rate - the op people judge a simulator by, and the one that makes a
  flowsheet look like a plant. Each stage is the isothermal flash above, coupled to its neighbours by the liquid
  going down and the vapour going up, so the whole column is one inner solve over `N` temperatures and `N`
  vapour flows (bubble-point method to start; Newton on all of it later if it is slow). Needs non-ideal VLE to give
  a believable answer on anything but a near-ideal pair, and it is the ~5,000x-a-mixer op [docs/design.md](docs/design.md#when-parallelism-will-pay) names as what
  finally makes the rayon item worth benching.
- [ ] Screen, and size classes. A partition curve over size fractions is structurally `Flotation` with a different
  name - one inlet, two outlets, a dense per-species vector - so the op is not the work. The work is that a size
  fraction has to become a species (`"Quartz(150um)"`), which is the composite key item generalised from phase to an
  arbitrary qualifier. Decide whether a key carries one qualifier or a list before writing the op.
- [ ] Design constraints. Vary a unit parameter until a stream quantity hits a target - Aspen's Design Spec, a secant
  or Newton loop wrapped around `Solver::solve` rather than inside it. `solve_with` and `SolveEvent` already provide
  the progress hook; what is new is that a flowsheet parameter becomes an unknown, which means naming a parameter
  generically across an open set of ops.
- [ ] `--csv` output: one row per stream with a column per species key, temperature and pressure, and a second
  block for unit duties. `--json` is a document and the table is for eyes; neither pastes into a spreadsheet, which
  is where a solved flowsheet usually goes next. Same shape as `--json` - stdout, and the shell owns the file. It
  sits here rather than earlier because the scenario item below wants a machine-readable row format for its
  percentile summary, and this is that format.
- [ ] Scenario runs and Monte Carlo over feed uncertainty. Many independent solves of one flowsheet with perturbed
  feeds, summarised by percentile rather than printed one by one. Embarrassingly parallel at the *solve* level, which
  is a far better rayon target than a wave of units and should be benched as such.
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off. Gated
  on an op with an inner solve (the flash above) - see [docs/design.md](docs/design.md#performance) for why the
  measurement is meaningless before that, and for the finding that making allocation cheaper made parallelism worse rather than better.
- [ ] Dynamic simulation — inventory/holdup, fixed-step integrator. Two things wait here because neither has a
  steady-state meaning: a kinetic reactor, which needs a residence time, and the hydraulic network solve that makes
  pressures and flows simultaneous unknowns.
- [ ] C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke the solver, with a UI on top of that. The UI toolkit is
  undecided.

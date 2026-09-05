# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `src/main.rs`.

- [ ] Rust book ch. 5-17
- [ ] Rename `SolveError::Cycle` — a cycle is no longer an error, so "flowsheet contains a cycle through N units"
  misreports what happened. It now fires only when `tear_streams` returned too few tears to order the graph, which
  today means interlocking loops inside one SCC. Something like `Untearable`, plus the `Display` message, the
  `solve` doc line, and the `cycle_message_counts_the_units_it_could_not_order` test. Do it before serde and the
  CLI freeze the public surface.
- [ ] serde derive on species/streams/units/flowsheet; JSON load + save; round-trip test
- [ ] clap CLI: `flowsheet <flowsheet.json>`
    - can pass args to determine output format (print to console, save to a file, specify which values to
      include/ignore)
- [ ] Split into a workspace: `crates/flowsheet-core` (no CLI deps) and `crates/flowsheet-cli`
- [ ] Convert `UnitOp` enum to `Box<dyn UnitOp>` trait objects; write up the tradeoff (dispatch cost, open extension) in
  the README
- [ ] Flotation cell unit: 1 in / 2 out, per-species recovery; replace the tank and re-derive the expected numbers
- [ ] Criterion benches: solve time vs unit count, vs tear count, enum vs trait-object dispatch
- [ ] Energy balance: per-species enthalpy `h(T)`, mixer outlet T by hand-rolled Newton solve, convergence tests
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off
- [ ] README: what it is, quickstart, JSON format, one worked example, rename project 'flowsheet'
- [ ] MIT/Apache-2.0 dual licence, `cargo fmt`/`clippy -D warnings` in GitHub Actions, publish to crates.io
- [ ] Later: better tear heuristics. `tear_streams` takes the lowest-id stream inside each SCC, one per component.
  Both parts cost passes: the tear set is the solver's unknown vector, so extra tears mean more to converge, and
  interlocking loops currently return too few tears to order at all. Prefer Tarjan's back edge (the recycle, whose
  zero guess is physically sensible) and tear repeatedly until the graph orders. A minimal set is minimum feedback
  arc set — NP-hard, hence heuristics.
- [ ] Later: more unit ops (heat exchanger, reactor, screen) and design constraints
- [ ] Later: allow streaming solve events (e.g. `UnitEvaluated { id, iteration, residual }`) to subscribers during a run
- [ ] Later: dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] Later: scenario runs and Monte Carlo over feed uncertainty
- [ ] Later: C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3?

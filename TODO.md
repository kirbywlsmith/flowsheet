# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `src/main.rs`.

- [ ] Split into a workspace: `crates/flowsheet-core` (no CLI deps) and `crates/flowsheet-cli`
- [ ] Convert `UnitOp` enum to `Box<dyn UnitOp>` trait objects; write up the tradeoff (dispatch cost, open extension) in
  the README
- [ ] Flotation cell unit: 1 in / 2 out, per-species recovery; replace the tank and re-derive the expected numbers
- [ ] Criterion benches: solve time vs unit count, vs tear count, enum vs trait-object dispatch
- [ ] Energy balance: per-species enthalpy `h(T)`, mixer outlet T by hand-rolled Newton solve, convergence tests.
  Blocked first by the name-keyed JSON flows map: it rejects `H2O` liquid + `H2O` gas, so no phase change until the key
  becomes composite (e.g. `"H2O(g)"`).
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off
- [ ] README: what it is, quickstart, JSON format, one worked example, rename project 'flowsheet'
    - Show `flowsheet --json circuit.json > solved.json` in the quickstart. There is deliberately no `--output` flag:
      the shell already does it, and `--json` output is tested to reload as a legal input. Saying so keeps the
      omission reading as a decision rather than a gap.
- [ ] MIT/Apache-2.0 dual licence, `cargo fmt`/`clippy -D warnings` in GitHub Actions, publish to crates.io
- [ ] Later: better tear heuristics. `tear_streams` takes the lowest-id stream inside each SCC, one per component. Both
  parts cost passes: the tear set is the solver's unknown vector, so extra tears mean more to converge, and interlocking
  loops currently return too few tears to order at all. Prefer Tarjan's back edge (the recycle, whose zero guess is
  physically sensible) and tear repeatedly until the graph orders. A minimal set is minimum feedback arc set — NP-hard,
  hence heuristics.
- [ ] Later: more unit ops (heat exchanger, reactor, screen) and design constraints
- [ ] Later: allow streaming solve events (e.g. `UnitEvaluated { id, iteration, residual }`) to subscribers during a run
- [ ] Later: dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] Later: scenario runs and Monte Carlo over feed uncertainty
- [ ] Later: C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3?

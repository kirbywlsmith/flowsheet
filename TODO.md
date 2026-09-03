# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `src/main.rs`.

- [ ] Rust book ch. 5-17
- [ ] Tear stream selection: pick one stream per SCC (start with "first stream into the SCC", note why heuristics
  matter)
- [ ] Direct substitution loop: guess tear = zeros, iterate to convergence
- [ ] Test: error shrinks by exactly f each pass; ~14 iterations at f=0.3, ~130 at f=0.9
- [ ] Wegstein acceleration with clamped q; test it beats direct substitution at f=0.9
- [ ] ✅ Checkpoint: recycle circuit converges, internal amplification = 1/ (1-f)
- [ ] serde derive on species/streams/units/flowsheet; JSON load + save; round-trip test
- [ ] clap CLI: `simulate <flowsheet.json>`, printed per-stream balance table
- [ ] Split into a workspace: `crates/sim-core` (no CLI deps) and `crates/sim-cli`
- [ ] Convert `UnitOp` enum to `Box<dyn UnitOp>` trait objects; write up the tradeoff (dispatch cost, open extension) in
  the README
- [ ] Flotation cell unit: 1 in / 2 out, per-species recovery; replace the tank and re-derive the expected numbers
- [ ] Criterion benches: solve time vs unit count, vs tear count, enum vs trait-object dispatch
- [ ] Energy balance: per-species enthalpy `h(T)`, mixer outlet T by hand-rolled Newton solve, convergence tests
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off
- [ ] README: what it is, quickstart, JSON format, one worked example, rename project 'flowsheet'
- [ ] MIT/Apache-2.0 dual licence, `cargo fmt`/`clippy -D warnings` in GitHub Actions, publish to crates.io
- [ ] Later: more unit ops (heat exchanger, reactor, screen) and design constraints
- [ ] Later: allow streaming solve events (e.g. `UnitEvaluated { id, iteration, residual }`) to subscribers during a run
- [ ] Later: dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] Later: scenario runs and Monte Carlo over feed uncertainty
- [ ] Later: C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3?

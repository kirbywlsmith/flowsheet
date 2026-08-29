# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `src/main.rs`.

- [ ] Rust book ch. 1-10 (ownership, borrows, generics, traits) — enough to start; finish ch. 13-17 while building
- [ ] `Stream` helpers: `total()`, `mass_fractions()`, `add_assign`, `scale`
- [ ] Mixer and splitter as plain free functions over `&[Stream]` — no traits, no graph yet
- [ ] Tests: mixer and splitter close on mass per species and in total (`assert_relative_eq`, pick a tolerance and stick
  to it)
- [ ] Arena in `flowsheet.rs`: `Vec<UnitOp>`, `Vec<Stream>`, `UnitId`/`StreamId` new types, units store inlet/outlet
  `StreamId`s
- [ ] `UnitOp` as an enum first (Feed, Mixer, Tank, Splitter, Product) with a `fn evaluate(&self, ...)` match — trait
  objects come later, deliberately
- [ ] `build_flowsheet()` wiring the target circuit by hand, minus the recycle (S4 deleted, mixer takes only S0)
- [ ] Kahn topological sort, hand-rolled — in-degree count, queue, detect leftover nodes
- [ ] Sequential-modular solve over the topo order; acyclic case only
- [ ] ✅ Checkpoint: acyclic flowsheet solves, `S3 == S0` to 1e-9
- [ ] Error type: hand-rolled enum + `Display` + `std::error::Error` (do it manually once before reaching for
  `thiserror`)
- [ ] Kill every `unwrap`/`panic` in library code; solver returns `Result`
- [ ] Add S4 back; cycle detection via DFS colouring or Tarjan SCC — written by hand
- [ ] Tear stream selection: pick one stream per SCC (start with "first stream into the SCC", note why heuristics
  matter)
- [ ] Direct substitution loop: guess tear = zeros, iterate to convergence
- [ ] `SolverConfig` (tolerance, max iterations) + `SolveReport` (iterations, final residual, converged flag)
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
- [ ] README: what it is, quickstart, JSON format, one worked example
- [ ] MIT/Apache-2.0 dual licence, `cargo fmt`/`clippy -D warnings` in GitHub Actions, publish to crates.io
- [ ] Later: more unit ops (heat exchanger, reactor, screen) and design constraints
- [ ] Later: dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] Later: scenario runs and Monte Carlo over feed uncertainty
- [ ] Later: C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that

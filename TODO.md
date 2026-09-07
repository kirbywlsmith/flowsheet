# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `crates/flowsheet-cli/src/main.rs`.

- [ ] Criterion benches: solve time vs unit count, vs tear count, and trait-object dispatch against the enum
  baseline (recover the enum from commit `1612492`, behind a bench-only module)
- [ ] Energy balance: per-species enthalpy `h(T)`, mixer outlet T by hand-rolled Newton solve, convergence tests.
  Blocked first by the name-keyed JSON flows map: it rejects `H2O` liquid + `H2O` gas, so no phase change until the key
  becomes composite (e.g. `"H2O(g)"`).
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off
- [ ] README: what it is, quickstart, JSON format, one worked example.
    - Show `flowsheet --json circuit.json > solved.json` in the quickstart. There is deliberately no `--output` flag:
      the shell already does it, and `--json` output is tested to reload as a legal input. Saying so keeps the
      omission reading as a decision rather than a gap.
- [ ] MIT/Apache-2.0 dual licence, then publish to crates.io. The name `flowsheet` was unclaimed as of 2026-09-06.
    - Both manifests need `description`, `license` and `repository` first, and `flowsheet-cli`'s path dependency needs
      a `version` next to the `path`. Cargo ignores that field for local builds — the path always wins inside a
      workspace — and only writes it into the uploaded manifest, so it looks redundant right up until
      `cargo publish` refuses the crate without it. `cargo publish -p <crate> --dry-run` lists every missing piece.
- [ ] Later: better tear heuristics. `tear_streams` takes the lowest-id stream inside each SCC, one per component. Both
  parts cost passes: the tear set is the solver's unknown vector, so extra tears mean more to converge, and interlocking
  loops currently return too few tears to order at all. Prefer Tarjan's back edge (the recycle, whose zero guess is
  physically sensible) and tear repeatedly until the graph orders. A minimal set is minimum feedback arc set — NP-hard,
  hence heuristics.
- [ ] Later: let a downstream unit op round-trip through a document. `Box<dyn UnitOp>` opened the set of ops the
  solver accepts, but not the set the wire format can carry: `serial::UnitOp` is a closed enum, so a third-party op
  has no variant of its own and `ToDocument` forces it to describe itself as one of the six built-ins — which
  `to_domain` then loads back as that built-in, silently, with no error at either boundary. The fix is a name-keyed
  registry: `ToDocument` returns a `&'static str` tag plus a `serde_json::Value` payload rather than an enum variant,
  and loading looks the tag up in a table the caller can extend. Two costs to weigh first — `serde_json` stops being a
  dev-dependency of the library, and `deny_unknown_fields` no longer fires for free on an op's own parameters, so each
  op has to opt back in. Not worth doing until something outside this crate actually implements `UnitOp`; until then
  the honest position is documented on `serial::ToDocument`.
- [ ] Later: chemical reactions. Carved out of the unit-op item below, because a reactor is not one more op like
  a screen: it needs reaction stoichiometry, a conversion or extent-of-reaction spec, and enthalpies of formation
  feeding the energy balance above. Three models in ascending cost: conversion (specify fractional conversion of a
  limiting reactant), equilibrium (Gibbs free energy minimisation), kinetic (rate laws, plus the residence time that
  only the dynamic-simulation item below can supply). Start with conversion. It is isothermal, so it needs none of
  the energy balance, and it is structurally the sibling of `Flotation`: one inlet, a dense `Vec<f64>` parameter in
  `SpeciesId` order validated by a free function, composition changed by transforming species rather than
  partitioning them. That makes it the cheap second test of whether the op set is really open.
- [ ] Later: more unit ops (heat exchanger, screen) and design constraints
- [ ] Later: allow streaming solve events (e.g. `UnitEvaluated { id, iteration, residual }`) to subscribers during a run
- [ ] Later: dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] Later: scenario runs and Monte Carlo over feed uncertainty
- [ ] Later: C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3?

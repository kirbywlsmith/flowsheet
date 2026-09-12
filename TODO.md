# TODO

Rough sequential order. Target flowsheet + expected numbers live in the header comment of `crates/flowsheet-cli/src/main.rs`.

- [ ] Chemical reactions. Carved out of the unit-op item below, because a reactor is not one more op like
  a screen: it needs reaction stoichiometry, a conversion or extent-of-reaction spec, and enthalpies of formation
  for the energy balance - `thermo` drops the Shomate `F` and `H` terms that carry them, and they have to come back.
  Three models in ascending cost: conversion (specify fractional conversion of a
  limiting reactant), equilibrium (Gibbs free energy minimisation), kinetic (rate laws, plus the residence time that
  only the dynamic-simulation item below can supply). Start with conversion. Held isothermal it can ignore the heat
  of reaction to begin with, and it is structurally the sibling of `Flotation`: one inlet, a dense `Vec<f64>` parameter in
  `SpeciesId` order validated by a free function, composition changed by transforming species rather than
  partitioning them. That makes it the cheap second test of whether the op set is really open.
- [ ] Pressure. Nothing reads it and nothing solves it - `Stream::add_assign` says so in its doc comment, and the
  field survives a whole solve untouched. Aspen Plus and HYSYS specify it rather than solve it at this level too:
  each op declares a pressure drop and pressure propagates downstream, one more field `evaluate` writes. Solving it
  properly makes flows and pressures simultaneous unknowns, which is the hydraulic network problem and belongs with
  the dynamic item below. Wait until an op reads pressure. A pump adds head, a heat exchanger declares a drop, and
  a flash cannot pick a phase without both P and T. Until one of those lands, `pressure` is a label the documents
  carry and the solver ignores.
- [ ] Phase change. Latent heat needs `H2O` liquid and `H2O` gas in one flowsheet, and the name-keyed JSON flows
  map rejects that on both boundaries, so the key has to become composite first (e.g. `"H2O(g)"`). A flash also needs
  pressure - see the item above.
- [ ] More unit ops (heat exchanger, screen) and design constraints
- [ ] Scenario runs and Monte Carlo over feed uncertainty
- [ ] Rayon parallel solve of independent branches; bench against serial and record where it stops paying off
- [ ] README, then MIT/Apache-2.0 dual licence and publish to crates.io. The README covers what it is, quickstart,
  JSON format, one worked example.
    - Show `flowsheet --json circuit.json > solved.json` in the quickstart. There is deliberately no `--output` flag:
      the shell already does it, and `--json` output is tested to reload as a legal input. Saying so keeps the
      omission reading as a decision rather than a gap.
    - Both manifests need `description`, `license` and `repository` first, and `flowsheet-cli`'s path dependency needs
      a `version` next to the `path`. Cargo ignores that field for local builds — the path always wins inside a
      workspace — and only writes it into the uploaded manifest, so it looks redundant right up until
      `cargo publish` refuses the crate without it. `cargo publish -p <crate> --dry-run` lists every missing piece.
- [ ] Dynamic simulation — inventory/holdup, fixed-step integrator
- [ ] C ABI (`extern "C"` + `cbindgen`) so .NET can P/Invoke it; UI on top of that - WinUI 3? Ratatui? indicatif

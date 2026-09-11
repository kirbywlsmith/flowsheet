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
- Saving is a **`serial::ToDocument` supertrait on `UnitOp`**, not a `match` in `serial.rs`. Loading can stay a match —
  a document names its op with a string, and something must own the name-to-constructor table — but saving cannot,
  because `Box<dyn UnitOp>` has erased the concrete type. The alternative, downcasting through `Any`, would put a closed
  list of types back in `serial.rs` and give up exactly the open set the trait object was adopted for. The `impl` blocks
  still live in `serial.rs`, so the wire format remains one module; only the trait's declaration leaks into `unit.rs`.
  The open set stops at the wire format, though: `serial::UnitOp` is a closed enum, so a downstream op can be *solved*
  but not *round-tripped* — it has no variant of its own to return. Widening that means a name-keyed constructor
  registry, and nothing needs one yet.
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
  duty on an early pass carrying a sliver of the flow can drive Newton below 0 K and panic, which is user input
  surfacing as a panic mid-solve - the same hole as a Shomate fit that goes negative away from 298.15 K. There is no
  `LoadError` for a non-finite duty because JSON cannot write one; `serde_json` rejects `NaN`, `Infinity` and `1e400`,
  and a test pins that. `tests/heated_recycle.rs` asserts `T_product = T_feed + Q / C_feed` by hand, independent of
  the recycle fraction.
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
  a silently dropped key. Two serde traps make that harder than one attribute: `deny_unknown_fields` is illegal with
  `#[serde(flatten)]`, and it is silently *ignored* on an internally tagged enum. So `serial::Unit` nests `op` instead
  of flattening it, and `serial::UnitOp`'s variants are newtypes over per-op spec structs (`SplitterSpec`, and `NoSpec`
  for the parameterless ops) that each deny unknown fields themselves. A newtype variant is deserialised as a plain
  struct with the `type` key already stripped, so its own attribute fires. The JSON shape is unchanged by all of this.
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

## Layout

Cargo workspace. The root `Cargo.toml` is a **virtual manifest** — `[workspace]`, no `[package]` — so `cargo test`
and `cargo clippy` at the root cover every member.

```
crates/flowsheet/       the library: domain types, solver, serial, report. Depends on serde only.
  src/unit.rs           the `UnitOp` trait, `Arity`, `Unit`, and the free functions ops are built from.
  src/unit/             one file per operation, re-exported flat from `unit.rs`.
  src/thermo.rs         Shomate heat capacity and enthalpy, per mole. `Species` converts to per kg.
  benches/solve.rs      criterion, `harness = false`. Solve time vs unit count and vs tear count.
crates/flowsheet-cli/   the `flowsheet` binary: clap parsing, file IO, error printing.
```

- The split exists so **`clap` stays out of the library's dependency graph**. Cargo has no per-target dependencies,
  so in a single crate the binary's deps are also the library's, for everyone downstream.
- `serde_json` is a **dev-dependency** of `flowsheet`, not a dependency. Every `serde_json` call in the library
  is inside a `#[cfg(test)]` module — the wire types carry the derives, and turning them into bytes is the caller's
  job. The CLI depends on it for real.
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

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
- Enum dispatch for unit ops first, trait objects later — the comparison is the point.
- Outlet order is positional: the Nth `add_stream` from a unit matches the Nth stream from `UnitOp::evaluate`. Wiring a
  splitter backwards still balances mass, so it fails silently.
- Flowsheets are built loosely then validated: `Flowsheet::validate` consumes `self` and hands back a `ValidFlowsheet`,
  which is the only thing `Solver::solve` accepts. Illegal-states-unrepresentable does not work here — a recycle forces
  a unit to be referenced before its upstream exists, and arity is only decidable once construction stops.
  `ValidFlowsheet` has `Deref` but deliberately no `DerefMut`, so the topology cannot change behind the validation.
- Kahn's topological sort emits **waves** (`Vec<Vec<UnitId>>`), not a flat order, so parallelism is free later.
- Float equality: never `==`. `approx` (dev-dependency) in tests; `Stream::max_flow_residual` for the solver.
- JSON is a **separate wire format** in `src/serial.rs`, not serde attributes on the domain types. `serial::Flowsheet`
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
  drop one of the two flows. Revisit with a composite key (`"H2O(g)"`) when the energy balance lands.
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
- The **table formatter lives in the library** (`src/report.rs`), not in `main.rs`. A stream's
  endpoints are only recoverable from the `inlets`/`outlets` of the units that list it, and those
  are `pub(crate)` — `main.rs` is a separate crate and cannot see them. Keeping the formatter in
  the library avoids widening `Unit`'s public surface just to compute a display column, and since
  it has no `clap` dependency it moves into `flowsheet-core` unchanged at the workspace split.
- Stream labels are `{from}.{to}`, deduped with a **single running counter**: the first occurrence
  stays bare, repeats become `#2`, `#3`. No counting pre-pass is needed, because only occurrences
  after the first are ever suffixed.
- `main` deliberately does **not** return `Result`. Rust prints a returned error with `Debug`, not
  `Display`, so every message these error types carefully write would surface as a struct literal.
  Instead `main` calls `run()`, prints `{e}` to stderr, and exits 1.
- The CLI has **no `--output` flag**. `--json` writes a whole document to stdout and a test proves it reloads as a
  legal input, so `> solved.json` is the supported way to save one — the shell already owns that job. Column
  include/ignore flags were dropped from the original clap item for the same reason they were never missed: nothing
  needs them yet.

## Testing

- Unit tests in-file (`#[cfg(test)] mod tests`) — can see private items.
- `tests/` for end-to-end flowsheet solves, and for running the binary over a document in `tests/fixtures/`.
  `recycle.json` is the demo circuit saved as a document; `tests/cli.rs` asserts it byte-for-byte against
  `serial::Flowsheet::from(&demo::build_flowsheet())`, so it cannot drift from the demo.
- Every completed TODO item ships with tests. Demo and example code is illustrative and exempt.

## Commands

```bash
cargo test
cargo clippy --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

All three must be clean before removing a TODO item.

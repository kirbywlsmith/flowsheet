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

## Testing

- Unit tests in-file (`#[cfg(test)] mod tests`) — can see private items.
- `tests/` for end-to-end flowsheet solves. Empty until the solver exists.
- Every completed TODO item ships with tests. Demo and example code is illustrative and exempt.

## Commands

```bash
cargo test
cargo clippy --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

All three must be clean before removing a TODO item.

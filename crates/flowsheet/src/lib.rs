#![warn(missing_docs)]

//! Steady-state material balances over a flowsheet: a directed graph of process units
//! connected by streams, recycles included.
//!
//! Build a [`Flowsheet`] loosely, turn it into a [`ValidFlowsheet`] with
//! [`Flowsheet::validate`], and hand that to a [`Solver`]. Rendering is [`report::table`];
//! the JSON wire format lives in [`serial`].

pub mod demo;
pub mod flowsheet;
pub mod report;
pub mod serial;
pub mod solver;
pub mod species;
pub mod stream;
pub mod thermo;
pub mod unit;

#[cfg(test)]
mod test_support;

// The headline types are re-exported here so callers write `flowsheet::Flowsheet` rather than
// `flowsheet::flowsheet::Flowsheet` - the crate and its main module share a name, and the
// stutter shows up at every import. The modules stay public, so the long paths keep working.
//
// `serial` is deliberately left out. Its `Flowsheet`, `Unit`, `UnitOp` and `Stream` are the
// wire format, told apart from the domain types by module path alone; re-exporting them here
// would collide with the names below and erase that distinction.
//
// Free functions are left out too: `unit::mix(..)` and `report::table(..)` read better with
// the module than a bare `mix` would.
pub use crate::flowsheet::{Flowsheet, FlowsheetError, StreamId, UnitId, ValidFlowsheet};
pub use crate::solver::{
    ConvergenceMethod, SolveError, SolveEvent, SolveReport, Solver, SolverConfig,
};
pub use crate::species::{Phase, Species, SpeciesId, SpeciesRegistry};
pub use crate::stream::Stream;
pub use crate::thermo::Shomate;
pub use crate::unit::{
    Arity, Feed, Flotation, Heater, Mixer, Product, Splitter, SplitterN, Tank, Unit, UnitOp,
};

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

/// The most units, streams or species one `u16` id space can address.
///
/// `UnitId`, `StreamId` and `SpeciesId` are all `u16` newtypes built by casting a `Vec` index, so
/// one limit covers all three. It lives at the crate root rather than in a module because
/// `species` sits *below* `flowsheet` in the layering - `flowsheet` uses `species`, so putting the
/// constant in `flowsheet` and importing it back down would invert that.
pub(crate) const MAX_IDS: usize = u16::MAX as usize + 1;

/// Panics if `len` is already [`MAX_IDS`], because one more push would wrap the new `u16` id
/// round to 0 and silently alias the first entry.
///
/// `what` names the collection. The message matches `serial::LoadError::TooMany` word for word:
/// that rejects an oversized *document*, this catches the same mistake made through the builder,
/// and a caller who hits one and then the other should not have to learn two phrasings.
///
/// `assert!` rather than `debug_assert!`, because release is exactly where someone generates
/// 70,000 units. A panic rather than a `Result` because the rest of the crate already treats a
/// bad id as a bug in the calling code - `Result` is reserved for real user input.
pub(crate) fn assert_id_space(len: usize, what: &str) {
    assert!(
        len < MAX_IDS,
        "{} {what} exceeds the {MAX_IDS} a u16 id can address",
        len + 1
    );
}

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
    Arity, ConversionReactor, EvalError, Feed, Flotation, Heater, Mixer, Product, Reaction,
    Splitter, SplitterN, Tank, Unit, UnitOp,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_addressable_slot_is_allowed() {
        // 65,536 entries already present would be one too many; 65,535 leaves room for the last
        // id a u16 can hold.
        assert_id_space(MAX_IDS - 1, "units");
    }

    #[test]
    #[should_panic(expected = "65537 units exceeds the 65536 a u16 id can address")]
    fn one_past_the_last_slot_panics() {
        assert_id_space(MAX_IDS, "units");
    }

    #[test]
    #[should_panic(expected = "65537 streams exceeds the 65536 a u16 id can address")]
    fn the_message_names_the_collection_that_overflowed() {
        assert_id_space(MAX_IDS, "streams");
    }
}

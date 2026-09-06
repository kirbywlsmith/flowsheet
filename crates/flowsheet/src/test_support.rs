//! Shared fixtures for tests. Compiled only under `cfg(test)`.
//!
//! Thin wrappers over [`crate::demo`] so there is one definition of the demo species.

use crate::species::{Phase, SpeciesId, SpeciesRegistry};
use crate::stream::Stream;

pub use crate::demo::{AMBIENT_K, AMBIENT_KPA};

/// The three species of the demo circuit.
pub fn demo_registry() -> SpeciesRegistry {
    crate::demo::registry()
}

/// The plant feed S0 — 40 / 360 / 600 t/h, 1000 t/h total.
pub fn feed(registry: &SpeciesRegistry) -> Stream {
    crate::demo::feed_stream(registry)
}

/// Every [`SpeciesId`] in the registry, in index order.
pub fn all_ids(registry: &SpeciesRegistry) -> Vec<SpeciesId> {
    [
        ("CuFeS2", Phase::Solid),
        ("SiO2", Phase::Solid),
        ("H2O", Phase::Liquid),
    ]
    .iter()
    .map(|(n, p)| registry.find(n, *p).expect("fixture species missing"))
    .collect()
}

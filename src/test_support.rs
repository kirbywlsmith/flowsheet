//! Shared fixtures for tests. Compiled only under `cfg(test)`.

use crate::species::{Phase, Species, SpeciesRegistry};
use crate::stream::Stream;

/// Ambient conditions used by the fixtures: 25 °C, 1 atm.
pub const AMBIENT_K: f64 = 298.15;
pub const AMBIENT_KPA: f64 = 101.325;

/// The three species of the target flowsheet: the valuable mineral, the gangue, and water.
pub fn demo_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    r.insert(Species {
        name: "CuFeS2".into(),
        phase: Phase::Solid,
        molar_mass: 183.5,
    });
    r.insert(Species {
        name: "SiO2".into(),
        phase: Phase::Solid,
        molar_mass: 60.08,
    });
    r.insert(Species {
        name: "H2O".into(),
        phase: Phase::Liquid,
        molar_mass: 18.015,
    });
    r
}

/// The plant feed S0 — 40 / 360 / 600 t/h, 1000 t/h total.
pub fn feed(registry: &SpeciesRegistry) -> Stream {
    Stream::from_flows(
        registry,
        vec![40.0, 360.0, 600.0],
        AMBIENT_K,
        AMBIENT_KPA,
    )
}

/// Every `SpeciesId` in the registry, in index order.
pub fn all_ids(registry: &SpeciesRegistry) -> Vec<crate::species::SpeciesId> {
    [
        ("CuFeS2", Phase::Solid),
        ("SiO2", Phase::Solid),
        ("H2O", Phase::Liquid),
    ]
    .iter()
    .map(|(n, p)| registry.find(n, *p).expect("fixture species missing"))
    .collect()
}

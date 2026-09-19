//! Shared fixtures for tests. Compiled only under `cfg(test)`.
//!
//! Thin wrappers over [`crate::demo`] so there is one definition of the demo species.

use crate::species::{Phase, Species, SpeciesId, SpeciesRegistry};
use crate::stream::Stream;
use crate::thermo::Shomate;
use crate::unit::Reaction;

/// Molar masses of CH4, O2, CO2 and H2O to three decimals. Both sides of the combustion equation
/// sum to 80.039 g/mol, so the reactor's mass correction is 1 to rounding.
pub const COMBUSTION_MASSES: [f64; 4] = [16.043, 31.998, 44.009, 18.015];

/// The same four molar masses to two decimals. Reactants sum to 80.04 g/mol and products to
/// 80.05, so the correction is 80.04 / 80.05 - not 1, but well inside the closure tolerance.
pub const ROUNDED_COMBUSTION_MASSES: [f64; 4] = [16.04, 32.00, 44.01, 18.02];

/// NIST standard enthalpies of formation of CH4, O2, CO2 and H2O as gases, kJ/mol. Oxygen is an
/// element in its standard state, so zero by definition.
pub const COMBUSTION_FORMATION: [f64; 4] = [-74.87, 0.0, -393.52, -241.83];

/// CH4, O2, CO2 and H2O, in that order, as gases with the given molar masses and
/// [`COMBUSTION_FORMATION`].
///
/// The heat capacities are constant and round, so a temperature an adiabatic test reaches has a
/// closed form.
pub fn combustion_registry(masses: [f64; 4]) -> SpeciesRegistry {
    let mut registry = SpeciesRegistry::default();
    for ((name, molar_mass), h_f) in ["CH4", "O2", "CO2", "H2O"]
        .into_iter()
        .zip(masses)
        .zip(COMBUSTION_FORMATION)
    {
        registry.insert(Species {
            name: name.into(),
            phase: Phase::Gas,
            molar_mass,
            shomate: Shomate::constant(35.0),
            enthalpy_of_formation: Some(h_f),
            density: None,
            vapour_pressure: None,
        });
    }
    registry
}

/// `CH4 + 2 O2 -> CO2 + 2 H2O`, limited by the methane, over [`combustion_registry`].
pub fn combustion(registry: &SpeciesRegistry, conversion: f64) -> Reaction {
    Reaction {
        stoichiometry: vec![-1.0, -2.0, 1.0, 2.0],
        limiting: registry
            .find("CH4", Phase::Gas)
            .expect("fixture species missing"),
        conversion,
    }
}

pub use crate::demo::{AMBIENT_K, AMBIENT_KPA};

/// The three species of the demo circuit.
pub fn demo_registry() -> SpeciesRegistry {
    crate::demo::registry()
}

/// The demo species with standard enthalpies of formation, kJ/mol, for a test that runs a reaction
/// over them. Quartz and liquid water are NIST values; chalcopyrite's is approximate.
pub fn demo_registry_with_formation() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (species, h_f) in demo_registry().all().iter().zip([-190.8, -910.86, -285.83]) {
        r.insert(Species {
            enthalpy_of_formation: Some(h_f),
            density: None,
            vapour_pressure: None,
            ..species.clone()
        });
    }
    r
}

/// Densities of chalcopyrite, quartz and water in kg/m³, for a test that pumps the demo slurry.
pub const DEMO_DENSITIES: [f64; 3] = [4200.0, 2650.0, 997.0];

/// The demo species with [`DEMO_DENSITIES`], so a pump can find the volume it moves.
pub fn demo_registry_with_density() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (species, density) in demo_registry().all().iter().zip(DEMO_DENSITIES) {
        r.insert(Species {
            density: Some(density),
            vapour_pressure: None,
            ..species.clone()
        });
    }
    r
}

/// Nitrogen as a gas over the NIST-JANAF Shomate fit for 100-500 K (Chase, 1998), so that a
/// compression has a temperature-dependent cp to integrate.
pub fn nitrogen_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    r.insert(Species {
        name: "N2".into(),
        phase: Phase::Gas,
        molar_mass: 28.0134,
        shomate: Shomate {
            a: 28.98641,
            b: 1.853978,
            c: -9.647459,
            d: 16.63537,
            e: 0.000117,
        },
        enthalpy_of_formation: None,
        density: None,
        vapour_pressure: None,
    });
    r
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

/// Liquid water, steam and nitrogen, in that order, exactly as the shipped library has them - so
/// the water carries its vapour pressure and both phases their formation enthalpies, and a flash
/// has something to evaporate and something that never condenses.
pub fn humid_nitrogen() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (name, phase) in [
        ("H2O", Phase::Liquid),
        ("H2O", Phase::Gas),
        ("N2", Phase::Gas),
    ] {
        let entry = crate::library::find(name, phase).expect("shipped species missing");
        r.insert(entry.species.clone());
    }
    r
}

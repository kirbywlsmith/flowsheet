//! Species data structures.

use crate::thermo::Shomate;
use serde::{Deserialize, Serialize};

/// A distinct form in which matter can exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    /// A state of matter in which atoms are closely packed.
    Solid,
    /// A state of matter with a definite volume but no fixed shape.
    Liquid,
    /// A state of matter with neither fixed volume nor fixed shape.
    Gas,
}

/// Used to index a [`SpeciesRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeciesId(u16);

impl SpeciesId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for SpeciesId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A species is a distinct particle type within a system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Species {
    /// The name of the species.
    ///
    /// e.g. H2O
    pub name: String,
    /// The phase of the species.
    ///
    /// e.g. [`Phase::Liquid`]
    pub phase: Phase,
    /// The molar mass of the species, in g/mol.
    ///
    /// e.g. `18.015`
    pub molar_mass: f64,
    /// Heat capacity coefficients, per mole.
    ///
    /// e.g. `Shomate::constant(75.3)` for liquid water near 25 °C
    pub shomate: Shomate,
}

impl Species {
    /// Specific heat capacity at `temperature` (K), in kJ/(kg·K).
    pub fn heat_capacity(&self, temperature: f64) -> f64 {
        // J/(mol·K) over g/mol is J/(g·K), which is already kJ/(kg·K).
        self.shomate.heat_capacity(temperature) / self.molar_mass
    }

    /// Specific enthalpy at `temperature` (K) relative to [`crate::thermo::REFERENCE_K`], in
    /// kJ/kg.
    pub fn enthalpy(&self, temperature: f64) -> f64 {
        // kJ/mol over g/mol is kJ/g, and a kilogram is a thousand grams.
        1000.0 * self.shomate.enthalpy(temperature) / self.molar_mass
    }
}

/// Contains a specific set of [`Species`].
#[derive(Debug, Default)]
pub struct SpeciesRegistry {
    species: Vec<Species>,
}

impl SpeciesRegistry {
    /// Inserts `s` into the registry if it doesn't already exist.
    ///
    /// # Panics
    /// If `s` is new and the registry already holds every species a [`SpeciesId`] can address
    /// (65,536 of them), since the cast below would otherwise wrap the new id round to 0. The
    /// check sits in the `None` arm on purpose: a duplicate hands back an id that already exists
    /// and allocates nothing, so a full registry can still answer for what it already has.
    pub fn insert(&mut self, s: Species) -> SpeciesId {
        // TODO: should handle duplicate insert with differing molar masses
        match self.find(&s.name, s.phase) {
            None => {
                crate::assert_id_space(self.species.len(), "species");
                self.species.push(s);
                SpeciesId((self.species.len() - 1) as u16)
            }
            Some(id) => id,
        }
    }

    /// Returns an optional reference to the specified [`Species`].
    pub fn get(&self, id: SpeciesId) -> Option<&Species> {
        self.species.get(id.as_usize())
    }

    /// Searches for a species by `name` and `phase`, returning its optional [`SpeciesId`].
    pub fn find(&self, name: &str, phase: Phase) -> Option<SpeciesId> {
        self.species
            .iter()
            .position(|s| s.name == name && s.phase == phase)
            .map(|i| SpeciesId(i as u16))
    }

    /// Returns the number of species in the registry.
    pub fn len(&self) -> usize {
        self.species.len()
    }

    /// Returns `true` if the registry contains no species.
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }

    /// Every [`Species`], in [`SpeciesId`] order.
    pub fn all(&self) -> &[Species] {
        &self.species
    }
}

impl std::ops::Index<SpeciesId> for SpeciesRegistry {
    type Output = Species;
    fn index(&self, id: SpeciesId) -> &Species {
        &self.species[id.as_usize()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{QUARTZ_CP, WATER_CP};
    use crate::thermo::REFERENCE_K;
    use approx::assert_relative_eq;

    fn water() -> Species {
        Species {
            name: "H2O".into(),
            phase: Phase::Liquid,
            molar_mass: 18.015,
            shomate: WATER_CP,
        }
    }

    #[test]
    fn insert_returns_sequential_ids() {
        let mut reg = SpeciesRegistry::default();
        let a = reg.insert(water());
        let b = reg.insert(Species {
            name: "SiO2".into(),
            phase: Phase::Solid,
            molar_mass: 60.08,
            shomate: QUARTZ_CP,
        });
        assert_eq!(a.as_usize(), 0);
        assert_eq!(b.as_usize(), 1);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn insert_dedupes_on_name_and_phase() {
        let mut reg = SpeciesRegistry::default();
        let a = reg.insert(water());
        let b = reg.insert(water());
        assert_eq!(a, b);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn same_name_different_phase_is_a_distinct_species() {
        let mut reg = SpeciesRegistry::default();
        let liquid = reg.insert(water());
        let gas = reg.insert(Species {
            phase: Phase::Gas,
            ..water()
        });
        assert_ne!(liquid, gas);
        assert_eq!(reg[gas].phase, Phase::Gas);
    }

    #[test]
    fn get_is_none_for_unknown_and_find_round_trips() {
        let mut reg = SpeciesRegistry::default();
        let id = reg.insert(water());
        assert_eq!(reg.find("H2O", Phase::Liquid), Some(id));
        assert_eq!(reg.find("H2O", Phase::Gas), None);
        assert_eq!(reg[id].molar_mass, 18.015);
    }

    #[test]
    fn the_mass_basis_divides_the_molar_properties_by_molar_mass() {
        // 18.015 J/(mol·K) over 18.015 g/mol is exactly 1 J/(g·K), which is 1 kJ/(kg·K)...
        let s = Species {
            shomate: Shomate::constant(18.015),
            ..water()
        };
        assert_relative_eq!(s.heat_capacity(350.0), 1.0, max_relative = 1e-12);
        // ...so 10 K above the reference is 10 kJ/kg.
        assert_relative_eq!(s.enthalpy(REFERENCE_K + 10.0), 10.0, max_relative = 1e-12);
    }

    #[test]
    fn liquid_water_takes_4_18_kilojoules_per_kilogram_kelvin() {
        assert_relative_eq!(
            water().heat_capacity(REFERENCE_K),
            4.18,
            max_relative = 2e-3
        );
    }

    /// A registry holding `n` distinct species, filled by pushing rather than by `insert`.
    ///
    /// `insert` runs `find` first, and `find` is a linear scan, so filling 65,536 species through
    /// the public path would be O(n^2) - roughly two billion comparisons. Pushing straight into
    /// the vector reaches the same state, and the names stay distinct so that the one `insert`
    /// each test does still takes the branch it is meant to.
    fn filled_to(n: usize) -> SpeciesRegistry {
        SpeciesRegistry {
            species: (0..n)
                .map(|i| Species {
                    name: format!("s{i}"),
                    phase: Phase::Solid,
                    molar_mass: 1.0,
                    shomate: Shomate::constant(1.0),
                })
                .collect(),
        }
    }

    #[test]
    #[should_panic(expected = "65537 species exceeds the 65536 a u16 id can address")]
    fn one_species_past_the_u16_id_space_panics() {
        // `water` is liquid H2O and every filler species is a solid, so `find` misses and the
        // insert takes the `None` arm where the new id would be allocated.
        filled_to(crate::MAX_IDS).insert(water());
    }

    #[test]
    fn a_full_registry_still_answers_for_a_species_it_already_holds() {
        // The guard sits in the `None` arm, so a duplicate costs no id and must not panic even
        // when there is no id left to hand out.
        let mut registry = filled_to(crate::MAX_IDS);
        let duplicate = Species {
            name: "s0".into(),
            phase: Phase::Solid,
            molar_mass: 1.0,
            shomate: Shomate::constant(1.0),
        };
        assert_eq!(registry.insert(duplicate).as_usize(), 0);
    }
}

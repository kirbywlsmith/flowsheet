//! Species data structures.

/// A distinct form in which matter can exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// A state of matter in which atoms are closely packed.
    Solid,
    /// A state of matter with a definite volume but no fixed shape.
    Liquid,
    /// A state of matter with neither fixed volume nor fixed shape.
    Gas,
}

/// Used to index a `SpeciesRegistry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeciesId(u16);

impl SpeciesId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// A species is a distinct particle type within a system.
#[derive(Debug, Clone)]
pub struct Species {
    /// The name of the species.
    ///
    /// e.g. H2O
    pub name: String,
    /// The phase of the species.
    ///
    /// e.g. `Phase::Liquid`
    pub phase: Phase,
    /// The molar mass of the species, in g/mol.
    ///
    /// e.g. `18.015`
    pub molar_mass: f64,
}

/// Contains a specific set of `Species`.
#[derive(Debug, Default)]
pub struct SpeciesRegistry {
    species: Vec<Species>,
}

impl SpeciesRegistry {
    /// Inserts `s` into the registry if it doesn't already exist.
    pub fn insert(&mut self, s: Species) -> SpeciesId {
        // TODO: should handle duplicate insert with differing molar masses
        match self.find(&s.name, s.phase) {
            None => {
                self.species.push(s);
                SpeciesId((self.species.len() - 1) as u16)
            }
            Some(id) => id,
        }
    }

    /// Returns an optional reference to the specified species.
    pub fn get(&self, id: SpeciesId) -> Option<&Species> {
        self.species.get(id.as_usize())
    }

    /// Searches for a species by `name` and `phase`, returning its optional `SpeciesId`.
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

    fn water() -> Species {
        Species {
            name: "H2O".into(),
            phase: Phase::Liquid,
            molar_mass: 18.015,
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
}

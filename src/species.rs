#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeciesId(u16);

impl SpeciesId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Solid,
    Liquid,
    Gas,
}

#[derive(Debug, Clone)]
pub struct Species {
    pub name: String,
    pub phase: Phase,
    pub molar_mass: f64,
}

#[derive(Debug, Default)]
pub struct SpeciesRegistry {
    species: Vec<Species>,
}

impl SpeciesRegistry {
    // TODO: should handle duplicate insert with differing molar masses
    pub fn insert(&mut self, s: Species) -> SpeciesId {
        match self.find(&s.name, s.phase) {
            None => {
                self.species.push(s);
                SpeciesId((self.species.len() - 1) as u16)
            }
            Some(id) => id,
        }
    }
    pub fn get(&self, id: SpeciesId) -> Option<&Species> {
        self.species.get(id.as_usize())
    }
    pub fn find(&self, name: &str, phase: Phase) -> Option<SpeciesId> {
        self.species
            .iter()
            .position(|s| s.name == name && s.phase == phase)
            .map(|i| SpeciesId(i as u16))
    }
    pub fn len(&self) -> usize {
        self.species.len()
    }
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


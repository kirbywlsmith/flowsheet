use crate::species::{SpeciesId, SpeciesRegistry};

#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// Absolute mass flow per species (t/h), indexed by `SpeciesId`
    flows: Vec<f64>,
    /// Kelvin
    temperature: f64,
    /// kPa
    pressure: f64,
}

impl Stream {
    pub fn zeros(registry: &SpeciesRegistry, temperature: f64, pressure: f64) -> Self {
        Self {
            flows: vec![0.0; registry.len()],
            temperature,
            pressure,
        }
    }
    /// # Panics
    /// If `flows.len()` doesn't match `registry`
    pub fn from_flows(
        registry: &SpeciesRegistry,
        flows: Vec<f64>,
        temperature: f64,
        pressure: f64,
    ) -> Self {
        assert_eq!(
            flows.len(),
            registry.len(),
            "stream has {} flows but registry has {} species",
            flows.len(),
            registry.len()
        );
        Self {
            flows,
            temperature,
            pressure,
        }
    }
    pub fn total(&self) -> f64 {
        self.flows.iter().sum()
    }
    pub fn species_count(&self) -> usize {
        self.flows.len()
    }
}

impl std::ops::Index<SpeciesId> for Stream {
    type Output = f64;
    fn index(&self, id: SpeciesId) -> &f64 {
        &self.flows[id.as_usize()]
    }
}

impl std::ops::IndexMut<SpeciesId> for Stream {
    fn index_mut(&mut self, id: SpeciesId) -> &mut f64 {
        &mut self.flows[id.as_usize()]
    }
}

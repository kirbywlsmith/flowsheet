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
        // TODO: should handle negative / NaN flows

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
    pub fn temperature(&self) -> f64 {
        self.temperature
    }
    pub fn pressure(&self) -> f64 {
        self.pressure
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::{Phase, Species};
    use approx::assert_relative_eq;

    fn registry() -> SpeciesRegistry {
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

    #[test]
    fn zeros_matches_registry_length_and_has_no_flow() {
        let r = registry();
        let s = Stream::zeros(&r, 298.15, 101.325);
        assert_eq!(s.species_count(), 3);
        assert_relative_eq!(s.total(), 0.0);
    }

    #[test]
    fn index_mut_writes_are_visible_and_change_the_total() {
        let r = registry();
        let mut s = Stream::zeros(&r, 298.15, 101.325);
        let water = r.find("H2O", Phase::Liquid).unwrap();
        s[water] = 600.0;
        assert_relative_eq!(s[water], 600.0);
        assert_relative_eq!(s.total(), 600.0);
    }

    #[test]
    fn from_flows_totals_the_feed_stream() {
        let r = registry();
        let s = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 298.15, 101.325);
        assert_relative_eq!(s.total(), 1000.0);
    }

    #[test]
    #[should_panic(expected = "registry has 3 species")]
    fn from_flows_rejects_a_length_mismatch() {
        let r = registry();
        Stream::from_flows(&r, vec![40.0, 360.0], 298.15, 101.325);
    }
}

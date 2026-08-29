use crate::species::{SpeciesId, SpeciesRegistry};

#[derive(Debug, Clone)]
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
    /// Largest per-species flow difference, normalised by the larger stream total.
    /// A result of 1e-6 means "every species agrees to within 1 ppm of the stream's
    /// total mass flow". Compares flows only - not temperature or pressure.
    ///
    /// Returns NaN if either stream contains NaN.
    ///
    /// # Panics
    /// If the two streams have different species counts.
    pub fn max_flow_residual(&self, other: &Stream) -> f64 {
        assert_eq!(
            self.flows.len(),
            other.flows.len(),
            "cannot compare streams with {} and {} species",
            self.flows.len(),
            other.flows.len()
        );

        let scale = self.total().max(other.total());
        if scale == 0.0 {
            return 0.0; // both streams are empty
        }

        self.flows
            .iter()
            .zip(&other.flows)
            .map(|(a, b)| (a - b).abs() / scale)
            .fold(0.0_f64, |acc, d| {
                if acc.is_nan() || d.is_nan() {
                    f64::NAN
                } else {
                    acc.max(d)
                }
            })
    }
    /// True if `max_flow_residual` is within `tolerance`. Typical `tolerance` is 1e-6.
    pub fn flows_approx_eq(&self, other: &Stream, tolerance: f64) -> bool {
        self.max_flow_residual(other) <= tolerance
    }
    /// A `Stream` with zero total flow returns a zero vector.
    pub fn mass_fractions(&self) -> Vec<f64> {
        let total = self.total();
        if total == 0.0 {
            return vec![0.0; self.flows.len()];
        }
        self.flows.iter().map(|f| f / total).collect()
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

    #[test]
    fn identical_streams_have_zero_residual() {
        let r = registry();
        let a = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 298.15, 101.325);
        assert_relative_eq!(a.max_flow_residual(&a), 0.0);
        assert!(a.flows_approx_eq(&a, 1e-9));
    }

    #[test]
    fn residual_is_the_fraction_of_total_mass() {
        let r = registry();
        let a = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 298.15, 101.325);
        let b = Stream::from_flows(&r, vec![40.1, 360.0, 600.0], 298.15, 101.325);
        // 0.1 t/h out of a 1000.1 t/h total
        assert_relative_eq!(b.max_flow_residual(&a), 0.1 / 1000.1, epsilon = 1e-12);
        assert!(!a.flows_approx_eq(&b, 1e-6));
        assert!(a.flows_approx_eq(&b, 1e-3));
    }

    #[test]
    fn nan_never_reports_converged() {
        let r = registry();
        let a = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 298.15, 101.325);
        let b = Stream::from_flows(&r, vec![f64::NAN, 360.0, 600.0], 298.15, 101.325);
        assert!(a.max_flow_residual(&b).is_nan());
        assert!(!a.flows_approx_eq(&b, 1e9));
    }

    #[test]
    fn two_empty_streams_are_equal() {
        let r = registry();
        let a = Stream::zeros(&r, 298.15, 101.325);
        assert!(a.flows_approx_eq(&Stream::zeros(&r, 298.15, 101.325), 1e-9));
    }
}

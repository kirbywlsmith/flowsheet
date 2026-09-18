//! [`Splitter`] and [`SplitterN`] - composition-preserving flow division.

use super::{Arity, EvalError, UnitOp, drop_pressures, split, split_n};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, two outlets: `fraction` and `1.0 - fraction`.
#[derive(Debug, Clone, Copy)]
pub struct Splitter {
    /// The `fraction` to pass to [`split`].
    pub fraction: f64,
    /// Pressure lost across the unit, in kPa, by both outlets. Never negative.
    pub pressure_drop: f64,
}

/// One inlet, one outlet per ratio.
#[derive(Debug, Clone)]
pub struct SplitterN {
    /// The `ratios` to pass to [`split_n`].
    pub ratios: Vec<f64>,
    /// Pressure lost across the unit, in kPa, by every outlet. Never negative.
    pub pressure_drop: f64,
}

impl UnitOp for Splitter {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(2)
    }

    /// # Panics
    /// If `fraction` is outside `0.0..=1.0`. That is caught at the JSON boundary by
    /// [`crate::serial::LoadError`], so reaching it is a bug rather than bad input - see
    /// [`split`].
    fn evaluate(
        &self,
        _registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        let (a, b) = split(inlets[0], self.fraction);
        let mut outlets = vec![a, b];
        drop_pressures(&mut outlets, self.pressure_drop)?;
        Ok(outlets)
    }
}

impl UnitOp for SplitterN {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(self.ratios.len())
    }

    /// # Panics
    /// If `ratios` is empty, holds a negative or NaN value, or sums to zero. See [`split_n`].
    fn evaluate(
        &self,
        _registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        let mut outlets = split_n(inlets[0], &self.ratios);
        drop_pressures(&mut outlets, self.pressure_drop)?;
        Ok(outlets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_registry, feed};
    use approx::assert_relative_eq;

    #[test]
    fn splitter_returns_the_fraction_side_first() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Splitter {
            fraction: 0.3,
            pressure_drop: 0.0,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert_eq!(outs.len(), 2);
        assert_relative_eq!(outs[0].total(), 300.0, max_relative = 1e-12);
        assert_relative_eq!(outs[1].total(), 700.0, max_relative = 1e-12);
    }

    #[test]
    fn splitter_n_returns_one_outlet_per_ratio() {
        let r = demo_registry();
        let inlet = feed(&r);
        let op = SplitterN {
            ratios: vec![1.0, 1.0, 2.0],
            pressure_drop: 0.0,
        };

        let outs = op.evaluate(&r, &[&inlet]).unwrap();

        assert_eq!(outs.len(), 3);
        assert_relative_eq!(outs[0].total(), 250.0, max_relative = 1e-12);
        assert_relative_eq!(outs[2].total(), 500.0, max_relative = 1e-12);
    }
}

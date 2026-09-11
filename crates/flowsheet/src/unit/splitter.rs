//! [`Splitter`] and [`SplitterN`] - composition-preserving flow division.

use super::{Arity, UnitOp, split, split_n};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, two outlets: `fraction` and `1.0 - fraction`.
#[derive(Debug, Clone, Copy)]
pub struct Splitter {
    /// The `fraction` to pass to [`split`].
    pub fraction: f64,
}

/// One inlet, one outlet per ratio.
#[derive(Debug, Clone)]
pub struct SplitterN {
    /// The `ratios` to pass to [`split_n`].
    pub ratios: Vec<f64>,
}

impl UnitOp for Splitter {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(2)
    }

    fn evaluate(&self, _registry: &SpeciesRegistry, inlets: &[&Stream]) -> Vec<Stream> {
        let (a, b) = split(inlets[0], self.fraction);
        vec![a, b]
    }
}

impl UnitOp for SplitterN {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(self.ratios.len())
    }

    fn evaluate(&self, _registry: &SpeciesRegistry, inlets: &[&Stream]) -> Vec<Stream> {
        split_n(inlets[0], &self.ratios)
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

        let outs = Splitter { fraction: 0.3 }.evaluate(&r, &[&inlet]);

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
        };

        let outs = op.evaluate(&r, &[&inlet]);

        assert_eq!(outs.len(), 3);
        assert_relative_eq!(outs[0].total(), 250.0, max_relative = 1e-12);
        assert_relative_eq!(outs[2].total(), 500.0, max_relative = 1e-12);
    }
}

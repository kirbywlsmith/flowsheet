//! [`Mixer`] - the only operation with an unbounded side.

use super::{Arity, EvalError, UnitOp, mix};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// Any number of inlets, one outlet: combines them all with [`mix`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Mixer;

impl UnitOp for Mixer {
    fn inlet_arity(&self) -> Arity {
        // The only unbounded side in the model.
        Arity::at_least(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Errors
    /// If no positive temperature carries the combined enthalpy. See [`mix`].
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        // Two layers, and they mean different things: `?` forwards a real failure to the
        // solver, while `expect` asserts the arity the caller already guaranteed.
        let outlet =
            mix(registry, inlets.iter().copied())?.expect("mixer needs at least one inlet");
        Ok(vec![outlet])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use approx::assert_relative_eq;

    #[test]
    fn mixer_sums_all_inlets_into_one_outlet() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let outs = Mixer.evaluate(&r, &[&a, &b]).unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1060.0, max_relative = 1e-12);
    }
}

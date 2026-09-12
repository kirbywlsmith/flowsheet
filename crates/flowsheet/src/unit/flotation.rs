//! [`Flotation`] - the first operation that changes a stream's composition.

use super::{Arity, EvalError, UnitOp, recover};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, two outlets: concentrate first, tails second.
///
/// Unlike a [`super::Splitter`], which sends the same composition to both outlets, a flotation
/// cell recovers each species at its own rate: `recovery[i]` is the fraction of species `i`
/// reporting to the concentrate, and the remainder leaves in the tails. That is what lets a
/// circuit built from these actually upgrade an ore rather than just move it around.
///
/// `recovery` is a dense vector in [`crate::species::SpeciesId`] order, the same shape as
/// [`Stream::flows`]. It is validated by [`recover`] rather than on construction, because the
/// field is public and the species count is not known until an inlet arrives.
#[derive(Debug, Clone)]
pub struct Flotation {
    /// The fraction of each species reporting to the concentrate, in `SpeciesId` order.
    pub recovery: Vec<f64>,
}

impl UnitOp for Flotation {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(2)
    }

    /// # Panics
    /// If `recovery` does not match the inlet's species count, or holds a value outside
    /// `0.0..=1.0`. See [`recover`].
    fn evaluate(
        &self,
        _registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        let (concentrate, tails) = recover(inlets[0], &self.recovery);
        Ok(vec![concentrate, tails])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{all_ids, demo_registry, feed};
    use approx::assert_relative_eq;

    /// 85% of the chalcopyrite, 5% of the gangue, 30% of the water.
    fn rougher() -> Flotation {
        Flotation {
            recovery: vec![0.85, 0.05, 0.30],
        }
    }

    #[test]
    fn flotation_returns_the_concentrate_first() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = rougher().evaluate(&r, &[&inlet]).unwrap();

        assert_eq!(outs.len(), 2);
        // 0.85 * 40.0 t/h of chalcopyrite reports to the concentrate.
        assert_relative_eq!(outs[0].flows()[0], 34.0, max_relative = 1e-12);
        assert_relative_eq!(outs[1].flows()[0], 6.0, max_relative = 1e-12);
    }

    #[test]
    fn flotation_conserves_every_species() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = rougher().evaluate(&r, &[&inlet]).unwrap();

        for id in all_ids(&r) {
            assert_relative_eq!(outs[0][id] + outs[1][id], inlet[id], max_relative = 1e-12);
        }
    }

    #[test]
    fn flotation_upgrades_the_valuable_species() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = rougher().evaluate(&r, &[&inlet]).unwrap();

        // Grade is the point: 4% chalcopyrite in, 34.0 / 232.0 = 14.7% in the concentrate.
        // The circuit reaches 12.33% instead - the recycle brings gangue back round with it.
        assert_relative_eq!(inlet.mass_fractions()[0], 0.04, max_relative = 1e-12);
        assert!(outs[0].mass_fractions()[0] > inlet.mass_fractions()[0]);
        assert!(outs[1].mass_fractions()[0] < inlet.mass_fractions()[0]);
    }

    #[test]
    fn all_zero_recovery_sends_everything_to_the_tails() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Flotation {
            recovery: vec![0.0; 3],
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert_relative_eq!(outs[0].total(), 0.0);
        assert!(inlet.flows_approx_eq(&outs[1], 1e-12));
    }

    #[test]
    #[should_panic(expected = "one recovery per species")]
    fn flotation_rejects_a_recovery_of_the_wrong_length() {
        let r = demo_registry();
        // `let _` because `evaluate` now returns a `#[must_use]` Result; the panic fires before
        // it ever produces one, but the unused-result lint is a static check.
        let _ = Flotation {
            recovery: vec![0.85, 0.05],
        }
        .evaluate(&r, &[&feed(&r)]);
    }

    #[test]
    #[should_panic(expected = "recovery must be between 0.0 and 1.0")]
    fn flotation_rejects_a_recovery_above_one() {
        let r = demo_registry();
        let _ = Flotation {
            recovery: vec![1.5, 0.05, 0.30],
        }
        .evaluate(&r, &[&feed(&r)]);
    }
}

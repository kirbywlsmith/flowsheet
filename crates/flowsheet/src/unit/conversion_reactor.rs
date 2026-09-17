//! [`ConversionReactor`] - the first operation that turns one species into another.

use super::{Arity, EvalError, Reaction, UnitOp, react};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, one outlet: runs one [`Reaction`] to a fixed conversion with [`react`].
///
/// Structurally [`super::Flotation`]'s sibling - a dense per-species vector changing a stream's
/// composition - except that it transforms species rather than partitioning them, so the outlet
/// holds species the inlet did not.
///
/// **Isothermal.** The outlet leaves at the inlet temperature, so for any reaction that releases
/// or absorbs heat the flowsheet's energy balance is wrong, and nothing reports it. That waits on
/// the reactor energy balance item in TODO.md. Mass is still conserved exactly; see [`react`].
///
/// One reaction, named `reaction`. Several reactions in one reactor is a later item, and it will
/// make this a `Vec<Reaction>` under the same op.
#[derive(Debug, Clone)]
pub struct ConversionReactor {
    /// The reaction this reactor runs.
    pub reaction: Reaction,
}

impl UnitOp for ConversionReactor {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Errors
    /// If the inlet carries too little of a non-limiting reactant. See [`react`].
    ///
    /// # Panics
    /// If the reaction is malformed. See [`react`].
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        Ok(vec![react(registry, inlets[0], &self.reaction)?])
    }

    /// A reactor destroys its reactants and creates its products, so only total mass balances.
    fn conserves_species(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{AMBIENT_KPA, COMBUSTION_MASSES, combustion, combustion_registry};
    use approx::assert_relative_eq;

    #[test]
    fn a_reactor_returns_one_outlet_with_the_mass_it_was_given() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 60.0, 0.0, 0.0], 350.0, AMBIENT_KPA);
        let reactor = ConversionReactor {
            reaction: combustion(&r, 0.9),
        };

        let outs = reactor.evaluate(&r, &[&inlet]).unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), inlet.total(), max_relative = 1e-12);
        assert_relative_eq!(outs[0].flows()[0], 1.0, max_relative = 1e-12);
        // Isothermal: the temperature rides through, whatever the reaction released.
        assert_eq!(outs[0].temperature(), 350.0);
    }

    #[test]
    fn a_reactor_does_not_conserve_species() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let reactor = ConversionReactor {
            reaction: combustion(&r, 0.9),
        };
        assert!(!reactor.conserves_species());
    }

    #[test]
    fn a_reactant_the_inlet_lacks_is_an_error_not_a_panic() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 1.0, 0.0, 0.0], 350.0, AMBIENT_KPA);
        let reactor = ConversionReactor {
            reaction: combustion(&r, 0.9),
        };

        let e = reactor
            .evaluate(&r, &[&inlet])
            .expect_err("1 t/h of oxygen cannot burn 9 t/h of methane");

        assert!(e.to_string().contains("more O2"), "{e}");
    }
}

//! [`ConversionReactor`] - the first operation that turns one species into another.

use super::{Arity, EvalError, Reaction, ReactorEnergy, UnitOp, react, solve_temperature};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, one outlet: runs its [`Reaction`]s in declared order, each to a fixed conversion
/// with [`react`].
///
/// Structurally [`super::Flotation`]'s sibling - a dense per-species vector changing a stream's
/// composition - except that it transforms species rather than partitioning them, so the outlet
/// holds species the inlet did not.
///
/// **In declared order**, the shape of Aspen's `RStoic` in series mode: each reaction runs on what
/// the one before it left, so its conversion is a fraction of the limiting reactant *at that
/// point*, not at the inlet. Two reactions competing for one reactant are therefore well
/// defined - the first takes its share and the second takes its share of the rest - and no
/// ordering of conversions can consume more than arrived. A reactant the earlier reactions
/// leave too little of for a later one is an [`EvalError`] naming that reaction, from the same
/// check [`react`] makes for one.
///
/// `energy` says what happens to the heat of reaction; see [`ReactorEnergy`]. Mass is conserved
/// exactly either way; see [`react`].
#[derive(Debug, Clone)]
pub struct ConversionReactor {
    /// The reactions this reactor runs, first to last. Never empty.
    pub reactions: Vec<Reaction>,
    /// Whether the outlet holds the inlet's temperature or its enthalpy.
    pub energy: ReactorEnergy,
}

impl UnitOp for ConversionReactor {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Errors
    /// If the stream reaching a reaction carries too little of a non-limiting reactant (see
    /// [`react`]); the message starts `reaction {i}: `, counting from 0. Or, when adiabatic, if no
    /// positive temperature holds the inlet's enthalpy in the outlet's composition (see
    /// [`solve_temperature`]) - an endothermic reaction asking for more heat than the stream has
    /// above absolute zero.
    ///
    /// # Panics
    /// If `reactions` is empty, or a reaction is malformed (see [`react`]).
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        assert!(
            !self.reactions.is_empty(),
            "a conversion reactor needs at least one reaction"
        );
        let inlet = inlets[0];

        let mut outlet = inlet.clone();
        for (i, reaction) in self.reactions.iter().enumerate() {
            outlet = react(registry, &outlet, reaction)
                .map_err(|e| EvalError::new(format!("reaction {i}: {e}")))?;
        }

        // Once, after the last reaction: enthalpy is a state function, so the temperatures in
        // between would change nothing. An empty inlet has no heat capacity, and
        // `solve_temperature` panics on one. A reactor downstream of a tear sees exactly that on
        // the first pass; the same guard as `heat`.
        if self.energy == ReactorEnergy::Adiabatic && outlet.heat_capacity(registry) != 0.0 {
            // Newton starts from the inlet temperature, which `react` left on the outlet.
            solve_temperature(registry, &mut outlet, inlet.enthalpy(registry))?;
        }

        Ok(vec![outlet])
    }

    /// A reactor destroys its reactants and creates its products, so only total mass balances.
    fn conserves_species(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        AMBIENT_K, AMBIENT_KPA, COMBUSTION_MASSES, combustion, combustion_registry,
    };
    use approx::assert_relative_eq;

    fn reactor(r: &SpeciesRegistry, energy: ReactorEnergy) -> ConversionReactor {
        ConversionReactor {
            reactions: vec![combustion(r, 0.9)],
            energy,
        }
    }

    #[test]
    fn an_isothermal_reactor_returns_one_outlet_at_the_inlet_temperature() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 60.0, 0.0, 0.0], 350.0, AMBIENT_KPA);

        let outs = reactor(&r, ReactorEnergy::Isothermal)
            .evaluate(&r, &[&inlet])
            .unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), inlet.total(), max_relative = 1e-12);
        assert_relative_eq!(outs[0].flows()[0], 1.0, max_relative = 1e-12);
        assert_eq!(outs[0].temperature(), 350.0);
    }

    #[test]
    fn an_adiabatic_reactor_keeps_the_enthalpy_and_burning_raises_the_temperature() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 60.0, 0.0, 0.0], 350.0, AMBIENT_KPA);

        let out = reactor(&r, ReactorEnergy::Adiabatic)
            .evaluate(&r, &[&inlet])
            .unwrap()
            .remove(0);

        assert_relative_eq!(out.enthalpy(&r), inlet.enthalpy(&r), max_relative = 1e-12);
        assert!(out.temperature() > 1000.0, "{}", out.temperature());
    }

    #[test]
    fn an_adiabatic_reactor_passes_an_empty_inlet_through() {
        // What a reactor downstream of a tear sees on the first pass.
        let r = combustion_registry(COMBUSTION_MASSES);
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

        let out = reactor(&r, ReactorEnergy::Adiabatic)
            .evaluate(&r, &[&empty])
            .unwrap()
            .remove(0);

        assert_eq!(out.flows(), [0.0; 4]);
        assert_eq!(out.temperature(), AMBIENT_K);
    }

    #[test]
    fn an_endothermic_reaction_no_temperature_can_pay_for_is_an_error_not_a_panic() {
        // Combustion run backwards: 802 kJ per mole of methane made, drawn from a stream whose
        // sensible heat above 0 K is a few percent of that.
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![0.0, 0.0, 44.0, 36.0], AMBIENT_K, AMBIENT_KPA);
        let unburn = ConversionReactor {
            reactions: vec![Reaction {
                stoichiometry: vec![1.0, 2.0, -1.0, -2.0],
                limiting: r.find("CO2", crate::Phase::Gas).unwrap(),
                conversion: 0.9,
            }],
            energy: ReactorEnergy::Adiabatic,
        };

        let e = unburn
            .evaluate(&r, &[&inlet])
            .expect_err("nothing pays for un-burning methane");

        assert!(e.to_string().contains("no positive temperature"), "{e}");
    }

    #[test]
    #[should_panic(expected = "a conversion reactor needs at least one reaction")]
    fn a_reactor_with_no_reactions_panics() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 60.0, 0.0, 0.0], 350.0, AMBIENT_KPA);
        let empty = ConversionReactor {
            reactions: Vec::new(),
            energy: ReactorEnergy::Isothermal,
        };
        let _ = empty.evaluate(&r, &[&inlet]);
    }

    #[test]
    fn a_reactor_does_not_conserve_species() {
        let r = combustion_registry(COMBUSTION_MASSES);
        assert!(!reactor(&r, ReactorEnergy::Isothermal).conserves_species());
    }

    #[test]
    fn a_reactant_the_inlet_lacks_is_an_error_not_a_panic() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 1.0, 0.0, 0.0], 350.0, AMBIENT_KPA);

        let e = reactor(&r, ReactorEnergy::Isothermal)
            .evaluate(&r, &[&inlet])
            .expect_err("1 t/h of oxygen cannot burn 9 t/h of methane");

        assert!(e.to_string().contains("more O2"), "{e}");
    }
}

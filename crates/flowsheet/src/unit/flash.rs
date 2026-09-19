//! [`Flash`] - the operation that splits a stream into its vapour and its liquid.

use super::{Arity, EvalError, UnitOp, flash};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, two outlets: brings the inlet to `temperature` and `pressure` and splits it into
/// the vapour and liquid in equilibrium there, with [`flash`]. Outlets are positional, **vapour
/// first, liquid second** - the same order Aspen's `Flash2` lists them.
///
/// Isothermal: both outlets leave at `temperature`, and the heat that took - mostly the latent
/// heat of whatever evaporated - is what [`crate::report::duty`] reports.
///
/// `pressure` is set outright, like a feed's, rather than dropped by a `pressure_drop`: a flash
/// drum is specified by the pressure it runs at, and that pressure is the other half of what
/// decides the split.
#[derive(Debug, Clone, Copy)]
pub struct Flash {
    /// The temperature both outlets leave at, in K. Positive.
    pub temperature: f64,
    /// The pressure both outlets leave at, in kPa. Positive.
    pub pressure: f64,
}

impl UnitOp for Flash {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(2)
    }

    /// # Errors
    /// If a flowing species cannot be placed: see [`flash`].
    ///
    /// # Panics
    /// If `temperature` or `pressure` is not finite and positive. See [`flash`].
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        let (vapour, liquid) = flash(registry, inlets[0], self.temperature, self.pressure)?;
        Ok(vec![vapour, liquid])
    }

    /// Evaporating water turns `H2O(l)` into `H2O(g)`, two species by id, so only total mass
    /// balances - the same switch the reactor throws.
    fn conserves_species(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::Phase;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed, humid_nitrogen};

    #[test]
    fn a_flash_returns_the_vapour_first_and_the_liquid_second() {
        let r = humid_nitrogen();
        let inlet = Stream::from_flows(&r, vec![100.0, 0.0, 28.0], 350.0, AMBIENT_KPA);

        let outs = Flash {
            temperature: 350.0,
            pressure: AMBIENT_KPA,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        let (water, steam, nitrogen) = (
            r.find("H2O", Phase::Liquid).unwrap(),
            r.find("H2O", Phase::Gas).unwrap(),
            r.find("N2", Phase::Gas).unwrap(),
        );
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0][nitrogen], 28.0);
        assert!(outs[0][steam] > 0.0);
        assert_eq!(outs[1][nitrogen], 0.0);
        assert!(outs[1][water] > 0.0);
    }

    #[test]
    fn a_flash_of_the_demo_slurry_leaves_everything_in_the_liquid() {
        // The demo's water has no vapour pressure and no gas entry, so it never evaporates.
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Flash {
            temperature: AMBIENT_K,
            pressure: AMBIENT_KPA,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert_eq!(outs[0].total(), 0.0);
        assert_eq!(outs[1].flows(), inlet.flows());
    }

    #[test]
    fn a_flash_does_not_conserve_species() {
        let flash = Flash {
            temperature: AMBIENT_K,
            pressure: AMBIENT_KPA,
        };
        assert!(!flash.conserves_species());
    }
}

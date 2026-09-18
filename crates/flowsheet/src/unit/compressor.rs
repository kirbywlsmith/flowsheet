//! [`Compressor`] - the pump's counterpart for a gas.

use super::{Arity, EvalError, UnitOp, compress};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, one outlet: raises the pressure by `pressure_rise` as an ideal gas along an
/// isentropic path, with [`compress`].
///
/// The `efficiency` is isentropic: the least work that could compress the gas, over the work
/// this machine does. Every joule of it ends up in the gas as temperature, because an ideal gas's
/// enthalpy does not depend on pressure - so unlike a [`super::Pump`], a compressor's whole shaft
/// work shows up in [`crate::report::duty`].
#[derive(Debug, Clone, Copy)]
pub struct Compressor {
    /// Pressure added to the stream, in kPa. Never negative.
    pub pressure_rise: f64,
    /// Isentropic efficiency, in `(0.0, 1.0]`.
    pub efficiency: f64,
}

impl UnitOp for Compressor {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Errors
    /// If a species flowing through is not a gas, or a temperature solve fails. See
    /// [`compress`].
    ///
    /// # Panics
    /// If `pressure_rise` is negative or `efficiency` is outside `(0.0, 1.0]`. See
    /// [`compress`].
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        Ok(vec![compress(
            registry,
            inlets[0],
            self.pressure_rise,
            self.efficiency,
        )?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        AMBIENT_K, AMBIENT_KPA, COMBUSTION_MASSES, combustion_registry, demo_registry, feed,
    };
    use approx::assert_relative_eq;

    #[test]
    fn compressor_returns_one_hotter_outlet_at_the_raised_pressure() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![1.0, 4.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let outs = Compressor {
            pressure_rise: 300.0,
            efficiency: 0.8,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].pressure(), AMBIENT_KPA + 300.0);
        assert!(outs[0].temperature() > AMBIENT_K);
        assert!(inlet.flows_approx_eq(&outs[0], 0.0));
    }

    #[test]
    fn a_liquid_is_an_error_not_a_panic() {
        let r = demo_registry();
        let inlet = feed(&r);

        let e = Compressor {
            pressure_rise: 300.0,
            efficiency: 0.8,
        }
        .evaluate(&r, &[&inlet])
        .expect_err("the demo feed is a slurry");

        assert!(e.to_string().contains("is a solid"), "{e}");
    }
}

//! [`Pump`] - the operation that puts pressure back.

use super::{Arity, EvalError, UnitOp, pump};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, one outlet: raises the pressure by `pressure_rise` as an incompressible fluid,
/// with [`pump`].
///
/// Sized by the rise it delivers, not the pressure it delivers to, so a recycle whose pump does
/// not keep up with the loop's drops still fails to converge rather than being quietly held at a
/// set point - the pressure a loop settles at is a consequence, not a parameter.
///
/// The work is `V * dP / efficiency`, and only the inefficient part of it warms the stream:
/// see [`pump`] for where the rest goes, and [`super::pump_work`] for the whole.
#[derive(Debug, Clone, Copy)]
pub struct Pump {
    /// Pressure added to the stream, in kPa. Never negative.
    pub pressure_rise: f64,
    /// The share of the shaft work that becomes pressure, in `(0.0, 1.0]`. The rest is heat.
    pub efficiency: f64,
}

impl UnitOp for Pump {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Errors
    /// If a species flowing through is a gas or has no density. See [`pump`].
    ///
    /// # Panics
    /// If `pressure_rise` is negative or `efficiency` is outside `(0.0, 1.0]`. See [`pump`].
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        Ok(vec![pump(
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
    use crate::test_support::{demo_registry, demo_registry_with_density, feed};
    use approx::assert_relative_eq;

    #[test]
    fn pump_returns_one_outlet_at_the_raised_pressure() {
        let r = demo_registry_with_density();
        let inlet = feed(&r);

        let outs = Pump {
            pressure_rise: 500.0,
            efficiency: 0.7,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].pressure(), inlet.pressure() + 500.0);
        assert!(inlet.flows_approx_eq(&outs[0], 0.0));
    }

    #[test]
    fn an_inefficient_pump_warms_the_stream() {
        let r = demo_registry_with_density();
        let inlet = feed(&r);

        let outs = Pump {
            pressure_rise: 500.0,
            efficiency: 0.7,
        }
        .evaluate(&r, &[&inlet])
        .unwrap();

        assert!(outs[0].temperature() > inlet.temperature());
    }

    #[test]
    fn a_species_without_a_density_is_an_error_not_a_panic() {
        let r = demo_registry();
        let inlet = feed(&r);

        let e = Pump {
            pressure_rise: 500.0,
            efficiency: 0.7,
        }
        .evaluate(&r, &[&inlet])
        .expect_err("the demo species carry no density");

        assert!(e.to_string().contains("has no `density`"), "{e}");
    }
}

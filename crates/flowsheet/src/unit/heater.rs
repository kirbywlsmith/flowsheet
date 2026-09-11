//! [`Heater`] - the only operation that exchanges heat with its surroundings.

use super::{Arity, UnitOp, heat};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// One inlet, one outlet: adds a fixed `duty` to the inlet's enthalpy flow with [`heat`].
///
/// `duty` is in MJ/h, the same unit as [`Stream::enthalpy`]. A negative duty makes this a cooler,
/// so there is no separate `Cooler`: the two would differ only in the sign of one field.
///
/// The duty is the input and the outlet temperature is solved for, which is why this op goes
/// through the energy balance. A heater can also be specified the other way round, fixing the
/// outlet temperature and reporting the duty. That version needs no temperature solve, and it is
/// not implemented here.
#[derive(Debug, Clone, Copy)]
pub struct Heater {
    /// Heat added to the stream, in MJ/h. Negative removes it.
    pub duty: f64,
}

impl UnitOp for Heater {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    /// # Panics
    /// If `duty` is not finite, or cools the inlet to absolute zero or below. See [`heat`].
    fn evaluate(&self, registry: &SpeciesRegistry, inlets: &[&Stream]) -> Vec<Stream> {
        vec![heat(registry, inlets[0], self.duty)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_registry, feed};
    use approx::assert_relative_eq;

    #[test]
    fn heater_returns_one_outlet_carrying_the_duty() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Heater { duty: 20_000.0 }.evaluate(&r, &[&inlet]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(
            outs[0].enthalpy(&r) - inlet.enthalpy(&r),
            20_000.0,
            max_relative = 1e-9
        );
    }

    #[test]
    fn a_negative_duty_makes_it_a_cooler() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Heater { duty: -20_000.0 }.evaluate(&r, &[&inlet]);

        assert!(outs[0].temperature() < inlet.temperature());
    }
}

//! [`Tank`] - a pass-through at steady state.

use super::{Arity, UnitOp};
use crate::stream::Stream;

/// One inlet, one outlet: passes its inlet straight through.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tank;

impl UnitOp for Tank {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn evaluate(&self, inlets: &[&Stream]) -> Vec<Stream> {
        vec![inlets[0].clone()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_registry, feed};

    #[test]
    fn tank_passes_its_inlet_straight_through() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = Tank.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 1);
        assert!(inlet.flows_approx_eq(&outs[0], 1e-12));
    }
}

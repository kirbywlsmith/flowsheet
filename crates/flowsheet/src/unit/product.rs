//! [`Product`] - where material leaves the flowsheet.

use super::{Arity, UnitOp};
use crate::stream::Stream;

/// One inlet, no outlets: where material leaves the flowsheet.
#[derive(Debug, Clone, Copy, Default)]
pub struct Product;

impl UnitOp for Product {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(0)
    }

    fn evaluate(&self, _inlets: &[&Stream]) -> Vec<Stream> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_registry, feed};

    #[test]
    fn product_consumes_its_inlet_and_emits_nothing() {
        let r = demo_registry();
        let outs = Product.evaluate(&[&feed(&r)]);
        assert!(outs.is_empty());
    }
}

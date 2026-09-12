//! [`Feed`] - where material enters the flowsheet.

use super::{Arity, EvalError, UnitOp};
use crate::species::SpeciesRegistry;
use crate::stream::Stream;

/// No inlets, one outlet: emits a fixed [`Stream`] into the flowsheet.
#[derive(Debug, Clone)]
pub struct Feed {
    /// The outlet [`Stream`].
    pub stream: Stream,
}

impl UnitOp for Feed {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(0)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn evaluate(
        &self,
        _registry: &SpeciesRegistry,
        _inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError> {
        Ok(vec![self.stream.clone()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{demo_registry, feed};
    use approx::assert_relative_eq;

    #[test]
    fn feed_emits_its_stream_and_ignores_inlets() {
        let r = demo_registry();
        let op = Feed { stream: feed(&r) };

        let outs = op.evaluate(&r, &[]).unwrap();

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1000.0);
    }
}

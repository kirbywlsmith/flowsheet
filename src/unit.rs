//! Unit operations — what a unit is, how many streams it accepts, and what it produces.

use crate::flowsheet::StreamId;
use crate::stream::Stream;

/// The different types of supported unit operations and their parameters.
#[derive(Debug, Clone)]
pub enum UnitOp {
    /// One outlet.
    Feed {
        /// The outlet [`Stream`].
        stream: Stream,
    },
    /// Combines all inlets into one outlet.
    Mixer,
    /// One inlet, two outlets: `fraction` and `1.0 - fraction`.
    Splitter {
        /// The `fraction` to pass to [`split`]
        fraction: f64,
    },
    /// One inlet, one outlet per ratio.
    SplitterN {
        /// The `ratios` to pass to [`split_n`]
        ratios: Vec<f64>,
    },
    /// One inlet, one outlet
    Tank,
    /// One inlet
    Product,
}

/// How many streams a [`UnitOp`] accepts on one side. `max` of `None` means unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Arity {
    pub(crate) min: usize,
    pub(crate) max: Option<usize>,
}

impl Arity {
    /// Exactly `n` streams.
    pub(crate) const fn exactly(n: usize) -> Self {
        Self {
            min: n,
            max: Some(n),
        }
    }

    /// At least `n` streams, with no upper bound.
    pub(crate) const fn at_least(n: usize) -> Self {
        Self { min: n, max: None }
    }

    /// Whether `found` streams satisfies this arity.
    pub(crate) fn permits(self, found: usize) -> bool {
        found >= self.min && self.max.is_none_or(|max| found <= max)
    }
}

impl UnitOp {
    /// How many inlet [`Stream`]s this operation requires.
    pub(crate) fn inlet_arity(&self) -> Arity {
        match self {
            UnitOp::Feed { .. } => Arity::exactly(0),
            UnitOp::Mixer => Arity::at_least(1),
            UnitOp::Splitter { .. } | UnitOp::SplitterN { .. } => Arity::exactly(1),
            UnitOp::Tank => Arity::exactly(1),
            UnitOp::Product => Arity::exactly(1),
        }
    }

    /// How many outlet [`Stream`]s this operation produces.
    pub(crate) fn outlet_arity(&self) -> Arity {
        match self {
            UnitOp::Feed { .. } => Arity::exactly(1),
            UnitOp::Mixer => Arity::exactly(1),
            UnitOp::Splitter { .. } => Arity::exactly(2),
            UnitOp::SplitterN { ratios } => Arity::exactly(ratios.len()),
            UnitOp::Tank => Arity::exactly(1),
            UnitOp::Product => Arity::exactly(0),
        }
    }

    /// Evaluates a unit operation's outlet [`Stream`]s.
    pub(crate) fn evaluate(&self, inlets: &[&Stream]) -> Vec<Stream> {
        match self {
            UnitOp::Feed { stream } => vec![stream.clone()],
            UnitOp::Mixer => {
                vec![mix(inlets.iter().copied()).expect("mixer needs at least one inlet")]
            }
            UnitOp::Tank => vec![inlets[0].clone()],
            UnitOp::Splitter { fraction } => {
                let (a, b) = split(inlets[0], *fraction);
                vec![a, b]
            }
            UnitOp::SplitterN { ratios } => split_n(inlets[0], ratios),
            UnitOp::Product => vec![],
        }
    }
}

/// A distinct section of a system that takes inlet [`Stream`]s and performs a [`UnitOp`] to produce outlet streams.
#[derive(Debug)]
pub struct Unit {
    pub(crate) name: String,
    pub(crate) op: UnitOp,
    pub(crate) inlets: Vec<StreamId>,
    pub(crate) outlets: Vec<StreamId>,
}

/// Mixes a number of inlets into a single combined [`Stream`].
pub fn mix<'a>(inlets: impl IntoIterator<Item = &'a Stream>) -> Option<Stream> {
    let mut inlets = inlets.into_iter();
    let mut result = inlets.next()?.clone();
    for s in inlets {
        result += s;
    }
    Some(result)
}

/// Splits an inlet into a (`fraction`, `1.0 - fraction`) scaled [`Stream`] tuple.
///
/// # Panics
/// If `fraction` is:
/// - Less than 0.0; or
/// - Greater than 1.0; or
/// - NaN.
pub fn split(inlet: &Stream, fraction: f64) -> (Stream, Stream) {
    assert!(
        (0.0..=1.0).contains(&fraction),
        "split fraction must be between 0.0 and 1.0, got {fraction}"
    );

    (inlet.scaled(fraction), inlet.scaled(1.0 - fraction))
}

/// Splits an inlet into one outlet per entry in `ratios`, in the same order.
///
/// Ratios are relative: `[3.0, 7.0]` and `[0.3, 0.7]` both give a 30/70 split.
///
/// # Panics
/// If `ratios` is empty, contains a negative or NaN value, or sums to zero.
pub fn split_n(inlet: &Stream, ratios: &[f64]) -> Vec<Stream> {
    assert!(!ratios.is_empty(), "split_n needs at least one ratio");
    assert!(
        ratios.iter().all(|r| *r >= 0.0),
        "split ratios must be non-negative and not NaN, got {ratios:?}"
    );

    let sum: f64 = ratios.iter().sum();
    assert!(sum > 0.0, "split ratios must not all be zero");

    ratios.iter().map(|r| inlet.scaled(r / sum)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, all_ids, demo_registry, feed};
    use approx::assert_relative_eq;

    // ---- mix ----

    #[test]
    fn mixing_nothing_gives_nothing() {
        assert!(mix(&[]).is_none());
    }

    #[test]
    fn mixing_one_inlet_reproduces_it() {
        let r = demo_registry();
        let s = feed(&r);
        let out = mix(std::slice::from_ref(&s)).unwrap();
        assert!(s.flows_approx_eq(&out, 1e-12));
    }

    #[test]
    fn mixing_sums_each_species_and_the_total() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let out = mix(&[a.clone(), b.clone()]).unwrap();

        for id in all_ids(&r) {
            assert_relative_eq!(out[id], a[id] + b[id], max_relative = 1e-12);
        }
        assert_relative_eq!(out.total(), 1060.0, max_relative = 1e-12);
    }

    #[test]
    fn mixing_takes_the_first_inlets_temperature() {
        // Documents a known limitation: the true mixed temperature needs an
        // enthalpy balance, so `+=` leaves the accumulator's T and P alone.
        let r = demo_registry();
        let cold = feed(&r);
        let hot = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], 400.0, 500.0);

        let out = mix(&[cold, hot]).unwrap();

        assert_relative_eq!(out.temperature(), AMBIENT_K);
        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    // ---- split ----

    #[test]
    fn splitting_conserves_every_species() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (a, b) = split(&inlet, 0.3);

        for id in all_ids(&r) {
            assert_relative_eq!(a[id] + b[id], inlet[id], max_relative = 1e-12);
        }
        assert_relative_eq!(a.total() + b.total(), inlet.total(), max_relative = 1e-12);
        assert_relative_eq!(a.total(), 300.0, max_relative = 1e-12);
    }

    #[test]
    fn splitting_does_not_change_the_composition() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (a, b) = split(&inlet, 0.3);

        assert_eq!(a.mass_fractions(), inlet.mass_fractions());
        assert_eq!(b.mass_fractions(), inlet.mass_fractions());
        assert_relative_eq!(a.temperature(), inlet.temperature());
    }

    #[test]
    fn splitting_at_the_endpoints_is_allowed() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (all, none) = split(&inlet, 1.0);
        assert_relative_eq!(all.total(), 1000.0, max_relative = 1e-12);
        assert_relative_eq!(none.total(), 0.0);

        let (none, all) = split(&inlet, 0.0);
        assert_relative_eq!(none.total(), 0.0);
        assert_relative_eq!(all.total(), 1000.0, max_relative = 1e-12);
    }

    #[test]
    #[should_panic(expected = "split fraction must be between 0.0 and 1.0")]
    fn splitting_above_one_panics() {
        let r = demo_registry();
        split(&feed(&r), 1.5);
    }

    #[test]
    #[should_panic(expected = "split fraction must be between 0.0 and 1.0")]
    fn splitting_by_nan_panics() {
        let r = demo_registry();
        split(&feed(&r), f64::NAN);
    }

    // ---- split_n ----

    #[test]
    fn split_n_normalises_ratios_that_do_not_sum_to_one() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = split_n(&inlet, &[3.0, 7.0]);

        assert_eq!(outs.len(), 2);
        assert_relative_eq!(outs[0].total(), 300.0, max_relative = 1e-12);
        assert_relative_eq!(outs[1].total(), 700.0, max_relative = 1e-12);
    }

    #[test]
    fn split_n_conserves_mass_whatever_the_ratio_order() {
        // [0.7, 0.2, 0.1] sums to 0.9999999999999999 in f64, so an exact
        // `sum == 1.0` check would reject this valid input.
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = split_n(&inlet, &[0.7, 0.2, 0.1]);
        let recombined = mix(&outs).unwrap();

        assert_eq!(outs.len(), 3);
        assert!(inlet.flows_approx_eq(&recombined, 1e-12));
    }

    #[test]
    fn split_n_into_equal_parts() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = split_n(&inlet, &[1.0, 1.0, 1.0]);

        for out in &outs {
            assert_relative_eq!(out.total(), 1000.0 / 3.0, max_relative = 1e-12);
        }
        assert!(inlet.flows_approx_eq(&mix(&outs).unwrap(), 1e-12));
    }

    #[test]
    #[should_panic(expected = "at least one ratio")]
    fn split_n_rejects_an_empty_ratio_list() {
        let r = demo_registry();
        split_n(&feed(&r), &[]);
    }

    #[test]
    #[should_panic(expected = "non-negative")]
    fn split_n_rejects_a_negative_ratio() {
        let r = demo_registry();
        split_n(&feed(&r), &[0.5, -0.5]);
    }

    #[test]
    #[should_panic(expected = "must not all be zero")]
    fn split_n_rejects_all_zero_ratios() {
        let r = demo_registry();
        split_n(&feed(&r), &[0.0, 0.0]);
    }

    // ---- evaluate ----

    #[test]
    fn feed_emits_its_stream_and_ignores_inlets() {
        let r = demo_registry();
        let op = UnitOp::Feed { stream: feed(&r) };

        let outs = op.evaluate(&[]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1000.0);
    }

    #[test]
    fn mixer_sums_all_inlets_into_one_outlet() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let outs = UnitOp::Mixer.evaluate(&[&a, &b]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1060.0, max_relative = 1e-12);
    }

    #[test]
    fn tank_passes_its_inlet_straight_through() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Tank.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 1);
        assert!(inlet.flows_approx_eq(&outs[0], 1e-12));
    }

    #[test]
    fn splitter_returns_the_fraction_side_first() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Splitter { fraction: 0.3 }.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 2);
        assert_relative_eq!(outs[0].total(), 300.0, max_relative = 1e-12);
        assert_relative_eq!(outs[1].total(), 700.0, max_relative = 1e-12);
    }

    #[test]
    fn splitter_n_returns_one_outlet_per_ratio() {
        let r = demo_registry();
        let inlet = feed(&r);
        let op = UnitOp::SplitterN {
            ratios: vec![1.0, 1.0, 2.0],
        };

        let outs = op.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 3);
        assert_relative_eq!(outs[0].total(), 250.0, max_relative = 1e-12);
        assert_relative_eq!(outs[2].total(), 500.0, max_relative = 1e-12);
    }

    #[test]
    fn product_consumes_its_inlet_and_emits_nothing() {
        let r = demo_registry();
        let outs = UnitOp::Product.evaluate(&[&feed(&r)]);
        assert!(outs.is_empty());
    }

    // ---- arity ----

    #[test]
    fn arity_permits_only_counts_inside_the_range() {
        assert!(!Arity::exactly(2).permits(1));
        assert!(Arity::exactly(2).permits(2));
        assert!(!Arity::exactly(2).permits(3));

        assert!(!Arity::at_least(1).permits(0));
        assert!(Arity::at_least(1).permits(1));
        assert!(Arity::at_least(1).permits(9));
    }

    #[test]
    fn every_op_declares_the_arity_its_evaluate_assumes() {
        let r = demo_registry();
        let cases = [
            (
                UnitOp::Feed { stream: feed(&r) },
                (0, Some(0)),
                (1, Some(1)),
            ),
            // The mixer is the only unbounded side in the model.
            (UnitOp::Mixer, (1, None), (1, Some(1))),
            (UnitOp::Tank, (1, Some(1)), (1, Some(1))),
            (
                UnitOp::Splitter { fraction: 0.3 },
                (1, Some(1)),
                (2, Some(2)),
            ),
            (
                UnitOp::SplitterN {
                    ratios: vec![1.0, 1.0, 2.0],
                },
                (1, Some(1)),
                (3, Some(3)),
            ),
            (UnitOp::Product, (1, Some(1)), (0, Some(0))),
        ];

        for (op, inlets, outlets) in cases {
            assert_eq!(
                (op.inlet_arity().min, op.inlet_arity().max),
                inlets,
                "inlet arity of {op:?}"
            );
            assert_eq!(
                (op.outlet_arity().min, op.outlet_arity().max),
                outlets,
                "outlet arity of {op:?}"
            );

            let stream = feed(&r);
            let supplied: Vec<&Stream> = vec![&stream; op.inlet_arity().min];
            assert_eq!(
                op.evaluate(&supplied).len(),
                op.outlet_arity().min,
                "{op:?} returned an outlet count its arity does not declare"
            );
        }
    }
}

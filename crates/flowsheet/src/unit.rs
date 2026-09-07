//! Unit operations — what a unit is, how many streams it accepts, and what it produces.

use crate::flowsheet::StreamId;
use crate::serial::ToDocument;
use crate::stream::Stream;
use std::fmt::Debug;

/// What a unit does: how many streams it connects, and how it turns inlets into outlets.
///
/// Implementations are held as `Box<dyn UnitOp>`, so the set of operations is open - a
/// downstream crate can add one without touching this module. The trade against the enum this
/// replaced is dispatch: every call here goes through a vtable rather than a jump table.
///
/// [`Debug`] is a supertrait because [`crate::flowsheet::Flowsheet`] derives it, and a derive
/// can only reach through the box if the trait object itself is `Debug`. [`ToDocument`] is a
/// supertrait for the opposite reason: saving needs to know *which* operation this is, and a
/// trait object has erased that, so the operation has to say so itself.
///
/// [`Send`] and [`Sync`] are supertraits so that boxing an operation does not cost
/// [`crate::flowsheet::Flowsheet`] its own auto-derived `Send`/`Sync`. The enum had them for
/// free; a bare `Box<dyn UnitOp>` has neither, and a `Flowsheet` that is not `Sync` cannot be
/// shared across threads - which is the whole reason the topological sort emits waves. The
/// price is that an operation may not hold an [`std::rc::Rc`] or a [`std::cell::Cell`].
pub trait UnitOp: Debug + Send + Sync + ToDocument {
    /// How many inlet [`Stream`]s this operation requires.
    fn inlet_arity(&self) -> Arity;

    /// How many outlet [`Stream`]s this operation produces.
    fn outlet_arity(&self) -> Arity;

    /// Evaluates this operation's outlet [`Stream`]s.
    ///
    /// `inlets` is guaranteed to satisfy [`UnitOp::inlet_arity`] - [`crate::flowsheet::Flowsheet::check`]
    /// rejects anything else before a solve starts - so an implementation may index it directly.
    fn evaluate(&self, inlets: &[&Stream]) -> Vec<Stream>;
}

/// Boxes any operation, so [`crate::flowsheet::Flowsheet::add_unit`] accepts either a bare
/// `Mixer` or an already-boxed operation - the reflexive `impl From<T> for T` covers the latter.
///
/// Same shape as the standard library's `impl<E: Error> From<E> for Box<dyn Error>`.
impl<T: UnitOp + 'static> From<T> for Box<dyn UnitOp> {
    fn from(op: T) -> Self {
        Box::new(op)
    }
}

/// How many streams a [`UnitOp`] accepts on one side. `max` of `None` means unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arity {
    /// The fewest streams accepted.
    pub(crate) min: usize,
    /// The most streams accepted, or `None` when unbounded.
    pub(crate) max: Option<usize>,
}

impl Arity {
    /// Exactly `n` streams.
    pub const fn exactly(n: usize) -> Self {
        Self {
            min: n,
            max: Some(n),
        }
    }

    /// At least `n` streams, with no upper bound.
    pub const fn at_least(n: usize) -> Self {
        Self { min: n, max: None }
    }

    /// Whether `found` streams satisfies this arity.
    pub fn permits(self, found: usize) -> bool {
        found >= self.min && self.max.is_none_or(|max| found <= max)
    }
}

mod feed;
mod flotation;
mod mixer;
mod product;
mod splitter;
mod tank;

// Re-exported flat, so every call site keeps writing `unit::Mixer` rather than
// `unit::mixer::Mixer`. The submodules stay private: they are a file-layout detail, and one
// operation per file is the only thing they buy.
pub use feed::Feed;
pub use flotation::Flotation;
pub use mixer::Mixer;
pub use product::Product;
pub use splitter::{Splitter, SplitterN};
pub use tank::Tank;

/// A distinct section of a system that takes inlet [`Stream`]s and performs a [`UnitOp`] to produce outlet streams.
#[derive(Debug)]
pub struct Unit {
    pub(crate) name: String,
    pub(crate) op: Box<dyn UnitOp>,
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

/// Splits an inlet into a (concentrate, tails) pair by recovering each species separately.
///
/// `recovery[i]` is the fraction of species `i` reporting to the concentrate. Unlike [`split`],
/// this changes the composition of both outlets - it is what a [`Flotation`] cell does.
///
/// # Panics
/// If `recovery`:
/// - Has a different length than the inlet's species count; or
/// - Contains a value below 0.0, above 1.0, or NaN.
pub fn recover(inlet: &Stream, recovery: &[f64]) -> (Stream, Stream) {
    assert_eq!(
        recovery.len(),
        inlet.species_count(),
        "flotation needs one recovery per species"
    );
    assert!(
        recovery.iter().all(|r| (0.0..=1.0).contains(r)),
        "flotation recovery must be between 0.0 and 1.0, got {recovery:?}"
    );

    let mut concentrate = inlet.clone();
    let mut tails = inlet.clone();
    for (i, r) in recovery.iter().enumerate() {
        concentrate.flows_mut()[i] = inlet.flows()[i] * r;
        // Not `inlet - concentrate`: `1.0 - r` keeps the two sides symmetric, and neither
        // can come out negative from a rounding error the way a subtraction could.
        tails.flows_mut()[i] = inlet.flows()[i] * (1.0 - r);
    }

    (concentrate, tails)
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

    // ---- recover ----

    #[test]
    fn recover_conserves_every_species() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (concentrate, tails) = recover(&inlet, &[0.85, 0.05, 0.30]);

        for id in all_ids(&r) {
            assert_relative_eq!(concentrate[id] + tails[id], inlet[id], max_relative = 1e-12);
        }
    }

    #[test]
    fn recover_moves_each_species_at_its_own_rate() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (concentrate, _) = recover(&inlet, &[0.85, 0.05, 0.30]);

        assert_relative_eq!(concentrate.flows()[0], 34.0, max_relative = 1e-12);
        assert_relative_eq!(concentrate.flows()[1], 18.0, max_relative = 1e-12);
        assert_relative_eq!(concentrate.flows()[2], 180.0, max_relative = 1e-12);
    }

    #[test]
    fn recovering_everything_leaves_empty_tails() {
        let r = demo_registry();
        let inlet = feed(&r);

        let (concentrate, tails) = recover(&inlet, &[1.0, 1.0, 1.0]);

        assert!(inlet.flows_approx_eq(&concentrate, 1e-12));
        assert_relative_eq!(tails.total(), 0.0);
    }

    #[test]
    fn recover_keeps_the_inlet_temperature_on_both_sides() {
        // Same limitation as `mix`: no energy balance yet, so T and P ride along unchanged.
        let r = demo_registry();
        let inlet = feed(&r);

        let (concentrate, tails) = recover(&inlet, &[0.85, 0.05, 0.30]);

        assert_relative_eq!(concentrate.temperature(), inlet.temperature());
        assert_relative_eq!(tails.pressure(), inlet.pressure());
    }

    #[test]
    #[should_panic(expected = "one recovery per species")]
    fn recover_rejects_a_recovery_of_the_wrong_length() {
        let r = demo_registry();
        recover(&feed(&r), &[0.85, 0.05]);
    }

    #[test]
    #[should_panic(expected = "recovery must be between 0.0 and 1.0")]
    fn recover_rejects_a_negative_recovery() {
        let r = demo_registry();
        recover(&feed(&r), &[-0.1, 0.05, 0.30]);
    }

    #[test]
    #[should_panic(expected = "recovery must be between 0.0 and 1.0")]
    fn recover_by_nan_panics() {
        let r = demo_registry();
        recover(&feed(&r), &[f64::NAN, 0.05, 0.30]);
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

        // `Box<dyn UnitOp>` is what lets one array hold seven different concrete types. The
        // enum version of this test relied on them all being the same type instead.
        type Case = (
            Box<dyn UnitOp>,
            (usize, Option<usize>),
            (usize, Option<usize>),
        );
        let cases: Vec<Case> = vec![
            (
                Box::new(Feed { stream: feed(&r) }),
                (0, Some(0)),
                (1, Some(1)),
            ),
            // The mixer is the only unbounded side in the model.
            (Box::new(Mixer), (1, None), (1, Some(1))),
            (Box::new(Tank), (1, Some(1)), (1, Some(1))),
            (
                Box::new(Splitter { fraction: 0.3 }),
                (1, Some(1)),
                (2, Some(2)),
            ),
            (
                Box::new(SplitterN {
                    ratios: vec![1.0, 1.0, 2.0],
                }),
                (1, Some(1)),
                (3, Some(3)),
            ),
            (
                Box::new(Flotation {
                    recovery: vec![0.85, 0.05, 0.30],
                }),
                (1, Some(1)),
                (2, Some(2)),
            ),
            (Box::new(Product), (1, Some(1)), (0, Some(0))),
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

    /// The `Send + Sync` supertraits on [`UnitOp`] exist only to keep this true; nothing else
    /// in the crate would fail if they were dropped, so the guarantee needs its own test.
    #[test]
    fn a_valid_flowsheet_can_still_cross_a_thread_boundary() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::flowsheet::ValidFlowsheet>();
    }
}

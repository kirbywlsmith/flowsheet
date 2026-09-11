//! Unit operations — what a unit is, how many streams it accepts, and what it produces.

use crate::flowsheet::StreamId;
use crate::serial::ToDocument;
use crate::species::SpeciesRegistry;
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
    fn evaluate(&self, registry: &SpeciesRegistry, inlets: &[&Stream]) -> Vec<Stream>;
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
mod heater;
mod mixer;
mod product;
mod splitter;
mod tank;

// Re-exported flat, so every call site keeps writing `unit::Mixer` rather than
// `unit::mixer::Mixer`. The submodules stay private: they are a file-layout detail, and one
// operation per file is the only thing they buy.
pub use feed::Feed;
pub use flotation::Flotation;
pub use heater::Heater;
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
///
/// Flows add. Mixing is adiabatic (no heat enters or leaves), so the outlet's enthalpy flow is
/// the sum of the inlets', and [`solve_temperature`] finds the temperature that makes that true. The outlet takes the first inlet's pressure, because nothing solves
/// pressure yet.
///
/// If every inlet is empty there is no mass to put a temperature on, so the first inlet's
/// temperature is kept.
///
/// Returns `None` if `inlets` is empty.
///
/// # Panics
/// If an inlet's species count does not match `registry`.
pub fn mix<'a>(
    registry: &SpeciesRegistry,
    inlets: impl IntoIterator<Item = &'a Stream>,
) -> Option<Stream> {
    let mut inlets = inlets.into_iter().peekable();
    let first = *inlets.peek()?;
    let mut outlet = Stream::zeros(registry, first.temperature(), first.pressure());

    let mut enthalpy = 0.0;
    // The first guess for the temperature is sum(C * T) / sum(C), with C each inlet's heat
    // capacity flow at its own temperature. When cp is constant that ratio *is* the answer, and
    // when it varies it is close enough that Newton needs a step or two.
    let mut heat_capacity = 0.0;
    let mut weighted_temperature = 0.0;

    for inlet in inlets {
        outlet += inlet;
        let c = inlet.heat_capacity(registry);
        enthalpy += inlet.enthalpy(registry);
        heat_capacity += c;
        weighted_temperature += c * inlet.temperature();
    }

    if heat_capacity == 0.0 {
        return Some(outlet); // exact-zero guard: every inlet is empty
    }

    outlet.set_temperature(weighted_temperature / heat_capacity);
    solve_temperature(registry, &mut outlet, enthalpy);
    Some(outlet)
}

/// How close two Newton iterates must come, in Kelvin, before [`solve_temperature`] stops.
///
/// Far tighter than the solver's own tolerance, on purpose: an inner solve that stops early
/// shows up as noise in the outer loop's residual, and the outer loop cannot converge past it.
const TEMPERATURE_TOLERANCE_K: f64 = 1e-9;

/// Newton needs two or three steps from a good guess. This bound only exists to catch a bug.
const MAX_NEWTON_STEPS: usize = 50;

/// Sets `stream`'s temperature to the one at which its enthalpy flow equals `enthalpy` (MJ/h),
/// starting Newton's method from the temperature the stream already has.
///
/// The function being zeroed is `f(T) = H(T) - enthalpy`, and its slope `H'(T)` is the stream's
/// heat capacity flow. Wherever cp is positive so is that slope, so `f` only ever rises and
/// crosses zero exactly once. Newton has no second root to wander towards.
///
/// # Panics
/// If the stream has no heat capacity (an empty stream has zero enthalpy at every temperature),
/// if an iterate leaves the positive reals, or if 50 steps pass without converging.
pub fn solve_temperature(registry: &SpeciesRegistry, stream: &mut Stream, enthalpy: f64) {
    for _ in 0..MAX_NEWTON_STEPS {
        let slope = stream.heat_capacity(registry);
        assert!(
            slope > 0.0,
            "cannot solve the temperature of a stream with no heat capacity"
        );

        let step = (stream.enthalpy(registry) - enthalpy) / slope;
        let next = stream.temperature() - step;
        assert!(
            next.is_finite() && next > 0.0,
            "Newton's method left the physical range at {next} K"
        );
        stream.set_temperature(next);

        if step.abs() <= TEMPERATURE_TOLERANCE_K {
            return;
        }
    }
    panic!("no temperature converged after {MAX_NEWTON_STEPS} Newton steps");
}

/// Adds `duty` (MJ/h) to an inlet's enthalpy flow and returns the stream at the temperature that
/// results. A positive duty heats, a negative one cools. Flows and pressure pass through unchanged.
///
/// An empty inlet is returned unchanged and the duty is ignored. Its heat capacity is zero, so no
/// finite temperature would absorb the duty. A heater downstream of a tear stream sees an empty
/// inlet on the first pass, while the tear still holds its all-zero guess, so this case returns
/// the inlet instead of panicking.
///
/// # Panics
/// If `duty` is not finite, or if it cools the stream to absolute zero or below (see
/// [`solve_temperature`]). A duty sized for the converged loop can do that on an early pass that
/// carries a fraction of the converged flow.
pub fn heat(registry: &SpeciesRegistry, inlet: &Stream, duty: f64) -> Stream {
    assert!(duty.is_finite(), "heater duty must be finite, got {duty}");

    let mut outlet = inlet.clone();
    if inlet.heat_capacity(registry) == 0.0 {
        return outlet; // exact-zero guard: the inlet is empty
    }

    // Newton starts from the inlet temperature, so its first step lands on T + duty / C. That is
    // exact when cp is constant, and within a step or two of the answer otherwise.
    solve_temperature(registry, &mut outlet, inlet.enthalpy(registry) + duty);
    outlet
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
        let r = demo_registry();
        assert!(mix(&r, &[]).is_none());
    }

    #[test]
    fn mixing_one_inlet_reproduces_it() {
        let r = demo_registry();
        let s = feed(&r);
        let out = mix(&r, std::slice::from_ref(&s)).unwrap();
        assert!(s.flows_approx_eq(&out, 1e-12));
    }

    #[test]
    fn mixing_sums_each_species_and_the_total() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let out = mix(&r, &[a.clone(), b.clone()]).unwrap();

        for id in all_ids(&r) {
            assert_relative_eq!(out[id], a[id] + b[id], max_relative = 1e-12);
        }
        assert_relative_eq!(out.total(), 1060.0, max_relative = 1e-12);
    }

    #[test]
    fn mixing_conserves_enthalpy() {
        let r = demo_registry();
        let cold = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 290.0, AMBIENT_KPA);
        let hot = Stream::from_flows(&r, vec![10.0, 20.0, 300.0], 370.0, AMBIENT_KPA);

        let out = mix(&r, [&cold, &hot]).unwrap();

        assert_relative_eq!(
            out.enthalpy(&r),
            cold.enthalpy(&r) + hot.enthalpy(&r),
            max_relative = 1e-9
        );
        assert!(290.0 < out.temperature() && out.temperature() < 370.0);
    }

    #[test]
    fn a_constant_heat_capacity_mixes_to_the_mass_weighted_mean() {
        // Chalcopyrite is the demo's one constant-cp species, and with a single species the heat
        // capacity weights are just the masses: (100 * 300 + 300 * 400) / 400 = 375 K.
        let r = demo_registry();
        let a = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], 300.0, AMBIENT_KPA);
        let b = Stream::from_flows(&r, vec![300.0, 0.0, 0.0], 400.0, AMBIENT_KPA);

        let out = mix(&r, [&a, &b]).unwrap();

        assert_relative_eq!(out.temperature(), 375.0, max_relative = 1e-12);
    }

    #[test]
    fn inlets_at_one_temperature_mix_to_that_temperature() {
        let r = demo_registry();
        let a = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 340.0, AMBIENT_KPA);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], 340.0, AMBIENT_KPA);

        let out = mix(&r, [&a, &b]).unwrap();

        assert_relative_eq!(out.temperature(), 340.0, max_relative = 1e-12);
    }

    #[test]
    fn an_empty_inlet_leaves_the_mixed_temperature_alone() {
        // What a recycle's first pass looks like: the feed meets a tear stream with no flow yet.
        // The empty one goes first, to prove its temperature is not simply kept.
        let r = demo_registry();
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let hot = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 350.0, AMBIENT_KPA);

        let out = mix(&r, [&empty, &hot]).unwrap();

        assert_relative_eq!(out.temperature(), 350.0, max_relative = 1e-12);
    }

    #[test]
    fn mixing_only_empty_inlets_keeps_the_first_temperature() {
        let r = demo_registry();
        let a = Stream::zeros(&r, 310.0, AMBIENT_KPA);
        let b = Stream::zeros(&r, 290.0, AMBIENT_KPA);

        let out = mix(&r, [&a, &b]).unwrap();

        assert_relative_eq!(out.temperature(), 310.0);
    }

    #[test]
    fn mixing_keeps_the_first_inlets_pressure() {
        // Nothing solves pressure yet. See the pressure item in TODO.md.
        let r = demo_registry();
        let low = feed(&r);
        let high = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, 500.0);

        let out = mix(&r, [&low, &high]).unwrap();

        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    #[test]
    fn solve_temperature_inverts_enthalpy() {
        let r = demo_registry();
        let mut s = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 420.0, AMBIENT_KPA);
        let enthalpy = s.enthalpy(&r);

        s.set_temperature(300.0); // a deliberately poor first guess
        solve_temperature(&r, &mut s, enthalpy);

        assert_relative_eq!(s.temperature(), 420.0, epsilon = TEMPERATURE_TOLERANCE_K);
    }

    #[test]
    #[should_panic(expected = "no heat capacity")]
    fn an_empty_stream_has_no_temperature_to_solve_for() {
        let r = demo_registry();
        let mut empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        solve_temperature(&r, &mut empty, 1.0);
    }

    // ---- heat ----

    #[test]
    fn heating_adds_the_duty_to_the_enthalpy_flow() {
        let r = demo_registry();
        let inlet = feed(&r);

        let out = heat(&r, &inlet, 50_000.0);

        assert_relative_eq!(
            out.enthalpy(&r),
            inlet.enthalpy(&r) + 50_000.0,
            max_relative = 1e-9
        );
        assert!(out.temperature() > inlet.temperature());
    }

    #[test]
    fn a_negative_duty_cools() {
        let r = demo_registry();
        let inlet = feed(&r);

        let out = heat(&r, &inlet, -50_000.0);

        assert!(out.temperature() < inlet.temperature());
    }

    #[test]
    fn a_constant_heat_capacity_rises_by_duty_over_heat_capacity() {
        // Chalcopyrite alone, so cp is constant and the answer is linear:
        // 100 t/h * 95.0 / 183.5 kJ/(kg·K) = 51.77 MJ/(h·K), so 5177 MJ/h raises it 100 K.
        let r = demo_registry();
        let inlet = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], 300.0, AMBIENT_KPA);
        let c = 100.0 * 95.0 / 183.5;

        let out = heat(&r, &inlet, 100.0 * c);

        assert_relative_eq!(out.temperature(), 400.0, max_relative = 1e-12);
    }

    #[test]
    fn heating_changes_neither_flows_nor_pressure() {
        let r = demo_registry();
        let inlet = feed(&r);

        let out = heat(&r, &inlet, 50_000.0);

        assert!(inlet.flows_approx_eq(&out, 0.0));
        assert_relative_eq!(out.pressure(), inlet.pressure());
    }

    #[test]
    fn zero_duty_keeps_the_inlet_temperature() {
        let r = demo_registry();
        let inlet = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 350.0, AMBIENT_KPA);

        let out = heat(&r, &inlet, 0.0);

        assert_relative_eq!(out.temperature(), 350.0, epsilon = TEMPERATURE_TOLERANCE_K);
    }

    #[test]
    fn heating_an_empty_stream_passes_it_through() {
        let r = demo_registry();
        let empty = Stream::zeros(&r, 310.0, AMBIENT_KPA);

        let out = heat(&r, &empty, 50_000.0);

        assert_relative_eq!(out.total(), 0.0);
        assert_relative_eq!(out.temperature(), 310.0);
    }

    #[test]
    #[should_panic(expected = "heater duty must be finite")]
    fn heating_by_nan_panics() {
        let r = demo_registry();
        heat(&r, &feed(&r), f64::NAN);
    }

    #[test]
    #[should_panic(expected = "left the physical range")]
    fn cooling_past_absolute_zero_panics() {
        // The feed's heat capacity flow is about 2800 MJ/(h·K), so at constant cp taking it from
        // 298 K to 0 K removes roughly 830,000 MJ/h. A billion is over a thousand times that.
        let r = demo_registry();
        heat(&r, &feed(&r), -1e9);
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
        let recombined = mix(&r, &outs).unwrap();

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
        assert!(inlet.flows_approx_eq(&mix(&r, &outs).unwrap(), 1e-12));
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

        // `Box<dyn UnitOp>` is what lets one array hold eight different concrete types. The
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
                Box::new(Heater { duty: 1000.0 }),
                (1, Some(1)),
                (1, Some(1)),
            ),
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
                op.evaluate(&r, &supplied).len(),
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

//! Unit operations — what a unit is, how many streams it accepts, and what it produces.

use crate::flowsheet::StreamId;
use crate::serial::ToDocument;
use crate::species::{SpeciesId, SpeciesRegistry};
use crate::stream::Stream;
use std::fmt::{self, Debug};

/// Why a [`UnitOp`] could not produce an answer for the inlets it was given.
///
/// Opaque, wrapping a boxed error rather than enumerating the reasons, for the same reason
/// saving is a [`ToDocument`] supertrait rather than a `match`: the set of operations is open,
/// and a closed enum of failure reasons here would put back exactly the fixed list that
/// `Box<dyn UnitOp>` exists to avoid. A downstream operation keeps its own error type and hands
/// it over with [`EvalError::new`]; [`std::error::Error::source`] gives it back.
///
/// The inner box is `Send + Sync` so that returning one costs [`UnitOp`] nothing: without those
/// bounds the error would not be `Send`, and a `SolveError` carrying it would stop crossing a
/// thread boundary.
///
/// This is for **user input the numerics cannot answer** - a heater duty that cools a stream
/// past absolute zero, a reaction that consumes more of a reactant than the inlet carries, later
/// a flash on a composition with no two-phase split. Bad ids,
/// mismatched arities and a split fraction outside `0.0..=1.0` stay panics: those are bugs in
/// the calling code, and the JSON boundary already rejects them with a
/// [`crate::serial::LoadError`].
#[derive(Debug)]
pub struct EvalError(Box<dyn std::error::Error + Send + Sync>);

impl EvalError {
    /// Wraps the reason an evaluation failed.
    ///
    /// A `String` or a `&str` works, because the standard library already converts both into
    /// the boxed error this holds; so does any concrete `Error + Send + Sync + 'static`.
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(source.into())
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Spelled out, not `self.0.fmt(f)`: `Box` implements both `Debug` and `Display`, so
        // method syntax cannot tell which `fmt` is meant.
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for EvalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

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
/// To make a downstream operation round-trip through a document, not just solve: implement
/// [`ToDocument`] for the tag and parameters it writes, register a matching constructor under
/// that tag with [`crate::serial::OpRegistry::register`], and load through
/// [`crate::serial::Flowsheet::into_domain`]. Saving needs no registration - [`ToDocument`] is a
/// supertrait, so the operation already carries everything a document needs.
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
    ///
    /// # Errors
    ///
    /// Return an [`EvalError`] when the inlets and this operation's parameters have no physical
    /// answer between them - the case the solver turns into
    /// [`crate::solver::SolveError::Evaluation`] and reports to the user. Keep panicking for
    /// what is a bug in the calling code; see [`EvalError`] for where the line falls.
    ///
    /// An operation that cannot fail returns `Ok`, and most do. [`Mixer`] and [`Heater`] fail
    /// when their temperature solve does, and [`ConversionReactor`] when its reaction asks for
    /// more of a reactant than the inlet carries - which needs no solve at all.
    fn evaluate(
        &self,
        registry: &SpeciesRegistry,
        inlets: &[&Stream],
    ) -> Result<Vec<Stream>, EvalError>;

    /// Whether every species that enters this operation leaves it as the same species.
    ///
    /// True for everything that only mixes, splits or heats. An operation that turns one
    /// species into another returns `false`, and [`crate::report::imbalance`] then checks the
    /// flowsheet's total mass instead of each species separately - a reactor's outlet is
    /// *meant* to disagree with its inlet species by species.
    ///
    /// A default method, so adding it broke no downstream operation.
    fn conserves_species(&self) -> bool {
        true
    }
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

mod conversion_reactor;
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
pub use conversion_reactor::ConversionReactor;
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
/// Returns `Ok(None)` if `inlets` is empty.
///
/// # Errors
/// If no positive temperature carries the combined enthalpy. See [`solve_temperature`].
///
/// # Panics
/// If an inlet's species count does not match `registry`.
pub fn mix<'a>(
    registry: &SpeciesRegistry,
    inlets: impl IntoIterator<Item = &'a Stream>,
) -> Result<Option<Stream>, EvalError> {
    let mut inlets = inlets.into_iter().peekable();
    // `?` on the `Option` would return from a `Result` function, so the early exit is spelled
    // out. `peek` hands back `&&Stream`, hence the deref.
    let Some(&first) = inlets.peek() else {
        return Ok(None);
    };
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
        return Ok(Some(outlet)); // exact-zero guard: every inlet is empty
    }

    outlet.set_temperature(weighted_temperature / heat_capacity);
    solve_temperature(registry, &mut outlet, enthalpy)?;
    Ok(Some(outlet))
}

/// How close two Newton iterates must come, in Kelvin, before [`solve_temperature`] stops.
///
/// Far tighter than the solver's own tolerance, on purpose: an inner solve that stops early
/// shows up as noise in the outer loop's residual, and the outer loop cannot converge past it.
const TEMPERATURE_TOLERANCE_K: f64 = 1e-9;

/// Newton needs two or three steps from a good guess. Exhausting this is a failure rather than a
/// bug: with cp positive everywhere the function is monotonic and Newton cannot miss, but a
/// [`crate::thermo::Shomate`] fit extrapolated far from its range can put cp negative for one
/// species while the stream's total stays positive, and then the iteration can wander.
const MAX_NEWTON_STEPS: usize = 50;

/// Sets `stream`'s temperature to the one at which its enthalpy flow equals `enthalpy` (MJ/h),
/// starting Newton's method from the temperature the stream already has.
///
/// The function being zeroed is `f(T) = H(T) - enthalpy`, and its slope `H'(T)` is the stream's
/// heat capacity flow. Wherever cp is positive so is that slope, so `f` only ever rises and
/// crosses zero exactly once. Newton has no second root to wander towards.
///
/// On failure `stream` keeps whatever temperature the last good iterate left on it. Nothing
/// reads it: both callers propagate the error and drop the stream.
///
/// # Errors
/// If an iterate leaves the positive reals - the target enthalpy is below what this stream holds
/// at 0 K, so no temperature answers it - or if 50 Newton steps pass without converging.
///
/// # Panics
/// If the stream has no heat capacity: an empty stream has zero enthalpy at every temperature,
/// so asking for its temperature is a bug rather than bad input. Both callers guard it.
pub fn solve_temperature(
    registry: &SpeciesRegistry,
    stream: &mut Stream,
    enthalpy: f64,
) -> Result<(), EvalError> {
    for _ in 0..MAX_NEWTON_STEPS {
        let slope = stream.heat_capacity(registry);
        assert!(
            slope > 0.0,
            "cannot solve the temperature of a stream with no heat capacity"
        );

        let step = (stream.enthalpy(registry) - enthalpy) / slope;
        let next = stream.temperature() - step;
        if !next.is_finite() || next <= 0.0 {
            // The user-facing half first; the iterate is a diagnostic and reads as noise if it
            // leads. `{:e}` so a huge or tiny value stays short.
            return Err(EvalError::new(format!(
                "no positive temperature holds an enthalpy flow of {enthalpy:e} MJ/h \
                 (Newton reached {next:e} K)"
            )));
        }
        stream.set_temperature(next);

        if step.abs() <= TEMPERATURE_TOLERANCE_K {
            return Ok(());
        }
    }
    Err(EvalError::new(format!(
        "no temperature converged after {MAX_NEWTON_STEPS} Newton steps"
    )))
}

/// Adds `duty` (MJ/h) to an inlet's enthalpy flow and returns the stream at the temperature that
/// results. A positive duty heats, a negative one cools. Flows and pressure pass through unchanged.
///
/// An empty inlet is returned unchanged and the duty is ignored. Its heat capacity is zero, so no
/// finite temperature would absorb the duty. A heater downstream of a tear stream sees an empty
/// inlet on the first pass, while the tear still holds its all-zero guess, so this case returns
/// the inlet instead of panicking.
///
/// # Errors
/// If `duty` cools the stream to absolute zero or below (see [`solve_temperature`]). A duty
/// sized for the converged loop can do that on an early pass that carries a fraction of the
/// converged flow, which is why this is an error the solver reports and not a panic.
///
/// # Panics
/// If `duty` is not finite. `serde_json` rejects `NaN`, `Infinity` and `1e400`, so no document
/// can put one here; reaching this is a bug in the calling code.
pub fn heat(registry: &SpeciesRegistry, inlet: &Stream, duty: f64) -> Result<Stream, EvalError> {
    assert!(duty.is_finite(), "heater duty must be finite, got {duty}");

    let mut outlet = inlet.clone();
    if inlet.heat_capacity(registry) == 0.0 {
        return Ok(outlet); // exact-zero guard: the inlet is empty
    }

    // Newton starts from the inlet temperature, so its first step lands on T + duty / C. That is
    // exact when cp is constant, and within a step or two of the answer otherwise.
    solve_temperature(registry, &mut outlet, inlet.enthalpy(registry) + duty)?;
    Ok(outlet)
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

/// One chemical reaction, run to a fixed conversion of its limiting reactant.
///
/// Plain data, validated by [`react`] rather than on construction, for the same reason
/// [`Flotation::recovery`] is: the fields are public and the species count is not known until an
/// inlet arrives. It lives here rather than beside [`ConversionReactor`] so that `unit.rs` never
/// imports from one of its own submodules.
///
/// `Debug` is not optional: it is a supertrait of [`UnitOp`], so the reactor's derive needs every
/// field to have it.
#[derive(Debug, Clone)]
pub struct Reaction {
    /// Molar stoichiometric coefficients in [`SpeciesId`] order: negative for a reactant,
    /// positive for a product, zero for a species the reaction does not touch.
    ///
    /// e.g. `CH4 + 2 O2 -> CO2 + 2 H2O` is `[-1.0, -2.0, 1.0, 2.0]`.
    pub stoichiometry: Vec<f64>,
    /// The reactant that `conversion` is a fraction of. Its coefficient must be negative.
    pub limiting: SpeciesId,
    /// The fraction of the limiting reactant's inlet flow that reacts, `0.0..=1.0`.
    pub conversion: f64,
}

/// How far a stoichiometry's mass balance may miss, in g/mol per unit of coefficient.
///
/// Published molar masses are rounded, so `sum(nu_i * M_i)` of a correct equation is rarely zero.
/// Rounding to two decimals puts each mass off by at most 0.005 g/mol, so the error is at most
/// `0.005 * sum(|nu_i|)` - which is why the residual is divided by `sum(|nu_i|)`, and why this
/// tolerance does not loosen as molecules get heavier. A missing hydrogen is 1.008 g/mol, far
/// outside it.
pub(crate) const MASS_CLOSURE_TOLERANCE: f64 = 0.01;

/// Checks that `stoichiometry` conserves mass, to within [`MASS_CLOSURE_TOLERANCE`].
///
/// Returns the residual, `|sum(nu_i * M_i)| / sum(|nu_i|)`, on failure: both [`react`] and the
/// JSON loader call this, and doing the comparison here rather than in each is what stops the
/// tolerance drifting between them.
///
/// Written as `residual <= tolerance`, never `residual > tolerance`: a `NaN` coefficient, or an
/// all-zero stoichiometry's `0 / 0`, makes the residual `NaN`, and the negated form would accept it.
///
/// # Panics
/// If `stoichiometry` does not hold one coefficient per species in `registry`.
pub(crate) fn mass_closure(registry: &SpeciesRegistry, stoichiometry: &[f64]) -> Result<(), f64> {
    assert_eq!(
        stoichiometry.len(),
        registry.len(),
        "a reaction needs one stoichiometric coefficient per species"
    );

    let (mut mass, mut coefficients) = (0.0, 0.0);
    for (nu, species) in stoichiometry.iter().zip(registry.all()) {
        mass += nu * species.molar_mass;
        coefficients += nu.abs();
    }

    let residual = mass.abs() / coefficients;
    if residual <= MASS_CLOSURE_TOLERANCE {
        Ok(())
    } else {
        Err(residual)
    }
}

/// How far below zero a reactant's outlet flow may land, as a fraction of *its own* inlet flow,
/// before it counts as consumed beyond what was there rather than as round-off.
///
/// A near-zero outlet is the difference of two numbers that are both about the inlet flow, so
/// its error is a few ULP of that flow; this leaves four orders of magnitude of margin. Scaled by
/// the species' own flow, not the stream total, so a reactant absent from the inlet has no window
/// at all and consuming any of it is an error.
const REACTANT_ROUND_OFF: f64 = 1e-12;

/// Runs `reaction` on an inlet, returning the outlet at the inlet's temperature and pressure.
///
/// The extent, in Mmol/h, is `conversion * n_limiting / |nu_limiting|`, where `n_limiting` is the
/// limiting reactant's inlet flow over its molar mass. No conversion factor appears: t/h over
/// g/mol is Mmol/h, and Mmol/h times g/mol is t/h again.
///
/// **Isothermal, and so not energy-conserving.** The outlet leaves at the inlet temperature, so
/// the heat an exothermic reaction releases, or an endothermic one absorbs, silently goes
/// nowhere. That waits on an absolute enthalpy basis; see the reactor energy balance in TODO.md.
///
/// Mass is conserved exactly, even though the molar masses are rounded. Every product's mass
/// change is scaled by `k = sum(-nu_i * M_i, reactants) / sum(nu_i * M_i, products)`, which is 1
/// for exact masses and within the closure tolerance of it (0.01 g/mol per unit of coefficient) for any stoichiometry that loads.
/// `k` comes from the coefficients alone, not from the extent, so an extent of zero - no
/// limiting reactant in the inlet, or a recycle's all-zero first pass - cannot make it `0 / 0`.
///
/// # Errors
/// If a reactant other than the limiting one would leave with a negative flow: the inlet does not
/// carry enough of it for the conversion asked. Only an inlet can reveal that, and a recycle's
/// early passes may carry a different composition from its converged one.
///
/// # Panics
/// Bugs rather than bad input - a document that asks for any of these is a
/// [`crate::serial::LoadError`] instead:
/// - `stoichiometry` does not hold one coefficient per species;
/// - `conversion` is outside `0.0..=1.0`, or `NaN`;
/// - the limiting species' coefficient is not negative;
/// - the mass balance does not close to within 0.01 g/mol per unit of coefficient, or a coefficient is `NaN`.
pub fn react(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    reaction: &Reaction,
) -> Result<Stream, EvalError> {
    let Reaction {
        stoichiometry,
        limiting,
        conversion,
    } = reaction;
    assert_eq!(
        stoichiometry.len(),
        inlet.species_count(),
        "a reaction needs one stoichiometric coefficient per species"
    );
    assert!(
        (0.0..=1.0).contains(conversion),
        "conversion must be between 0.0 and 1.0, got {conversion}"
    );
    let lim = limiting.as_usize();
    let nu_limiting = stoichiometry[lim];
    assert!(
        nu_limiting < 0.0,
        "the limiting species must be a reactant, but its coefficient is {nu_limiting}"
    );
    if let Err(residual) = mass_closure(registry, stoichiometry) {
        panic!(
            "the reaction's mass balance misses by {residual} g/mol per unit of coefficient, \
             more than {MASS_CLOSURE_TOLERANCE}"
        );
    }

    let species = registry.all();
    let (mut consumed, mut produced) = (0.0, 0.0);
    for (&nu, s) in stoichiometry.iter().zip(species) {
        if nu < 0.0 {
            consumed -= nu * s.molar_mass;
        } else {
            produced += nu * s.molar_mass;
        }
    }
    let k = consumed / produced;

    let extent = conversion * inlet.flows()[lim] / (species[lim].molar_mass * -nu_limiting);

    let mut outlet = inlet.clone();
    for (i, (&nu, s)) in stoichiometry.iter().zip(species).enumerate() {
        let before = inlet.flows()[i];
        let after = if i == lim {
            // Not through `extent`: `n - (n / (M * |nu|)) * |nu| * M` does not cancel exactly, and
            // at conversion 1 it lands a ULP either side of zero. Same reason `recover` writes
            // `1.0 - r` rather than subtracting.
            before * (1.0 - conversion)
        } else if nu > 0.0 {
            before + k * nu * extent * s.molar_mass
        } else {
            before + nu * extent * s.molar_mass
        };

        outlet.flows_mut()[i] = if after >= 0.0 {
            after
        } else if after >= -REACTANT_ROUND_OFF * before {
            0.0
        } else {
            return Err(EvalError::new(format!(
                "the reaction needs more {} than the inlet carries ({before} t/h in, {:e} t/h \
                 short)",
                s.name, -after
            )));
        };
    }

    Ok(outlet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::{Phase, Species};
    use crate::test_support::{
        AMBIENT_K, AMBIENT_KPA, COMBUSTION_MASSES, ROUNDED_COMBUSTION_MASSES, all_ids, combustion,
        combustion_registry, demo_registry, feed,
    };
    use crate::thermo::Shomate;
    use approx::assert_relative_eq;

    /// [`mix`] for the tests that expect both a successful solve and at least one inlet.
    ///
    /// `Result<Option<Stream>, _>` needs two unwraps and they mean different things, so the
    /// tests below say which one they are making by calling this instead.
    fn mixed<'a>(
        registry: &SpeciesRegistry,
        inlets: impl IntoIterator<Item = &'a Stream>,
    ) -> Stream {
        mix(registry, inlets)
            .expect("mix should not fail here")
            .expect("mix should have at least one inlet here")
    }

    // ---- mix ----

    #[test]
    fn mixing_nothing_gives_nothing() {
        let r = demo_registry();
        assert!(mix(&r, &[]).unwrap().is_none());
    }

    #[test]
    fn mixing_one_inlet_reproduces_it() {
        let r = demo_registry();
        let s = feed(&r);
        let out = mixed(&r, std::slice::from_ref(&s));
        assert!(s.flows_approx_eq(&out, 1e-12));
    }

    #[test]
    fn mixing_sums_each_species_and_the_total() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let out = mixed(&r, &[a.clone(), b.clone()]);

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

        let out = mixed(&r, [&cold, &hot]);

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

        let out = mixed(&r, [&a, &b]);

        assert_relative_eq!(out.temperature(), 375.0, max_relative = 1e-12);
    }

    #[test]
    fn inlets_at_one_temperature_mix_to_that_temperature() {
        let r = demo_registry();
        let a = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 340.0, AMBIENT_KPA);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], 340.0, AMBIENT_KPA);

        let out = mixed(&r, [&a, &b]);

        assert_relative_eq!(out.temperature(), 340.0, max_relative = 1e-12);
    }

    #[test]
    fn an_empty_inlet_leaves_the_mixed_temperature_alone() {
        // What a recycle's first pass looks like: the feed meets a tear stream with no flow yet.
        // The empty one goes first, to prove its temperature is not simply kept.
        let r = demo_registry();
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let hot = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 350.0, AMBIENT_KPA);

        let out = mixed(&r, [&empty, &hot]);

        assert_relative_eq!(out.temperature(), 350.0, max_relative = 1e-12);
    }

    #[test]
    fn mixing_only_empty_inlets_keeps_the_first_temperature() {
        let r = demo_registry();
        let a = Stream::zeros(&r, 310.0, AMBIENT_KPA);
        let b = Stream::zeros(&r, 290.0, AMBIENT_KPA);

        let out = mixed(&r, [&a, &b]);

        assert_relative_eq!(out.temperature(), 310.0);
    }

    #[test]
    fn mixing_keeps_the_first_inlets_pressure() {
        // Nothing solves pressure yet. See the pressure item in TODO.md.
        let r = demo_registry();
        let low = feed(&r);
        let high = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, 500.0);

        let out = mixed(&r, [&low, &high]);

        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    #[test]
    fn solve_temperature_inverts_enthalpy() {
        let r = demo_registry();
        let mut s = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 420.0, AMBIENT_KPA);
        let enthalpy = s.enthalpy(&r);

        s.set_temperature(300.0); // a deliberately poor first guess
        solve_temperature(&r, &mut s, enthalpy).unwrap();

        assert_relative_eq!(s.temperature(), 420.0, epsilon = TEMPERATURE_TOLERANCE_K);
    }

    #[test]
    #[should_panic(expected = "no heat capacity")]
    fn an_empty_stream_has_no_temperature_to_solve_for() {
        let r = demo_registry();
        let mut empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        // Still a panic, not an `EvalError`: both callers guard the empty case, so getting here
        // is a bug in the crate rather than something a document can ask for.
        let _ = solve_temperature(&r, &mut empty, 1.0);
    }

    // ---- heat ----

    #[test]
    fn heating_adds_the_duty_to_the_enthalpy_flow() {
        let r = demo_registry();
        let inlet = feed(&r);

        let out = heat(&r, &inlet, 50_000.0).unwrap();

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

        let out = heat(&r, &inlet, -50_000.0).unwrap();

        assert!(out.temperature() < inlet.temperature());
    }

    #[test]
    fn a_constant_heat_capacity_rises_by_duty_over_heat_capacity() {
        // Chalcopyrite alone, so cp is constant and the answer is linear:
        // 100 t/h * 95.0 / 183.5 kJ/(kg·K) = 51.77 MJ/(h·K), so 5177 MJ/h raises it 100 K.
        let r = demo_registry();
        let inlet = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], 300.0, AMBIENT_KPA);
        let c = 100.0 * 95.0 / 183.5;

        let out = heat(&r, &inlet, 100.0 * c).unwrap();

        assert_relative_eq!(out.temperature(), 400.0, max_relative = 1e-12);
    }

    #[test]
    fn heating_changes_neither_flows_nor_pressure() {
        let r = demo_registry();
        let inlet = feed(&r);

        let out = heat(&r, &inlet, 50_000.0).unwrap();

        assert!(inlet.flows_approx_eq(&out, 0.0));
        assert_relative_eq!(out.pressure(), inlet.pressure());
    }

    #[test]
    fn zero_duty_keeps_the_inlet_temperature() {
        let r = demo_registry();
        let inlet = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 350.0, AMBIENT_KPA);

        let out = heat(&r, &inlet, 0.0).unwrap();

        assert_relative_eq!(out.temperature(), 350.0, epsilon = TEMPERATURE_TOLERANCE_K);
    }

    #[test]
    fn heating_an_empty_stream_passes_it_through() {
        let r = demo_registry();
        let empty = Stream::zeros(&r, 310.0, AMBIENT_KPA);

        let out = heat(&r, &empty, 50_000.0).unwrap();

        assert_relative_eq!(out.total(), 0.0);
        assert_relative_eq!(out.temperature(), 310.0);
    }

    #[test]
    #[should_panic(expected = "heater duty must be finite")]
    fn heating_by_nan_panics() {
        let r = demo_registry();
        // Still a panic: `serde_json` rejects `NaN`, `Infinity` and `1e400`, so no document can
        // put one here and reaching it is a bug. `let _` only silences the unused-`Result` lint.
        let _ = heat(&r, &feed(&r), f64::NAN);
    }

    #[test]
    fn cooling_past_absolute_zero_is_an_error() {
        // The feed's heat capacity flow is about 2800 MJ/(h·K), so at constant cp taking it from
        // 298 K to 0 K removes roughly 830,000 MJ/h. A billion is over a thousand times that.
        // This *is* user input - a duty in a document - so it is an error, not a panic.
        let r = demo_registry();

        let e = heat(&r, &feed(&r), -1e9).expect_err("no positive temperature answers -1e9 MJ/h");

        assert!(
            e.to_string().contains("no positive temperature holds"),
            "{e}"
        );
    }

    #[test]
    fn an_eval_error_hands_back_the_error_it_wraps() {
        // The point of boxing rather than an enum: a downstream operation keeps its own error
        // type, and `source` gives it back to anyone who wants to downcast to it.
        let e = EvalError::new(std::io::Error::other(
            "rachford-rice did not bracket a root",
        ));

        assert_eq!(e.to_string(), "rachford-rice did not bracket a root");
        assert!(
            std::error::Error::source(&e)
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .is_some()
        );
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
        let recombined = mixed(&r, &outs);

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
        assert!(inlet.flows_approx_eq(&mixed(&r, &outs), 1e-12));
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

    // ---- react ----

    /// 10 t/h of methane in 60 t/h of oxygen: plenty of oxygen even at full conversion, which
    /// needs 2 * 10 / 16.043 * 31.998 = 39.9 t/h.
    fn lean(r: &SpeciesRegistry) -> Stream {
        Stream::from_flows(r, vec![10.0, 60.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA)
    }

    #[test]
    fn react_matches_the_extent_worked_by_hand() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let [m_ch4, m_o2, m_co2, m_h2o] = COMBUSTION_MASSES;

        let out = react(&r, &lean(&r), &combustion(&r, 0.9)).unwrap();

        // Mmol/h of methane burned: 90% of 10 t/h, over its molar mass and a coefficient of 1.
        let extent = 0.9 * 10.0 / m_ch4;
        assert_relative_eq!(out.flows()[0], 1.0, max_relative = 1e-12);
        assert_relative_eq!(
            out.flows()[1],
            60.0 - 2.0 * extent * m_o2,
            max_relative = 1e-12
        );
        assert_relative_eq!(out.flows()[2], extent * m_co2, max_relative = 1e-12);
        assert_relative_eq!(out.flows()[3], 2.0 * extent * m_h2o, max_relative = 1e-12);
    }

    #[test]
    fn react_conserves_mass_when_the_molar_masses_are_rounded() {
        // At two decimals the equation gains 0.01 g/mol, so without the correction the outlet
        // would carry more than came in. The products are scaled by 80.04 / 80.05 instead.
        let masses = ROUNDED_COMBUSTION_MASSES;
        let [m_ch4, _, m_co2, m_h2o] = masses;
        let r = combustion_registry(masses);
        let inlet = lean(&r);

        let out = react(&r, &inlet, &combustion(&r, 0.9)).unwrap();

        let k = 80.04 / 80.05;
        let extent = 0.9 * 10.0 / m_ch4;
        assert_relative_eq!(out.total(), inlet.total(), max_relative = 1e-12);
        assert_relative_eq!(out.flows()[2], k * extent * m_co2, max_relative = 1e-12);
        assert_relative_eq!(
            out.flows()[3],
            k * 2.0 * extent * m_h2o,
            max_relative = 1e-12
        );
    }

    #[test]
    fn zero_conversion_passes_the_inlet_through() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = lean(&r);

        let out = react(&r, &inlet, &combustion(&r, 0.0)).unwrap();

        assert_eq!(out.flows(), inlet.flows());
    }

    #[test]
    fn an_empty_inlet_passes_through_without_a_nan() {
        // What a reactor downstream of a tear sees on the first pass.
        let r = combustion_registry(COMBUSTION_MASSES);
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

        let out = react(&r, &empty, &combustion(&r, 0.9)).unwrap();

        assert_eq!(out.flows(), [0.0; 4]);
    }

    #[test]
    fn an_inlet_without_the_limiting_reactant_passes_through() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![0.0, 60.0, 5.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let out = react(&r, &inlet, &combustion(&r, 0.9)).unwrap();

        assert_eq!(out.flows(), inlet.flows());
    }

    #[test]
    fn full_conversion_exhausts_the_limiting_reactant_exactly() {
        let r = combustion_registry(COMBUSTION_MASSES);

        let out = react(&r, &lean(&r), &combustion(&r, 1.0)).unwrap();

        assert_eq!(out.flows()[0], 0.0);
    }

    #[test]
    fn a_stoichiometric_feed_at_full_conversion_leaves_no_oxygen_and_no_error() {
        // The oxygen is sized to burn the methane exactly, but by a different sequence of float
        // operations than `react` uses, so the outlet lands a few ULP either side of zero. Across
        // fifty feeds some land below it, and those must clamp rather than fail.
        let [m_ch4, m_o2, _, _] = COMBUSTION_MASSES;
        let r = combustion_registry(COMBUSTION_MASSES);

        for methane in (1..=50).map(|n| n as f64 * 0.37) {
            let oxygen = methane * 2.0 * m_o2 / m_ch4;
            let inlet =
                Stream::from_flows(&r, vec![methane, oxygen, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);

            let out = react(&r, &inlet, &combustion(&r, 1.0))
                .unwrap_or_else(|e| panic!("{methane} t/h of methane: {e}"));

            let left = out.flows()[1];
            assert!(
                (0.0..=1e-12 * oxygen).contains(&left),
                "{methane} t/h of methane left {left:e} t/h of oxygen"
            );
        }
    }

    #[test]
    fn a_scarce_non_limiting_reactant_is_an_error() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 1.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let e = react(&r, &inlet, &combustion(&r, 0.9)).expect_err("1 t/h of O2 is not enough");

        assert!(
            e.to_string()
                .starts_with("the reaction needs more O2 than the inlet carries (1 t/h in, "),
            "{e}"
        );
    }

    #[test]
    fn consuming_a_reactant_the_inlet_does_not_carry_is_an_error_not_a_clamp() {
        // A clamp window scaled by the stream total would round this to zero and create mass.
        // Scaled by the species' own flow, zero oxygen has no window at all.
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 0.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let e = react(&r, &inlet, &combustion(&r, 1e-15)).expect_err("there is no oxygen");

        assert!(e.to_string().contains("more O2"), "{e}");
    }

    #[test]
    fn the_outlet_keeps_the_inlet_temperature_and_pressure() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![10.0, 60.0, 0.0, 0.0], 800.0, 250.0);

        let out = react(&r, &inlet, &combustion(&r, 0.9)).unwrap();

        assert_eq!(out.temperature(), 800.0);
        assert_eq!(out.pressure(), 250.0);
    }

    #[test]
    #[should_panic(expected = "one stoichiometric coefficient per species")]
    fn react_rejects_a_stoichiometry_of_the_wrong_length() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let reaction = Reaction {
            stoichiometry: vec![-1.0, -2.0, 1.0],
            ..combustion(&r, 0.9)
        };
        let _ = react(&r, &lean(&r), &reaction);
    }

    #[test]
    #[should_panic(expected = "conversion must be between 0.0 and 1.0")]
    fn react_rejects_a_conversion_above_one() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let _ = react(&r, &lean(&r), &combustion(&r, 1.5));
    }

    #[test]
    #[should_panic(expected = "conversion must be between 0.0 and 1.0")]
    fn react_rejects_a_nan_conversion() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let _ = react(&r, &lean(&r), &combustion(&r, f64::NAN));
    }

    #[test]
    #[should_panic(expected = "the limiting species must be a reactant")]
    fn react_rejects_a_product_as_the_limiting_species() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let reaction = Reaction {
            limiting: r.find("CO2", Phase::Gas).unwrap(),
            ..combustion(&r, 0.9)
        };
        let _ = react(&r, &lean(&r), &reaction);
    }

    #[test]
    #[should_panic(expected = "mass balance misses")]
    fn react_rejects_a_nan_coefficient() {
        // `NaN > tolerance` is false, so a check written the negated way would wave this through.
        let r = combustion_registry(COMBUSTION_MASSES);
        let reaction = Reaction {
            stoichiometry: vec![-1.0, -2.0, f64::NAN, 2.0],
            ..combustion(&r, 0.9)
        };
        let _ = react(&r, &lean(&r), &reaction);
    }

    #[test]
    #[should_panic(expected = "mass balance misses by 0.22")]
    fn react_rejects_an_equation_short_one_hydrogen_molecule() {
        // Triolein + 2.5 H2 -> tristearin; the right coefficient is 3. The equation misses by
        // 1.008 g/mol, which a tolerance relative to sum(|nu| * M) would have accepted, because
        // these molecules are heavy. Per unit of coefficient it is 1.008 / 4.5 = 0.224.
        let mut r = SpeciesRegistry::default();
        let triolein = [
            ("triolein", 885.453),
            ("H2", 2.016),
            ("tristearin", 891.501),
        ]
        .map(|(name, molar_mass)| {
            r.insert(Species {
                name: name.into(),
                phase: Phase::Liquid,
                molar_mass,
                shomate: Shomate::constant(1000.0),
            })
        })[0];
        let inlet = Stream::from_flows(&r, vec![100.0, 1.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let _ = react(
            &r,
            &inlet,
            &Reaction {
                stoichiometry: vec![-1.0, -2.5, 1.0],
                limiting: triolein,
                conversion: 0.5,
            },
        );
    }

    #[test]
    fn mass_closure_accepts_rounded_masses_and_reports_the_residual_otherwise() {
        let rounded = combustion_registry(ROUNDED_COMBUSTION_MASSES);
        assert_eq!(mass_closure(&rounded, &[-1.0, -2.0, 1.0, 2.0]), Ok(()));

        // Two and a half oxygens: -16.043 - 79.995 + 44.009 + 36.030 = -15.999 over 6.5.
        let r = combustion_registry(COMBUSTION_MASSES);
        let residual = mass_closure(&r, &[-1.0, -2.5, 1.0, 2.0]).unwrap_err();
        assert_relative_eq!(residual, 15.999 / 6.5, max_relative = 1e-9);
    }

    #[test]
    fn mass_closure_rejects_an_all_zero_stoichiometry() {
        let r = combustion_registry(COMBUSTION_MASSES);
        assert!(mass_closure(&r, &[0.0; 4]).unwrap_err().is_nan());
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
            // Not chemistry: the demo species have no real reaction between them, so this turns
            // chalcopyrite into its own mass of quartz, which is all the closure check asks.
            (
                Box::new(ConversionReactor {
                    reaction: Reaction {
                        stoichiometry: vec![-1.0, 183.5 / 60.08, 0.0],
                        limiting: all_ids(&r)[0],
                        conversion: 0.5,
                    },
                }),
                (1, Some(1)),
                (1, Some(1)),
            ),
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
                op.evaluate(&r, &supplied).unwrap().len(),
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

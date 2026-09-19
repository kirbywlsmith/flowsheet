//! Unit operations — what a unit is, how many streams it accepts, and what it produces.

use crate::flowsheet::StreamId;
use crate::serial::ToDocument;
use crate::species::{Phase, Species, SpeciesId, SpeciesRegistry};
use crate::stream::Stream;
use crate::thermo::{Antoine, GAS_CONSTANT};
use serde::{Deserialize, Serialize};
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
/// past absolute zero, a reaction that consumes more of a reactant than the inlet carries, a flash
/// reached by a substance it has no vapour pressure for. Bad ids,
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

mod compressor;
mod conversion_reactor;
mod feed;
mod flash;
mod flotation;
mod heater;
mod mixer;
mod product;
mod pump;
mod splitter;
mod tank;

// Re-exported flat, so every call site keeps writing `unit::Mixer` rather than
// `unit::mixer::Mixer`. The submodules stay private: they are a file-layout detail, and one
// operation per file is the only thing they buy.
pub use compressor::Compressor;
pub use conversion_reactor::ConversionReactor;
pub use feed::Feed;
pub use flash::Flash;
pub use flotation::Flotation;
pub use heater::Heater;
pub use mixer::Mixer;
pub use product::Product;
pub use pump::Pump;
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
/// the sum of the inlets', and [`solve_temperature`] finds the temperature that makes that true.
///
/// The outlet takes the **lowest pressure of the inlets that carry flow**. Lowest, because the
/// pipes physically meet at one pressure and the others would flow backwards at anything
/// higher - Aspen and HYSYS do the same. Only the inlets carrying flow count, because on a
/// recycle's first pass the tear stream is an empty placeholder at whatever pressure it was
/// built with; an empty pipe pushes back on nothing, so letting it set the outlet pressure would
/// hold the whole loop at the placeholder's value forever.
///
/// If every inlet is empty there is no mass to put a temperature or pressure on, so the first
/// inlet's are kept.
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
    let mut pressure = f64::INFINITY;

    for inlet in inlets {
        outlet += inlet;
        let c = inlet.heat_capacity(registry);
        enthalpy += inlet.enthalpy(registry);
        heat_capacity += c;
        weighted_temperature += c * inlet.temperature();
        if inlet.total() > 0.0 {
            pressure = pressure.min(inlet.pressure());
        }
    }

    if heat_capacity == 0.0 {
        return Ok(Some(outlet)); // exact-zero guard: every inlet is empty
    }

    outlet.set_pressure(pressure);
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
    solve_for_temperature(
        stream,
        enthalpy,
        |s| s.enthalpy(registry),
        |s| s.heat_capacity(registry),
        "enthalpy flow",
        "MJ/h",
    )
}

/// Newton's method on `stream`'s temperature until `property` of the stream reaches `target`.
/// `slope` is that property's derivative with respect to temperature, and must be positive.
///
/// The loop under [`solve_temperature`], where the property is enthalpy and the slope is heat
/// capacity, and under the isentropic step of [`compress`], where the property is entropy and
/// the slope is heat capacity over temperature. Both slopes are positive wherever cp is, so in
/// both cases the property only rises and Newton has one root to find. `quantity` and `unit`
/// name the target in the error, so a failed compression does not talk about enthalpy.
fn solve_for_temperature(
    stream: &mut Stream,
    target: f64,
    property: impl Fn(&Stream) -> f64,
    slope: impl Fn(&Stream) -> f64,
    quantity: &str,
    unit: &str,
) -> Result<(), EvalError> {
    for _ in 0..MAX_NEWTON_STEPS {
        let slope = slope(stream);
        assert!(
            slope > 0.0,
            "cannot solve the temperature of a stream with no heat capacity"
        );

        let step = (property(stream) - target) / slope;
        let next = stream.temperature() - step;
        if !next.is_finite() || next <= 0.0 {
            // The user-facing half first; the iterate is a diagnostic and reads as noise if it
            // leads. `{:e}` so a huge or tiny value stays short.
            return Err(EvalError::new(format!(
                "no positive temperature holds an {quantity} of {target:e} {unit} \
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

/// Lowers `outlet`'s pressure by `drop` (kPa), the pressure an operation costs the material
/// passing through it.
///
/// An empty outlet is left alone. It carries no material to lose pressure, and its pressure is
/// whatever placeholder it was built with - on a recycle's first pass that is the tear stream's,
/// which a drop larger than the placeholder would push below zero on a loop that is perfectly
/// well posed once it fills. The same reasoning as [`heat`] ignoring its duty on an empty inlet.
///
/// # Errors
/// If the drop would take a flowing outlet to 0 kPa or below. A drop is a property of the
/// equipment and a pressure is a property of the stream, so the two only meet once material
/// arrives, which makes this the solver's to report and not a panic - the same line
/// [`solve_temperature`] draws for a duty no positive temperature absorbs.
///
/// # Panics
/// If `drop` is negative or not finite. A negative drop would be a pump, and loading rejects
/// one, so reaching this is a bug in the calling code.
pub fn drop_pressure(outlet: &mut Stream, drop: f64) -> Result<(), EvalError> {
    assert!(
        drop.is_finite() && drop >= 0.0,
        "pressure drop must be finite and not negative, got {drop}"
    );
    if outlet.total() == 0.0 {
        return Ok(()); // exact-zero guard: an empty stream has no pressure to lose
    }

    let pressure = outlet.pressure() - drop;
    if pressure <= 0.0 {
        return Err(EvalError::new(format!(
            "a pressure drop of {drop} kPa takes the outlet from {} kPa to {pressure} kPa",
            outlet.pressure()
        )));
    }
    outlet.set_pressure(pressure);
    Ok(())
}

/// [`drop_pressure`] over every outlet of an operation, which is what every shipped operation
/// with a drop does.
///
/// # Errors
/// See [`drop_pressure`].
pub fn drop_pressures(outlets: &mut [Stream], drop: f64) -> Result<(), EvalError> {
    outlets
        .iter_mut()
        .try_for_each(|outlet| drop_pressure(outlet, drop))
}

/// Panics unless `rise` and `efficiency` are values a pump or compressor can be built with:
/// a finite rise of 0 kPa or more, and an efficiency in `(0.0, 1.0]`.
///
/// Loading rejects both, so a caller who gets here has a bug. A zero rise is allowed for the
/// same reason a zero drop is - it is the ideal case, and it does nothing.
fn assert_rise(rise: f64, efficiency: f64) {
    assert!(
        rise.is_finite() && rise >= 0.0,
        "pressure rise must be finite and not negative, got {rise}"
    );
    assert!(
        efficiency > 0.0 && efficiency <= 1.0,
        "efficiency must be greater than 0.0 and at most 1.0, got {efficiency}"
    );
}

/// A [`Phase`] with its article, for an error that says what a species is rather than naming
/// the variant.
fn describe(phase: Phase) -> &'static str {
    match phase {
        Phase::Solid => "a solid",
        Phase::Liquid => "a liquid",
        Phase::Gas => "a gas",
    }
}

/// The shaft work a pump takes to raise `inlet` by `rise` (kPa) at `efficiency`, in MJ/h.
///
/// `V * dP / efficiency`, with `V` the volume the inlet's mass occupies: each species' flow over
/// its [`Species::density`], summed. The units cancel with no factor - t/h over kg/m³ is a
/// thousand m³/h, and a thousand m³ raised a kPa is a MJ.
///
/// # Errors
/// If a species flowing through is a gas, which a pump cannot move, or has no density, which a
/// pump cannot do without. Neither is caught at the JSON boundary, because which species reach
/// a pump is a property of the solved flows and not of the document: a gas that never flows
/// through the pump, and a species with no density that never does, are both fine. That is why
/// these are errors the solver reports rather than panics.
///
/// # Panics
/// If `rise` is negative or not finite, or `efficiency` is outside `(0.0, 1.0]`. Loading rejects
/// both, so reaching either is a bug in the calling code.
pub fn pump_work(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    rise: f64,
    efficiency: f64,
) -> Result<f64, EvalError> {
    assert_rise(rise, efficiency);

    let mut volume = 0.0;
    for (&flow, s) in inlet.flows().iter().zip(registry.all()) {
        if flow == 0.0 {
            continue; // exact-zero guard: a species that is not there needs no density
        }
        if s.phase == Phase::Gas {
            return Err(EvalError::new(format!(
                "species `{}` is a gas, and a pump moves a liquid or a slurry - a compressor \
                 moves a gas",
                s.name
            )));
        }
        let Some(density) = s.density else {
            return Err(EvalError::new(format!(
                "species `{}` flows through the pump but has no `density`, so its volume is \
                 unknown",
                s.name
            )));
        };
        volume += flow / density;
    }
    Ok(volume * rise / efficiency)
}

/// Raises an inlet's pressure by `rise` (kPa) as an incompressible fluid and returns the outlet,
/// warmed by the part of [`pump_work`] that did not become pressure.
///
/// Where the work goes: `V * dP` of it is the pressure the outlet now carries, and the rest -
/// `work * (1 - efficiency)` - is friction, which is heat. The library's enthalpy is thermal,
/// `h(T)` with nothing of pressure in it (see [`Species::enthalpy`]), so the pressure part has
/// nowhere to land and only the heat reaches the stream, through [`heat`]. That is the right
/// temperature - a perfectly efficient pump warms nothing - and it means the enthalpy a pump
/// adds is its loss, not its shaft power. [`crate::report::duty`] reports the former;
/// [`pump_work`] is the latter.
///
/// An empty inlet passes through unchanged, pressure included, for the reason [`heat`] and
/// [`drop_pressure`] give: a recycle's first pass hands a pump downstream of the tear nothing,
/// and nothing has no volume to work on.
///
/// # Errors
/// See [`pump_work`].
///
/// # Panics
/// See [`pump_work`].
pub fn pump(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    rise: f64,
    efficiency: f64,
) -> Result<Stream, EvalError> {
    let work = pump_work(registry, inlet, rise, efficiency)?;
    if inlet.total() == 0.0 {
        return Ok(inlet.clone()); // exact-zero guard: the inlet is empty
    }

    // `work * (1 - efficiency)` rather than `work - volume * rise`, so that at an efficiency of
    // exactly 1 the duty is exactly 0 and `heat` leaves the temperature bit for bit alone.
    let mut outlet = heat(registry, inlet, work * (1.0 - efficiency))?;
    outlet.set_pressure(inlet.pressure() + rise);
    Ok(outlet)
}

/// Raises an inlet's pressure by `rise` (kPa) as an ideal gas along an isentropic path and
/// returns the outlet, carrying `1 / efficiency` times the enthalpy that path cost.
///
/// An ideal gas's entropy per mole is `s(T) - R ln P`. Holding it fixed while the pressure rises
/// means the sensible part has to climb by `R ln(P2 / P1)` per mole - [`GAS_CONSTANT`] times the
/// inlet's [`Stream::molar_flow`], as an entropy flow - and Newton on temperature finds where
/// the Shomate fit ([`Stream::entropy`]) has climbed that far. The enthalpy from the inlet
/// temperature to there is the least work that compresses the gas; over the isentropic
/// `efficiency`, it is the work a real machine does. All of it ends up in the gas, because an
/// ideal gas's enthalpy depends on temperature alone, so [`heat`] finds the outlet from it. A
/// compressor is a heater that works out its own duty.
///
/// The Newton seed is the constant-cp answer, `T1 * (P2 / P1) ^ (nR / C)`, with `C` the heat
/// capacity flow at the inlet: exact when cp is constant, a step or two away otherwise.
///
/// An empty inlet passes through unchanged, as in [`pump`].
///
/// # Errors
/// If a species flowing through is not a gas - the path assumes an ideal gas, and a liquid
/// would come out at a temperature that means nothing - or if either temperature solve fails.
///
/// # Panics
/// If `rise` is negative or not finite, or `efficiency` is outside `(0.0, 1.0]`. Loading rejects
/// both, so reaching either is a bug in the calling code.
pub fn compress(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    rise: f64,
    efficiency: f64,
) -> Result<Stream, EvalError> {
    assert_rise(rise, efficiency);
    if inlet.total() == 0.0 {
        return Ok(inlet.clone()); // exact-zero guard: the inlet is empty
    }
    for (&flow, s) in inlet.flows().iter().zip(registry.all()) {
        if flow > 0.0 && s.phase != Phase::Gas {
            return Err(EvalError::new(format!(
                "species `{}` is {}, and a compressor runs on an ideal gas - a pump moves a \
                 liquid or a slurry",
                s.name,
                describe(s.phase)
            )));
        }
    }

    let ratio = (inlet.pressure() + rise) / inlet.pressure();
    // Mmol/h times J/(mol·K) is MJ/(h·K), the unit of `Stream::entropy`.
    let n_r = inlet.molar_flow(registry) * GAS_CONSTANT;

    let mut isentropic = inlet.clone();
    isentropic
        .set_temperature(inlet.temperature() * ratio.powf(n_r / inlet.heat_capacity(registry)));
    solve_for_temperature(
        &mut isentropic,
        inlet.entropy(registry) + n_r * ratio.ln(),
        |s| s.entropy(registry),
        |s| s.heat_capacity(registry) / s.temperature(),
        "entropy flow",
        "MJ/(h·K)",
    )?;

    let ideal = isentropic.enthalpy(registry) - inlet.enthalpy(registry);
    let mut outlet = heat(registry, inlet, ideal / efficiency)?;
    outlet.set_pressure(inlet.pressure() + rise);
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

/// How close two iterates of [`rachford_rice`] must come before it stops, relative to the
/// smaller of `V` and `1 - V`.
///
/// Relative, and to the smaller side, because each outlet's flows scale with its own share: a
/// flash that barely boils has a vapour fraction of 1e-8, and an absolute tolerance of 1e-12
/// would leave its vapour flows good to four digits. Far inside the solver's 1e-9 for the same
/// reason [`TEMPERATURE_TOLERANCE_K`] is.
const VAPOUR_FRACTION_TOLERANCE: f64 = 1e-12;

/// Newton converges in a handful of steps once it is close; bisection halves the bracket on the
/// way there. 200 halvings would reach any fraction a `f64` can hold, so exhausting this is not
/// something the bracket allows.
const MAX_RACHFORD_RICE_STEPS: usize = 200;

/// The fraction of the moles in a feed that leave as vapour, `V` in `0.0..=1.0`, given each
/// component's molar amount and its K-value (vapour mole fraction over liquid mole fraction).
///
/// Solves the Rachford-Rice equation
///
/// ```text
/// f(V) = sum n_i (K_i - 1) / (1 + V (K_i - 1)) = 0
/// ```
///
/// which is what "the liquid's mole fractions sum to one, and so do the vapour's" becomes once
/// both are written in terms of `V`. `amounts` may be on any scale - moles, Mmol/h or mole
/// fractions - because `f` is linear in them and the root does not move.
///
/// **A feed that does not split is an answer, not an error.** Below its bubble point
/// (`sum n_i K_i <= sum n_i`) a feed is all liquid and this returns exactly `0.0`; above its dew
/// point (`sum n_i / K_i <= sum n_i`) it is all vapour and this returns exactly `1.0`. Only
/// between the two does `f` have a root inside `(0, 1)`, and there it has exactly one: every term
/// falls as `V` rises, so `f` does too.
///
/// A K-value of `f64::INFINITY` is a component that never condenses, like nitrogen over water. Its
/// term is the limit `n / V`, so a feed carrying any is never all liquid. A K-value of `0.0` is
/// one that never evaporates, and a feed carrying any is never all vapour. Both need no special
/// case in the two checks above - `n * inf` and `n / 0` are infinite, `n / inf` and `n * 0` are
/// zero - which is why the checks skip an absent component rather than letting `0 * inf` make a
/// `NaN`.
///
/// Newton from `V = 0.5`, held inside a bracket that every step narrows and falling back to
/// bisection whenever a step would leave it.
///
/// # Panics
/// If the two slices differ in length, an amount is negative or not finite, a K-value is
/// negative or `NaN`, or every amount is zero. The caller builds all of these from a stream and
/// a vapour pressure, so any of them is a bug.
pub fn rachford_rice(amounts: &[f64], k: &[f64]) -> f64 {
    assert_eq!(
        amounts.len(),
        k.len(),
        "rachford_rice needs one K-value per amount"
    );
    assert!(
        amounts.iter().all(|n| n.is_finite() && *n >= 0.0),
        "amounts must be finite and not negative, got {amounts:?}"
    );
    // `>=` is false for `NaN`, and true for infinity, which is allowed.
    assert!(
        k.iter().all(|k| *k >= 0.0),
        "K-values must not be negative or NaN, got {k:?}"
    );
    let total: f64 = amounts.iter().sum();
    assert!(total > 0.0, "rachford_rice needs something to flash");

    // Only the components that are there. A closure rather than a collected `Vec`, so each of
    // the three passes below re-walks the slices without allocating.
    let present = || amounts.iter().zip(k).filter(|(n, _)| **n > 0.0);

    if present().map(|(n, k)| n * k).sum::<f64>() <= total {
        return 0.0; // at or below the bubble point
    }
    if present().map(|(n, k)| n / k).sum::<f64>() <= total {
        return 1.0; // at or above the dew point
    }

    // `f` and its slope together, since they share every division.
    let residual = |v: f64| {
        present().fold((0.0, 0.0), |(f, slope), (&n, &k)| {
            if k.is_infinite() {
                (f + n / v, slope - n / (v * v))
            } else {
                let d = 1.0 + v * (k - 1.0);
                (
                    f + n * (k - 1.0) / d,
                    slope - n * (k - 1.0).powi(2) / (d * d),
                )
            }
        })
    };

    let (mut lo, mut hi, mut v) = (0.0, 1.0, 0.5);
    for _ in 0..MAX_RACHFORD_RICE_STEPS {
        let (f, slope) = residual(v);
        // `f` falls as `V` rises, so a positive residual puts the root to the right of `v`.
        if f > 0.0 {
            lo = v;
        } else {
            hi = v;
        }

        let newton = v - f / slope;
        let next = if newton > lo && newton < hi {
            newton
        } else {
            0.5 * (lo + hi)
        };
        if (next - v).abs() <= VAPOUR_FRACTION_TOLERANCE * next.min(1.0 - next) {
            return next;
        }
        v = next;
    }
    v
}

/// One substance, as [`flash`] sees it: every declared phase of a name that takes part in the
/// vapour-liquid split, with where its mass goes.
struct Component {
    /// The index of the liquid entry, if the substance has one.
    liquid: Option<usize>,
    /// The index of the gas entry, if the substance has one.
    gas: Option<usize>,
    /// Both phases' inlet flows together, t/h.
    mass: f64,
    /// The same flow in Mmol/h, over the molar mass of one phase.
    moles: f64,
    /// The K-value: `0.0` for a liquid that never evaporates, infinite for a gas that never
    /// condenses, `Psat(T) / P` for a substance declared in both phases.
    k: f64,
}

/// Brings `inlet` to `temperature` (K) and `pressure` (kPa) and splits it into a
/// (vapour, liquid) pair in equilibrium, both leaving at that temperature and pressure.
///
/// **Species are paired by name.** A name declared as both a `Liquid` and a `Gas` is one
/// substance in two phases: its two inlet flows are pooled, split by Rachford-Rice
/// ([`rachford_rice`]), and written back under the gas entry in the vapour and the liquid entry
/// in the liquid. Its K-value is Raoult's law, `Psat(T) / P`, with `Psat` read from the liquid
/// entry's [`Species::vapour_pressure`]. Everything else goes to the outlet its phase belongs in:
///
/// - a gas with no liquid entry never condenses (nitrogen over water). It leaves in the vapour,
///   and it takes part in the split - it is what the evaporating water's partial pressure has
///   to share the vapour with.
/// - a liquid with no gas entry and no vapour pressure never evaporates. It leaves in the
///   liquid, and it takes part in the split too, diluting the liquid the way Raoult's law says.
/// - a solid leaves in the liquid, as the drum's bottoms, and takes **no** part in the split: it
///   is a phase of its own and does not dissolve, so it dilutes nothing.
///
/// Mass is split rather than moles, so it is conserved exactly even if a document gives a
/// substance's two phases slightly different molar masses. The moles Rachford-Rice sees are the
/// pooled mass over the liquid entry's molar mass.
///
/// Isothermal: nothing solves a temperature, and the latent heat of whatever evaporates is what
/// [`crate::report::duty`] reports the drum took in.
///
/// An inlet with nothing to split - empty, or only solids - returns an empty vapour and a liquid
/// holding the solids, which is what a recycle's first pass hands a flash.
///
/// # Errors
/// If a flowing substance is declared in both phases but its liquid has no vapour pressure, so
/// its split is unknown; or if a flowing liquid has a vapour pressure but no gas entry, so the
/// vapour it would make has nowhere to go. Which species reach a flash is a property of the
/// solved flows and not of the document, the same reason [`pump_work`]'s missing density is an
/// error rather than a load check.
///
/// # Panics
/// If `temperature` or `pressure` is not finite and positive. Loading rejects both.
pub fn flash(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    temperature: f64,
    pressure: f64,
) -> Result<(Stream, Stream), EvalError> {
    assert!(
        temperature.is_finite() && temperature > 0.0,
        "flash temperature must be finite and positive, got {temperature}"
    );
    assert!(
        pressure.is_finite() && pressure > 0.0,
        "flash pressure must be finite and positive, got {pressure}"
    );

    let mut vapour = Stream::zeros(registry, temperature, pressure);
    let mut liquid = Stream::zeros(registry, temperature, pressure);

    let mut components = Vec::new();
    for (i, (s, &flow)) in registry.all().iter().zip(inlet.flows()).enumerate() {
        match s.phase {
            Phase::Solid => liquid.flows_mut()[i] = flow,
            Phase::Gas => {
                if registry.find(&s.name, Phase::Liquid).is_some() {
                    continue; // one substance in two phases; its liquid entry handles both
                }
                components.push(Component {
                    liquid: None,
                    gas: Some(i),
                    mass: flow,
                    moles: flow / s.molar_mass,
                    k: f64::INFINITY,
                });
            }
            Phase::Liquid => {
                let gas = registry.find(&s.name, Phase::Gas).map(SpeciesId::as_usize);
                let mass = flow + gas.map_or(0.0, |g| inlet.flows()[g]);
                if mass == 0.0 {
                    continue; // exact-zero guard: absent in both phases, so nothing to check
                }
                let k = match (gas, s.vapour_pressure) {
                    (Some(_), Some(antoine)) => antoine.vapour_pressure(temperature) / pressure,
                    (Some(_), None) => {
                        return Err(EvalError::new(format!(
                            "species `{}` is declared as a liquid and a gas, but its liquid has \
                             no `vapour_pressure`, so the flash cannot split it",
                            s.name
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(EvalError::new(format!(
                            "species `{}` has a `vapour_pressure` but no gas phase for its \
                             vapour to go to - declare `{}` as a `Gas` too",
                            s.name, s.name
                        )));
                    }
                    (None, None) => 0.0,
                };
                components.push(Component {
                    liquid: Some(i),
                    gas,
                    mass,
                    moles: mass / s.molar_mass,
                    k,
                });
            }
        }
    }

    let amounts: Vec<f64> = components.iter().map(|c| c.moles).collect();
    if amounts.iter().all(|&n| n == 0.0) {
        return Ok((vapour, liquid)); // exact-zero guard: nothing that can change phase
    }
    let k: Vec<f64> = components.iter().map(|c| c.k).collect();
    let v = rachford_rice(&amounts, &k);

    for c in &components {
        // The share of this component's moles that leaves as vapour, `V K / (1 + V (K - 1))`.
        // Written out for the infinite case, whose formula is `inf / inf`. Clamped because the
        // denominator can round a ULP below the numerator, and `1 - share` must not go negative.
        let share = if c.k.is_infinite() {
            1.0
        } else {
            (v * c.k / (1.0 + v * (c.k - 1.0))).clamp(0.0, 1.0)
        };
        // A component only ever has an entry on the side it can reach: a non-condensable gas has
        // no liquid entry and a share of 1, a non-volatile liquid no gas entry and a share of 0.
        if let Some(g) = c.gas {
            vapour.flows_mut()[g] = c.mass * share;
        }
        if let Some(l) = c.liquid {
            liquid.flows_mut()[l] = c.mass * (1.0 - share);
        }
    }

    Ok((vapour, liquid))
}

/// How many times [`flash_with_duty`] may double its step while looking for a temperature on
/// the far side of the answer, and how many regula falsi steps it may take once it has one.
/// Sixty doublings of a step of a kelvin or more pass any temperature a `f64` can hold.
const MAX_FLASH_TEMPERATURE_STEPS: usize = 60;

/// The pair of entries - liquid, then gas - of the one substance flowing through `inlet`, with
/// the liquid's vapour pressure, if exactly one substance that can change phase flows. Solids do not count: they take no part in
/// the split.
///
/// `None` when two substances flow, when nothing but solids does, and when the one substance is
/// declared in a single phase - pure nitrogen has a smooth `H(T)`, so it needs no special case -
/// and when the liquid has no vapour pressure, which [`flash`] then reports as an error.
fn single_substance(registry: &SpeciesRegistry, inlet: &Stream) -> Option<(usize, usize, Antoine)> {
    let mut name = None;
    for (s, &flow) in registry.all().iter().zip(inlet.flows()) {
        if flow == 0.0 || s.phase == Phase::Solid {
            continue;
        }
        match name {
            None => name = Some(&s.name),
            Some(n) if *n == s.name => {}
            Some(_) => return None, // a second substance: a mixture
        }
    }
    let name = name?;
    let liquid = registry.find(name, Phase::Liquid)?;
    let gas = registry.find(name, Phase::Gas)?;
    let antoine = registry[liquid].vapour_pressure?;
    Some((liquid.as_usize(), gas.as_usize(), antoine))
}

/// Brings `inlet` to `pressure` (kPa), adds `duty` (MJ/h), and splits it into a (vapour, liquid)
/// pair in equilibrium at whatever temperature holds that enthalpy. A duty of zero is the
/// adiabatic flash - a let-down drum, where the pressure falls and some liquid boils using its
/// own heat.
///
/// The temperature is found by searching over [`flash`]: each trial runs the isothermal flash
/// and sums its outlets' enthalpy, `H(T)`, which only rises with `T`. Rachford-Rice therefore
/// runs inside this search, which runs inside the flowsheet solver - three nested loops, at
/// 1e-12 relative, 1e-9 K and the solver's own tolerance.
///
/// **The search needs no derivative.** `dH/dT` is the heat capacity plus a latent heat times
/// `dV/dT`, and the second has no tidy formula and a kink at the bubble and dew points, so
/// Newton's slope is exactly what is hard to get. Instead:
///
/// 1. From the inlet temperature, step by the constant-cp guess `-residual / C` - which
///    *overshoots* whenever a phase changes, because latent heat absorbs part of the duty - and
///    keep doubling the step until the residual changes sign, so the answer is bracketed.
/// 2. Close the bracket by regula falsi (the secant through its two ends), with the Illinois
///    fix: whenever the same end survives twice, its residual is halved, so a lopsided bracket
///    cannot stall with one end fixed.
///
/// **One substance is the exception.** Pure water at a fixed pressure boils at one temperature,
/// so `H(T)` does not rise through the latent heat, it *jumps*, and no temperature lands inside
/// the jump. When only one substance declared in both phases flows (solids aside), the answer is
/// closed-form instead: if the enthalpy lies between all-liquid and all-vapour at the boiling
/// point ([`crate::thermo::Antoine::boiling_point`]), the drum sits there with the vapour share
/// given by where it lies between the two - the lever rule, exact because enthalpy is linear in
/// the split. Outside that range the outlet is one phase, and [`solve_temperature`] finds it.
///
/// An empty inlet returns two empty outlets at the inlet temperature and `pressure`, with the
/// duty ignored, as [`heat`] does: no temperature absorbs a duty into nothing.
///
/// # Errors
/// Whatever [`flash`] reports at a trial temperature; if no positive temperature holds the
/// target enthalpy, as for a duty that cools past absolute zero; or if a pure substance has no
/// boiling point at `pressure`, which Antoine gives when the pressure is above `10^a` kPa.
///
/// # Panics
/// If `duty` is not finite, or `pressure` is not finite and positive. Loading rejects both.
pub fn flash_with_duty(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    duty: f64,
    pressure: f64,
) -> Result<(Stream, Stream), EvalError> {
    assert!(duty.is_finite(), "flash duty must be finite, got {duty}");
    if inlet.total() == 0.0 {
        // exact-zero guard: nothing to hold the duty. `flash` checks the pressure.
        return flash(registry, inlet, inlet.temperature(), pressure);
    }
    let target = inlet.enthalpy(registry) + duty;

    if let Some((liquid, gas, antoine)) = single_substance(registry, inlet) {
        return flash_one_substance(registry, inlet, target, pressure, antoine, liquid, gas);
    }

    // The residual and the outlets it came from, so the last trial is the answer and does not
    // have to be flashed a second time.
    let trial = |t: f64| -> Result<(f64, (Stream, Stream)), EvalError> {
        let outlets = flash(registry, inlet, t, pressure)?;
        let h = outlets.0.enthalpy(registry) + outlets.1.enthalpy(registry);
        Ok((h - target, outlets))
    };

    // ---- bracket ----
    let t0 = inlet.temperature();
    let (r0, outlets) = trial(t0)?;
    if r0 == 0.0 {
        return Ok(outlets);
    }
    // `H` rises with `T`, so a positive residual means the answer is colder.
    let mut step = -r0 / inlet.heat_capacity(registry);
    let (mut near, mut near_r) = (t0, r0);
    let (mut far, mut far_r, mut far_outlets) = (t0, r0, outlets);
    for _ in 0..MAX_FLASH_TEMPERATURE_STEPS {
        // Never at or below 0 K: halve the distance to it instead.
        let t = if near + step > 0.0 {
            near + step
        } else {
            near / 2.0
        };
        let (r, outlets) = trial(t)?;
        (far, far_r, far_outlets) = (t, r, outlets);
        if r.signum() != near_r.signum() {
            break;
        }
        (near, near_r) = (t, r);
        step *= 2.0;
    }
    if far_r.signum() == near_r.signum() {
        return Err(EvalError::new(format!(
            "no positive temperature holds an enthalpy flow of {target:e} MJ/h at {pressure} \
             kPa (the search reached {far:e} K)"
        )));
    }
    if far_r == 0.0 {
        return Ok(far_outlets);
    }

    // ---- regula falsi, Illinois ----
    // `lo` has a negative residual and `hi` a positive one, whichever way the bracket was found.
    let ((mut lo, mut lo_r), (mut hi, mut hi_r)) = if near_r < 0.0 {
        ((near, near_r), (far, far_r))
    } else {
        ((far, far_r), (near, near_r))
    };
    let mut last = far;
    // Which end the last step left in place: -1 for `lo`, 1 for `hi`, 0 before the first.
    let mut kept: i8 = 0;
    for _ in 0..MAX_FLASH_TEMPERATURE_STEPS {
        let t = (lo * hi_r - hi * lo_r) / (hi_r - lo_r);
        let (r, outlets) = trial(t)?;
        if r == 0.0 || (t - last).abs() <= TEMPERATURE_TOLERANCE_K {
            return Ok(outlets);
        }
        last = t;
        if r < 0.0 {
            (lo, lo_r) = (t, r);
            if kept == 1 {
                hi_r /= 2.0;
            }
            kept = 1;
        } else {
            (hi, hi_r) = (t, r);
            if kept == -1 {
                lo_r /= 2.0;
            }
            kept = -1;
        }
    }
    Err(EvalError::new(format!(
        "no flash temperature converged after {MAX_FLASH_TEMPERATURE_STEPS} steps"
    )))
}

/// [`flash_with_duty`] for a feed whose only substance that can change phase is the pair at
/// `liquid` and `gas`: the closed form described there.
fn flash_one_substance(
    registry: &SpeciesRegistry,
    inlet: &Stream,
    target: f64,
    pressure: f64,
    antoine: Antoine,
    liquid: usize,
    gas: usize,
) -> Result<(Stream, Stream), EvalError> {
    let boiling = antoine.boiling_point(pressure);
    if !(boiling.is_finite() && boiling > 0.0) {
        return Err(EvalError::new(format!(
            "species `{}` has no boiling point at {pressure} kPa - its vapour pressure never \
             reaches it (Antoine gives {boiling:e} K)",
            registry.all()[liquid].name
        )));
    }

    // The whole substance in one phase, solids unchanged, at the boiling point.
    let mass = inlet.flows()[liquid] + inlet.flows()[gas];
    let all_in = |into: usize, from: usize| {
        let mut s = inlet.clone();
        s.flows_mut()[into] = mass;
        s.flows_mut()[from] = 0.0;
        s.set_temperature(boiling);
        s.set_pressure(pressure);
        s
    };
    let mut all_liquid = all_in(liquid, gas);
    let mut all_vapour = all_in(gas, liquid);
    let (h_liquid, h_vapour) = (all_liquid.enthalpy(registry), all_vapour.enthalpy(registry));

    // Subcooled or superheated: one phase, and a temperature solve on it. Newton starts at the
    // boiling point, and `H` is monotonic, so it lands on the side it should - where `flash`
    // then puts every molecule in the one phase, exactly.
    if target <= h_liquid || target >= h_vapour {
        let single = if target <= h_liquid {
            &mut all_liquid
        } else {
            &mut all_vapour
        };
        solve_temperature(registry, single, target)?;
        return flash(registry, single, single.temperature(), pressure);
    }

    // Boiling: the lever rule. `H` is linear in the share that boiled, so this is exact.
    let share = (target - h_liquid) / (h_vapour - h_liquid);
    let mut vapour = Stream::zeros(registry, boiling, pressure);
    vapour.flows_mut()[gas] = mass * share;
    let mut bottoms = all_liquid; // the solids, and the liquid entry to overwrite
    bottoms.flows_mut()[liquid] = mass * (1.0 - share);
    Ok((vapour, bottoms))
}

/// Which of its two energy specs a [`Flash`] holds: the temperature it runs at, or the heat it
/// takes in. Aspen's `Flash2` offers the same pair beside the pressure.
///
/// An enum rather than two `Option` fields, so a flash cannot hold both or neither - the
/// document's two optional keys are checked into one of these on load.
///
/// Not `serde`, unlike [`ReactorEnergy`]: on the wire the variant is which key is present,
/// `"temperature": 350` or `"duty": 0`, so that every flash document written before the duty
/// existed still reads as it did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlashEnergy {
    /// Both outlets leave at this temperature, in K, and the heat that takes is whatever
    /// [`crate::report::duty`] finds. See [`flash`].
    Temperature(f64),
    /// This much heat is added, in MJ/h - negative removes it, and zero is the adiabatic flash -
    /// and the outlets leave at whatever temperature holds the result. See [`flash_with_duty`].
    Duty(f64),
}

/// What a reactor does with the heat its reaction releases or absorbs.
///
/// There is no heat-of-reaction parameter anywhere: once every participant has an
/// [`crate::Species::enthalpy_of_formation`], the heat of reaction *is* the outlet's enthalpy
/// minus the inlet's, and this only says which of the two is held.
///
/// Derives `serde` directly, the rule for plain data with no invariant to protect, like
/// [`crate::Phase`]. Written in `snake_case` on the wire, to match the op tags beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactorEnergy {
    /// The outlet leaves at the inlet temperature. The heat of reaction is taken away or supplied
    /// from outside, and [`crate::report::duty`] says how much.
    Isothermal,
    /// No heat crosses the boundary: the outlet takes the temperature at which its enthalpy flow
    /// equals the inlet's. An exothermic reaction heats its own products.
    Adiabatic,
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

/// Checks that every species `stoichiometry` touches has an enthalpy of formation, returning the
/// first that does not.
///
/// [`crate::Species::enthalpy_of_formation`] is optional because a species that is never created
/// or destroyed has the same offset on both sides of a balance. A reaction's participants are
/// exactly the species that do not, and for them `None` would read as zero and fabricate a heat of
/// reaction. Shared by [`react`] and the JSON loader, like [`mass_closure`].
///
/// # Panics
/// If `stoichiometry` does not hold one coefficient per species in `registry`.
pub(crate) fn formation_enthalpies<'a>(
    registry: &'a SpeciesRegistry,
    stoichiometry: &[f64],
) -> Result<(), &'a Species> {
    assert_eq!(
        stoichiometry.len(),
        registry.len(),
        "a reaction needs one stoichiometric coefficient per species"
    );

    match stoichiometry
        .iter()
        .zip(registry.all())
        .find(|(nu, species)| **nu != 0.0 && species.enthalpy_of_formation.is_none())
    {
        Some((_, species)) => Err(species),
        None => Ok(()),
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
/// nowhere. That waits on the reactor energy balance item in TODO.md.
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
/// - the mass balance does not close to within 0.01 g/mol per unit of coefficient, or a coefficient is `NaN`;
/// - a species with a non-zero coefficient has no enthalpy of formation.
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
    if let Err(species) = formation_enthalpies(registry, stoichiometry) {
        panic!(
            "species `{}` takes part in the reaction but has no enthalpy of formation",
            species.name
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
        AMBIENT_K, AMBIENT_KPA, COMBUSTION_MASSES, DEMO_DENSITIES, ROUNDED_COMBUSTION_MASSES,
        all_ids, combustion, combustion_registry, demo_registry, demo_registry_with_density,
        demo_registry_with_formation, feed, humid_nitrogen, nitrogen_registry,
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
    fn mixing_takes_the_lowest_inlet_pressure() {
        let r = demo_registry();
        let low = feed(&r);
        let high = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, 500.0);

        assert_relative_eq!(mixed(&r, [&low, &high]).pressure(), AMBIENT_KPA);
        assert_relative_eq!(mixed(&r, [&high, &low]).pressure(), AMBIENT_KPA);
    }

    #[test]
    fn an_empty_inlet_does_not_set_the_mixed_pressure() {
        // A recycle's first pass: the tear is an empty placeholder at ambient, and the feed is
        // at 500 kPa. Taking the placeholder's pressure would hold the loop at ambient forever.
        let r = demo_registry();
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let feed = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, 500.0);

        assert_relative_eq!(mixed(&r, [&empty, &feed]).pressure(), 500.0);
    }

    #[test]
    fn mixing_only_empty_inlets_keeps_the_first_pressure() {
        let r = demo_registry();
        let a = Stream::zeros(&r, AMBIENT_K, 300.0);
        let b = Stream::zeros(&r, AMBIENT_K, 200.0);

        assert_relative_eq!(mixed(&r, [&a, &b]).pressure(), 300.0);
    }

    #[test]
    fn a_pressure_drop_comes_off_the_outlet() {
        let r = demo_registry();
        let mut out = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, 500.0);

        drop_pressure(&mut out, 20.0).unwrap();

        assert_relative_eq!(out.pressure(), 480.0);
    }

    #[test]
    fn a_pressure_drop_leaves_an_empty_stream_alone() {
        let r = demo_registry();
        let mut out = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

        drop_pressure(&mut out, 1e6).unwrap();

        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    #[test]
    fn a_pressure_drop_that_reaches_zero_is_an_error_not_a_panic() {
        let r = demo_registry();
        let mut out = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, 500.0);

        let e = drop_pressure(&mut out, 500.0).unwrap_err();

        assert_eq!(
            e.to_string(),
            "a pressure drop of 500 kPa takes the outlet from 500 kPa to 0 kPa"
        );
        assert_relative_eq!(out.pressure(), 500.0, epsilon = 0.0);
    }

    #[test]
    #[should_panic(expected = "pressure drop must be finite and not negative")]
    fn a_negative_pressure_drop_is_a_bug() {
        let r = demo_registry();
        let _ = drop_pressure(&mut feed(&r), -1.0);
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

    // ---- pump ----

    /// The demo feed's volume, in thousands of m³/h: each species' flow over its density.
    fn feed_volume() -> f64 {
        [40.0, 360.0, 600.0]
            .iter()
            .zip(DEMO_DENSITIES)
            .map(|(m, rho)| m / rho)
            .sum()
    }

    #[test]
    fn pump_work_is_volume_times_rise_over_efficiency() {
        let r = demo_registry_with_density();

        let work = pump_work(&r, &feed(&r), 500.0, 0.7).unwrap();

        assert_relative_eq!(work, feed_volume() * 500.0 / 0.7, max_relative = 1e-12);
    }

    #[test]
    fn pumping_raises_the_pressure_by_the_rise() {
        let r = demo_registry_with_density();
        let inlet = feed(&r);

        let out = pump(&r, &inlet, 500.0, 0.7).unwrap();

        assert_relative_eq!(out.pressure(), AMBIENT_KPA + 500.0);
        assert!(inlet.flows_approx_eq(&out, 0.0));
    }

    #[test]
    fn pumping_heats_the_stream_by_the_work_it_wastes() {
        // The library's enthalpy is thermal, so of `V * dP / eta` only the `1 - eta` share that
        // is friction reaches the stream; the `V * dP` that became pressure is not in `h(T)`.
        let r = demo_registry_with_density();
        let inlet = feed(&r);

        let out = pump(&r, &inlet, 500.0, 0.7).unwrap();

        let work = pump_work(&r, &inlet, 500.0, 0.7).unwrap();
        assert_relative_eq!(
            out.enthalpy(&r) - inlet.enthalpy(&r),
            work * 0.3,
            max_relative = 1e-6
        );
        assert!(out.temperature() > inlet.temperature());
    }

    #[test]
    fn an_ideal_pump_keeps_the_inlet_temperature_bit_for_bit() {
        let r = demo_registry_with_density();
        let inlet = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], 350.0, AMBIENT_KPA);

        let out = pump(&r, &inlet, 500.0, 1.0).unwrap();

        assert_eq!(out.temperature(), 350.0);
    }

    #[test]
    fn pumping_an_empty_stream_passes_it_through_at_its_own_pressure() {
        // The tear's first-pass placeholder: no volume to work on, and its pressure is not
        // real, so neither is touched. The demo registry has no densities and that is fine
        // too, because nothing flows.
        let r = demo_registry();
        let empty = Stream::zeros(&r, 310.0, AMBIENT_KPA);

        let out = pump(&r, &empty, 500.0, 0.7).unwrap();

        assert_relative_eq!(out.total(), 0.0);
        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
        assert_relative_eq!(out.temperature(), 310.0);
    }

    #[test]
    fn pumping_a_species_without_a_density_is_an_error_naming_it() {
        let r = demo_registry();

        let e = pump(&r, &feed(&r), 500.0, 0.7).expect_err("no density, no volume");

        assert_eq!(
            e.to_string(),
            "species `CuFeS2` flows through the pump but has no `density`, so its volume is \
             unknown"
        );
    }

    #[test]
    fn a_species_without_a_density_that_does_not_flow_is_no_obstacle() {
        // Only the water flows, and only the water has a density.
        let mut r = SpeciesRegistry::default();
        for (species, density) in demo_registry().all().iter().zip([None, None, Some(997.0)]) {
            r.insert(Species {
                density,
                ..species.clone()
            });
        }
        let inlet = Stream::from_flows(&r, vec![0.0, 0.0, 600.0], AMBIENT_K, AMBIENT_KPA);

        let work = pump_work(&r, &inlet, 500.0, 1.0).unwrap();

        assert_relative_eq!(work, 600.0 / 997.0 * 500.0, max_relative = 1e-12);
    }

    #[test]
    fn pumping_a_gas_is_an_error_naming_it() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = Stream::from_flows(&r, vec![1.0, 4.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);

        let e = pump(&r, &inlet, 500.0, 0.7).expect_err("a pump does not move a gas");

        assert!(e.to_string().starts_with("species `CH4` is a gas"), "{e}");
    }

    #[test]
    #[should_panic(expected = "pressure rise must be finite and not negative")]
    fn a_negative_pressure_rise_is_a_bug() {
        let r = demo_registry_with_density();
        let _ = pump(&r, &feed(&r), -5.0, 0.7);
    }

    #[test]
    #[should_panic(expected = "efficiency must be greater than 0.0 and at most 1.0")]
    fn a_zero_efficiency_is_a_bug() {
        let r = demo_registry_with_density();
        let _ = pump(&r, &feed(&r), 5.0, 0.0);
    }

    #[test]
    #[should_panic(expected = "efficiency must be greater than 0.0 and at most 1.0")]
    fn an_efficiency_above_one_is_a_bug() {
        let r = demo_registry_with_density();
        let _ = compress(&r, &feed(&r), 5.0, 1.5);
    }

    // ---- compress ----

    /// Methane and oxygen at 1 and 4 t/h: every combustion species has the same constant cp of
    /// 35 J/(mol·K), so the isentropic outlet has a closed form.
    fn gas(r: &SpeciesRegistry, temperature: f64) -> Stream {
        Stream::from_flows(r, vec![1.0, 4.0, 0.0, 0.0], temperature, AMBIENT_KPA)
    }

    #[test]
    fn an_isentropic_compression_of_a_constant_cp_gas_matches_the_textbook_exponent() {
        // `T2 / T1 = (P2 / P1) ^ (R / cp)` per mole, and with every species at the same cp the
        // mixture is no different.
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = gas(&r, 300.0);

        let out = compress(&r, &inlet, 3.0 * AMBIENT_KPA, 1.0).unwrap();

        let ratio: f64 = 4.0;
        assert_relative_eq!(
            out.temperature(),
            300.0 * ratio.powf(GAS_CONSTANT / 35.0),
            max_relative = 1e-9
        );
        assert_relative_eq!(out.pressure(), 4.0 * AMBIENT_KPA);
        assert!(inlet.flows_approx_eq(&out, 0.0));
    }

    #[test]
    fn an_isentropic_compression_holds_the_entropy_over_a_temperature_dependent_cp() {
        // Nitrogen over its Shomate fit, so the seed is only a guess and Newton has to work: the
        // outlet's sensible entropy must have climbed by exactly `n R ln(P2 / P1)`.
        let r = nitrogen_registry();
        let inlet = Stream::from_flows(&r, vec![10.0], 300.0, AMBIENT_KPA);

        let out = compress(&r, &inlet, 2.0 * AMBIENT_KPA, 1.0).unwrap();

        let n_r = 10.0 / 28.0134 * GAS_CONSTANT;
        assert_relative_eq!(
            out.entropy(&r) - inlet.entropy(&r),
            n_r * 3.0_f64.ln(),
            max_relative = 1e-9
        );
    }

    #[test]
    fn an_inefficient_compression_costs_the_ideal_work_over_the_efficiency() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = gas(&r, 300.0);

        let ideal = compress(&r, &inlet, 3.0 * AMBIENT_KPA, 1.0).unwrap();
        let real = compress(&r, &inlet, 3.0 * AMBIENT_KPA, 0.8).unwrap();

        assert_relative_eq!(
            real.enthalpy(&r) - inlet.enthalpy(&r),
            (ideal.enthalpy(&r) - inlet.enthalpy(&r)) / 0.8,
            max_relative = 1e-9
        );
        assert!(real.temperature() > ideal.temperature());
    }

    #[test]
    fn a_zero_rise_compresses_nothing() {
        let r = combustion_registry(COMBUSTION_MASSES);
        let inlet = gas(&r, 300.0);

        let out = compress(&r, &inlet, 0.0, 0.8).unwrap();

        assert_eq!(out.temperature(), 300.0);
        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    #[test]
    fn compressing_an_empty_stream_passes_it_through() {
        let r = demo_registry();
        let empty = Stream::zeros(&r, 310.0, AMBIENT_KPA);

        let out = compress(&r, &empty, 500.0, 0.8).unwrap();

        assert_relative_eq!(out.total(), 0.0);
        assert_relative_eq!(out.pressure(), AMBIENT_KPA);
    }

    #[test]
    fn compressing_a_liquid_is_an_error_naming_it() {
        let r = demo_registry();
        let inlet = Stream::from_flows(&r, vec![0.0, 0.0, 600.0], AMBIENT_K, AMBIENT_KPA);

        let e = compress(&r, &inlet, 500.0, 0.8).expect_err("water is not an ideal gas");

        assert!(
            e.to_string().starts_with("species `H2O` is a liquid"),
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

    // ---- rachford_rice ----

    #[test]
    fn rachford_rice_splits_a_symmetric_pair_in_half() {
        // 1/(1+V) = 0.5/(1-0.5V) has its root at V = 0.5.
        assert_relative_eq!(
            rachford_rice(&[1.0, 1.0], &[2.0, 0.5]),
            0.5,
            max_relative = 1e-12
        );
    }

    #[test]
    fn rachford_rice_does_not_care_about_scale() {
        let small = rachford_rice(&[1.0, 3.0], &[4.0, 0.2]);
        let large = rachford_rice(&[1000.0, 3000.0], &[4.0, 0.2]);
        assert_relative_eq!(small, large, max_relative = 1e-12);
    }

    #[test]
    fn a_feed_below_its_bubble_point_is_all_liquid_exactly() {
        assert_eq!(rachford_rice(&[1.0, 1.0], &[0.5, 0.8]), 0.0);
    }

    #[test]
    fn a_feed_above_its_dew_point_is_all_vapour_exactly() {
        assert_eq!(rachford_rice(&[1.0, 1.0], &[2.0, 3.0]), 1.0);
    }

    #[test]
    fn a_component_that_never_condenses_takes_the_limit_of_its_term() {
        // 1/V = 0.75/(1-0.75V), so V = 2/3: all of the inert and a third of the rest.
        assert_relative_eq!(
            rachford_rice(&[1.0, 1.0], &[f64::INFINITY, 0.25]),
            2.0 / 3.0,
            max_relative = 1e-12
        );
    }

    #[test]
    fn an_absent_component_does_not_turn_a_limit_into_nan() {
        // `0 * inf` would be NaN in the bubble-point check if the absent inert were not skipped.
        assert_eq!(rachford_rice(&[0.0, 1.0], &[f64::INFINITY, 0.5]), 0.0);
    }

    #[test]
    fn a_feed_that_barely_boils_keeps_its_small_vapour_fraction_to_full_precision() {
        // With K = [2, x] on equal amounts the root is V = x / (2 (1 - x)), about 5e-9 here. An
        // absolute tolerance of 1e-12 would get it right to three digits.
        let x = 1e-8;
        assert_relative_eq!(
            rachford_rice(&[1.0, 1.0], &[2.0, x]),
            x / (2.0 * (1.0 - x)),
            max_relative = 1e-10
        );
    }

    #[test]
    #[should_panic(expected = "rachford_rice needs something to flash")]
    fn rachford_rice_on_nothing_panics() {
        rachford_rice(&[0.0, 0.0], &[2.0, 0.5]);
    }

    #[test]
    #[should_panic(expected = "K-values must not be negative or NaN")]
    fn rachford_rice_on_a_nan_k_value_panics() {
        rachford_rice(&[1.0, 1.0], &[f64::NAN, 0.5]);
    }

    // ---- flash ----

    const FLASH_K: f64 = 350.0;

    /// Liquid water, steam and nitrogen in t/h, over [`humid_nitrogen`].
    fn humid(r: &SpeciesRegistry, flows: [f64; 3]) -> Stream {
        Stream::from_flows(r, flows.to_vec(), FLASH_K, AMBIENT_KPA)
    }

    fn flashed(r: &SpeciesRegistry, inlet: &Stream, temperature: f64) -> (Stream, Stream) {
        flash(r, inlet, temperature, AMBIENT_KPA).expect("the flash should succeed here")
    }

    #[test]
    fn water_over_nitrogen_saturates_the_nitrogen_and_no_more() {
        // Nitrogen dissolves in nothing, so the liquid is pure water, x = 1, and Raoult puts the
        // vapour at y = Psat / P. The steam the nitrogen carries is then n_N2 * Psat / (P - Psat).
        let r = humid_nitrogen();
        let inlet = humid(&r, [100.0, 0.0, 28.0]);
        let (vapour, liquid) = flashed(&r, &inlet, FLASH_K);

        let psat = r.all()[0].vapour_pressure.unwrap().vapour_pressure(FLASH_K);
        let nitrogen = 28.0 / r.all()[2].molar_mass;
        let steam = nitrogen * psat / (AMBIENT_KPA - psat) * r.all()[1].molar_mass;

        assert_relative_eq!(vapour.flows()[1], steam, max_relative = 1e-10);
        assert_eq!(vapour.flows()[2], 28.0);
        assert_relative_eq!(liquid.flows()[0], 100.0 - steam, max_relative = 1e-10);
        assert_eq!(
            (vapour.flows()[0], liquid.flows()[1], liquid.flows()[2]),
            (0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn a_flash_conserves_each_substance_across_its_two_phases() {
        let r = humid_nitrogen();
        let inlet = humid(&r, [60.0, 15.0, 28.0]);
        let (vapour, liquid) = flashed(&r, &inlet, FLASH_K);

        let water = |s: &Stream| s.flows()[0] + s.flows()[1];
        assert_relative_eq!(water(&vapour) + water(&liquid), 75.0, max_relative = 1e-14);
        assert_relative_eq!(
            vapour.total() + liquid.total(),
            inlet.total(),
            max_relative = 1e-14
        );
    }

    #[test]
    fn both_outlets_leave_at_the_flash_temperature_and_pressure() {
        let r = humid_nitrogen();
        let inlet = humid(&r, [100.0, 0.0, 28.0]);
        let (vapour, liquid) = flash(&r, &inlet, 340.0, 50.0).unwrap();

        for s in [&vapour, &liquid] {
            assert_eq!((s.temperature(), s.pressure()), (340.0, 50.0));
        }
    }

    #[test]
    fn pure_water_below_its_boiling_point_stays_liquid() {
        let r = humid_nitrogen();
        let (vapour, liquid) = flashed(&r, &humid(&r, [10.0, 0.0, 0.0]), 360.0);
        assert_eq!(vapour.total(), 0.0);
        assert_eq!(liquid.flows()[0], 10.0);
    }

    #[test]
    fn pure_water_above_its_boiling_point_is_all_steam() {
        let r = humid_nitrogen();
        let (vapour, liquid) = flashed(&r, &humid(&r, [10.0, 0.0, 0.0]), 380.0);
        assert_eq!(vapour.flows()[1], 10.0);
        assert_eq!(liquid.total(), 0.0);
    }

    #[test]
    fn steam_below_its_boiling_point_condenses() {
        // The two phases are pooled before the split, so it does not matter which one arrives.
        let r = humid_nitrogen();
        let (vapour, liquid) = flashed(&r, &humid(&r, [0.0, 10.0, 0.0]), FLASH_K);
        assert_eq!(vapour.total(), 0.0);
        assert_eq!(liquid.flows()[0], 10.0);
    }

    #[test]
    fn a_solid_leaves_in_the_liquid_and_dilutes_nothing() {
        let mut r = humid_nitrogen();
        r.insert(
            crate::library::find("SiO2", Phase::Solid)
                .unwrap()
                .species
                .clone(),
        );
        let plain = humid_nitrogen();

        let (sandy, bottoms) = flashed(
            &r,
            &Stream::from_flows(&r, vec![100.0, 0.0, 28.0, 50.0], FLASH_K, AMBIENT_KPA),
            FLASH_K,
        );
        let (clean, _) = flashed(&plain, &humid(&plain, [100.0, 0.0, 28.0]), FLASH_K);

        assert_eq!(bottoms.flows()[3], 50.0);
        assert_eq!(sandy.flows()[3], 0.0);
        // Bit for bit: the sand never enters Rachford-Rice, so the arithmetic is the same.
        assert_eq!(&sandy.flows()[..3], clean.flows());
    }

    #[test]
    fn a_liquid_that_never_evaporates_still_dilutes_the_water() {
        // Raoult's law: a second liquid lowers water's mole fraction, and so its partial pressure.
        let mut r = humid_nitrogen();
        r.insert(Species {
            name: "Oil".into(),
            phase: Phase::Liquid,
            molar_mass: 300.0,
            shomate: Shomate::constant(500.0),
            enthalpy_of_formation: None,
            density: None,
            vapour_pressure: None,
        });
        let plain = humid_nitrogen();

        let (oily, bottoms) = flashed(
            &r,
            &Stream::from_flows(&r, vec![100.0, 0.0, 28.0, 3000.0], FLASH_K, AMBIENT_KPA),
            FLASH_K,
        );
        let (clean, _) = flashed(&plain, &humid(&plain, [100.0, 0.0, 28.0]), FLASH_K);

        assert_eq!(bottoms.flows()[3], 3000.0);
        assert!(
            oily.flows()[1] < clean.flows()[1],
            "{} t/h of steam over oil against {} over pure water",
            oily.flows()[1],
            clean.flows()[1]
        );
    }

    #[test]
    fn flashing_an_empty_stream_gives_two_empty_outlets() {
        // What a flash downstream of a tear sees on the first pass.
        let r = humid_nitrogen();
        let (vapour, liquid) = flashed(&r, &Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA), FLASH_K);
        assert_eq!((vapour.total(), liquid.total()), (0.0, 0.0));
        assert_eq!(vapour.temperature(), FLASH_K);
    }

    /// Water in two phases whose liquid has no vapour pressure, beside nitrogen.
    fn water_without_vapour_pressure() -> SpeciesRegistry {
        let mut r = SpeciesRegistry::default();
        for s in humid_nitrogen().all() {
            r.insert(Species {
                vapour_pressure: None,
                ..s.clone()
            });
        }
        r
    }

    #[test]
    fn a_pair_without_a_vapour_pressure_is_an_error_naming_it() {
        let r = water_without_vapour_pressure();
        let e = flash(&r, &humid(&r, [100.0, 0.0, 28.0]), FLASH_K, AMBIENT_KPA)
            .expect_err("nothing says how the water splits");
        assert!(e.to_string().contains("`H2O`"), "{e}");
        assert!(e.to_string().contains("no `vapour_pressure`"), "{e}");
    }

    #[test]
    fn a_pair_without_a_vapour_pressure_that_does_not_flow_is_no_obstacle() {
        let r = water_without_vapour_pressure();
        let (vapour, _) = flashed(&r, &humid(&r, [0.0, 0.0, 28.0]), FLASH_K);
        assert_eq!(vapour.flows()[2], 28.0);
    }

    #[test]
    fn a_volatile_liquid_with_no_gas_entry_is_an_error_naming_it() {
        let mut r = SpeciesRegistry::default();
        r.insert(
            crate::library::find("H2O", Phase::Liquid)
                .unwrap()
                .species
                .clone(),
        );
        let inlet = Stream::from_flows(&r, vec![10.0], FLASH_K, AMBIENT_KPA);

        let e = flash(&r, &inlet, FLASH_K, AMBIENT_KPA).expect_err("the steam has nowhere to go");
        assert!(e.to_string().contains("no gas phase"), "{e}");
    }

    #[test]
    #[should_panic(expected = "flash pressure must be finite and positive")]
    fn a_zero_flash_pressure_is_a_bug() {
        let r = humid_nitrogen();
        let _ = flash(&r, &humid(&r, [1.0, 0.0, 0.0]), FLASH_K, 0.0);
    }

    // ---- flash_with_duty ----

    /// Both outlets' enthalpy flow together, MJ/h.
    fn outlet_enthalpy(r: &SpeciesRegistry, (vapour, liquid): &(Stream, Stream)) -> f64 {
        vapour.enthalpy(r) + liquid.enthalpy(r)
    }

    fn duty_flashed(r: &SpeciesRegistry, inlet: &Stream, duty: f64) -> (Stream, Stream) {
        flash_with_duty(r, inlet, duty, AMBIENT_KPA).expect("the flash should succeed here")
    }

    /// The steam that nitrogen at `temperature` carries away from pure water, t/h - the closed
    /// form from `tests/flash.rs`, for checking a found temperature is an equilibrium.
    fn saturated_steam(r: &SpeciesRegistry, nitrogen: f64, temperature: f64) -> f64 {
        let psat = r.all()[0]
            .vapour_pressure
            .unwrap()
            .vapour_pressure(temperature);
        nitrogen / r.all()[2].molar_mass * psat / (AMBIENT_KPA - psat) * r.all()[1].molar_mass
    }

    #[test]
    fn an_adiabatic_flash_keeps_the_enthalpy_and_cools_as_it_evaporates() {
        let r = humid_nitrogen();
        let inlet = Stream::from_flows(&r, vec![100.0, 0.0, 28.0], 360.0, AMBIENT_KPA);
        let outlets = duty_flashed(&r, &inlet, 0.0);
        let t = outlets.0.temperature();

        assert_relative_eq!(
            outlet_enthalpy(&r, &outlets),
            inlet.enthalpy(&r),
            max_relative = 1e-11
        );
        assert!(t < 360.0, "{t}");
        assert_eq!(outlets.1.temperature(), t);
        // And it is an equilibrium at the temperature it found.
        assert_relative_eq!(
            outlets.0.flows()[1],
            saturated_steam(&r, 28.0, t),
            max_relative = 1e-10
        );
    }

    #[test]
    fn re_flashing_an_equilibrium_adiabatically_stays_where_it_is() {
        let r = humid_nitrogen();
        let (mut both, liquid) = flashed(&r, &humid(&r, [100.0, 0.0, 28.0]), FLASH_K);
        both += &liquid;

        let (vapour, _) = duty_flashed(&r, &both, 0.0);
        assert_relative_eq!(vapour.temperature(), FLASH_K, max_relative = 1e-10);
    }

    #[test]
    fn a_positive_duty_boils_more_than_the_adiabatic_flash() {
        let r = humid_nitrogen();
        let inlet = humid(&r, [100.0, 0.0, 28.0]);
        let adiabatic = duty_flashed(&r, &inlet, 0.0);
        let heated = duty_flashed(&r, &inlet, 1e4);

        assert_relative_eq!(
            outlet_enthalpy(&r, &heated),
            inlet.enthalpy(&r) + 1e4,
            max_relative = 1e-11
        );
        assert!(heated.0.temperature() > adiabatic.0.temperature());
        assert!(heated.0.flows()[1] > adiabatic.0.flows()[1]);
    }

    #[test]
    fn cooling_humid_gas_condenses_some_of_its_steam() {
        // The search starts above the answer and brackets it from below.
        let r = humid_nitrogen();
        let inlet = Stream::from_flows(&r, vec![0.0, 20.0, 28.0], 400.0, AMBIENT_KPA);
        let outlets = duty_flashed(&r, &inlet, -3e4);

        assert_relative_eq!(
            outlet_enthalpy(&r, &outlets),
            inlet.enthalpy(&r) - 3e4,
            max_relative = 1e-11
        );
        assert!(outlets.1.flows()[0] > 0.0, "some steam should condense");
        assert!(outlets.0.temperature() < 400.0);
    }

    /// Water's boiling point at one atmosphere, from the shipped fit.
    fn boiling(r: &SpeciesRegistry) -> f64 {
        r.all()[0]
            .vapour_pressure
            .unwrap()
            .boiling_point(AMBIENT_KPA)
    }

    /// The heat that boils `mass` t/h of water at its boiling point, MJ/h.
    fn latent(r: &SpeciesRegistry, mass: f64) -> f64 {
        let per_mole = crate::species::latent_heat(&r.all()[0], &r.all()[1], boiling(r));
        1000.0 * mass / r.all()[0].molar_mass * per_mole
    }

    #[test]
    fn pure_water_given_half_its_latent_heat_boils_half_away_at_its_boiling_point() {
        // The case with no temperature to search for: H(T) jumps at the boiling point.
        let r = humid_nitrogen();
        let tb = boiling(&r);
        let inlet = Stream::from_flows(&r, vec![10.0, 0.0, 0.0], tb, AMBIENT_KPA);
        let (vapour, liquid) = duty_flashed(&r, &inlet, latent(&r, 5.0));

        assert_relative_eq!(vapour.flows()[1], 5.0, max_relative = 1e-10);
        assert_relative_eq!(liquid.flows()[0], 5.0, max_relative = 1e-10);
        assert_eq!((vapour.temperature(), liquid.temperature()), (tb, tb));
    }

    #[test]
    fn pure_water_given_more_than_its_latent_heat_leaves_as_superheated_steam() {
        let r = humid_nitrogen();
        let tb = boiling(&r);
        let inlet = Stream::from_flows(&r, vec![10.0, 0.0, 0.0], tb, AMBIENT_KPA);
        let outlets = duty_flashed(&r, &inlet, 2.0 * latent(&r, 10.0));

        assert_eq!((outlets.0.flows()[1], outlets.1.total()), (10.0, 0.0));
        assert!(outlets.0.temperature() > tb);
        assert_relative_eq!(
            outlet_enthalpy(&r, &outlets),
            inlet.enthalpy(&r) + 2.0 * latent(&r, 10.0),
            max_relative = 1e-11
        );
    }

    #[test]
    fn pure_water_short_of_boiling_warms_exactly_as_a_heater_would() {
        let r = humid_nitrogen();
        let inlet = Stream::from_flows(&r, vec![10.0, 0.0, 0.0], 300.0, AMBIENT_KPA);
        let (vapour, liquid) = duty_flashed(&r, &inlet, 1000.0);

        assert_eq!(vapour.total(), 0.0);
        assert_relative_eq!(
            liquid.temperature(),
            heat(&r, &inlet, 1000.0).unwrap().temperature(),
            max_relative = 1e-12
        );
    }

    #[test]
    fn steam_cooled_past_its_latent_heat_leaves_as_liquid() {
        let r = humid_nitrogen();
        let inlet = Stream::from_flows(&r, vec![0.0, 10.0, 0.0], 400.0, AMBIENT_KPA);
        // Enough to condense it all and then take a couple of thousand MJ/h off the liquid, which
        // at about 42 MJ/(h K) is some 35 K below the boiling point.
        let (vapour, liquid) = duty_flashed(&r, &inlet, -latent(&r, 10.0) - 2000.0);

        assert_eq!((vapour.total(), liquid.flows()[0]), (0.0, 10.0));
        assert!(liquid.temperature() < boiling(&r));
    }

    #[test]
    fn a_boiling_drum_keeps_its_solids_in_the_bottoms_at_the_boiling_point() {
        let mut r = humid_nitrogen();
        r.insert(
            crate::library::find("SiO2", Phase::Solid)
                .unwrap()
                .species
                .clone(),
        );
        let tb = boiling(&r);
        let inlet = Stream::from_flows(&r, vec![10.0, 0.0, 0.0, 5.0], tb, AMBIENT_KPA);
        let outlets = duty_flashed(&r, &inlet, latent(&r, 5.0));

        assert_eq!(outlets.0.flows()[3], 0.0);
        assert_eq!(outlets.1.flows()[3], 5.0);
        assert_relative_eq!(outlets.0.flows()[1], 5.0, max_relative = 1e-10);
        assert_relative_eq!(
            outlet_enthalpy(&r, &outlets),
            inlet.enthalpy(&r) + latent(&r, 5.0),
            max_relative = 1e-11
        );
    }

    #[test]
    fn single_substance_ignores_solids_and_needs_both_phases() {
        let mut r = humid_nitrogen();
        r.insert(
            crate::library::find("SiO2", Phase::Solid)
                .unwrap()
                .species
                .clone(),
        );
        let at = |flows: Vec<f64>| Stream::from_flows(&r, flows, FLASH_K, AMBIENT_KPA);

        assert!(single_substance(&r, &at(vec![1.0, 1.0, 0.0, 9.0])).is_some());
        assert!(single_substance(&r, &at(vec![1.0, 0.0, 1.0, 0.0])).is_none()); // with nitrogen
        assert!(single_substance(&r, &at(vec![0.0, 0.0, 1.0, 0.0])).is_none()); // gas only
        assert!(single_substance(&r, &at(vec![0.0, 0.0, 0.0, 9.0])).is_none()); // solids only
    }

    #[test]
    fn a_duty_flash_of_an_empty_stream_gives_two_empty_outlets() {
        let r = humid_nitrogen();
        let empty = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let (vapour, liquid) = duty_flashed(&r, &empty, 1e6);
        assert_eq!((vapour.total(), liquid.total()), (0.0, 0.0));
        assert_eq!(vapour.temperature(), AMBIENT_K);
    }

    #[test]
    fn a_duty_no_positive_temperature_absorbs_is_an_error_not_a_panic() {
        // Constant heat capacities, so each species holds a finite enthalpy at 0 K. The shipped
        // liquid water's Shomate `E` term sends its extrapolated enthalpy to minus infinity as T
        // falls to zero, so over that fit *some* positive temperature answers any duty - a
        // fraction of a kelvin, which the search duly finds.
        let mut r = SpeciesRegistry::default();
        for (s, cp) in humid_nitrogen().all().iter().zip([75.3, 33.6, 29.1]) {
            r.insert(Species {
                shomate: Shomate::constant(cp),
                ..s.clone()
            });
        }
        for inlet in [
            humid(&r, [100.0, 0.0, 28.0]), // a mixture: the search gives up
            humid(&r, [100.0, 0.0, 0.0]),  // pure water: the temperature solve does
        ] {
            let e =
                flash_with_duty(&r, &inlet, -1e9, AMBIENT_KPA).expect_err("nothing is that cold");
            assert!(e.to_string().contains("no positive temperature"), "{e}");
        }
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
                enthalpy_of_formation: None,
                density: None,
                vapour_pressure: None,
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

    /// [`combustion_registry`] with one species' enthalpy of formation taken away.
    fn without_formation(name: &str) -> SpeciesRegistry {
        let mut r = SpeciesRegistry::default();
        for species in combustion_registry(COMBUSTION_MASSES).all() {
            r.insert(Species {
                enthalpy_of_formation: species
                    .enthalpy_of_formation
                    .filter(|_| species.name != name),
                ..species.clone()
            });
        }
        r
    }

    #[test]
    fn formation_enthalpies_names_the_participant_without_one() {
        let r = without_formation("CO2");
        let missing = formation_enthalpies(&r, &[-1.0, -2.0, 1.0, 2.0]).unwrap_err();
        assert_eq!(missing.name, "CO2");
    }

    #[test]
    fn formation_enthalpies_ignores_a_species_the_reaction_does_not_touch() {
        // Burning methane with CO2 left out of the equation: not balanced, but this only asks
        // about the participants, and CO2 is not one.
        let r = without_formation("CO2");
        assert!(formation_enthalpies(&r, &[-1.0, -2.0, 0.0, 2.0]).is_ok());
    }

    #[test]
    #[should_panic(
        expected = "species `H2O` takes part in the reaction but has no enthalpy of formation"
    )]
    fn react_rejects_a_participant_without_a_formation_enthalpy() {
        let r = without_formation("H2O");
        let _ = react(&r, &lean(&r), &combustion(&r, 0.9));
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
        // With formation enthalpies, because the reactor below evaluates a reaction over them.
        let r = demo_registry_with_formation();

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
            (Box::new(Mixer::default()), (1, None), (1, Some(1))),
            (Box::new(Tank), (1, Some(1)), (1, Some(1))),
            (
                Box::new(Heater {
                    duty: 1000.0,
                    pressure_drop: 0.0,
                }),
                (1, Some(1)),
                (1, Some(1)),
            ),
            (
                Box::new(Splitter {
                    fraction: 0.3,
                    pressure_drop: 0.0,
                }),
                (1, Some(1)),
                (2, Some(2)),
            ),
            (
                Box::new(SplitterN {
                    ratios: vec![1.0, 1.0, 2.0],
                    pressure_drop: 0.0,
                }),
                (1, Some(1)),
                (3, Some(3)),
            ),
            (
                Box::new(Flotation {
                    recovery: vec![0.85, 0.05, 0.30],
                    pressure_drop: 0.0,
                }),
                (1, Some(1)),
                (2, Some(2)),
            ),
            (Box::new(Product), (1, Some(1)), (0, Some(0))),
            // Nothing in the demo slurry evaporates, so this returns an empty vapour - which is
            // still two outlets.
            (
                Box::new(Flash {
                    pressure: AMBIENT_KPA,
                    energy: FlashEnergy::Temperature(350.0),
                }),
                (1, Some(1)),
                (2, Some(2)),
            ),
            // Not chemistry: the demo species have no real reaction between them, so this turns
            // chalcopyrite into its own mass of quartz, which is all the closure check asks.
            (
                Box::new(ConversionReactor {
                    reactions: vec![Reaction {
                        stoichiometry: vec![-1.0, 183.5 / 60.08, 0.0],
                        limiting: all_ids(&r)[0],
                        conversion: 0.5,
                    }],
                    energy: ReactorEnergy::Isothermal,
                    pressure_drop: 0.0,
                }),
                (1, Some(1)),
                (1, Some(1)),
            ),
        ];

        for (op, inlets, outlets) in cases {
            assert_declared_arity(&*op, &r, &feed(&r), inlets, outlets);
        }

        // The two that cannot run on the demo feed as it stands: a pump needs densities, and a
        // compressor needs a gas.
        let pumped = demo_registry_with_density();
        assert_declared_arity(
            &Pump {
                pressure_rise: 100.0,
                efficiency: 0.7,
            },
            &pumped,
            &feed(&pumped),
            (1, Some(1)),
            (1, Some(1)),
        );
        let gases = combustion_registry(COMBUSTION_MASSES);
        assert_declared_arity(
            &Compressor {
                pressure_rise: 100.0,
                efficiency: 0.8,
            },
            &gases,
            &Stream::from_flows(&gases, vec![1.0, 4.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA),
            (1, Some(1)),
            (1, Some(1)),
        );
    }

    /// Checks `op` declares `inlets` and `outlets`, and that `evaluate` over the minimum inlet
    /// count returns the minimum outlet count.
    fn assert_declared_arity(
        op: &dyn UnitOp,
        r: &SpeciesRegistry,
        stream: &Stream,
        inlets: (usize, Option<usize>),
        outlets: (usize, Option<usize>),
    ) {
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

        let supplied: Vec<&Stream> = vec![stream; op.inlet_arity().min];
        assert_eq!(
            op.evaluate(r, &supplied).unwrap().len(),
            op.outlet_arity().min,
            "{op:?} returned an outlet count its arity does not declare"
        );
    }

    /// The `Send + Sync` supertraits on [`UnitOp`] exist only to keep this true; nothing else
    /// in the crate would fail if they were dropped, so the guarantee needs its own test.
    #[test]
    fn a_valid_flowsheet_can_still_cross_a_thread_boundary() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::flowsheet::ValidFlowsheet>();
    }
}

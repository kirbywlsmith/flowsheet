//! The on-disk representation of a flowsheet.
//!
//! These types mirror the domain types but hold no invariants - they are whatever the file said.
//! Validation happens on the way out, in the conversion into [`crate::flowsheet::Flowsheet`].

use crate::MAX_IDS;
use crate::flowsheet::{self, UnitId};
use crate::species::{Phase, Species, SpeciesId, SpeciesRegistry};
use crate::thermo::REFERENCE_K;
use crate::{stream, unit};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Stream temperature (K) assumed when the file omits one.
pub const AMBIENT_K: f64 = 298.15;
/// Stream pressure (kPa) assumed when the file omits one.
pub const AMBIENT_KPA: f64 = 101.325;

/// A whole flowsheet, as read from or written to a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flowsheet {
    /// Every species. Position determines [`crate::species::SpeciesId`] order.
    pub species: Vec<Species>,
    /// Every unit.
    pub units: Vec<Unit>,
    /// Every stream. Order determines port order on the units it connects.
    pub streams: Vec<Stream>,
}

/// The composition and thermodynamic state of a stream.
///
/// The fields are declared in alphabetical order, which reads oddly and is deliberate. A stream's
/// state is serialised straight from this struct, in declaration order, but a feed's state is
/// nested inside a spec that [`ToDocument::spec`] turns into a `serde_json::Value` first - and
/// that pass rewrites every struct as a map, which sorts. Declaring the fields in the order the
/// map produces keeps both paths writing the same bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// Absolute mass flow per species (t/h), keyed by [`SpeciesRegistry::key`]: the bare name, or
    /// `"H2O(g)"` where two phases share one. Absent species are zero.
    pub flows: BTreeMap<String, f64>,
    /// Pressure, in kPa.
    pub pressure: f64,
    /// Temperature, in Kelvin.
    pub temperature: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            flows: BTreeMap::new(),
            pressure: AMBIENT_KPA,
            temperature: AMBIENT_K,
        }
    }
}

/// One unit operation, named so streams can refer to it.
///
/// `op` is a nested object rather than a flattened one: `serde` cannot combine `flatten` with
/// `deny_unknown_fields`, and silently swallowing a `fraction` typed onto a mixer costs more than
/// the extra nesting level does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// Unique within the document. Referenced by [`Stream::from`] and [`Stream::to`].
    pub name: String,
    /// What the unit does.
    pub op: Op,
}

/// What a unit does: a name, and whatever parameters that name implies.
///
/// An enum with one variant per built-in operation is the obvious shape, and it closes the wire
/// format while [`crate::unit::UnitOp`] stays open: a downstream operation has no variant of its
/// own, so it can only describe itself as one of the built-ins, and loading hands that built-in
/// back with nothing wrong at either boundary. A string tag and an uninterpreted JSON object put
/// the name-to-constructor table in [`OpRegistry`] instead, where a caller can add to it. The JSON
/// shape is the same either way: `{ "type": "splitter", "fraction": 0.3 }`.
///
/// That costs two things. `serde_json` is a dependency of this crate rather than a dev-dependency,
/// and `deny_unknown_fields` cannot fire while parsing, because `flatten` forbids it here. A stray
/// `fraction` on a mixer lands in `spec` and is caught one step later, when [`OpRegistry`] hands it
/// to the mixer's constructor: still an error, still naming the field, but a
/// [`LoadError::BadOp`] rather than a parse error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Op {
    /// Which operation this is. The key its constructor is registered under in [`OpRegistry`].
    #[serde(rename = "type")]
    pub tag: String,
    /// Everything else in the object - the operation's own parameters, uninterpreted.
    #[serde(flatten)]
    pub spec: serde_json::Map<String, serde_json::Value>,
}

impl Op {
    /// Builds an `Op` from a tag and an already-serialised spec.
    ///
    /// # Panics
    /// If `spec` is not a JSON object. It is flattened beside `type`, so nothing else can be
    /// written there - a spec type must serialise to a map.
    fn new(tag: &'static str, spec: serde_json::Value) -> Self {
        match spec {
            serde_json::Value::Object(spec) => Self {
                tag: tag.to_string(),
                spec,
            },
            other => panic!("`{tag}` produced {other}, but an op spec must be a JSON object"),
        }
    }
}

/// The parameters of a `feed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedSpec {
    /// The state this feed emits.
    pub state: State,
}

/// `skip_serializing_if` hands its predicate a reference, hence `&f64`.
fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// The parameters of a `mixer`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixerSpec {
    /// Pressure lost across the unit, in kPa. Optional; an omitted or zero drop is left off on
    /// save, so a document written before the field existed reads back byte for byte.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// The parameters of a `splitter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterSpec {
    /// The fraction sent to the first outlet.
    pub fraction: f64,
    /// Pressure lost across the unit, in kPa. See [`MixerSpec::pressure_drop`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// The parameters of a `splitter_n`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterNSpec {
    /// The ratios, one per outlet.
    pub ratios: Vec<f64>,
    /// Pressure lost across the unit, in kPa. See [`MixerSpec::pressure_drop`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// The parameters of a `flotation`.
///
/// `recovery` is keyed by species name, for the same reason [`State::flows`] is: a document
/// should not depend on the order of the `species` list. A species the map omits recovers
/// nothing and leaves entirely in the tails, the same way an omitted flow is zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlotationSpec {
    /// The fraction of each species reporting to the concentrate, keyed by species name.
    pub recovery: BTreeMap<String, f64>,
    /// Pressure lost across the unit, in kPa. See [`MixerSpec::pressure_drop`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// One reaction, as a document writes it.
///
/// `stoichiometry` is keyed by species name, like [`FlotationSpec::recovery`], but a species it
/// omits simply takes no part in the reaction, and saving drops zero coefficients: a zero recovery
/// says a species does not float, while an equation only ever lists its participants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionSpec {
    /// Molar coefficients keyed by species name: negative consumed, positive produced.
    pub stoichiometry: BTreeMap<String, f64>,
    /// The name of the reactant `conversion` is a fraction of.
    pub limiting: String,
    /// The fraction of the limiting reactant that reacts.
    pub conversion: f64,
}

/// The parameters of a `conversion_reactor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionReactorSpec {
    /// `"isothermal"` or `"adiabatic"`. Required, with no default: a silent isothermal default is
    /// how a combustion chamber ends up at 25 °C.
    pub energy: unit::ReactorEnergy,
    /// The reactions the reactor runs, in the order it runs them. At least one.
    pub reactions: Vec<ReactionSpec>,
    /// Pressure lost across the unit, in kPa. See [`MixerSpec::pressure_drop`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// The parameters of a `heater`.
///
/// `duty` has no load-time check. The domain rejects only a non-finite duty, and a document
/// cannot contain one: `serde_json` refuses `NaN`, `Infinity` and out-of-range literals such as
/// `1e400`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaterSpec {
    /// Heat added, in MJ/h. Negative removes it.
    pub duty: f64,
    /// Pressure lost across the unit, in kPa. See [`MixerSpec::pressure_drop`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pressure_drop: f64,
}

/// The parameters of a `pump` or a `compressor`: the same two fields, read the same way, so one
/// spec type serves both and the tag alone tells them apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PumpSpec {
    /// Pressure added, in kPa. Never negative.
    pub pressure_rise: f64,
    /// The efficiency, in `(0.0, 1.0]`. Optional, defaulting to the ideal machine, and left off
    /// on save when it is 1 - the same rule as a zero `pressure_drop`.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub efficiency: f64,
}

/// `serde(default = ..)` wants a function, not a literal.
fn one() -> f64 {
    1.0
}

fn is_one(x: &f64) -> bool {
    *x == 1.0
}

/// The parameters of a unit operation that takes none. Empty, but not omitted: it is what rejects
/// a stray field on a `tank` or `product`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoSpec {}

/// A stream connecting two units.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    /// The name of the unit this stream leaves.
    pub from: String,
    /// The name of the unit this stream enters.
    pub to: String,
    /// Solver output on save; only the initial guess on load, so it may be omitted.
    #[serde(default)]
    pub state: State,
}

// ---------------------------------------------------------------------------
// Loading: serial -> domain
// ---------------------------------------------------------------------------

/// Where in the document a problem was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The `species` entry with this name.
    Species(String),
    /// The unit with this name.
    Unit(String),
    /// The `streams` entry at this index.
    Stream(usize),
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Species(name) => write!(f, "species `{name}`"),
            Location::Unit(name) => write!(f, "unit `{name}`"),
            Location::Stream(i) => write!(f, "streams[{i}]"),
        }
    }
}

/// A type of error returned when a [`Flowsheet`] document cannot be replayed into a
/// [`crate::flowsheet::Flowsheet`].
///
/// Unlike [`crate::flowsheet::Flowsheet::check`], which collects every structural problem, this
/// stops at the first one: load errors are typos, and they are fixed one at a time.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// A stream endpoint names a unit that is not in `units`.
    UnknownUnit {
        /// The index of the offending `streams` entry.
        stream: usize,
        /// The name that could not be resolved.
        name: String,
    },
    /// A flows map names a species that is not in `species`.
    UnknownSpecies {
        /// Where the name appeared.
        at: Location,
        /// The name that could not be resolved.
        name: String,
    },
    /// Two units share a name, so a stream endpoint would be ambiguous.
    DuplicateUnit {
        /// The repeated name.
        name: String,
    },
    /// A name map uses a bare name that more than one phase shares, so it cannot say which it
    /// means.
    AmbiguousSpecies {
        /// Where the name appeared.
        at: Location,
        /// The bare name.
        name: String,
        /// The keys that would have been unambiguous, in declaration order.
        keys: Vec<String>,
    },
    /// A species-keyed map names one species twice, once bare and once with its phase suffix -
    /// `"H2O"` and `"H2O(l)"` - so one of the two values would be silently dropped.
    DuplicateSpeciesKey {
        /// Where the map appeared.
        at: Location,
        /// The key that came first, in the map's sorted order.
        first: String,
        /// The key that named the same species again.
        second: String,
    },
    /// Two species share a name *and* a phase. The same name in two phases is legal, keyed as
    /// `"H2O(l)"` and `"H2O(g)"`.
    DuplicateSpecies {
        /// The repeated name.
        name: String,
        /// The phase both entries declare.
        phase: Phase,
    },
    /// A species name ends in a phase suffix such as `(g)`, which a key would read as its phase.
    ReservedSpeciesName {
        /// The offending name.
        name: String,
    },
    /// A numeric field is outside the range the domain accepts.
    BadValue {
        /// Where the value appeared.
        at: Location,
        /// The field, or the species name for a flow.
        field: String,
        /// What the document said.
        value: f64,
        /// What was required.
        expected: String,
    },
    /// A reaction names a species that has no `enthalpy_of_formation`.
    ///
    /// Optional everywhere else, because a species nothing creates or destroys carries the same
    /// offset on both sides of a balance. A reaction's participants do not, and a missing value
    /// would read as zero and invent a heat of reaction.
    MissingFormationEnthalpy {
        /// The unit whose reaction names the species.
        at: Location,
        /// The species without one.
        species: String,
    },
    /// A unit's `type` is not a tag the [`OpRegistry`] knows.
    UnknownOp {
        /// The unit the tag appeared on.
        at: Location,
        /// The tag that could not be resolved.
        tag: String,
    },
    /// A unit's parameters did not match the shape its operation expects.
    ///
    /// Where `deny_unknown_fields` lands: [`Op`] flattens the parameters, so a stray `fraction`
    /// on a `mixer` parses fine and fails here.
    BadOp {
        /// The unit the parameters appeared on.
        at: Location,
        /// The operation that rejected them.
        tag: String,
        /// What `serde` said. Held as text because [`serde_json::Error`] is neither [`Clone`]
        /// nor [`PartialEq`], and this type is both.
        message: String,
    },
    /// The document has more units or streams than a `u16` id can address.
    TooMany {
        /// Either `"species"`, `"units"` or `"streams"`.
        what: &'static str,
        /// How many the document has.
        count: usize,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::UnknownUnit { stream, name } => {
                write!(
                    f,
                    "streams[{stream}] names unit `{name}`, which is not declared"
                )
            }
            LoadError::UnknownSpecies { at, name } => {
                write!(f, "{at} names species `{name}`, which is not declared")
            }
            LoadError::DuplicateUnit { name } => {
                write!(f, "unit `{name}` is declared more than once")
            }
            LoadError::AmbiguousSpecies { at, name, keys } => write!(
                f,
                "{at} names species `{name}`, which more than one phase shares - write one of `{}`",
                keys.join("`, `")
            ),
            LoadError::DuplicateSpeciesKey { at, first, second } => write!(
                f,
                "{at} names one species twice, as `{first}` and `{second}` - write it once"
            ),
            LoadError::DuplicateSpecies { name, phase } => {
                write!(f, "species `{name}` ({phase:?}) is declared more than once")
            }
            LoadError::ReservedSpeciesName { name } => write!(
                f,
                "species `{name}` ends in a phase suffix - `(s)`, `(l)` and `(g)` are reserved \
                 for telling phases apart"
            ),
            LoadError::BadValue {
                at,
                field,
                value,
                expected,
            } => write!(f, "{at}: `{field}` is {value}, expected {expected}"),
            LoadError::MissingFormationEnthalpy { at, species } => write!(
                f,
                "{at}: species `{species}` takes part in the reaction but has no \
                 `enthalpy_of_formation`"
            ),
            LoadError::UnknownOp { at, tag } => {
                write!(f, "{at}: `{tag}` is not a registered unit operation")
            }
            LoadError::BadOp { at, tag, message } => {
                write!(f, "{at}: `{tag}` parameters are invalid: {message}")
            }
            LoadError::TooMany { what, count } => {
                write!(
                    f,
                    "{count} {what} exceeds the {MAX_IDS} a u16 id can address"
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Returns [`LoadError::BadValue`] unless `ok`.
fn require(
    ok: bool,
    at: &Location,
    field: &str,
    value: f64,
    expected: &str,
) -> Result<(), LoadError> {
    if ok {
        Ok(())
    } else {
        Err(LoadError::BadValue {
            at: at.clone(),
            field: field.to_string(),
            value,
            expected: expected.to_string(),
        })
    }
}

/// Resolves a document's species key through [`SpeciesRegistry::resolve`], telling a bare name
/// two phases share apart from one that is not declared at all.
fn resolve_species(
    registry: &SpeciesRegistry,
    key: &str,
    at: &Location,
) -> Result<SpeciesId, LoadError> {
    registry.resolve(key).ok_or_else(|| {
        if registry.shares_name(key) {
            LoadError::AmbiguousSpecies {
                at: at.clone(),
                name: key.to_string(),
                keys: registry
                    .all()
                    .iter()
                    .filter(|s| s.name == key)
                    .map(|s| format!("{key}{}", s.phase.suffix()))
                    .collect(),
            }
        } else {
            LoadError::UnknownSpecies {
                at: at.clone(),
                name: key.to_string(),
            }
        }
    })
}

/// Resolves a species-keyed map into a dense vector in [`SpeciesId`] order, running `check` on
/// each value. Species the map leaves out are zero.
///
/// One species has two keys once a suffixed form is accepted - `"H2O"` and `"H2O(l)"` - so a map
/// can name it twice, and writing the second value over the first would drop one without a word.
/// That is a [`LoadError::DuplicateSpeciesKey`] instead.
fn species_vector<'m>(
    registry: &SpeciesRegistry,
    map: &'m BTreeMap<String, f64>,
    at: &Location,
    mut check: impl FnMut(&'m str, f64) -> Result<(), LoadError>,
) -> Result<Vec<f64>, LoadError> {
    let mut dense = vec![0.0; registry.len()];
    let mut seen: Vec<Option<&str>> = vec![None; registry.len()];
    for (key, &value) in map {
        let id = resolve_species(registry, key, at)?;
        if let Some(first) = seen[id.as_usize()].replace(key) {
            return Err(LoadError::DuplicateSpeciesKey {
                at: at.clone(),
                first: first.to_string(),
                second: key.clone(),
            });
        }
        check(key, value)?;
        dense[id.as_usize()] = value;
    }
    Ok(dense)
}

impl State {
    /// Resolves the species-keyed flows against `registry`, producing a [`stream::Stream`] whose
    /// flows are in [`SpeciesId`] order. Species the document omits are zero.
    fn to_stream(
        &self,
        registry: &SpeciesRegistry,
        at: &Location,
    ) -> Result<stream::Stream, LoadError> {
        require(
            self.temperature.is_finite() && self.temperature > 0.0,
            at,
            "temperature",
            self.temperature,
            "greater than 0.0 K",
        )?;
        // Strictly positive, like temperature: kPa is absolute, and the solver's pressure
        // residual divides by it.
        require(
            self.pressure.is_finite() && self.pressure > 0.0,
            at,
            "pressure",
            self.pressure,
            "greater than 0.0 kPa",
        )?;

        let flows = species_vector(registry, &self.flows, at, |name, value| {
            require(
                value.is_finite() && value >= 0.0,
                at,
                name,
                value,
                "0.0 t/h or greater",
            )
        })?;

        Ok(stream::Stream::from_flows(
            registry,
            flows,
            self.temperature,
            self.pressure,
        ))
    }
}

/// Everything a constructor needs to turn one [`Op`] into a domain operation.
///
/// A struct rather than three parameters, because a constructor is stored behind a `dyn Fn` and
/// every added argument would be a breaking change to the [`OpRegistry`] table's type.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    /// The operation's parameters, exactly as the document wrote them.
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    /// Every species, in [`SpeciesId`] order.
    pub registry: &'a SpeciesRegistry,
    /// The unit being loaded, for error messages.
    pub at: &'a Location,
}

impl<'a> Spec<'a> {
    /// Resolves a species key from the parameters - a bare name, or `"H2O(g)"` where two phases
    /// share one - for turning a species-keyed map into a dense vector.
    ///
    /// # Errors
    ///
    /// [`LoadError::UnknownSpecies`] for a key that names nothing, and
    /// [`LoadError::AmbiguousSpecies`] for a bare name more than one phase shares.
    pub fn species(&self, key: &str) -> Result<SpeciesId, LoadError> {
        resolve_species(self.registry, key, self.at)
    }

    /// Checks a `pressure_drop` parameter: the one value [`unit::drop_pressure`] panics on,
    /// caught here so it cannot reach the solver. A drop is never negative - a rise is a pump.
    ///
    /// # Errors
    ///
    /// [`LoadError::BadValue`] for a negative drop. A non-finite one cannot be written: JSON has
    /// no `NaN` or `Infinity`.
    pub fn pressure_drop(&self, drop: f64) -> Result<(), LoadError> {
        require(
            drop.is_finite() && drop >= 0.0,
            self.at,
            "pressure_drop",
            drop,
            "0.0 kPa or greater",
        )
    }

    /// Checks a `pressure_rise` parameter, the mirror of [`Spec::pressure_drop`]: never
    /// negative, because a fall is a drop and every other operation already has one.
    ///
    /// # Errors
    ///
    /// [`LoadError::BadValue`] for a negative rise.
    pub fn pressure_rise(&self, rise: f64) -> Result<(), LoadError> {
        require(
            rise.is_finite() && rise >= 0.0,
            self.at,
            "pressure_rise",
            rise,
            "0.0 kPa or greater",
        )
    }

    /// Checks an `efficiency` parameter: the values [`unit::pump`] and [`unit::compress`] panic
    /// on, caught here so they cannot reach the solver. Zero would divide the work by nothing,
    /// and more than one would make a machine that gives back more than it takes.
    ///
    /// # Errors
    ///
    /// [`LoadError::BadValue`] for an efficiency outside `(0.0, 1.0]`.
    pub fn efficiency(&self, efficiency: f64) -> Result<(), LoadError> {
        require(
            efficiency > 0.0 && efficiency <= 1.0,
            self.at,
            "efficiency",
            efficiency,
            "greater than 0.0 and at most 1.0",
        )
    }

    /// Deserialises the parameters into an operation's own spec type.
    ///
    /// Where a spec type's `deny_unknown_fields` fires - see [`Op`] for why it cannot fire at
    /// parse time.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError::BadOp`] if the parameters do not match `T`.
    pub fn parse<T: serde::de::DeserializeOwned>(&self, tag: &str) -> Result<T, LoadError> {
        // Cloning the map is the price of `from_value` taking ownership. A spec is a handful of
        // keys and this runs once per unit per load, not once per solver pass.
        serde_json::from_value(serde_json::Value::Object(self.params.clone())).map_err(|e| {
            LoadError::BadOp {
                at: self.at.clone(),
                tag: tag.to_string(),
                message: e.to_string(),
            }
        })
    }
}

/// A constructor: one [`Op`] in, one domain operation out.
type Constructor = Box<dyn Fn(Spec<'_>) -> Result<Box<dyn unit::UnitOp>, LoadError> + Send + Sync>;

/// The name-to-constructor table loading resolves a unit's `type` against.
///
/// This is the half of the round trip a trait object cannot do for itself. Saving can be a
/// method on the operation ([`ToDocument`]) because the operation still knows what it is;
/// loading has only a string, so something has to own the mapping. Making that something a value
/// rather than a `match` is what lets a downstream crate add to it:
///
/// ```
/// use flowsheet::serial::{OpRegistry, Spec, ToDocument};
/// use flowsheet::{EvalError, SpeciesRegistry, Stream, UnitOp, unit::Arity};
///
/// /// A vent: one inlet, one outlet, discarding `rate` of every species.
/// #[derive(Debug)]
/// struct Bleed { rate: f64 }
///
/// /// Its document form. `deny_unknown_fields` fires here, in `Spec::parse`.
/// #[derive(serde::Serialize, serde::Deserialize)]
/// #[serde(deny_unknown_fields)]
/// struct BleedSpec { rate: f64 }
///
/// impl ToDocument for Bleed {
///     fn tag(&self) -> &'static str { "bleed" }
///     fn spec(&self, _: &SpeciesRegistry) -> serde_json::Value {
///         serde_json::to_value(BleedSpec { rate: self.rate }).unwrap()
///     }
/// }
/// # impl UnitOp for Bleed {
/// #     fn inlet_arity(&self) -> Arity { Arity::exactly(1) }
/// #     fn outlet_arity(&self) -> Arity { Arity::exactly(1) }
/// #     fn evaluate(&self, _: &SpeciesRegistry, inlets: &[&Stream])
/// #         -> Result<Vec<Stream>, EvalError> {
/// #         Ok(vec![flowsheet::unit::split(inlets[0], 1.0 - self.rate).0])
/// #     }
/// # }
///
/// let mut ops = OpRegistry::builtin();
/// ops.register("bleed", |s: Spec<'_>| {
///     let BleedSpec { rate } = s.parse("bleed")?;
///     Ok(Box::new(Bleed { rate }))
/// });
/// ```
///
/// The tag has to be the same string on both sides, which is why the built-ins keep theirs in
/// one place - `unit::Splitter::TAG` and friends - rather than spelling it twice.
///
/// Registering over an existing tag replaces it, so a caller can also override a built-in.
pub struct OpRegistry {
    table: BTreeMap<&'static str, Constructor>,
}

impl fmt::Debug for OpRegistry {
    /// A constructor is a closure and has no `Debug`, so only the tags are shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OpRegistry")
            .field(&self.table.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for OpRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl OpRegistry {
    /// An empty table. Nothing loads until something is registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: BTreeMap::new(),
        }
    }

    /// The nine operations this crate ships, under the tags they save themselves as.
    #[must_use]
    pub fn builtin() -> Self {
        let mut ops = Self::new();

        ops.register(unit::Feed::TAG, |s| {
            let spec: FeedSpec = s.parse(unit::Feed::TAG)?;
            Ok(Box::new(unit::Feed {
                stream: spec.state.to_stream(s.registry, s.at)?,
            }))
        });

        ops.register(unit::Mixer::TAG, |s| {
            let MixerSpec { pressure_drop } = s.parse(unit::Mixer::TAG)?;
            s.pressure_drop(pressure_drop)?;
            Ok(Box::new(unit::Mixer { pressure_drop }))
        });

        ops.register(unit::Splitter::TAG, |s| {
            let SplitterSpec {
                fraction,
                pressure_drop,
            } = s.parse(unit::Splitter::TAG)?;
            require(
                (0.0..=1.0).contains(&fraction),
                s.at,
                "fraction",
                fraction,
                "between 0.0 and 1.0",
            )?;
            s.pressure_drop(pressure_drop)?;
            Ok(Box::new(unit::Splitter {
                fraction,
                pressure_drop,
            }))
        });

        ops.register(unit::SplitterN::TAG, |s| {
            let SplitterNSpec {
                ratios,
                pressure_drop,
            } = s.parse(unit::SplitterN::TAG)?;
            s.pressure_drop(pressure_drop)?;
            for &r in &ratios {
                require(
                    r.is_finite() && r >= 0.0,
                    s.at,
                    "ratios",
                    r,
                    "0.0 or greater",
                )?;
            }
            // An empty `ratios` sums to zero, so this catches that case too.
            let sum: f64 = ratios.iter().sum();
            require(sum > 0.0, s.at, "ratios", sum, "a sum greater than 0.0")?;
            Ok(Box::new(unit::SplitterN {
                ratios,
                pressure_drop,
            }))
        });

        ops.register(unit::Flotation::TAG, |s| {
            let FlotationSpec {
                recovery,
                pressure_drop,
            } = s.parse(unit::Flotation::TAG)?;
            s.pressure_drop(pressure_drop)?;
            // Resolved into a dense `SpeciesId`-ordered vector, the same shape as a stream's
            // flows. A species the map leaves out recovers nothing.
            let recovery = species_vector(s.registry, &recovery, s.at, |name, value| {
                require(
                    (0.0..=1.0).contains(&value),
                    s.at,
                    name,
                    value,
                    "between 0.0 and 1.0",
                )
            })?;
            Ok(Box::new(unit::Flotation {
                recovery,
                pressure_drop,
            }))
        });

        ops.register(unit::ConversionReactor::TAG, |s| {
            let ConversionReactorSpec {
                energy,
                reactions,
                pressure_drop,
            } = s.parse(unit::ConversionReactor::TAG)?;
            s.pressure_drop(pressure_drop)?;

            // The one panic in `ConversionReactor::evaluate` itself. A count is not an `f64`, but
            // `BadValue` prints it as one without trouble and a variant of its own would buy nothing.
            require(
                !reactions.is_empty(),
                s.at,
                "reactions",
                0.0,
                "at least one reaction",
            )?;

            let mut domain = Vec::with_capacity(reactions.len());
            for (i, reaction) in reactions.iter().enumerate() {
                // Every field is named with its reaction, so a bad value in the third reaction of
                // five says which.
                let field = |name: &str| format!("reactions[{i}].{name}");

                // A species the map leaves out takes no part in the reaction.
                let stoichiometry =
                    species_vector(s.registry, &reaction.stoichiometry, s.at, |_, _| Ok(()))?;
                let limiting = s.species(&reaction.limiting)?;

                // Every panic in `unit::react` has a matching check here. The wrong-length one
                // does not: the vector is built above to the registry's length.
                require(
                    (0.0..=1.0).contains(&reaction.conversion),
                    s.at,
                    &field("conversion"),
                    reaction.conversion,
                    "between 0.0 and 1.0",
                )?;
                let nu_limiting = stoichiometry[limiting.as_usize()];
                require(
                    nu_limiting < 0.0,
                    s.at,
                    &field("limiting"),
                    nu_limiting,
                    "the coefficient of a reactant, which is negative",
                )?;
                unit::mass_closure(s.registry, &stoichiometry).map_err(|residual| {
                    LoadError::BadValue {
                        at: s.at.clone(),
                        field: field("stoichiometry"),
                        value: residual,
                        expected: format!(
                            "a mass balance within {} g/mol per unit of coefficient",
                            unit::MASS_CLOSURE_TOLERANCE
                        ),
                    }
                })?;
                unit::formation_enthalpies(s.registry, &stoichiometry).map_err(|species| {
                    // Named by its key, so the message matches what the document wrote.
                    let species = if s.registry.shares_name(&species.name) {
                        format!("{}{}", species.name, species.phase.suffix())
                    } else {
                        species.name.clone()
                    };
                    LoadError::MissingFormationEnthalpy {
                        at: s.at.clone(),
                        species,
                    }
                })?;

                domain.push(unit::Reaction {
                    stoichiometry,
                    limiting,
                    conversion: reaction.conversion,
                });
            }

            Ok(Box::new(unit::ConversionReactor {
                reactions: domain,
                energy,
                pressure_drop,
            }))
        });

        ops.register(unit::Heater::TAG, |s| {
            let HeaterSpec {
                duty,
                pressure_drop,
            } = s.parse(unit::Heater::TAG)?;
            s.pressure_drop(pressure_drop)?;
            Ok(Box::new(unit::Heater {
                duty,
                pressure_drop,
            }))
        });

        ops.register(unit::Pump::TAG, |s| {
            let PumpSpec {
                pressure_rise,
                efficiency,
            } = s.parse(unit::Pump::TAG)?;
            s.pressure_rise(pressure_rise)?;
            s.efficiency(efficiency)?;
            Ok(Box::new(unit::Pump {
                pressure_rise,
                efficiency,
            }))
        });

        ops.register(unit::Compressor::TAG, |s| {
            let PumpSpec {
                pressure_rise,
                efficiency,
            } = s.parse(unit::Compressor::TAG)?;
            s.pressure_rise(pressure_rise)?;
            s.efficiency(efficiency)?;
            Ok(Box::new(unit::Compressor {
                pressure_rise,
                efficiency,
            }))
        });

        ops.register(unit::Tank::TAG, |s| {
            s.parse::<NoSpec>(unit::Tank::TAG)?;
            Ok(Box::new(unit::Tank))
        });

        ops.register(unit::Product::TAG, |s| {
            s.parse::<NoSpec>(unit::Product::TAG)?;
            Ok(Box::new(unit::Product))
        });

        ops
    }

    /// Registers `constructor` under `tag`, replacing whatever was there.
    ///
    /// `tag` is `&'static str` to match [`ToDocument::tag`]; taking a `String` here would invite
    /// the two to drift.
    pub fn register<F>(&mut self, tag: &'static str, constructor: F)
    where
        F: Fn(Spec<'_>) -> Result<Box<dyn unit::UnitOp>, LoadError> + Send + Sync + 'static,
    {
        self.table.insert(tag, Box::new(constructor));
    }

    /// Every registered tag, in order. Useful for an error message that lists what *is* known.
    pub fn tags(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.table.keys().copied()
    }

    /// Builds the domain operation `op` names.
    ///
    /// # Errors
    ///
    /// [`LoadError::UnknownOp`] if the tag is not registered, or whatever the constructor
    /// returns.
    fn build(
        &self,
        op: &Op,
        registry: &SpeciesRegistry,
        at: &Location,
    ) -> Result<Box<dyn unit::UnitOp>, LoadError> {
        let constructor = self
            .table
            .get(op.tag.as_str())
            .ok_or_else(|| LoadError::UnknownOp {
                at: at.clone(),
                tag: op.tag.clone(),
            })?;

        constructor(Spec {
            params: &op.spec,
            registry,
            at,
        })
    }
}

impl TryFrom<Flowsheet> for flowsheet::Flowsheet {
    type Error = LoadError;

    /// Replays the document through the builder using [`OpRegistry::builtin`].
    ///
    /// Call [`Flowsheet::into_domain`] instead to load a document naming an operation this crate
    /// does not ship.
    ///
    /// # Errors
    ///
    /// Returns the first [`LoadError`] found.
    fn try_from(doc: Flowsheet) -> Result<Self, Self::Error> {
        doc.into_domain(&OpRegistry::builtin())
    }
}

impl Flowsheet {
    /// Replays the document through the [`crate::flowsheet::Flowsheet`] builder, so every
    /// invariant those methods maintain still holds.
    ///
    /// The result is unvalidated: call [`crate::flowsheet::Flowsheet::validate`] to check arity.
    ///
    /// # Errors
    ///
    /// Returns the first [`LoadError`] found.
    pub fn into_domain(self, ops: &OpRegistry) -> Result<flowsheet::Flowsheet, LoadError> {
        let doc = self;
        if doc.species.len() > MAX_IDS {
            return Err(LoadError::TooMany {
                what: "species",
                count: doc.species.len(),
            });
        }
        if doc.units.len() > MAX_IDS {
            return Err(LoadError::TooMany {
                what: "units",
                count: doc.units.len(),
            });
        }
        if doc.streams.len() > MAX_IDS {
            return Err(LoadError::TooMany {
                what: "streams",
                count: doc.streams.len(),
            });
        }

        let mut registry = SpeciesRegistry::default();
        for s in &doc.species {
            let at = Location::Species(s.name.clone());
            require(
                s.molar_mass.is_finite() && s.molar_mass > 0.0,
                &at,
                "molar_mass",
                s.molar_mass,
                "greater than 0.0 g/mol",
            )?;
            // Checked at one temperature only. That catches a typo, but a fit that turns
            // negative somewhere else in its range still loads.
            let cp = s.shomate.heat_capacity(REFERENCE_K);
            require(
                cp.is_finite() && cp > 0.0,
                &at,
                "shomate",
                cp,
                "a heat capacity at 298.15 K greater than 0.0 J/(mol·K)",
            )?;
            if let Some(antoine) = s.vapour_pressure {
                // `b` is the one coefficient with a sign the physics fixes: a negative one
                // would have vapour pressure fall with temperature. `a` and `c` can be either
                // sign, and JSON cannot write a non-finite one.
                require(
                    antoine.b > 0.0,
                    &at,
                    "vapour_pressure.b",
                    antoine.b,
                    "greater than 0.0 K",
                )?;
            }
            if let Some(density) = s.density {
                require(
                    density.is_finite() && density > 0.0,
                    &at,
                    "density",
                    density,
                    "greater than 0.0 kg/m³",
                )?;
            }
            if Phase::split_suffix(&s.name).is_some() {
                return Err(LoadError::ReservedSpeciesName {
                    name: s.name.clone(),
                });
            }
            // `insert` would hand back the first entry's id and silently drop this one.
            if registry.find(&s.name, s.phase).is_some() {
                return Err(LoadError::DuplicateSpecies {
                    name: s.name.clone(),
                    phase: s.phase,
                });
            }
            registry.insert(s.clone());
        }

        let mut fs = flowsheet::Flowsheet::new(registry);

        // Every unit is added before any stream, so a recycle's consumer already exists by the
        // time the stream that closes the loop is wired.
        let mut unit_ids: BTreeMap<&str, UnitId> = BTreeMap::new();
        for u in &doc.units {
            if unit_ids.contains_key(u.name.as_str()) {
                return Err(LoadError::DuplicateUnit {
                    name: u.name.clone(),
                });
            }
            let at = Location::Unit(u.name.clone());
            let op = ops.build(&u.op, fs.registry(), &at)?;
            unit_ids.insert(&u.name, fs.add_unit(u.name.clone(), op));
        }

        for (i, s) in doc.streams.iter().enumerate() {
            let at = Location::Stream(i);
            let from = *unit_ids
                .get(s.from.as_str())
                .ok_or_else(|| LoadError::UnknownUnit {
                    stream: i,
                    name: s.from.clone(),
                })?;
            let to = *unit_ids
                .get(s.to.as_str())
                .ok_or_else(|| LoadError::UnknownUnit {
                    stream: i,
                    name: s.to.clone(),
                })?;
            let stream = s.state.to_stream(fs.registry(), &at)?;
            fs.add_stream(from, stream, to);
        }

        Ok(fs)
    }
}

// ---------------------------------------------------------------------------
// Saving: domain -> serial
// ---------------------------------------------------------------------------

impl State {
    /// Captures a stream as a document state, keying flows by [`SpeciesRegistry::key`].
    ///
    /// Zero flows are omitted: they load back as zero anyway, and dropping them keeps a saved
    /// flowsheet readable when most streams carry only a few of the species.
    fn from_stream(s: &stream::Stream, registry: &SpeciesRegistry) -> Self {
        let flows = s
            .flows()
            .iter()
            .zip(registry.keys())
            .filter(|(f, _)| **f != 0.0)
            .map(|(&f, key)| (key, f))
            .collect();

        Self {
            flows,
            temperature: s.temperature(),
            pressure: s.pressure(),
        }
    }
}

/// Captures a domain unit operation as its document form: the `type` tag it writes, and the
/// parameters that sit beside it.
///
/// This is a supertrait of [`unit::UnitOp`] rather than a `match` inside this module, because
/// there is nothing left to match on: `Box<dyn unit::UnitOp>` has erased the concrete type, and
/// only the operation itself still knows what it is. The alternative - downcasting through
/// [`std::any::Any`] - would put a closed list of concrete types back in this file and give up
/// the open set the trait object was adopted for.
///
/// [`ToDocument::tag`] returns `&'static str` rather than a `String` because the same string is
/// the key the operation's constructor is registered under in [`OpRegistry`]. Saving and loading
/// have to agree on it, and there is one per type, not one per instance.
///
/// The `impl` blocks live here, next to the [`OpRegistry::builtin`] entries that load them back,
/// so the wire format stays one module. A downstream operation implements this trait and
/// registers a matching constructor; nothing in this file has to know about it.
pub trait ToDocument {
    /// What this operation writes as its `type`.
    fn tag(&self) -> &'static str;

    /// This operation's parameters, resolved against `registry`.
    ///
    /// Must serialise to a JSON object: the result is flattened beside `type`, so there is
    /// nowhere for a bare number or string to go. An operation with no parameters returns
    /// `serde_json::json!({})`, or serialises a [`NoSpec`].
    fn spec(&self, registry: &SpeciesRegistry) -> serde_json::Value;
}

/// Serialises an operation's spec struct. This cannot fail: every spec type here is plain data,
/// with string map keys and finite floats, so there is nothing `serde_json` would refuse.
fn spec_of<T: Serialize>(spec: &T) -> serde_json::Value {
    serde_json::to_value(spec).expect("a unit operation's spec always serialises")
}

impl unit::Feed {
    /// The `type` a feed writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "feed";
}

impl ToDocument for unit::Feed {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&FeedSpec {
            state: State::from_stream(&self.stream, registry),
        })
    }
}

impl unit::Mixer {
    /// The `type` a mixer writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "mixer";
}

impl ToDocument for unit::Mixer {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&MixerSpec {
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::Splitter {
    /// The `type` a splitter writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "splitter";
}

impl ToDocument for unit::Splitter {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&SplitterSpec {
            fraction: self.fraction,
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::SplitterN {
    /// The `type` an n-way splitter writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "splitter_n";
}

impl ToDocument for unit::SplitterN {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&SplitterNSpec {
            ratios: self.ratios.clone(),
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::Flotation {
    /// The `type` a flotation cell writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "flotation";
}

impl ToDocument for unit::Flotation {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    /// # Panics
    /// If `recovery` does not hold exactly one entry per species. `zip` would otherwise
    /// truncate to the shorter side and write a document that reloads as a *different* cell -
    /// silently, and with nothing for the reload to complain about.
    fn spec(&self, registry: &SpeciesRegistry) -> serde_json::Value {
        assert_eq!(
            self.recovery.len(),
            registry.len(),
            "flotation needs one recovery per species"
        );

        // Every species is written out, including the ones that recover nothing. A zero flow is
        // omitted because it says nothing - a zero recovery is a deliberate statement that the
        // species does not float, and it belongs on the page next to the ones that do.
        let recovery = self
            .recovery
            .iter()
            .zip(registry.keys())
            .map(|(&r, key)| (key, r))
            .collect();

        spec_of(&FlotationSpec {
            recovery,
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::ConversionReactor {
    /// The `type` a conversion reactor writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "conversion_reactor";
}

impl ToDocument for unit::ConversionReactor {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    /// # Panics
    /// If a reaction's `stoichiometry` does not hold exactly one coefficient per species, for the
    /// reason [`unit::Flotation`]'s `spec` gives: `zip` would truncate, and the document would
    /// reload as a different reaction.
    fn spec(&self, registry: &SpeciesRegistry) -> serde_json::Value {
        let reactions = self
            .reactions
            .iter()
            .map(|reaction| {
                assert_eq!(
                    reaction.stoichiometry.len(),
                    registry.len(),
                    "a reaction needs one stoichiometric coefficient per species"
                );

                // Zero coefficients are dropped, unlike zero recoveries: an equation lists only
                // the species that take part in it.
                let stoichiometry = reaction
                    .stoichiometry
                    .iter()
                    .zip(registry.keys())
                    .filter(|(nu, _)| **nu != 0.0)
                    .map(|(&nu, key)| (key, nu))
                    .collect();

                ReactionSpec {
                    stoichiometry,
                    limiting: registry.key(reaction.limiting),
                    conversion: reaction.conversion,
                }
            })
            .collect();

        spec_of(&ConversionReactorSpec {
            energy: self.energy,
            reactions,
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::Heater {
    /// The `type` a heater writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "heater";
}

impl ToDocument for unit::Heater {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&HeaterSpec {
            duty: self.duty,
            pressure_drop: self.pressure_drop,
        })
    }
}

impl unit::Pump {
    /// The `type` a pump writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "pump";
}

impl ToDocument for unit::Pump {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&PumpSpec {
            pressure_rise: self.pressure_rise,
            efficiency: self.efficiency,
        })
    }
}

impl unit::Compressor {
    /// The `type` a compressor writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "compressor";
}

impl ToDocument for unit::Compressor {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&PumpSpec {
            pressure_rise: self.pressure_rise,
            efficiency: self.efficiency,
        })
    }
}

impl unit::Tank {
    /// The `type` a tank writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "tank";
}

impl ToDocument for unit::Tank {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&NoSpec {})
    }
}

impl unit::Product {
    /// The `type` a product writes, and the [`OpRegistry`] key it loads back from.
    pub const TAG: &'static str = "product";
}

impl ToDocument for unit::Product {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        spec_of(&NoSpec {})
    }
}

impl From<&flowsheet::ValidFlowsheet> for Flowsheet {
    /// Captures a flowsheet as a document, including whatever the solver last wrote into its
    /// streams.
    ///
    /// This takes a [`crate::flowsheet::ValidFlowsheet`] rather than a
    /// [`crate::flowsheet::Flowsheet`] because only validation rules out the two kinds of name a
    /// document cannot survive: two units sharing a name, which a stream endpoint could not tell
    /// apart, and a species name ending in a phase suffix, whose key would read back as a
    /// different species.
    fn from(fs: &flowsheet::ValidFlowsheet) -> Self {
        let registry = fs.registry();

        let units = fs
            .units()
            .iter()
            .map(|u| Unit {
                name: u.name.clone(),
                op: Op::new(u.op.tag(), u.op.spec(registry)),
            })
            .collect();

        // A stream's endpoints are whichever units list it, which is exactly the pairing
        // `add_stream` recorded. Walking units rather than streams keeps that the only source.
        let mut from: Vec<Option<&str>> = vec![None; fs.streams().len()];
        let mut to: Vec<Option<&str>> = vec![None; fs.streams().len()];
        for u in fs.units() {
            for &s in &u.outlets {
                from[s.as_usize()] = Some(&u.name);
            }
            for &s in &u.inlets {
                to[s.as_usize()] = Some(&u.name);
            }
        }

        let streams = fs
            .streams()
            .iter()
            .enumerate()
            .map(|(i, s)| Stream {
                from: from[i].expect("every stream leaves a unit").to_string(),
                to: to[i].expect("every stream enters a unit").to_string(),
                state: State::from_stream(s, registry),
            })
            .collect();

        Self {
            species: registry.all().to_vec(),
            units,
            streams,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowsheet::Flowsheet as DomainFlowsheet;

    /// A recycle circuit as a document: feed -> mixer -> tank -> splitter, with the splitter's
    /// first outlet recycling to the mixer and its second going to product.
    ///
    /// Its own circuit, not `demo::build_flowsheet` - that one runs a flotation cell into two
    /// products, and the fixture tracking it byte-for-byte lives in `flowsheet-cli`.
    fn recycle_json() -> &'static str {
        r#"{
          "species": [
            { "name": "CuFeS2", "phase": "Solid",  "molar_mass": 183.5,  "shomate": { "a": 95.0 } },
            { "name": "SiO2",   "phase": "Solid",  "molar_mass": 60.08,  "shomate": { "a": 44.6 } },
            { "name": "H2O",    "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } }
          ],
          "units": [
            { "name": "feed", "op": { "type": "feed",
              "state": { "flows": { "CuFeS2": 40.0, "SiO2": 360.0, "H2O": 600.0 } } } },
            { "name": "mixer", "op": { "type": "mixer" } },
            { "name": "tank", "op": { "type": "tank" } },
            { "name": "splitter", "op": { "type": "splitter", "fraction": 0.3 } },
            { "name": "product", "op": { "type": "product" } }
          ],
          "streams": [
            { "from": "feed",     "to": "mixer" },
            { "from": "mixer",    "to": "tank" },
            { "from": "tank",     "to": "splitter" },
            { "from": "splitter", "to": "mixer" },
            { "from": "splitter", "to": "product" }
          ]
        }"#
    }

    fn doc(json: &str) -> Flowsheet {
        serde_json::from_str(json).expect("test document should parse")
    }

    fn load(json: &str) -> Result<DomainFlowsheet, LoadError> {
        DomainFlowsheet::try_from(doc(json))
    }

    /// Reads a saved unit's parameters back into its spec type, checking the tag on the way.
    ///
    /// An [`Op`] holds its parameters as an uninterpreted map, so a test that wants to see
    /// inside one has to parse it.
    fn spec<T: serde::de::DeserializeOwned>(doc: &Flowsheet, unit: usize, tag: &str) -> T {
        let op = &doc.units[unit].op;
        assert_eq!(op.tag, tag, "units[{unit}] should be a {tag}");
        serde_json::from_value(serde_json::Value::Object(op.spec.clone()))
            .expect("a saved spec parses back")
    }

    /// A minimal document with one species and one unit, for poking at individual fields.
    fn minimal(units: &str, streams: &str) -> String {
        format!(
            r#"{{ "species": [{{ "name": "H2O", "phase": "Liquid", "molar_mass": 18.015,
                   "shomate": {{ "a": 75.3 }} }}],
                  "units": [{units}], "streams": [{streams}] }}"#
        )
    }

    #[test]
    fn the_recycle_document_loads_and_validates() {
        let fs = load(recycle_json()).expect("document should load");
        assert_eq!(fs.registry().len(), 3);
        fs.validate()
            .expect("document should be structurally valid");
    }

    #[test]
    fn an_omitted_species_is_zero_not_an_error() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": {} } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        let fs = load(&json).expect("an empty flows map is legal");
        fs.validate().expect("feed -> product is valid");
    }

    #[test]
    fn an_unknown_stream_endpoint_names_the_stream_and_the_unit() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": {} } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "typo" }"#,
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e,
            LoadError::UnknownUnit {
                stream: 0,
                name: "typo".into()
            }
        );
        assert_eq!(
            e.to_string(),
            "streams[0] names unit `typo`, which is not declared"
        );
    }

    #[test]
    fn an_unknown_species_in_a_flows_map_names_where_it_appeared() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": { "CO2": 1.0 } } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e,
            LoadError::UnknownSpecies {
                at: Location::Unit("f".into()),
                name: "CO2".into()
            }
        );
        assert_eq!(
            e.to_string(),
            "unit `f` names species `CO2`, which is not declared"
        );
    }

    #[test]
    fn a_repeated_unit_name_is_rejected() {
        let json = minimal(
            r#"{ "name": "m", "op": { "type": "mixer" } },
               { "name": "m", "op": { "type": "tank" } }"#,
            "",
        );
        assert_eq!(
            load(&json).unwrap_err(),
            LoadError::DuplicateUnit { name: "m".into() }
        );
    }

    /// A stray field parses, then fails when the registry hands the parameters to the op's own
    /// spec type - so the error names the field but is a [`LoadError`], not a parse error.
    #[test]
    fn a_field_that_belongs_to_another_unit_op_is_rejected() {
        // `fraction` is a splitter's field, not a mixer's.
        let json = minimal(
            r#"{ "name": "m", "op": { "type": "mixer", "fraction": 0.9 } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert!(
            matches!(e, LoadError::BadOp { ref tag, .. } if tag == "mixer"),
            "{e:?}"
        );
        assert!(e.to_string().contains("fraction"), "{e}");
    }

    #[test]
    fn a_stray_field_on_a_parameterised_op_is_rejected() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter", "fraction": 0.3, "junk": 1 } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert!(e.to_string().contains("junk"), "{e}");
    }

    #[test]
    fn an_unregistered_op_names_the_tag_and_the_unit() {
        let json = minimal(r#"{ "name": "x", "op": { "type": "screen" } }"#, "");
        let e = load(&json).unwrap_err();
        assert_eq!(
            e,
            LoadError::UnknownOp {
                at: Location::Unit("x".into()),
                tag: "screen".into()
            }
        );
        assert_eq!(
            e.to_string(),
            "unit `x`: `screen` is not a registered unit operation"
        );
    }

    #[test]
    fn a_missing_required_parameter_is_rejected() {
        let json = minimal(r#"{ "name": "s", "op": { "type": "splitter" } }"#, "");
        let e = load(&json).unwrap_err();
        assert!(e.to_string().contains("fraction"), "{e}");
    }

    #[test]
    fn the_builtin_registry_holds_every_shipped_op() {
        let tags: Vec<_> = OpRegistry::builtin().tags().collect();
        assert_eq!(
            tags,
            [
                "compressor",
                "conversion_reactor",
                "feed",
                "flotation",
                "heater",
                "mixer",
                "product",
                "pump",
                "splitter",
                "splitter_n",
                "tank"
            ]
        );
    }

    #[test]
    fn a_misspelt_unit_field_is_rejected() {
        let json = minimal(
            r#"{ "name": "t", "op": { "type": "tank" }, "nmae": "t" }"#,
            "",
        );
        let e = serde_json::from_str::<Flowsheet>(&json).unwrap_err();
        assert!(e.to_string().contains("nmae"), "{e}");
    }

    #[test]
    fn a_misspelt_species_field_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 }, "mm": 18.0 }
          ], "units": [], "streams": [] }"#;
        let e = serde_json::from_str::<Flowsheet>(json).unwrap_err();
        assert!(e.to_string().contains("mm"), "{e}");
    }

    #[test]
    fn an_enthalpy_of_formation_survives_a_round_trip_and_is_left_off_when_absent() {
        let json = r#"{"species":[{"name":"H2O","phase":"Liquid","molar_mass":18.015,"shomate":{"a":75.3},"enthalpy_of_formation":-285.83},{"name":"SiO2","phase":"Solid","molar_mass":60.08,"shomate":{"a":44.6}}],"units":[],"streams":[]}"#;

        let fs = load(json).unwrap().validate().unwrap();
        assert_eq!(fs.registry().all()[0].enthalpy_of_formation, Some(-285.83));
        assert_eq!(fs.registry().all()[1].enthalpy_of_formation, None);
        assert_eq!(serde_json::to_string(&Flowsheet::from(&fs)).unwrap(), json);
    }

    /// Water in two phases plus a single-phase solid, a feed naming `flows` and a flotation cell
    /// naming `recovery`, so both kinds of species-keyed map are exercised.
    fn two_phase_json(feed_flows: &str) -> String {
        format!(
            r#"{{ "species": [
                {{ "name": "H2O",  "phase": "Liquid", "molar_mass": 18.015, "shomate": {{ "a": 75.3 }} }},
                {{ "name": "H2O",  "phase": "Gas",    "molar_mass": 18.015, "shomate": {{ "a": 33.6 }} }},
                {{ "name": "SiO2", "phase": "Solid",  "molar_mass": 60.08,  "shomate": {{ "a": 44.6 }} }}
              ],
              "units": [
                {{ "name": "f", "op": {{ "type": "feed", "state": {{ "flows": {{ {feed_flows} }} }} }} }},
                {{ "name": "c", "op": {{ "type": "flotation",
                   "recovery": {{ "H2O(g)": 0.9, "H2O(l)": 0.1, "SiO2": 0.05 }} }} }},
                {{ "name": "conc", "op": {{ "type": "product" }} }},
                {{ "name": "tail", "op": {{ "type": "product" }} }}
              ],
              "streams": [
                {{ "from": "f", "to": "c" }},
                {{ "from": "c", "to": "conc" }},
                {{ "from": "c", "to": "tail" }}
              ] }}"#
        )
    }

    #[test]
    fn a_name_shared_across_phases_is_keyed_with_its_phase_and_survives_a_round_trip() {
        let json = two_phase_json(r#""H2O(l)": 600.0, "H2O(g)": 5.0, "SiO2": 400.0"#);
        let mut fs = load(&json).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();

        let saved = Flowsheet::from(&fs);
        let feed: FeedSpec = spec(&saved, 0, "feed");
        let keys: Vec<&str> = feed.state.flows.keys().map(String::as_str).collect();
        assert_eq!(keys, ["H2O(g)", "H2O(l)", "SiO2"]);
        let cell: FlotationSpec = spec(&saved, 1, "flotation");
        assert_eq!(cell.recovery["H2O(g)"], 0.9);
        assert_eq!(cell.recovery["H2O(l)"], 0.1);

        let reloaded = DomainFlowsheet::try_from(saved).unwrap();
        for (a, b) in fs.streams().iter().zip(reloaded.streams()) {
            assert_eq!(a.flows(), b.flows());
        }
        // The concentrate carries 90% of the steam and 10% of the liquid, so the two phases
        // really did land in different columns.
        let concentrate = reloaded.streams()[1].flows();
        for (&got, want) in concentrate.iter().zip([60.0, 4.5, 20.0]) {
            approx::assert_relative_eq!(got, want, max_relative = 1e-12);
        }
    }

    #[test]
    fn a_bare_name_two_phases_share_is_ambiguous() {
        let e = load(&two_phase_json(r#""H2O": 600.0"#)).unwrap_err();
        assert_eq!(
            e,
            LoadError::AmbiguousSpecies {
                at: Location::Unit("f".into()),
                name: "H2O".into(),
                keys: vec!["H2O(l)".into(), "H2O(g)".into()],
            }
        );
        assert_eq!(
            e.to_string(),
            "unit `f` names species `H2O`, which more than one phase shares - write one of \
             `H2O(l)`, `H2O(g)`"
        );
    }

    #[test]
    fn a_flow_named_both_bare_and_suffixed_is_rejected() {
        // Both keys resolve to the one liquid water, so one value would otherwise vanish.
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed",
                 "state": { "flows": { "H2O": 600.0, "H2O(l)": 5.0 } } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e,
            LoadError::DuplicateSpeciesKey {
                at: Location::Unit("f".into()),
                first: "H2O".into(),
                second: "H2O(l)".into(),
            }
        );
        assert_eq!(
            e.to_string(),
            "unit `f` names one species twice, as `H2O` and `H2O(l)` - write it once"
        );
    }

    #[test]
    fn a_recovery_named_both_bare_and_suffixed_is_rejected() {
        let json = two_phase_json(r#""H2O(l)": 600.0"#)
            .replace(r#""SiO2": 0.05"#, r#""SiO2": 0.05, "SiO2(s)": 0.5"#);
        assert!(matches!(
            load(&json).unwrap_err(),
            LoadError::DuplicateSpeciesKey { first, second, .. }
                if first == "SiO2" && second == "SiO2(s)"
        ));
    }

    #[test]
    fn a_suffixed_key_is_accepted_where_the_bare_name_would_do() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": { "H2O(l)": 1.0 } } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        let mut fs = load(&json).unwrap().validate().unwrap();
        // A feed writes its stream when it runs, not when it loads.
        crate::solver::Solver::default().solve(&mut fs).unwrap();
        assert_eq!(fs.streams()[0].flows(), &[1.0]);
    }

    #[test]
    fn a_name_repeated_in_the_same_phase_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } },
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } }
          ], "units": [], "streams": [] }"#;
        assert_eq!(
            load(json).unwrap_err(),
            LoadError::DuplicateSpecies {
                name: "H2O".into(),
                phase: Phase::Liquid
            }
        );
    }

    #[test]
    fn a_species_name_ending_in_a_phase_suffix_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O(g)", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } }
          ], "units": [], "streams": [] }"#;
        let e = load(json).unwrap_err();
        assert_eq!(
            e,
            LoadError::ReservedSpeciesName {
                name: "H2O(g)".into()
            }
        );
        assert!(
            e.to_string()
                .starts_with("species `H2O(g)` ends in a phase suffix"),
            "{e}"
        );
    }

    #[test]
    fn a_split_fraction_above_one_is_an_error_not_a_panic() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter", "fraction": 1.5 } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unit `s`: `fraction` is 1.5, expected between 0.0 and 1.0"
        );
    }

    #[test]
    fn split_ratios_that_sum_to_zero_are_an_error_not_a_panic() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter_n", "ratios": [0.0, 0.0] } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unit `s`: `ratios` is 0, expected a sum greater than 0.0"
        );
    }

    #[test]
    fn empty_split_ratios_are_caught_by_the_same_sum_check() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter_n", "ratios": [] } }"#,
            "",
        );
        assert!(matches!(
            load(&json).unwrap_err(),
            LoadError::BadValue { ref field, .. } if field == "ratios"
        ));
    }

    #[test]
    fn a_negative_flow_names_the_species_it_came_from() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": { "H2O": -1.0 } } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unit `f`: `H2O` is -1, expected 0.0 t/h or greater"
        );
    }

    #[test]
    fn absolute_zero_is_rejected() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "temperature": 0.0 } } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p" }"#,
        );
        assert!(matches!(
            load(&json).unwrap_err(),
            LoadError::BadValue { ref field, .. } if field == "temperature"
        ));
    }

    #[test]
    fn a_stream_state_error_is_located_by_stream_index() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": {} } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "p", "state": { "pressure": -1.0 } }"#,
        );
        assert!(matches!(
            load(&json).unwrap_err(),
            LoadError::BadValue {
                at: Location::Stream(0),
                ..
            }
        ));
    }

    #[test]
    fn a_non_positive_molar_mass_is_rejected() {
        let json = r#"{ "species": [{ "name": "X", "phase": "Gas", "molar_mass": 0.0,
                                      "shomate": { "a": 29.1 } }],
                        "units": [], "streams": [] }"#;
        assert!(matches!(
            load(json).unwrap_err(),
            LoadError::BadValue {
                at: Location::Species(_),
                ..
            }
        ));
    }

    #[test]
    fn a_species_without_heat_capacity_data_is_rejected() {
        let json = r#"{ "species": [{ "name": "H2O", "phase": "Liquid", "molar_mass": 18.015 }],
                        "units": [], "streams": [] }"#;
        let e = serde_json::from_str::<Flowsheet>(json).unwrap_err();
        assert!(e.to_string().contains("missing field `shomate`"), "{e}");
    }

    #[test]
    fn a_non_positive_heat_capacity_is_rejected() {
        let json = r#"{ "species": [{ "name": "X", "phase": "Gas", "molar_mass": 28.0,
                                      "shomate": { "a": 0.0 } }],
                        "units": [], "streams": [] }"#;
        assert_eq!(
            load(json).unwrap_err().to_string(),
            "species `X`: `shomate` is 0, expected a heat capacity at 298.15 K greater than 0.0 \
             J/(mol·K)"
        );
    }

    #[test]
    fn saving_drops_only_the_species_whose_flow_is_zero() {
        let json = r#"{ "species": [
            { "name": "H2O",  "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } },
            { "name": "SiO2", "phase": "Solid",  "molar_mass": 60.08,  "shomate": { "a": 44.6 } }
          ],
          "units": [
            { "name": "f", "op": { "type": "feed", "state": { "flows": { "SiO2": 5.0 } } } },
            { "name": "p", "op": { "type": "product" } }
          ],
          "streams": [{ "from": "f", "to": "p" }] }"#;
        let fs = load(json).unwrap().validate().unwrap();
        let doc = Flowsheet::from(&fs);

        let feed: FeedSpec = spec(&doc, 0, unit::Feed::TAG);
        let state = &feed.state;
        assert_eq!(state.flows.len(), 1, "the zero H2O should be dropped");
        assert!(state.flows.contains_key("SiO2"));
    }

    #[test]
    fn a_recovery_above_one_is_an_error_not_a_panic() {
        let json = minimal(
            r#"{ "name": "c", "op": { "type": "flotation", "recovery": { "H2O": 1.5 } } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unit `c`: `H2O` is 1.5, expected between 0.0 and 1.0"
        );
    }

    #[test]
    fn an_unknown_species_in_a_recovery_map_names_where_it_appeared() {
        let json = minimal(
            r#"{ "name": "c", "op": { "type": "flotation", "recovery": { "CO2": 0.5 } } }"#,
            "",
        );
        assert_eq!(
            load(&json).unwrap_err(),
            LoadError::UnknownSpecies {
                at: Location::Unit("c".into()),
                name: "CO2".into()
            }
        );
    }

    #[test]
    fn a_species_left_out_of_a_recovery_map_recovers_nothing() {
        let json = r#"{ "species": [
            { "name": "H2O",  "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } },
            { "name": "SiO2", "phase": "Solid",  "molar_mass": 60.08,  "shomate": { "a": 44.6 } }
          ],
          "units": [
            { "name": "f", "op": { "type": "feed",
              "state": { "flows": { "H2O": 10.0, "SiO2": 5.0 } } } },
            { "name": "c", "op": { "type": "flotation", "recovery": { "SiO2": 0.4 } } },
            { "name": "conc", "op": { "type": "product" } },
            { "name": "tails", "op": { "type": "product" } }
          ],
          "streams": [
            { "from": "f", "to": "c" },
            { "from": "c", "to": "conc" },
            { "from": "c", "to": "tails" }
          ] }"#;
        let mut fs = load(json).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();

        // Saving writes the omitted species back explicitly, as a zero.
        let doc = Flowsheet::from(&fs);
        let cell: FlotationSpec = spec(&doc, 1, unit::Flotation::TAG);
        assert_eq!(cell.recovery.len(), 2, "a zero recovery is not dropped");
        assert_eq!(cell.recovery["H2O"], 0.0);
        assert_eq!(cell.recovery["SiO2"], 0.4);
    }

    #[test]
    #[should_panic(expected = "one recovery per species")]
    fn saving_a_flotation_cell_with_the_wrong_recovery_length_panics() {
        // `check` does not inspect `recovery`, so this flowsheet validates. Without the
        // assert, `zip` would quietly write a one-species map that reloads as a different
        // cell - the one corruption the wire format cannot detect on the way back in.
        // Two identical registries: `Flowsheet::new` takes ownership and `SpeciesRegistry`
        // is deliberately not `Clone`, so the streams are built against a second copy.
        let r = crate::demo::registry();
        let mut fs = DomainFlowsheet::new(crate::demo::registry());
        let f = fs.add_unit(
            "f",
            unit::Feed {
                stream: crate::demo::feed_stream(&r),
            },
        );
        let c = fs.add_unit(
            "cell",
            unit::Flotation {
                recovery: vec![0.85],
                pressure_drop: 0.0,
            },
        );
        let conc = fs.add_unit("conc", unit::Product);
        let tails = fs.add_unit("tails", unit::Product);
        let blank = crate::stream::Stream::zeros(&r, 298.15, 101.325);
        fs.add_stream(f, blank.clone(), c);
        fs.add_stream(c, blank.clone(), conc);
        fs.add_stream(c, blank, tails);

        let fs = fs
            .validate()
            .expect("a short recovery is not a validation error");
        let _ = Flowsheet::from(&fs);
    }

    #[test]
    fn a_heater_duty_survives_load_solve_save() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": { "H2O": 10.0 } } } },
               { "name": "cooler", "op": { "type": "heater", "duty": -500.0 } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "cooler" }, { "from": "cooler", "to": "p" }"#,
        );
        let mut fs = load(&json).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();

        assert!(
            fs.streams()[1].temperature() < AMBIENT_K,
            "a negative duty should cool"
        );
        let doc = Flowsheet::from(&fs);
        let heater: HeaterSpec = spec(&doc, 1, unit::Heater::TAG);
        assert_eq!(heater.duty, -500.0);
    }

    #[test]
    fn a_pressure_drop_survives_a_round_trip_and_a_zero_one_is_left_off() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "flows": { "H2O": 10.0 }, "pressure": 300.0 } } },
               { "name": "m", "op": { "type": "mixer", "pressure_drop": 20.0 } },
               { "name": "h", "op": { "type": "heater", "duty": 0.0 } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "m" }, { "from": "m", "to": "h" }, { "from": "h", "to": "p" }"#,
        );
        let mut fs = load(&json).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();
        assert_eq!(fs.streams()[1].pressure(), 280.0);
        assert_eq!(fs.streams()[2].pressure(), 280.0);

        let doc = Flowsheet::from(&fs);
        assert_eq!(doc.units[1].op.spec["pressure_drop"], 20.0);
        assert!(
            !doc.units[2].op.spec.contains_key("pressure_drop"),
            "a zero drop is the default and should not be written: {:?}",
            doc.units[2].op.spec
        );
        // The mixer's saved form is exactly the hand-written one.
        assert_eq!(
            serde_json::to_string(&doc.units[1].op).unwrap(),
            r#"{"type":"mixer","pressure_drop":20.0}"#
        );
    }

    #[test]
    fn a_negative_pressure_drop_is_rejected() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter", "fraction": 0.3, "pressure_drop": -5.0 } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert_eq!(
            e,
            LoadError::BadValue {
                at: Location::Unit("s".into()),
                field: "pressure_drop".into(),
                value: -5.0,
                expected: "0.0 kPa or greater".into(),
            }
        );
    }

    #[test]
    fn a_zero_stream_pressure_is_rejected() {
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": { "pressure": 0.0 } } }"#,
            "",
        );
        let e = load(&json).unwrap_err();
        assert!(
            e.to_string()
                .ends_with("`pressure` is 0, expected greater than 0.0 kPa"),
            "{e}"
        );
    }

    /// A pumped loop: feed -> mixer -> splitter, the first outlet back through a pump to the
    /// mixer and the second to product. `pump` is the body of the pump's op object after its
    /// `type`.
    fn pumped_json(pump: &str) -> String {
        format!(
            r#"{{ "species": [{{ "name": "H2O", "phase": "Liquid", "molar_mass": 18.015,
                   "shomate": {{ "a": 75.3 }}, "density": 997.0 }}],
                  "units": [
                    {{ "name": "f", "op": {{ "type": "feed",
                       "state": {{ "flows": {{ "H2O": 100.0 }}, "pressure": 300.0 }} }} }},
                    {{ "name": "m", "op": {{ "type": "mixer", "pressure_drop": 20.0 }} }},
                    {{ "name": "s", "op": {{ "type": "splitter", "fraction": 0.5 }} }},
                    {{ "name": "pump", "op": {{ "type": "pump", {pump} }} }},
                    {{ "name": "p", "op": {{ "type": "product" }} }}
                  ],
                  "streams": [
                    {{ "from": "f", "to": "m" }}, {{ "from": "m", "to": "s" }},
                    {{ "from": "s", "to": "pump" }}, {{ "from": "pump", "to": "m" }},
                    {{ "from": "s", "to": "p" }}
                  ] }}"#
        )
    }

    #[test]
    fn a_pump_survives_load_solve_save_and_a_unit_efficiency_is_left_off() {
        let json = pumped_json(r#""pressure_rise": 20.0, "efficiency": 0.7"#);
        let mut fs = load(&json).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();
        // The pump puts back what the mixer takes, so the loop holds the feed pressure.
        assert_eq!(fs.streams()[3].pressure(), 300.0);
        assert!(
            fs.streams()[3].temperature() > AMBIENT_K,
            "an inefficient pump should warm the recycle"
        );

        let doc = Flowsheet::from(&fs);
        let pump: PumpSpec = spec(&doc, 3, unit::Pump::TAG);
        assert_eq!(pump.pressure_rise, 20.0);
        assert_eq!(pump.efficiency, 0.7);
        assert_eq!(doc.species[0].density, Some(997.0));

        // And the ideal pump writes only its rise, the way an ideal mixer writes nothing.
        let json = pumped_json(r#""pressure_rise": 20.0"#);
        let fs = load(&json).unwrap().validate().unwrap();
        let doc = Flowsheet::from(&fs);
        assert_eq!(
            serde_json::to_string(&doc.units[3].op).unwrap(),
            r#"{"type":"pump","pressure_rise":20.0}"#
        );
    }

    #[test]
    fn a_compressor_reads_the_same_two_fields() {
        // Wired but not solved: the one species is liquid water, which a compressor refuses,
        // and this is about the document and not the numbers.
        let json = minimal(
            r#"{ "name": "f", "op": { "type": "feed", "state": {} } },
               { "name": "c", "op": { "type": "compressor", "pressure_rise": 200.0, "efficiency": 0.75 } },
               { "name": "p", "op": { "type": "product" } }"#,
            r#"{ "from": "f", "to": "c" }, { "from": "c", "to": "p" }"#,
        );
        let fs = load(&json).unwrap().validate().unwrap();
        let doc = Flowsheet::from(&fs);
        let c: PumpSpec = spec(&doc, 1, unit::Compressor::TAG);
        assert_eq!((c.pressure_rise, c.efficiency), (200.0, 0.75));
    }

    #[test]
    fn a_negative_pressure_rise_is_rejected() {
        let json = minimal(
            r#"{ "name": "c", "op": { "type": "compressor", "pressure_rise": -5.0 } }"#,
            "",
        );
        assert_eq!(
            load(&json).unwrap_err(),
            LoadError::BadValue {
                at: Location::Unit("c".into()),
                field: "pressure_rise".into(),
                value: -5.0,
                expected: "0.0 kPa or greater".into(),
            }
        );
    }

    #[test]
    fn an_efficiency_outside_the_unit_interval_is_rejected() {
        for efficiency in [0.0, -0.5, 1.5] {
            let json = minimal(
                &format!(
                    r#"{{ "name": "pump", "op": {{ "type": "pump", "pressure_rise": 5.0, "efficiency": {efficiency} }} }}"#
                ),
                "",
            );
            assert_eq!(
                load(&json).unwrap_err(),
                LoadError::BadValue {
                    at: Location::Unit("pump".into()),
                    field: "efficiency".into(),
                    value: efficiency,
                    expected: "greater than 0.0 and at most 1.0".into(),
                },
                "{efficiency}"
            );
        }
    }

    #[test]
    fn a_vapour_pressure_survives_a_round_trip_and_is_left_off_when_absent() {
        let json = r#"{"species":[{"name":"H2O","phase":"Liquid","molar_mass":18.015,"shomate":{"a":75.3},"vapour_pressure":{"a":7.08354,"b":1663.125,"c":-45.622}},{"name":"H2O","phase":"Gas","molar_mass":18.015,"shomate":{"a":33.6}}],"units":[],"streams":[]}"#;

        let fs = load(json).unwrap().validate().unwrap();
        let antoine = fs.registry().all()[0]
            .vapour_pressure
            .expect("the liquid carries the fit");
        assert_eq!(
            (antoine.a, antoine.b, antoine.c),
            (7.08354, 1663.125, -45.622)
        );
        assert!(fs.registry().all()[1].vapour_pressure.is_none());
        assert_eq!(serde_json::to_string(&Flowsheet::from(&fs)).unwrap(), json);
    }

    #[test]
    fn a_vapour_pressure_that_falls_with_temperature_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 },
              "vapour_pressure": { "a": 7.08354, "b": -1663.125, "c": -45.622 } }
          ], "units": [], "streams": [] }"#;
        assert_eq!(
            load(json).unwrap_err(),
            LoadError::BadValue {
                at: Location::Species("H2O".into()),
                field: "vapour_pressure.b".into(),
                value: -1663.125,
                expected: "greater than 0.0 K".into(),
            }
        );
    }

    #[test]
    fn a_misspelt_antoine_coefficient_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 },
              "vapour_pressure": { "a": 7.08354, "b": 1663.125, "C": -45.622 } }
          ], "units": [], "streams": [] }"#;
        let e = serde_json::from_str::<Flowsheet>(json).unwrap_err();
        assert!(e.to_string().contains("unknown field `C`"), "{e}");
    }

    #[test]
    fn a_non_positive_density_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 }, "density": 0.0 }
          ], "units": [], "streams": [] }"#;
        assert_eq!(
            load(json).unwrap_err(),
            LoadError::BadValue {
                at: Location::Species("H2O".into()),
                field: "density".into(),
                value: 0.0,
                expected: "greater than 0.0 kg/m³".into(),
            }
        );
    }

    #[test]
    fn a_pump_on_a_species_without_a_density_fails_the_solve_not_the_load() {
        // Which species reach a pump is a property of the flows, so this cannot be a
        // `LoadError`; it is the solver that reports it, naming the pump.
        let json = pumped_json(r#""pressure_rise": 20.0"#).replace(r#", "density": 997.0"#, "");
        let mut fs = load(&json).unwrap().validate().unwrap();
        let e = crate::solver::Solver::default()
            .solve(&mut fs)
            .expect_err("water without a density cannot be pumped");
        assert!(
            e.to_string().starts_with(
                "unit 'pump' failed on pass 1: species `H2O` flows through the pump but has no `density`"
            ),
            "{e}"
        );
    }

    /// Why `to_domain` has no `require` for a heater: JSON cannot express the only duty the domain
    /// rejects.
    #[test]
    fn a_non_finite_duty_cannot_be_written_in_json() {
        for duty in ["NaN", "Infinity", "1e400"] {
            let json = minimal(
                &format!(r#"{{ "name": "h", "op": {{ "type": "heater", "duty": {duty} }} }}"#),
                "",
            );
            assert!(
                serde_json::from_str::<Flowsheet>(&json).is_err(),
                "{duty} should not parse"
            );
        }
    }

    /// Methane burning in oxygen, with nitrogen along for the ride: feed -> reactor -> product.
    /// `reaction` is the body of the reactor's one reaction object.
    fn combustion_json(reaction: &str) -> String {
        combustion_json_with(&format!("{{ {reaction} }}"))
    }

    /// [`combustion_json`] with `reactions` as the whole body of the reactor's `reactions` array.
    fn combustion_json_with(reactions: &str) -> String {
        format!(
            r#"{{ "species": [
                {{ "name": "CH4", "phase": "Gas", "molar_mass": 16.043, "shomate": {{ "a": 35.7 }}, "enthalpy_of_formation": -74.87 }},
                {{ "name": "O2",  "phase": "Gas", "molar_mass": 31.998, "shomate": {{ "a": 29.4 }}, "enthalpy_of_formation": 0.0 }},
                {{ "name": "CO2", "phase": "Gas", "molar_mass": 44.009, "shomate": {{ "a": 37.1 }}, "enthalpy_of_formation": -393.52 }},
                {{ "name": "H2O", "phase": "Gas", "molar_mass": 18.015, "shomate": {{ "a": 33.6 }}, "enthalpy_of_formation": -241.83 }},
                {{ "name": "N2",  "phase": "Gas", "molar_mass": 28.014, "shomate": {{ "a": 29.1 }} }}
              ],
              "units": [
                {{ "name": "f", "op": {{ "type": "feed",
                  "state": {{ "flows": {{ "CH4": 10.0, "O2": 60.0, "N2": 200.0 }} }} }} }},
                {{ "name": "r", "op": {{ "type": "conversion_reactor", "energy": "adiabatic", "reactions": [{reactions}] }} }},
                {{ "name": "p", "op": {{ "type": "product" }} }}
              ],
              "streams": [{{ "from": "f", "to": "r" }}, {{ "from": "r", "to": "p" }}] }}"#
        )
    }

    const BURN: &str = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 0.9"#;

    #[test]
    fn several_reactions_survive_a_round_trip_in_order() {
        // Half the methane, then all of what is left: the same equation twice, told apart only by
        // conversion, so a save that reordered or merged them would show.
        let reactions = r#"
            { "stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 }, "limiting": "CH4", "conversion": 0.5 },
            { "stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 }, "limiting": "CH4", "conversion": 1.0 }"#;
        let fs = load(&combustion_json_with(reactions))
            .unwrap()
            .validate()
            .unwrap();
        let saved = Flowsheet::from(&fs);

        let reactor: ConversionReactorSpec = spec(&saved, 1, unit::ConversionReactor::TAG);
        let conversions: Vec<f64> = reactor.reactions.iter().map(|r| r.conversion).collect();
        assert_eq!(conversions, [0.5, 1.0]);

        let json = serde_json::to_string(&saved).unwrap();
        let reloaded = DomainFlowsheet::try_from(doc(&json))
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(
            serde_json::to_string(&Flowsheet::from(&reloaded)).unwrap(),
            json
        );
    }

    #[test]
    fn a_reactor_with_no_reactions_is_an_error_not_a_panic() {
        assert_eq!(
            load(&combustion_json_with("")).unwrap_err().to_string(),
            "unit `r`: `reactions` is 0, expected at least one reaction"
        );
    }

    #[test]
    fn a_bad_value_in_a_later_reaction_names_that_reaction() {
        let reactions = r#"
            { "stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 }, "limiting": "CH4", "conversion": 0.5 },
            { "stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 }, "limiting": "CH4", "conversion": 2.0 }"#;
        assert_eq!(
            load(&combustion_json_with(reactions))
                .unwrap_err()
                .to_string(),
            "unit `r`: `reactions[1].conversion` is 2, expected between 0.0 and 1.0"
        );
    }

    #[test]
    fn a_stoichiometry_naming_a_species_twice_is_rejected() {
        // `H2O(g)` and `H2O` are the one steam; keeping either coefficient would be a different
        // equation from the one written.
        let json = combustion_json(
            r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2, "H2O(g)": 1 },
               "limiting": "CH4", "conversion": 0.9"#,
        );
        assert_eq!(
            load(&json).unwrap_err(),
            LoadError::DuplicateSpeciesKey {
                at: Location::Unit("r".into()),
                first: "H2O".into(),
                second: "H2O(g)".into(),
            }
        );
    }

    #[test]
    fn the_old_singular_reaction_field_is_rejected() {
        let json = combustion_json(BURN).replace(r#""reactions": ["#, r#""reaction": ["#);
        let e = load(&json).unwrap_err();
        assert!(e.to_string().contains("reaction"), "{e}");
        assert!(matches!(e, LoadError::BadOp { .. }), "{e:?}");
    }

    #[test]
    fn a_conversion_reactor_survives_load_solve_save_load() {
        let mut fs = load(&combustion_json(BURN)).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();
        let saved = Flowsheet::from(&fs);

        let reactor: ConversionReactorSpec = spec(&saved, 1, unit::ConversionReactor::TAG);
        assert_eq!(reactor.energy, unit::ReactorEnergy::Adiabatic);
        assert_eq!(reactor.reactions[0].limiting, "CH4");
        assert_eq!(reactor.reactions[0].conversion, 0.9);
        assert_eq!(
            reactor.reactions[0].stoichiometry,
            BTreeMap::from([
                ("CH4".to_string(), -1.0),
                ("CO2".to_string(), 1.0),
                ("H2O".to_string(), 2.0),
                ("O2".to_string(), -2.0),
            ])
        );

        // Reloading the saved document and saving again writes the same bytes.
        let json = serde_json::to_string(&saved).unwrap();
        let reloaded = DomainFlowsheet::try_from(doc(&json))
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(
            serde_json::to_string(&Flowsheet::from(&reloaded)).unwrap(),
            json
        );
    }

    #[test]
    fn a_reactor_without_an_energy_spec_is_rejected() {
        let json = combustion_json(BURN).replace(r#""energy": "adiabatic", "#, "");
        let e = load(&json).unwrap_err();
        assert!(
            matches!(e, LoadError::BadOp { .. }) && e.to_string().contains("energy"),
            "{e}"
        );
    }

    #[test]
    fn an_unknown_energy_spec_is_rejected() {
        let json = combustion_json(BURN).replace(r#""adiabatic""#, r#""exothermic""#);
        let e = load(&json).unwrap_err();
        assert!(e.to_string().contains("exothermic"), "{e}");
    }

    #[test]
    fn saving_drops_zero_coefficients() {
        // The nitrogen is written with an explicit zero, and does not come back.
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2, "N2": 0 },
                          "limiting": "CH4", "conversion": 0.9"#;
        let fs = load(&combustion_json(reaction))
            .unwrap()
            .validate()
            .unwrap();

        let reactor: ConversionReactorSpec =
            spec(&Flowsheet::from(&fs), 1, unit::ConversionReactor::TAG);

        assert_eq!(reactor.reactions[0].stoichiometry.len(), 4);
        assert!(!reactor.reactions[0].stoichiometry.contains_key("N2"));
    }

    #[test]
    fn a_conversion_above_one_is_an_error_not_a_panic() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 1.5"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err().to_string(),
            "unit `r`: `reactions[0].conversion` is 1.5, expected between 0.0 and 1.0"
        );
    }

    #[test]
    fn a_product_as_the_limiting_species_is_an_error_not_a_panic() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CO2", "conversion": 0.9"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err().to_string(),
            "unit `r`: `reactions[0].limiting` is 1, expected the coefficient of a reactant, which is negative"
        );
    }

    #[test]
    fn a_limiting_species_missing_from_the_equation_is_an_error_not_a_panic() {
        // N2 is declared, so this is not an unknown species - it just takes no part.
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "N2", "conversion": 0.9"#;
        assert!(matches!(
            load(&combustion_json(reaction)).unwrap_err(),
            LoadError::BadValue { ref field, value, .. } if field == "reactions[0].limiting" && value == 0.0
        ));
    }

    #[test]
    fn an_equation_that_does_not_conserve_mass_is_an_error_not_a_panic() {
        // Two and a half oxygens: misses by 15.999 g/mol over 6.5 units of coefficient.
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2.5, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 0.9"#;
        let e = load(&combustion_json(reaction)).unwrap_err();
        let LoadError::BadValue {
            ref field, value, ..
        } = e
        else {
            panic!("expected a bad value, got {e:?}");
        };
        assert_eq!(field, "reactions[0].stoichiometry");
        assert!((value - 15.999 / 6.5).abs() < 1e-9, "{value}");
        assert!(
            e.to_string()
                .ends_with("expected a mass balance within 0.01 g/mol per unit of coefficient"),
            "{e}"
        );
    }

    #[test]
    fn an_unknown_species_in_an_equation_names_where_it_appeared() {
        let reaction = r#""stoichiometry": { "CH4": -1, "Ar": 0 },
                          "limiting": "CH4", "conversion": 0.9"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err(),
            LoadError::UnknownSpecies {
                at: Location::Unit("r".into()),
                name: "Ar".into()
            }
        );
    }

    #[test]
    fn an_unknown_limiting_species_names_where_it_appeared() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "C2H6", "conversion": 0.9"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err(),
            LoadError::UnknownSpecies {
                at: Location::Unit("r".into()),
                name: "C2H6".into()
            }
        );
    }

    #[test]
    fn a_reaction_participant_without_a_formation_enthalpy_is_an_error_not_a_panic() {
        // N2 has none in the fixture. As a spectator it loads (see `saving_drops_zero_coefficients`);
        // as a participant it does not. The fixture has no NO to make a real reaction with, so this
        // turns nitrogen into its own mass of oxygen, which is all the closure check asks.
        let reaction = r#""stoichiometry": { "N2": -1, "O2": 0.87549 },
                          "limiting": "N2", "conversion": 0.5"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err().to_string(),
            "unit `r`: species `N2` takes part in the reaction but has no `enthalpy_of_formation`"
        );
    }

    #[test]
    fn a_stray_field_on_a_reaction_is_rejected() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 0.9, "extent": 1.0"#;
        let e = load(&combustion_json(reaction)).unwrap_err();
        assert!(e.to_string().contains("extent"), "{e}");
    }

    #[test]
    fn the_loaded_recycle_circuit_solves() {
        // Proves the loaded topology carries the recycle: a torn loop needs more than one pass.
        let mut fs = load(recycle_json()).unwrap().validate().unwrap();
        let report = crate::solver::Solver::default().solve(&mut fs).unwrap();
        assert!(
            report.iterations > 1,
            "a recycle should take several passes"
        );
    }
}

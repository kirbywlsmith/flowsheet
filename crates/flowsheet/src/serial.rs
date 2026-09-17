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
    /// Absolute mass flow per species (t/h), keyed by species name. Absent species are zero.
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

/// The parameters of a `splitter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterSpec {
    /// The fraction sent to the first outlet.
    pub fraction: f64,
}

/// The parameters of a `splitter_n`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterNSpec {
    /// The ratios, one per outlet.
    pub ratios: Vec<f64>,
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
    /// The one reaction the reactor runs.
    pub reaction: ReactionSpec,
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
}

/// The parameters of a unit operation that takes none. Empty, but not omitted: it is what rejects
/// a stray field on a `mixer`, `tank` or `product`.
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
    /// Two species share a name. Flows are keyed by name, so names must be unique even across
    /// phases.
    DuplicateSpecies {
        /// The repeated name.
        name: String,
        /// The phase of the second entry.
        phase: Phase,
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
            LoadError::DuplicateSpecies { name, phase } => write!(
                f,
                "species `{name}` ({phase:?}) repeats a name already declared - flows are keyed \
                 by name, so names must be unique across phases"
            ),
            LoadError::BadValue {
                at,
                field,
                value,
                expected,
            } => write!(f, "{at}: `{field}` is {value}, expected {expected}"),
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

impl State {
    /// Resolves the name-keyed flows against `registry`, producing a [`stream::Stream`] whose
    /// flows are in [`SpeciesId`] order. Species the document omits are zero.
    fn to_stream(
        &self,
        registry: &SpeciesRegistry,
        species_ids: &BTreeMap<String, SpeciesId>,
        at: &Location,
    ) -> Result<stream::Stream, LoadError> {
        require(
            self.temperature.is_finite() && self.temperature > 0.0,
            at,
            "temperature",
            self.temperature,
            "greater than 0.0 K",
        )?;
        require(
            self.pressure.is_finite() && self.pressure >= 0.0,
            at,
            "pressure",
            self.pressure,
            "0.0 kPa or greater",
        )?;

        let mut flows = vec![0.0; registry.len()];
        for (name, &value) in &self.flows {
            let id = species_ids
                .get(name)
                .ok_or_else(|| LoadError::UnknownSpecies {
                    at: at.clone(),
                    name: name.clone(),
                })?;
            require(
                value.is_finite() && value >= 0.0,
                at,
                name,
                value,
                "0.0 t/h or greater",
            )?;
            flows[id.as_usize()] = value;
        }

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
/// A struct rather than four parameters, because a constructor is stored behind a `dyn Fn` and
/// every added argument would be a breaking change to the [`OpRegistry`] table's type.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    /// The operation's parameters, exactly as the document wrote them.
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    /// Every species, in [`SpeciesId`] order.
    pub registry: &'a SpeciesRegistry,
    /// Species by name, for resolving a name-keyed map into a dense vector.
    pub species_ids: &'a BTreeMap<String, SpeciesId>,
    /// The unit being loaded, for error messages.
    pub at: &'a Location,
}

impl<'a> Spec<'a> {
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
                stream: spec.state.to_stream(s.registry, s.species_ids, s.at)?,
            }))
        });

        ops.register(unit::Mixer::TAG, |s| {
            s.parse::<NoSpec>(unit::Mixer::TAG)?;
            Ok(Box::new(unit::Mixer))
        });

        ops.register(unit::Splitter::TAG, |s| {
            let SplitterSpec { fraction } = s.parse(unit::Splitter::TAG)?;
            require(
                (0.0..=1.0).contains(&fraction),
                s.at,
                "fraction",
                fraction,
                "between 0.0 and 1.0",
            )?;
            Ok(Box::new(unit::Splitter { fraction }))
        });

        ops.register(unit::SplitterN::TAG, |s| {
            let SplitterNSpec { ratios } = s.parse(unit::SplitterN::TAG)?;
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
            Ok(Box::new(unit::SplitterN { ratios }))
        });

        ops.register(unit::Flotation::TAG, |s| {
            let FlotationSpec { recovery } = s.parse(unit::Flotation::TAG)?;
            // Resolved into a dense `SpeciesId`-ordered vector, the same shape as a stream's
            // flows. A species the map leaves out recovers nothing.
            let mut dense = vec![0.0; s.registry.len()];
            for (name, &value) in &recovery {
                let id = s
                    .species_ids
                    .get(name)
                    .ok_or_else(|| LoadError::UnknownSpecies {
                        at: s.at.clone(),
                        name: name.clone(),
                    })?;
                require(
                    (0.0..=1.0).contains(&value),
                    s.at,
                    name,
                    value,
                    "between 0.0 and 1.0",
                )?;
                dense[id.as_usize()] = value;
            }
            Ok(Box::new(unit::Flotation { recovery: dense }))
        });

        ops.register(unit::ConversionReactor::TAG, |s| {
            let ConversionReactorSpec { reaction } = s.parse(unit::ConversionReactor::TAG)?;
            let unknown = |name: &String| LoadError::UnknownSpecies {
                at: s.at.clone(),
                name: name.clone(),
            };

            // A species the map leaves out takes no part in the reaction.
            let mut stoichiometry = vec![0.0; s.registry.len()];
            for (name, &nu) in &reaction.stoichiometry {
                let id = s.species_ids.get(name).ok_or_else(|| unknown(name))?;
                stoichiometry[id.as_usize()] = nu;
            }
            let limiting = *s
                .species_ids
                .get(&reaction.limiting)
                .ok_or_else(|| unknown(&reaction.limiting))?;

            // Every panic in `unit::react` has a matching check here. The wrong-length one does
            // not: the vector is built above to the registry's length.
            require(
                (0.0..=1.0).contains(&reaction.conversion),
                s.at,
                "conversion",
                reaction.conversion,
                "between 0.0 and 1.0",
            )?;
            let nu_limiting = stoichiometry[limiting.as_usize()];
            require(
                nu_limiting < 0.0,
                s.at,
                "limiting",
                nu_limiting,
                "the coefficient of a reactant, which is negative",
            )?;
            unit::mass_closure(s.registry, &stoichiometry).map_err(|residual| {
                LoadError::BadValue {
                    at: s.at.clone(),
                    field: "stoichiometry".to_string(),
                    value: residual,
                    expected: format!(
                        "a mass balance within {} g/mol per unit of coefficient",
                        unit::MASS_CLOSURE_TOLERANCE
                    ),
                }
            })?;

            Ok(Box::new(unit::ConversionReactor {
                reaction: unit::Reaction {
                    stoichiometry,
                    limiting,
                    conversion: reaction.conversion,
                },
            }))
        });

        ops.register(unit::Heater::TAG, |s| {
            let HeaterSpec { duty } = s.parse(unit::Heater::TAG)?;
            Ok(Box::new(unit::Heater { duty }))
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
        species_ids: &BTreeMap<String, SpeciesId>,
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
            species_ids,
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
        let mut species_ids: BTreeMap<String, SpeciesId> = BTreeMap::new();
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
            if species_ids.contains_key(&s.name) {
                return Err(LoadError::DuplicateSpecies {
                    name: s.name.clone(),
                    phase: s.phase,
                });
            }
            species_ids.insert(s.name.clone(), registry.insert(s.clone()));
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
            let op = ops.build(&u.op, fs.registry(), &species_ids, &at)?;
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
            let stream = s.state.to_stream(fs.registry(), &species_ids, &at)?;
            fs.add_stream(from, stream, to);
        }

        Ok(fs)
    }
}

// ---------------------------------------------------------------------------
// Saving: domain -> serial
// ---------------------------------------------------------------------------

impl State {
    /// Captures a stream as a document state, keying flows by species name.
    ///
    /// Zero flows are omitted: they load back as zero anyway, and dropping them keeps a saved
    /// flowsheet readable when most streams carry only a few of the species.
    fn from_stream(s: &stream::Stream, registry: &SpeciesRegistry) -> Self {
        let flows = s
            .flows()
            .iter()
            .zip(registry.all())
            .filter(|(f, _)| **f != 0.0)
            .map(|(&f, species)| (species.name.clone(), f))
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
        spec_of(&NoSpec {})
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
            .zip(registry.all())
            .map(|(&r, species)| (species.name.clone(), r))
            .collect();

        spec_of(&FlotationSpec { recovery })
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
    /// If `stoichiometry` does not hold exactly one coefficient per species, for the reason
    /// [`unit::Flotation`]'s `spec` gives: `zip` would truncate, and the document would reload as
    /// a different reaction.
    fn spec(&self, registry: &SpeciesRegistry) -> serde_json::Value {
        let reaction = &self.reaction;
        assert_eq!(
            reaction.stoichiometry.len(),
            registry.len(),
            "a reaction needs one stoichiometric coefficient per species"
        );

        // Zero coefficients are dropped, unlike zero recoveries: an equation lists only the
        // species that take part in it.
        let stoichiometry = reaction
            .stoichiometry
            .iter()
            .zip(registry.all())
            .filter(|(nu, _)| **nu != 0.0)
            .map(|(&nu, species)| (species.name.clone(), nu))
            .collect();

        spec_of(&ConversionReactorSpec {
            reaction: ReactionSpec {
                stoichiometry,
                limiting: registry[reaction.limiting].name.clone(),
                conversion: reaction.conversion,
            },
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
        spec_of(&HeaterSpec { duty: self.duty })
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
    /// [`crate::flowsheet::Flowsheet`] because only validation rules out the two kinds of
    /// duplicate name a document cannot survive: two units sharing a name, which a stream
    /// endpoint could not tell apart, and two species sharing one, which would collide in the
    /// name-keyed `flows` map and silently drop a flow.
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
                "conversion_reactor",
                "feed",
                "flotation",
                "heater",
                "mixer",
                "product",
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
    fn a_name_repeated_across_phases_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "shomate": { "a": 75.3 } },
            { "name": "H2O", "phase": "Gas",    "molar_mass": 18.015, "shomate": { "a": 33.6 } }
          ], "units": [], "streams": [] }"#;
        assert_eq!(
            load(json).unwrap_err(),
            LoadError::DuplicateSpecies {
                name: "H2O".into(),
                phase: Phase::Gas
            }
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
    /// `reaction` is the body of the reactor's `reaction` object.
    fn combustion_json(reaction: &str) -> String {
        format!(
            r#"{{ "species": [
                {{ "name": "CH4", "phase": "Gas", "molar_mass": 16.043, "shomate": {{ "a": 35.7 }} }},
                {{ "name": "O2",  "phase": "Gas", "molar_mass": 31.998, "shomate": {{ "a": 29.4 }} }},
                {{ "name": "CO2", "phase": "Gas", "molar_mass": 44.009, "shomate": {{ "a": 37.1 }} }},
                {{ "name": "H2O", "phase": "Gas", "molar_mass": 18.015, "shomate": {{ "a": 33.6 }} }},
                {{ "name": "N2",  "phase": "Gas", "molar_mass": 28.014, "shomate": {{ "a": 29.1 }} }}
              ],
              "units": [
                {{ "name": "f", "op": {{ "type": "feed",
                  "state": {{ "flows": {{ "CH4": 10.0, "O2": 60.0, "N2": 200.0 }} }} }} }},
                {{ "name": "r", "op": {{ "type": "conversion_reactor", "reaction": {{ {reaction} }} }} }},
                {{ "name": "p", "op": {{ "type": "product" }} }}
              ],
              "streams": [{{ "from": "f", "to": "r" }}, {{ "from": "r", "to": "p" }}] }}"#
        )
    }

    const BURN: &str = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 0.9"#;

    #[test]
    fn a_conversion_reactor_survives_load_solve_save_load() {
        let mut fs = load(&combustion_json(BURN)).unwrap().validate().unwrap();
        crate::solver::Solver::default().solve(&mut fs).unwrap();
        let saved = Flowsheet::from(&fs);

        let reactor: ConversionReactorSpec = spec(&saved, 1, unit::ConversionReactor::TAG);
        assert_eq!(reactor.reaction.limiting, "CH4");
        assert_eq!(reactor.reaction.conversion, 0.9);
        assert_eq!(
            reactor.reaction.stoichiometry,
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

        assert_eq!(reactor.reaction.stoichiometry.len(), 4);
        assert!(!reactor.reaction.stoichiometry.contains_key("N2"));
    }

    #[test]
    fn a_conversion_above_one_is_an_error_not_a_panic() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CH4", "conversion": 1.5"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err().to_string(),
            "unit `r`: `conversion` is 1.5, expected between 0.0 and 1.0"
        );
    }

    #[test]
    fn a_product_as_the_limiting_species_is_an_error_not_a_panic() {
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "CO2", "conversion": 0.9"#;
        assert_eq!(
            load(&combustion_json(reaction)).unwrap_err().to_string(),
            "unit `r`: `limiting` is 1, expected the coefficient of a reactant, which is negative"
        );
    }

    #[test]
    fn a_limiting_species_missing_from_the_equation_is_an_error_not_a_panic() {
        // N2 is declared, so this is not an unknown species - it just takes no part.
        let reaction = r#""stoichiometry": { "CH4": -1, "O2": -2, "CO2": 1, "H2O": 2 },
                          "limiting": "N2", "conversion": 0.9"#;
        assert!(matches!(
            load(&combustion_json(reaction)).unwrap_err(),
            LoadError::BadValue { ref field, value, .. } if field == "limiting" && value == 0.0
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
        assert_eq!(field, "stoichiometry");
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

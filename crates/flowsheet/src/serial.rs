//! The on-disk representation of a flowsheet.
//!
//! These types mirror the domain types but hold no invariants - they are whatever the file said.
//! Validation happens on the way out, in the conversion into [`crate::flowsheet::Flowsheet`].

use crate::flowsheet::{self, UnitId};
use crate::species::{Phase, Species, SpeciesId, SpeciesRegistry};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// Absolute mass flow per species (t/h), keyed by species name. Absent species are zero.
    pub flows: BTreeMap<String, f64>,
    /// Temperature, in Kelvin.
    pub temperature: f64,
    /// Pressure, in kPa.
    pub pressure: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            flows: BTreeMap::new(),
            temperature: AMBIENT_K,
            pressure: AMBIENT_KPA,
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
    pub op: UnitOp,
}

/// What a unit does, mirroring [`crate::unit::UnitOp`].
///
/// Each variant wraps its own struct rather than declaring fields inline. `serde` silently ignores
/// `deny_unknown_fields` on an internally tagged enum, so inline struct variants would accept a
/// `fraction` typed onto a mixer; a newtype variant is deserialised as a plain struct with the
/// `type` key already removed, and that struct's `deny_unknown_fields` does fire. The JSON shape is
/// identical either way - `{ "type": "splitter", "fraction": 0.3 }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UnitOp {
    /// One outlet, emitting a fixed state.
    Feed(FeedSpec),
    /// Combines all inlets into one outlet.
    Mixer(NoSpec),
    /// One inlet, two outlets: `fraction` and `1.0 - fraction`.
    Splitter(SplitterSpec),
    /// One inlet, one outlet per ratio.
    SplitterN(SplitterNSpec),
    /// One inlet, two outlets: concentrate and tails, at a recovery per species.
    Flotation(FlotationSpec),
    /// One inlet, one outlet.
    Tank(NoSpec),
    /// One inlet.
    Product(NoSpec),
}

/// The parameters of a [`UnitOp::Feed`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedSpec {
    /// The state this feed emits.
    pub state: State,
}

/// The parameters of a [`UnitOp::Splitter`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterSpec {
    /// The fraction sent to the first outlet.
    pub fraction: f64,
}

/// The parameters of a [`UnitOp::SplitterN`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitterNSpec {
    /// The ratios, one per outlet.
    pub ratios: Vec<f64>,
}

/// The parameters of a [`UnitOp::Flotation`].
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

/// The most units or streams a `u16` id can address.
const MAX_IDS: usize = u16::MAX as usize + 1;

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

impl UnitOp {
    /// Converts into a domain [`unit::UnitOp`], rejecting the values its constructors panic on.
    ///
    /// This is the half of the round trip a trait object cannot do for itself: a document names
    /// its operation with a string, and something has to own the name-to-constructor table. The
    /// other half is [`ToDocument`], which each operation implements.
    ///
    /// Each arm builds a different concrete type; they unify because the return type is
    /// `Box<dyn unit::UnitOp>`, so every arm unsize-coerces to it.
    fn to_domain(
        &self,
        registry: &SpeciesRegistry,
        species_ids: &BTreeMap<String, SpeciesId>,
        at: &Location,
    ) -> Result<Box<dyn unit::UnitOp>, LoadError> {
        Ok(match self {
            UnitOp::Feed(spec) => Box::new(unit::Feed {
                stream: spec.state.to_stream(registry, species_ids, at)?,
            }),
            UnitOp::Mixer(_) => Box::new(unit::Mixer),
            UnitOp::Splitter(SplitterSpec { fraction }) => {
                require(
                    (0.0..=1.0).contains(fraction),
                    at,
                    "fraction",
                    *fraction,
                    "between 0.0 and 1.0",
                )?;
                Box::new(unit::Splitter {
                    fraction: *fraction,
                })
            }
            UnitOp::SplitterN(SplitterNSpec { ratios }) => {
                for r in ratios {
                    require(
                        r.is_finite() && *r >= 0.0,
                        at,
                        "ratios",
                        *r,
                        "0.0 or greater",
                    )?;
                }
                // An empty `ratios` sums to zero, so this catches that case too.
                let sum: f64 = ratios.iter().sum();
                require(sum > 0.0, at, "ratios", sum, "a sum greater than 0.0")?;
                Box::new(unit::SplitterN {
                    ratios: ratios.clone(),
                })
            }
            UnitOp::Flotation(FlotationSpec { recovery }) => {
                // Resolved into a dense `SpeciesId`-ordered vector, the same shape as a
                // stream's flows. A species the map leaves out recovers nothing.
                let mut dense = vec![0.0; registry.len()];
                for (name, &value) in recovery {
                    let id = species_ids
                        .get(name)
                        .ok_or_else(|| LoadError::UnknownSpecies {
                            at: at.clone(),
                            name: name.clone(),
                        })?;
                    require(
                        (0.0..=1.0).contains(&value),
                        at,
                        name,
                        value,
                        "between 0.0 and 1.0",
                    )?;
                    dense[id.as_usize()] = value;
                }
                Box::new(unit::Flotation { recovery: dense })
            }
            UnitOp::Tank(_) => Box::new(unit::Tank),
            UnitOp::Product(_) => Box::new(unit::Product),
        })
    }
}

impl TryFrom<Flowsheet> for flowsheet::Flowsheet {
    type Error = LoadError;

    /// Replays the document through the [`crate::flowsheet::Flowsheet`] builder, so every
    /// invariant those methods maintain still holds.
    ///
    /// The result is unvalidated: call [`crate::flowsheet::Flowsheet::validate`] to check arity.
    ///
    /// # Errors
    ///
    /// Returns the first [`LoadError`] found.
    fn try_from(doc: Flowsheet) -> Result<Self, Self::Error> {
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
            let op = u.op.to_domain(fs.registry(), &species_ids, &at)?;
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

/// Captures a domain unit operation as its document form.
///
/// This is a supertrait of [`unit::UnitOp`] rather than a `match` inside this module, because
/// there is nothing left to match on: `Box<dyn unit::UnitOp>` has erased the concrete type, and
/// only the operation itself still knows what it is. The alternative - downcasting through
/// [`std::any::Any`] - would put a closed list of concrete types back in this file and give up
/// the open set the trait object was adopted for.
///
/// The `impl` blocks live here, next to the private `UnitOp::to_domain` that loads them back,
/// so the wire format stays one module.
///
/// The open set stops at this boundary, and deliberately so: [`UnitOp`] is a closed enum, so a
/// third-party operation has no variant of its own to return and can only describe itself as one
/// of the built-in ones - which `to_domain` would then load back as that built-in operation, not
/// as the original. So the set of operations a flowsheet can *solve* is open; the set it can
/// *round-trip through a document* is not. Widening it means a name-keyed registry of
/// constructors, which is the next move here if a downstream operation ever needs saving.
pub trait ToDocument {
    /// Captures this operation as its document form, resolving flows against `registry`.
    fn to_document(&self, registry: &SpeciesRegistry) -> UnitOp;
}

impl ToDocument for unit::Feed {
    fn to_document(&self, registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::Feed(FeedSpec {
            state: State::from_stream(&self.stream, registry),
        })
    }
}

impl ToDocument for unit::Mixer {
    fn to_document(&self, _registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::Mixer(NoSpec {})
    }
}

impl ToDocument for unit::Splitter {
    fn to_document(&self, _registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::Splitter(SplitterSpec {
            fraction: self.fraction,
        })
    }
}

impl ToDocument for unit::SplitterN {
    fn to_document(&self, _registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::SplitterN(SplitterNSpec {
            ratios: self.ratios.clone(),
        })
    }
}

impl ToDocument for unit::Flotation {
    /// # Panics
    /// If `recovery` does not hold exactly one entry per species. `zip` would otherwise
    /// truncate to the shorter side and write a document that reloads as a *different* cell -
    /// silently, and with nothing for the reload to complain about.
    fn to_document(&self, registry: &SpeciesRegistry) -> UnitOp {
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

        UnitOp::Flotation(FlotationSpec { recovery })
    }
}

impl ToDocument for unit::Tank {
    fn to_document(&self, _registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::Tank(NoSpec {})
    }
}

impl ToDocument for unit::Product {
    fn to_document(&self, _registry: &SpeciesRegistry) -> UnitOp {
        UnitOp::Product(NoSpec {})
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
                op: u.op.to_document(registry),
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
            { "name": "CuFeS2", "phase": "Solid",  "molar_mass": 183.5 },
            { "name": "SiO2",   "phase": "Solid",  "molar_mass": 60.08 },
            { "name": "H2O",    "phase": "Liquid", "molar_mass": 18.015 }
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

    /// A minimal document with one species and one unit, for poking at individual fields.
    fn minimal(units: &str, streams: &str) -> String {
        format!(
            r#"{{ "species": [{{ "name": "H2O", "phase": "Liquid", "molar_mass": 18.015 }}],
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

    /// `deny_unknown_fields` fires during parsing, before any [`LoadError`] can be raised, so
    /// these are `serde_json` errors rather than load errors.
    #[test]
    fn a_field_that_belongs_to_another_unit_op_is_rejected() {
        // `fraction` is a splitter's field. Flattening `op` into `Unit` would have swallowed it.
        let json = minimal(
            r#"{ "name": "m", "op": { "type": "mixer", "fraction": 0.9 } }"#,
            "",
        );
        let e = serde_json::from_str::<Flowsheet>(&json).unwrap_err();
        assert!(e.to_string().contains("fraction"), "{e}");
    }

    #[test]
    fn a_stray_field_on_a_parameterised_op_is_rejected() {
        let json = minimal(
            r#"{ "name": "s", "op": { "type": "splitter", "fraction": 0.3, "junk": 1 } }"#,
            "",
        );
        let e = serde_json::from_str::<Flowsheet>(&json).unwrap_err();
        assert!(e.to_string().contains("junk"), "{e}");
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
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015, "mm": 18.0 }
          ], "units": [], "streams": [] }"#;
        let e = serde_json::from_str::<Flowsheet>(json).unwrap_err();
        assert!(e.to_string().contains("mm"), "{e}");
    }

    #[test]
    fn a_name_repeated_across_phases_is_rejected() {
        let json = r#"{ "species": [
            { "name": "H2O", "phase": "Liquid", "molar_mass": 18.015 },
            { "name": "H2O", "phase": "Gas",    "molar_mass": 18.015 }
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
        let json = r#"{ "species": [{ "name": "X", "phase": "Gas", "molar_mass": 0.0 }],
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
    fn saving_drops_only_the_species_whose_flow_is_zero() {
        let json = r#"{ "species": [
            { "name": "H2O",  "phase": "Liquid", "molar_mass": 18.015 },
            { "name": "SiO2", "phase": "Solid",  "molar_mass": 60.08 }
          ],
          "units": [
            { "name": "f", "op": { "type": "feed", "state": { "flows": { "SiO2": 5.0 } } } },
            { "name": "p", "op": { "type": "product" } }
          ],
          "streams": [{ "from": "f", "to": "p" }] }"#;
        let fs = load(json).unwrap().validate().unwrap();
        let doc = Flowsheet::from(&fs);

        let UnitOp::Feed(spec) = &doc.units[0].op else {
            panic!("unit 0 is the feed")
        };
        let state = &spec.state;
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
            { "name": "H2O",  "phase": "Liquid", "molar_mass": 18.015 },
            { "name": "SiO2", "phase": "Solid",  "molar_mass": 60.08 }
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
        let UnitOp::Flotation(spec) = &doc.units[1].op else {
            panic!("unit 1 is the flotation cell")
        };
        assert_eq!(spec.recovery.len(), 2, "a zero recovery is not dropped");
        assert_eq!(spec.recovery["H2O"], 0.0);
        assert_eq!(spec.recovery["SiO2"], 0.4);
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

//! A unit operation defined outside the library, solved *and* round-tripped through a document.
//!
//! A closed wire format has one failure mode worth testing for: an operation with no name of its
//! own can only describe itself as one of the built-ins, and loading hands that built-in back
//! with nothing wrong at either boundary. The assertions below are aimed at it. The saved
//! document names `bleed`. A registry without a `bleed` constructor says so instead of guessing.
//! A registry with one loads the operation back with its parameter intact.
//!
//! An integration test is a separate crate, so `Bleed` is defined outside `flowsheet` and can
//! only reach the public API - which is the constraint a downstream implementor works under.

use approx::assert_relative_eq;
use flowsheet::serial::{self, LoadError, Location, OpRegistry, Spec, ToDocument};
use flowsheet::unit::Arity;
use flowsheet::{Flowsheet, Phase, Shomate, Species, SpeciesRegistry, Stream, UnitOp};
use serde::{Deserialize, Serialize};

/// A vent: one inlet, one outlet, discarding `rate` of every species.
///
/// Deliberately unlike every built-in. A `Tank` passes everything through, so a `Bleed` loaded
/// as a tank shows up as a different product flow; a `Splitter` has two outlets, so it cannot be
/// mistaken for one without failing validation. Mass is not conserved: the bled stream leaves
/// the flowsheet.
#[derive(Debug, Clone)]
struct Bleed {
    /// The fraction of every species discarded.
    rate: f64,
}

impl Bleed {
    /// The `type` written to a document, and the key its constructor is registered under.
    const TAG: &'static str = "bleed";
}

/// The document form of [`Bleed`]. `deny_unknown_fields` fires in `Spec::parse`, not while the
/// document itself parses.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BleedSpec {
    rate: f64,
}

impl ToDocument for Bleed {
    fn tag(&self) -> &'static str {
        Self::TAG
    }

    fn spec(&self, _registry: &SpeciesRegistry) -> serde_json::Value {
        serde_json::to_value(BleedSpec { rate: self.rate }).expect("a rate serialises")
    }
}

impl UnitOp for Bleed {
    fn inlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn outlet_arity(&self) -> Arity {
        Arity::exactly(1)
    }

    fn evaluate(&self, _registry: &SpeciesRegistry, inlets: &[&Stream]) -> Vec<Stream> {
        // `split` gives the kept side first and the discarded side second; only the first is
        // wired to anything.
        let (kept, _vented) = flowsheet::unit::split(inlets[0], 1.0 - self.rate);
        vec![kept]
    }
}

/// The registry every built-in op knows, plus `bleed`.
fn ops() -> OpRegistry {
    let mut ops = OpRegistry::builtin();
    ops.register(Bleed::TAG, |s: Spec<'_>| {
        let BleedSpec { rate } = s.parse(Bleed::TAG)?;
        if !(0.0..=1.0).contains(&rate) {
            return Err(LoadError::BadOp {
                at: s.at.clone(),
                tag: Bleed::TAG.to_string(),
                message: format!("`rate` is {rate}, expected between 0.0 and 1.0"),
            });
        }
        Ok(Box::new(Bleed { rate }))
    });
    ops
}

fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    r.insert(Species {
        name: "H2O".into(),
        phase: Phase::Liquid,
        molar_mass: 18.015,
        shomate: Shomate::constant(75.3),
    });
    r
}

/// feed (100 t/h) -> bleed (10%) -> product, as a document.
fn document(rate: f64) -> String {
    format!(
        r#"{{
          "species": [
            {{ "name": "H2O", "phase": "Liquid", "molar_mass": 18.015,
               "shomate": {{ "a": 75.3 }} }}
          ],
          "units": [
            {{ "name": "f", "op": {{ "type": "feed",
               "state": {{ "flows": {{ "H2O": 100.0 }} }} }} }},
            {{ "name": "vent", "op": {{ "type": "bleed", "rate": {rate} }} }},
            {{ "name": "p", "op": {{ "type": "product" }} }}
          ],
          "streams": [
            {{ "from": "f", "to": "vent" }},
            {{ "from": "vent", "to": "p" }}
          ]
        }}"#
    )
}

fn load(json: &str, ops: &OpRegistry) -> Result<Flowsheet, LoadError> {
    let doc: serial::Flowsheet = serde_json::from_str(json).expect("the document parses");
    doc.into_domain(ops)
}

fn solved_product_flow(fs: Flowsheet) -> f64 {
    let mut fs = fs.validate().expect("feed -> bleed -> product is valid");
    flowsheet::Solver::default()
        .solve(&mut fs)
        .expect("it solves");
    fs.streams()[1].total()
}

#[test]
fn a_downstream_op_solves() {
    let flow = solved_product_flow(load(&document(0.1), &ops()).expect("it loads"));
    assert_relative_eq!(flow, 90.0, max_relative = 1e-12);
}

#[test]
fn a_downstream_op_saves_under_its_own_tag() {
    let mut fs = Flowsheet::new(registry());
    let f = fs.add_unit(
        "f",
        flowsheet::Feed {
            stream: Stream::from_flows(&registry(), vec![100.0], 298.15, 101.325),
        },
    );
    let vent = fs.add_unit("vent", Bleed { rate: 0.1 });
    let p = fs.add_unit("p", flowsheet::Product);
    let blank = Stream::zeros(&registry(), 298.15, 101.325);
    fs.add_stream(f, blank.clone(), vent);
    fs.add_stream(vent, blank, p);

    let fs = fs.validate().expect("the circuit is valid");
    let doc = serial::Flowsheet::from(&fs);

    // The saved unit names itself rather than borrowing a built-in's name.
    assert_eq!(doc.units[1].op.tag, "bleed");
    assert_eq!(doc.units[1].op.spec["rate"], 0.1);
}

#[test]
fn the_builtin_registry_refuses_an_op_it_does_not_know() {
    // Without a registered constructor there is nothing to build, so this is an error rather
    // than a guess at the nearest built-in.
    let e = load(&document(0.1), &OpRegistry::builtin()).unwrap_err();
    assert_eq!(
        e,
        LoadError::UnknownOp {
            at: Location::Unit("vent".into()),
            tag: "bleed".into()
        }
    );
}

#[test]
fn a_downstream_op_survives_load_solve_save_load() {
    let first = load(&document(0.1), &ops()).expect("it loads");
    let mut solved = first.validate().expect("it is valid");
    flowsheet::Solver::default()
        .solve(&mut solved)
        .expect("it solves");

    let json = serde_json::to_string(&serial::Flowsheet::from(&solved)).expect("it serialises");
    let reloaded = load(&json, &ops()).expect("the saved document loads back");

    // Same answer from the reloaded flowsheet: the `rate` made the round trip, so the operation
    // is the same one and not a pass-through wearing its name.
    assert_relative_eq!(solved_product_flow(reloaded), 90.0, max_relative = 1e-12);
}

#[test]
fn a_downstream_op_rejects_its_own_bad_parameters() {
    let e = load(&document(1.5), &ops()).unwrap_err();
    assert_eq!(
        e.to_string(),
        "unit `vent`: `bleed` parameters are invalid: `rate` is 1.5, expected between 0.0 and 1.0"
    );
}

#[test]
fn a_stray_field_on_a_downstream_op_is_still_rejected() {
    let json = document(0.1).replace(r#""rate": 0.1"#, r#""rate": 0.1, "fraction": 0.5"#);
    let e = load(&json, &ops()).unwrap_err();
    assert!(e.to_string().contains("fraction"), "{e}");
}

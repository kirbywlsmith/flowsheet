//! End-to-end checks that a flowsheet survives a trip through JSON.

use flowsheet::demo;
use flowsheet::serial;
use flowsheet::{Flowsheet, Solver};

/// Solves the demo recycle circuit and captures the result as a document.
fn solved_demo() -> serial::Flowsheet {
    let mut fs = demo::build_flowsheet()
        .validate()
        .expect("the demo flowsheet is valid");
    Solver::default()
        .solve(&mut fs)
        .expect("the demo circuit converges");
    serial::Flowsheet::from(&fs)
}

#[test]
fn a_solved_flowsheet_survives_save_load_save() {
    let first = solved_demo();
    let json = serde_json::to_string_pretty(&first).expect("a document serialises");

    let parsed: serial::Flowsheet = serde_json::from_str(&json).expect("it parses back");
    let reloaded = Flowsheet::try_from(parsed)
        .expect("it loads")
        .validate()
        .expect("it is still valid");

    // Serialising the reloaded flowsheet must give byte-identical JSON. This is the strongest
    // statement of round-trip fidelity available: names, wiring, port order, species order and
    // every flow all have to match, or the strings diverge.
    let second = serde_json::to_string_pretty(&serial::Flowsheet::from(&reloaded))
        .expect("a document serialises");
    assert_eq!(json, second);
}

#[test]
fn a_reloaded_flowsheet_solves_to_the_same_answer() {
    let json = serde_json::to_string_pretty(&solved_demo()).unwrap();
    let parsed: serial::Flowsheet = serde_json::from_str(&json).unwrap();
    let mut reloaded = Flowsheet::try_from(parsed).unwrap().validate().unwrap();
    // The saved streams start at the solution, so the solver has nothing left to move: it
    // should converge inside its tolerance rather than reproduce the file byte for byte, since
    // one more Wegstein pass still nudges the last few bits.
    let report = Solver::default()
        .solve(&mut reloaded)
        .expect("a saved solution re-solves");
    assert!(
        report.residual < 1e-9,
        "re-solving a saved solution should stay converged, got {}",
        report.residual
    );

    let again: serial::Flowsheet =
        serde_json::from_str(&serde_json::to_string(&serial::Flowsheet::from(&reloaded)).unwrap())
            .unwrap();
    let before: serial::Flowsheet = serde_json::from_str(&json).unwrap();
    for (a, b) in before.streams.iter().zip(&again.streams) {
        for (name, flow) in &a.state.flows {
            let after = b.state.flows[name];
            approx::assert_relative_eq!(*flow, after, epsilon = 1e-6);
        }
    }
}

#[test]
fn an_unsolved_flowsheet_round_trips_its_topology() {
    // Saving before solving means every stream is zero, so `flows` maps are omitted entirely.
    let fs = demo::build_flowsheet().validate().unwrap();
    let doc = serial::Flowsheet::from(&fs);
    let json = serde_json::to_string(&doc).unwrap();

    // Every flow is zero, so every `flows` map is empty - that is the omission working.
    assert!(
        doc.streams.iter().all(|s| s.state.flows.is_empty()),
        "zero flows should be dropped on save, got {json}"
    );

    let reloaded = Flowsheet::try_from(serde_json::from_str::<serial::Flowsheet>(&json).unwrap())
        .expect("an unsolved document loads")
        .validate()
        .expect("and is valid");

    assert_eq!(reloaded.unit_count(), fs.unit_count());
    assert_eq!(reloaded.streams().len(), fs.streams().len());
    assert_eq!(
        serde_json::to_string(&serial::Flowsheet::from(&reloaded)).unwrap(),
        json
    );
}

#[test]
fn saved_json_names_units_not_generated_ids() {
    let json = serde_json::to_string(&solved_demo()).unwrap();

    // Names come from the flowsheet, not from generated ids.
    assert!(json.contains(r#""name":"mixer""#), "{json}");
    assert!(json.contains(r#""name":"split""#), "{json}");
    // The demo circuit carries all three species everywhere, so none are dropped here.
    assert!(json.contains("CuFeS2"));
}

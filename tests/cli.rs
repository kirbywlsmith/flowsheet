//! End-to-end runs of the `flowsheet` binary over fixture documents.
//!
//! Cargo sets `CARGO_BIN_EXE_<name>` for every integration test, so the path to the freshly
//! built binary is a compile-time constant - no need to guess at `target/debug`. The working
//! directory is the package root, which is what makes the `tests/fixtures` paths below work.

use approx::assert_relative_eq;
use process_simulation::flowsheet::Flowsheet;
use process_simulation::serial;
use process_simulation::solver::Solver;
use std::process::Command;

/// What one run of the binary produced.
struct Run {
    stdout: String,
    stderr: String,
    ok: bool,
}

fn flowsheet(args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_process_simulation"))
        .args(args)
        .output()
        .expect("the binary runs");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is utf-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is utf-8"),
        ok: out.status.success(),
    }
}

#[test]
fn the_default_table_matches_the_worked_example() {
    // These are the numbers in the `src/main.rs` header comment: the product equals the feed,
    // and the internal flows are amplified by 1 / (1 - 0.3) = 1.42857.
    let run = flowsheet(&["tests/fixtures/recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[..6],
        [
            "  #  stream         CuFeS2     SiO2      H2O     total",
            "  0  feed.mixer     40.000  360.000  600.000  1000.000",
            "  1  mixer.tank     57.143  514.286  857.143  1428.571",
            "  2  tank.split     57.143  514.286  857.143  1428.571",
            "  3  split.mixer    17.143  154.286  257.143   428.571",
            "  4  split.product  40.000  360.000  600.000  1000.000",
        ]
    );
    // Direct substitution shrinks the error by exactly f = 0.3 a pass, so 0.3^n <= 1e-9 puts
    // the count at 18. The residual's last digits are float noise, so only its prefix is pinned.
    assert!(
        lines[6]
            .trim()
            .starts_with("converged in 18 iterations, residual "),
        "{}",
        lines[6]
    );
}

#[test]
fn json_mode_prints_a_document_carrying_the_solved_flows() {
    let run = flowsheet(&["--json", "tests/fixtures/recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let doc: serial::Flowsheet =
        serde_json::from_str(&run.stdout).expect("stdout is a whole document");

    // streams[4] is the product. At steady state it must equal the feed exactly.
    let product = &doc.streams[4].state.flows;
    assert_eq!(doc.streams[4].to, "product");
    for (name, expected) in [("CuFeS2", 40.0), ("SiO2", 360.0), ("H2O", 600.0)] {
        assert_relative_eq!(product[name], expected, epsilon = 1e-6);
    }

    // And the printed document must be a legal input in its own right - `> out.json` is the
    // only way this mode is meant to be used.
    let mut reloaded = Flowsheet::try_from(doc)
        .expect("it loads")
        .validate()
        .expect("it is valid");
    let report = Solver::default()
        .solve(&mut reloaded)
        .expect("a saved solution re-solves");
    assert!(report.residual <= 1e-9, "{}", report.residual);
}

#[test]
fn two_streams_between_the_same_units_get_distinct_labels() {
    let run = flowsheet(&["tests/fixtures/duplicate_label.json"]);
    assert!(run.ok, "{}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[..5],
        [
            "  #  stream         CuFeS2     SiO2      H2O     total",
            "  0  feed.split     40.000  360.000  600.000  1000.000",
            "  1  split.mixer    12.000  108.000  180.000   300.000",
            "  2  split.mixer#2  28.000  252.000  420.000   700.000",
            "  3  mixer.product  40.000  360.000  600.000  1000.000",
        ]
    );
}

#[test]
fn a_missing_file_fails_with_a_message_and_no_output() {
    let run = flowsheet(&["tests/fixtures/does_not_exist.json"]);
    assert!(!run.ok, "a missing file must not exit zero");
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    // The path has to be in the message: `io::Error` alone never names the file it failed on.
    assert!(
        run.stderr.contains("tests/fixtures/does_not_exist.json"),
        "{}",
        run.stderr
    );
}

#[test]
fn an_invalid_flowsheet_reports_what_validation_found() {
    // The splitter has one outlet where its arity demands two - a legal document, but not a
    // solvable flowsheet, so it fails after loading rather than during it.
    let run = flowsheet(&["tests/fixtures/half_wired_splitter.json"]);
    assert!(!run.ok, "an invalid flowsheet must not exit zero");
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert_eq!(
        run.stderr,
        "1 validation error:\n  unit 1 has 1 outlets, expected 2 to 2\n"
    );
}

/// `recycle.json` is the demo circuit saved as a document, so nothing here may be hand-edited:
/// if the two ever disagree the fixture is no longer the flowsheet the rest of the suite
/// reasons about. Regenerate it with `cargo run -- --json` on the previous copy, or by
/// pretty-printing the document this test builds.
#[test]
fn the_recycle_fixture_is_the_demo_circuit_saved() {
    let demo = process_simulation::demo::build_flowsheet()
        .validate()
        .expect("the demo circuit is valid");
    let generated = serde_json::to_string_pretty(&serial::Flowsheet::from(&demo))
        .expect("a document serialises");
    let on_disk =
        std::fs::read_to_string("tests/fixtures/recycle.json").expect("the fixture is readable");

    // The file ends with a newline that `to_string_pretty` does not write.
    assert_eq!(generated, on_disk.trim_end_matches('\n'));
}

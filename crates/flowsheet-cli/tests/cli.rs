//! End-to-end runs of the `flowsheet` binary over fixture documents.
//!
//! Cargo sets `CARGO_BIN_EXE_<name>` for every integration test, so the path to the freshly
//! built binary is a compile-time constant - no need to guess at `target/debug`. The working
//! directory is the package root, which is what makes the `tests/fixtures` paths below work.

use approx::assert_relative_eq;
use flowsheet::serial;
use flowsheet::{Flowsheet, Solver};
use std::process::Command;

/// What one run of the binary produced.
struct Run {
    stdout: String,
    stderr: String,
    ok: bool,
}

fn flowsheet(args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_flowsheet"))
        .args(args)
        .output()
        .expect("the binary runs");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is utf-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is utf-8"),
        ok: out.status.success(),
    }
}

/// `blend.json` is the README's worked example, pasted verbatim in its hand-written form - no
/// stream `state`, no default temperatures. The rows below are the table the README prints, so if
/// either side changes, this is the test that says the README is now wrong.
#[test]
fn the_default_table_matches_the_readme_worked_example() {
    let run = flowsheet(&["tests/fixtures/blend.json"]);
    assert!(run.ok, "{}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[..7],
        [
            "  #  stream             H2O  C2H5OH    total   T (K)",
            "  0  water.mixer     90.000   0.000   90.000  288.15",
            "  1  ethanol.mixer    0.000  10.000   10.000  298.15",
            "  2  mixer.heater   180.000  20.000  200.000  308.71",
            "  3  heater.split   180.000  20.000  200.000  328.68",
            "  4  split.mixer     90.000  10.000  100.000  328.68",
            "  5  split.product   90.000  10.000  100.000  328.68",
        ]
    );
    // Half of every pass's error comes back round the loop, so 0.5^n <= 1e-9 puts the count at
    // 30. As above, only the footer's prefix is pinned.
    assert!(
        lines[7]
            .trim()
            .starts_with("converged in 30 iterations, residual "),
        "{}",
        lines[7]
    );
    // The heater's duty comes back from the streams; the mixer's is noise and is not listed.
    assert_eq!(
        lines[8..],
        [
            "",
            "  unit    type    duty (MJ/h)",
            "  heater  heater      16000.0",
        ]
    );
}

#[test]
fn the_default_table_matches_the_demo_circuit() {
    // These are the numbers in this crate's `src/main.rs` header comment: the two products
    // together equal the feed, and the concentrate is upgraded from 4.00% to 12.33% CuFeS2.
    let run = flowsheet(&["tests/fixtures/recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[..7],
        [
            "  #  stream                 CuFeS2     SiO2      H2O     total   T (K)",
            "  0  feed.mixer             40.000  360.000  600.000  1000.000  298.15",
            "  1  mixer.flotation        41.885  503.497  759.494  1304.875  298.15",
            "  2  flotation.concentrate  35.602   25.175  227.848   288.625  298.15",
            "  3  flotation.split         6.283  478.322  531.646  1016.250  298.15",
            "  4  split.mixer             1.885  143.497  159.494   304.875  298.15",
            "  5  split.tailings          4.398  334.825  372.152   711.375  298.15",
        ]
    );
    // Direct substitution shrinks the error by f * (1 - r) a pass, worst for the gangue at
    // 0.3 * 0.95 = 0.285, so 0.285^n <= 1e-9 puts the count at 17. The residual's last digits
    // are float noise, so only its prefix is pinned.
    assert!(
        lines[7]
            .trim()
            .starts_with("converged in 17 iterations, residual "),
        "{}",
        lines[7]
    );
    // And the balance closes: the concentrate and the tailings above add back up to the feed,
    // to within whatever error the tear stream still carried when the solve stopped.
    assert!(lines[7].contains(", imbalance "), "{}", lines[7]);
}

#[test]
fn json_mode_prints_a_document_carrying_the_solved_flows() {
    let run = flowsheet(&["--json", "tests/fixtures/recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let doc: serial::Flowsheet =
        serde_json::from_str(&run.stdout).expect("stdout is a whole document");

    // streams[2] and streams[5] are the two plant products. At steady state they must add up
    // to the feed exactly - the recycle inflates the internal flows and nothing else.
    assert_eq!(doc.streams[2].to, "concentrate");
    assert_eq!(doc.streams[5].to, "tailings");
    let concentrate = &doc.streams[2].state.flows;
    let tailings = &doc.streams[5].state.flows;
    for (name, expected) in [("CuFeS2", 40.0), ("SiO2", 360.0), ("H2O", 600.0)] {
        assert_relative_eq!(concentrate[name] + tailings[name], expected, epsilon = 1e-6);
    }
    // And the cell did its job: most of the chalcopyrite floated, almost none of the gangue.
    assert_relative_eq!(concentrate["CuFeS2"], 35.602094, epsilon = 1e-6);
    assert_relative_eq!(concentrate["SiO2"], 25.174825, epsilon = 1e-6);

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
            "  #  stream         CuFeS2     SiO2      H2O     total   T (K)",
            "  0  feed.split     40.000  360.000  600.000  1000.000  298.15",
            "  1  split.mixer    12.000  108.000  180.000   300.000  298.15",
            "  2  split.mixer#2  28.000  252.000  420.000   700.000  298.15",
            "  3  mixer.product  40.000  360.000  600.000  1000.000  298.15",
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
    let demo = flowsheet::demo::build_flowsheet()
        .validate()
        .expect("the demo circuit is valid");
    let generated = serde_json::to_string_pretty(&serial::Flowsheet::from(&demo))
        .expect("a document serialises");
    let on_disk =
        std::fs::read_to_string("tests/fixtures/recycle.json").expect("the fixture is readable");

    // The file ends with a newline that `to_string_pretty` does not write.
    assert_eq!(generated, on_disk.trim_end_matches('\n'));
}

/// `stiff_recycle.json` is `recycle.json` with the splitter fraction at 0.9 and nothing else
/// changed. Direct substitution shrinks the error by `f * (1 - r)` a pass - 0.855 for the gangue -
/// so it needs 119 passes against a default cap of 100. That is the whole reason the solver flags
/// exist, and the four tests below are the two halves of it plus the two numeric flags.
#[test]
fn a_stiff_recycle_runs_out_of_passes_under_the_defaults() {
    let run = flowsheet(&["tests/fixtures/stiff_recycle.json"]);
    assert!(!run.ok, "a solve that gave up must not exit zero");
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(
        run.stderr.starts_with("no convergence after 100 passes"),
        "{}",
        run.stderr
    );
}

#[test]
fn wegstein_settles_the_stiff_recycle_the_defaults_give_up_on() {
    let run = flowsheet(&["--method", "wegstein", "tests/fixtures/stiff_recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let last = run.stdout.lines().last().expect("the table has a footer");
    assert!(
        last.trim()
            .starts_with("converged in 11 iterations, residual "),
        "{}",
        last
    );
}

#[test]
fn raising_the_cap_lets_direct_substitution_finish() {
    let run = flowsheet(&[
        "--max-iterations",
        "200",
        "tests/fixtures/stiff_recycle.json",
    ]);
    assert!(run.ok, "{}", run.stderr);

    let last = run.stdout.lines().last().expect("the table has a footer");
    assert!(
        last.trim()
            .starts_with("converged in 119 iterations, residual "),
        "{}",
        last
    );
}

#[test]
fn loosening_the_tolerance_lets_direct_substitution_finish_inside_the_default_cap() {
    let run = flowsheet(&["--tolerance", "1e-6", "tests/fixtures/stiff_recycle.json"]);
    assert!(run.ok, "{}", run.stderr);

    let last = run.stdout.lines().last().expect("the table has a footer");
    assert!(
        last.trim()
            .starts_with("converged in 75 iterations, residual "),
        "{}",
        last
    );
}

/// `impossible_duty.json` is a three-unit chain whose cooler asks for a thousand times the heat
/// the feed holds above absolute zero. Nothing on the JSON boundary can reject it: the duty is a
/// finite number and the flowsheet is wired correctly, so the contradiction only shows up once a
/// stream reaches the unit. Before `UnitOp::evaluate` returned a `Result` that was a panic in the
/// middle of the solve - a number out of a document taking the process down with a backtrace.
#[test]
fn a_duty_with_no_physical_answer_names_the_unit_and_the_pass() {
    let run = flowsheet(&["tests/fixtures/impossible_duty.json"]);
    assert!(
        !run.ok,
        "a solve that could not evaluate must not exit zero"
    );
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(
        run.stderr
            .starts_with("unit 'cooler' failed on pass 1: no positive temperature holds"),
        "{}",
        run.stderr
    );
}

/// `burner.json` preheats methane in oxygen and nitrogen, then burns 90% of it in an isothermal
/// reactor. Neither duty is printed anywhere in the document's streams: the heater's is an input,
/// and the reactor's is the heat of reaction it had to shed, which only `report::duty` computes.
#[test]
fn units_that_take_heat_in_or_give_it_out_are_listed_below_the_footer() {
    let run = flowsheet(&["tests/fixtures/burner.json"]);
    assert!(run.ok, "{}", run.stderr);

    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines[5..],
        [
            "",
            "  unit       type                duty (MJ/h)",
            "  preheater  heater                  20000.0",
            "  burner     conversion_reactor    -449704.1",
        ]
    );

    // The burner's duty by hand, with constant heat capacities so Kirchhoff's law is exact:
    // the heat of reaction at 25 °C, moved to the preheated temperature by the heat capacities
    // the reaction adds. Molar flows are Mmol/h and kJ/mol, so GJ/h, hence the 1000.
    let (m_ch4, m_o2, m_n2) = (16.043, 31.998, 28.014);
    let heat_capacity_flow = 10.0 * 35.7 / m_ch4 + 60.0 * 29.4 / m_o2 + 200.0 * 29.1 / m_n2;
    let preheated = 298.15 + 20000.0 / heat_capacity_flow;
    let at_25c = (-393.52 + 2.0 * -241.83) - (-74.87);
    let delta_cp = ((37.1 + 2.0 * 33.6) - (35.7 + 2.0 * 29.4)) / 1000.0;
    let extent = 0.9 * 10.0 / m_ch4;
    let expected = extent * (at_25c + delta_cp * (preheated - 298.15)) * 1000.0;

    let printed: f64 = lines[8]
        .split_whitespace()
        .last()
        .expect("a duty column")
        .parse()
        .expect("a number");
    assert!(
        (printed - expected).abs() <= 0.05,
        "{printed} vs {expected}"
    );
}

#[test]
fn a_flowsheet_that_only_mixes_and_splits_prints_no_duty_section() {
    // The demo circuit is all at 25 °C, so every duty is float noise and none is shown - not
    // even the header, so the footer is still the last line.
    let run = flowsheet(&["tests/fixtures/recycle.json"]);
    assert!(run.ok, "{}", run.stderr);
    let last = run.stdout.lines().last().expect("output");
    assert!(last.trim().starts_with("converged in"), "{last}");
}

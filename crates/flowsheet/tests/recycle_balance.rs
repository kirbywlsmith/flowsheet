//! Steady-state material balance across the demo flotation circuit.
//!
//! Three invariants, checked per species rather than on totals alone:
//!
//! 1. The two products together equal the feed. Nothing accumulates at steady state, so the
//!    recycle inflates the internal flows and never the plant output.
//! 2. Every internal stream matches [`flowsheet::demo::balance`], the scalar solution worked
//!    out by hand. Each species travels the loop independently, so one equation per species
//!    is the whole answer.
//! 3. The cell actually upgrades the ore: the concentrate is richer in chalcopyrite than the
//!    feed and the tailings are poorer. A splitter could not do that, which is the point of
//!    having replaced one.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::Wegstein;
use flowsheet::demo::{
    Balance, ROUGHER_RECOVERY, RecycleCircuit, balance, feed_stream, recycle_circuit, registry,
};
use flowsheet::{Solver, SolverConfig, Stream, StreamId, ValidFlowsheet};

/// A solved circuit: the flowsheet, plus the ids kept from build time.
struct Solved {
    flowsheet: ValidFlowsheet,
    feed: StreamId,
    mixer_out: StreamId,
    concentrate: StreamId,
    tails: StreamId,
    recycle: StreamId,
    tailings: StreamId,
}

/// Builds and solves the demo circuit at the given splitter `fraction`.
///
/// Uses Wegstein with a floor loose enough to reach the optimal `q` at every fraction tested,
/// so the balance holds to machine precision rather than to the tolerance.
fn solve(fraction: f64) -> Solved {
    // Destructured because `validate` consumes the flowsheet; the `StreamId`s are `Copy`
    // and survive on their own.
    let RecycleCircuit {
        flowsheet,
        feed,
        mixer_out,
        concentrate,
        tails,
        recycle,
        tailings,
    } = recycle_circuit(fraction);

    let mut flowsheet = flowsheet
        .validate()
        .expect("the demo circuit is well-formed at every fraction");

    Solver::new(SolverConfig {
        tolerance: 1e-12,
        max_iterations: 500,
        method: Wegstein {
            q_min: -20.0,
            q_max: 0.0,
        },
    })
    .solve(&mut flowsheet)
    .expect("one tear breaks the only loop");

    Solved {
        flowsheet,
        feed,
        mixer_out,
        concentrate,
        tails,
        recycle,
        tailings,
    }
}

/// The plant feed, 40 / 360 / 600 t/h.
fn feed() -> Stream {
    feed_stream(&registry())
}

/// Asserts that `actual` matches the hand-worked flows, species by species.
///
/// [`balance`] is scalar - one species at a time - so `pick` chooses which stream of that
/// species' solution to read, and the vector is assembled here.
fn assert_matches_balance(
    actual: &Stream,
    fraction: f64,
    pick: impl Fn(Balance) -> f64,
    what: &str,
) {
    let r = registry();
    let flows: Vec<f64> = feed()
        .flows()
        .iter()
        .zip(ROUGHER_RECOVERY)
        .map(|(&f, recovery)| pick(balance(f, recovery, fraction)))
        .collect();
    let expected = Stream::from_flows(&r, flows, actual.temperature(), actual.pressure());

    assert!(
        actual.flows_approx_eq(&expected, 1e-9),
        "at f = {fraction} the {what} should be {:?}, got {:?}",
        expected.flows(),
        actual.flows()
    );
}

/// Every fraction the balance is checked at. 0.9 is the tight loop Wegstein exists for.
const FRACTIONS: [f64; 3] = [0.3, 0.5, 0.9];

#[test]
fn the_two_products_together_match_the_feed_species_by_species() {
    for f in FRACTIONS {
        let c = solve(f);
        let mut out = c.flowsheet[c.concentrate].clone();
        out += &c.flowsheet[c.tailings];
        assert!(
            out.flows_approx_eq(&feed(), 1e-9),
            "at f = {f} concentrate + tailings should be the feed, got {:?}",
            out.flows()
        );
        assert!(c.flowsheet[c.feed].flows_approx_eq(&feed(), 1e-12));
    }
}

#[test]
fn every_internal_stream_matches_the_hand_balance() {
    for f in FRACTIONS {
        let c = solve(f);
        let fs = &c.flowsheet;

        assert_matches_balance(&fs[c.mixer_out], f, |b| b.mixer_out, "mixer outlet");
        assert_matches_balance(&fs[c.concentrate], f, |b| b.concentrate, "concentrate");
        assert_matches_balance(&fs[c.tails], f, |b| b.tails, "cell tails");
        assert_matches_balance(&fs[c.recycle], f, |b| b.recycle, "recycle");
        assert_matches_balance(&fs[c.tailings], f, |b| b.tailings, "tailings");
    }
}

#[test]
fn the_cell_upgrades_the_concentrate_and_strips_the_tailings() {
    // What a splitter could never do: the streams leaving no longer carry the feed's
    // composition.
    for f in FRACTIONS {
        let c = solve(f);
        let feed_grade = feed().mass_fractions()[0];

        assert!(
            c.flowsheet[c.concentrate].mass_fractions()[0] > feed_grade,
            "at f = {f} the concentrate should be richer than the feed"
        );
        assert!(
            c.flowsheet[c.tailings].mass_fractions()[0] < feed_grade,
            "at f = {f} the tailings should be poorer than the feed"
        );
    }
}

#[test]
fn recycling_more_of_the_tails_recovers_more_of_the_mineral() {
    // Why the recycle is there at all: a particle that fails to float gets another pass.
    let mut previous = 0.0;
    for f in [0.0, 0.3, 0.5, 0.9] {
        let c = solve(f);
        let recovered = c.flowsheet[c.concentrate].flows()[0];
        assert!(
            recovered > previous,
            "recovery to concentrate should rise with f, but f = {f} gave {recovered}"
        );
        previous = recovered;
    }
}

#[test]
fn every_unit_closes_its_own_balance() {
    for f in FRACTIONS {
        let c = solve(f);
        let fs = &c.flowsheet;

        // Mixer: feed + recycle in, circulating load out.
        let mut mixer_in = fs[c.feed].clone();
        mixer_in += &fs[c.recycle];
        assert!(
            mixer_in.flows_approx_eq(&fs[c.mixer_out], 1e-9),
            "at f = {f} the mixer should conserve every species"
        );

        // Flotation: circulating load in, concentrate + tails out.
        let mut cell_out = fs[c.concentrate].clone();
        cell_out += &fs[c.tails];
        assert!(
            cell_out.flows_approx_eq(&fs[c.mixer_out], 1e-9),
            "at f = {f} the cell should conserve every species"
        );

        // Splitter: cell tails in, recycle + tailings out.
        let mut split_out = fs[c.recycle].clone();
        split_out += &fs[c.tailings];
        assert!(
            split_out.flows_approx_eq(&fs[c.tails], 1e-9),
            "at f = {f} the splitter should conserve every species"
        );
    }
}

#[test]
fn the_demo_circuit_matches_the_hand_worked_numbers() {
    // The target table in `flowsheet-cli/src/main.rs`, at the documented f = 0.3.
    let c = solve(0.3);
    let fs = &c.flowsheet;

    assert_relative_eq!(fs[c.mixer_out].total(), 1304.874991, epsilon = 1e-6);
    assert_relative_eq!(fs[c.concentrate].total(), 288.625020, epsilon = 1e-6);
    assert_relative_eq!(fs[c.tails].total(), 1016.249971, epsilon = 1e-6);
    assert_relative_eq!(fs[c.recycle].total(), 304.874991, epsilon = 1e-6);
    assert_relative_eq!(fs[c.tailings].total(), 711.374980, epsilon = 1e-6);

    let concentrate = fs[c.concentrate].flows();
    assert_relative_eq!(concentrate[0], 35.602094, epsilon = 1e-6);
    assert_relative_eq!(concentrate[1], 25.174825, epsilon = 1e-6);
    assert_relative_eq!(concentrate[2], 227.848101, epsilon = 1e-6);

    // 4.00% chalcopyrite in the feed, 12.33% in the concentrate.
    assert_relative_eq!(feed().mass_fractions()[0], 0.04, epsilon = 1e-12);
    assert_relative_eq!(
        fs[c.concentrate].mass_fractions()[0],
        0.123350,
        epsilon = 1e-6
    );
}

#[test]
fn a_circuit_with_no_recycle_is_still_balanced() {
    // f = 0 makes the loop carry nothing, so the tear converges on the second pass and the
    // cell sees the feed itself rather than an inflated circulating load.
    let c = solve(0.0);
    assert!(c.flowsheet[c.mixer_out].flows_approx_eq(&feed(), 1e-12));
    assert_relative_eq!(c.flowsheet[c.recycle].total(), 0.0, epsilon = 1e-12);
    assert_relative_eq!(
        c.flowsheet[c.concentrate].flows()[0],
        34.0,
        max_relative = 1e-12
    );
}

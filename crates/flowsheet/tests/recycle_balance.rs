//! Steady-state material balance across the demo recycle circuit.
//!
//! Two invariants, checked per species rather than on totals alone:
//!
//! 1. The product equals the feed. Nothing accumulates at steady state, so the recycle
//!    inflates the internal flows and never the plant output.
//! 2. The circulating load is amplified by `1 / (1 - f)`, which also fixes the recycle
//!    itself at `f / (1 - f)` of the feed.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::Wegstein;
use flowsheet::demo::{RecycleCircuit, feed_stream, recycle_circuit, registry};
use flowsheet::{Solver, SolverConfig, Stream, StreamId, ValidFlowsheet};

/// A solved circuit: the flowsheet, plus the ids kept from build time.
struct Solved {
    flowsheet: ValidFlowsheet,
    feed: StreamId,
    mixer_out: StreamId,
    tank_out: StreamId,
    recycle: StreamId,
    product: StreamId,
}

/// Builds and solves the demo circuit at the given splitter `fraction`.
///
/// Uses Wegstein with a floor loose enough to reach `q = f / (f - 1)` at every fraction
/// tested, so the balance holds to machine precision rather than to the tolerance.
fn solve(fraction: f64) -> Solved {
    // Destructured because `validate` consumes the flowsheet; the `StreamId`s are `Copy`
    // and survive on their own.
    let RecycleCircuit {
        flowsheet,
        feed,
        mixer_out,
        tank_out,
        recycle,
        product,
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
        tank_out,
        recycle,
        product,
    }
}

/// The plant feed, 40 / 360 / 600 t/h.
fn feed() -> Stream {
    feed_stream(&registry())
}

/// Every fraction the balance is checked at. 0.9 is the tight loop Wegstein exists for.
const FRACTIONS: [f64; 3] = [0.3, 0.5, 0.9];

#[test]
fn the_product_matches_the_feed_species_by_species() {
    for f in FRACTIONS {
        let c = solve(f);
        assert!(
            c.flowsheet[c.product].flows_approx_eq(&feed(), 1e-9),
            "at f = {f} the product should be the feed, got {:?}",
            c.flowsheet[c.product].flows()
        );
        assert!(c.flowsheet[c.feed].flows_approx_eq(&feed(), 1e-12));
    }
}

#[test]
fn the_circulating_load_is_the_feed_amplified_by_one_over_one_minus_f() {
    for f in FRACTIONS {
        let c = solve(f);
        let expected = feed().scaled(1.0 / (1.0 - f));
        assert!(
            c.flowsheet[c.mixer_out].flows_approx_eq(&expected, 1e-9),
            "at f = {f} the mixer outlet should be {:?}, got {:?}",
            expected.flows(),
            c.flowsheet[c.mixer_out].flows()
        );
    }
}

#[test]
fn the_recycle_carries_f_over_one_minus_f_of_the_feed() {
    for f in FRACTIONS {
        let c = solve(f);
        let expected = feed().scaled(f / (1.0 - f));
        assert!(
            c.flowsheet[c.recycle].flows_approx_eq(&expected, 1e-9),
            "at f = {f} the recycle should be {:?}, got {:?}",
            expected.flows(),
            c.flowsheet[c.recycle].flows()
        );
    }
}

#[test]
fn the_tank_passes_the_circulating_load_straight_through() {
    for f in FRACTIONS {
        let c = solve(f);
        let mixer_out = c.flowsheet[c.mixer_out].clone();
        assert!(
            c.flowsheet[c.tank_out].flows_approx_eq(&mixer_out, 1e-12),
            "at f = {f} the tank outlet should equal its inlet"
        );
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

        // Splitter: circulating load in, recycle + product out.
        let mut split_out = fs[c.recycle].clone();
        split_out += &fs[c.product];
        assert!(
            split_out.flows_approx_eq(&fs[c.tank_out], 1e-9),
            "at f = {f} the splitter should conserve every species"
        );
    }
}

#[test]
fn the_demo_circuit_matches_the_hand_worked_numbers() {
    // The target table in `src/main.rs`, at the documented f = 0.3.
    let c = solve(0.3);

    assert_relative_eq!(
        c.flowsheet[c.mixer_out].total(),
        1428.571429,
        epsilon = 1e-6
    );
    assert_relative_eq!(c.flowsheet[c.recycle].total(), 428.571429, epsilon = 1e-6);
    assert_relative_eq!(c.flowsheet[c.product].total(), 1000.0, epsilon = 1e-9);

    let recycle = c.flowsheet[c.recycle].flows();
    assert_relative_eq!(recycle[0], 17.142857, epsilon = 1e-6);
    assert_relative_eq!(recycle[1], 154.285714, epsilon = 1e-6);
    assert_relative_eq!(recycle[2], 257.142857, epsilon = 1e-6);
}

#[test]
fn a_circuit_with_no_recycle_is_still_balanced() {
    // f = 0 makes the loop carry nothing, so the tear converges on the second pass.
    let c = solve(0.0);
    assert!(c.flowsheet[c.mixer_out].flows_approx_eq(&feed(), 1e-12));
    assert_relative_eq!(c.flowsheet[c.recycle].total(), 0.0, epsilon = 1e-12);
}

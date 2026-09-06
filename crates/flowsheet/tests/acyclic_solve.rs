//! End-to-end solves of the demo circuit with the recycle removed.
//!
//! The flowsheet is built here rather than taken from [`flowsheet::demo`]
//! so the tests keep the [`StreamId`]s that `add_stream` hands back — there is no
//! way to recover them from a built [`Flowsheet`].
//!
//! ```text
//!   Feed ──S0──▶ Mixer ──S1──▶ Tank ──S2──▶ Splitter ──S3──▶ Bleed    (f)
//!                                                   └───S4──▶ Product (1 - f)
//! ```
//!
//! The looped version lives in `recycle_balance.rs`, built from
//! [`flowsheet::demo::recycle_circuit`].

use approx::assert_relative_eq;
use flowsheet::demo::{AMBIENT_K, AMBIENT_KPA, feed_stream, registry};
use flowsheet::{Flowsheet, FlowsheetError, Solver, Stream, StreamId, UnitOp, ValidFlowsheet};

/// A built flowsheet plus the stream ids needed to inspect the solved result.
struct Circuit {
    flowsheet: ValidFlowsheet,
    feed: StreamId,
    mixer_out: StreamId,
    tank_out: StreamId,
    bleed: StreamId,
    product: StreamId,
}

/// Builds the acyclic circuit above, with the splitter sending `fraction` to the bleed.
fn build(fraction: f64) -> Circuit {
    let r = registry();

    // Both are built while `r` is still owned here, before it moves into the flowsheet.
    let feed_stream = feed_stream(&r);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(r);

    let u_feed = fs.add_unit(
        "feed",
        UnitOp::Feed {
            stream: feed_stream,
        },
    );
    let u_mixer = fs.add_unit("mixer", UnitOp::Mixer);
    let u_tank = fs.add_unit("tank", UnitOp::Tank);
    let u_split = fs.add_unit("split", UnitOp::Splitter { fraction });
    let u_bleed = fs.add_unit("bleed", UnitOp::Product);
    let u_product = fs.add_unit("product", UnitOp::Product);

    let feed = fs.add_stream(u_feed, blank.clone(), u_mixer);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_tank);
    let tank_out = fs.add_stream(u_tank, blank.clone(), u_split);

    // Outlet order is positional: the first stream out of the splitter gets `fraction`.
    let bleed = fs.add_stream(u_split, blank.clone(), u_bleed);
    let product = fs.add_stream(u_split, blank, u_product);

    Circuit {
        flowsheet: fs.validate().expect("the circuit above is correctly wired"),
        feed,
        mixer_out,
        tank_out,
        bleed,
        product,
    }
}

/// The unsolved feed stream, for comparing against solved results.
fn expected_feed() -> Stream {
    let r = registry();
    feed_stream(&r)
}

#[test]
fn acyclic_solve_converges_in_one_pass() {
    let mut circuit = build(0.3);

    let report = Solver::default()
        .solve(&mut circuit.flowsheet)
        .expect("acyclic flowsheet should solve");

    assert_eq!(report.iterations, 1, "no tears, so one pass is exact");
    assert_relative_eq!(report.residual, 0.0, epsilon = 1e-12);
}

#[test]
fn feed_propagates_unchanged_to_the_splitter() {
    let mut circuit = build(0.3);
    Solver::default().solve(&mut circuit.flowsheet).unwrap();

    let feed = expected_feed();

    // A one-inlet mixer and a pass-through tank must not alter anything.
    for id in [circuit.feed, circuit.mixer_out, circuit.tank_out] {
        assert!(
            circuit.flowsheet[id].flows_approx_eq(&feed, 1e-9),
            "stream {id:?} should still be the feed, got {:?}",
            circuit.flowsheet[id]
        );
    }
}

#[test]
fn splitter_outlets_follow_the_fraction() {
    let mut circuit = build(0.3);
    Solver::default().solve(&mut circuit.flowsheet).unwrap();

    let feed = expected_feed();

    assert!(
        circuit.flowsheet[circuit.bleed].flows_approx_eq(&feed.scaled(0.3), 1e-9),
        "first splitter outlet should carry the fraction"
    );
    assert!(
        circuit.flowsheet[circuit.product].flows_approx_eq(&feed.scaled(0.7), 1e-9),
        "second splitter outlet should carry the remainder"
    );
}

#[test]
fn mass_is_conserved_end_to_end() {
    let mut circuit = build(0.3);
    Solver::default().solve(&mut circuit.flowsheet).unwrap();

    let inlet = circuit.flowsheet[circuit.feed].total();
    let outlet =
        circuit.flowsheet[circuit.bleed].total() + circuit.flowsheet[circuit.product].total();

    assert_relative_eq!(outlet, inlet, epsilon = 1e-9);
    assert_relative_eq!(inlet, 1000.0, epsilon = 1e-9);
}

#[test]
fn solving_twice_gives_the_same_answer() {
    let mut circuit = build(0.3);

    Solver::default().solve(&mut circuit.flowsheet).unwrap();
    let once = circuit.flowsheet[circuit.product].clone();

    Solver::default().solve(&mut circuit.flowsheet).unwrap();

    assert!(
        circuit.flowsheet[circuit.product].flows_approx_eq(&once, 1e-12),
        "a solved flowsheet is a fixed point of the solver"
    );
}

#[test]
fn a_miswired_flowsheet_never_reaches_the_solver() {
    // The splitter's second outlet is missing, so `evaluate` would produce two streams
    // for one wired outlet and the extra would be dropped on the floor.
    let r = registry();
    let feed = feed_stream(&r);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(r);

    let u_feed = fs.add_unit("feed", UnitOp::Feed { stream: feed });
    let u_split = fs.add_unit("split", UnitOp::Splitter { fraction: 0.3 });
    let u_product = fs.add_unit("product", UnitOp::Product);

    fs.add_stream(u_feed, blank.clone(), u_split);
    fs.add_stream(u_split, blank, u_product);

    let errors = fs.validate().expect_err("the splitter is one outlet short");

    assert_eq!(
        errors,
        vec![FlowsheetError::WrongOutletCount {
            unit: u_split,
            min: 2,
            max: Some(2),
            found: 1,
        }]
    );
}

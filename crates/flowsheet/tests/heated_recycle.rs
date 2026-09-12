//! Steady-state energy balance around a recycle loop with a heater in it.
//!
//! ```text
//!   feed ──▶ Mixer ──▶ Heater ──▶ Splitter ──▶ Product
//!             ▲                      │ recycle
//!             └──────────────────────┘
//! ```
//!
//! The splitter changes neither composition nor temperature, so at steady state the product
//! carries the feed's flows at the heater outlet's temperature. The duty is the only heat that
//! crosses the flowsheet boundary, so:
//!
//! 1. The product carries the feed's enthalpy plus the duty, for any recycle fraction.
//! 2. With constant heat capacity that fixes the product temperature at `T_feed + Q / C_feed`,
//!    and the mixer outlet sits below it by `Q / C_loop`, where `C_loop = C_feed / (1 - f)` is
//!    the heat capacity flow the loop carries. This is the energy counterpart of `demo::balance`.
//! 3. With no duty, every stream stays at the feed temperature and the flows alone set the pass
//!    count. With one, temperature has to converge too.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::{self, DirectSubstitution, Wegstein};
use flowsheet::demo::{self, AMBIENT_K, AMBIENT_KPA};
use flowsheet::unit::{Feed, Heater, Mixer, Product, Splitter};
use flowsheet::{
    Flowsheet, SolveError, SolveReport, Solver, SolverConfig, Stream, StreamId, ValidFlowsheet,
};

/// The fraction of the heater outlet sent back round to the mixer.
const RECYCLE: f64 = 0.3;

/// A recycle at which direct substitution needs 171 passes, past the default cap of 100.
const HIGH_RECYCLE: f64 = 0.9;

const WEGSTEIN: ConvergenceMethod = Wegstein {
    q_min: -5.0,
    q_max: 0.0,
};

struct Solved {
    flowsheet: ValidFlowsheet,
    report: SolveReport,
    mixer_out: StreamId,
    heater_out: StreamId,
    recycle: StreamId,
    product: StreamId,
}

fn solve(feed: &Stream, duty: f64, recycle_fraction: f64, method: ConvergenceMethod) -> Solved {
    try_solve(feed, duty, recycle_fraction, method).expect("the loop converges")
}

fn try_solve(
    feed: &Stream,
    duty: f64,
    recycle_fraction: f64,
    method: ConvergenceMethod,
) -> Result<Solved, SolveError> {
    let r = demo::registry();
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(demo::registry());
    let u_feed = fs.add_unit(
        "feed",
        Feed {
            stream: feed.clone(),
        },
    );
    let u_mixer = fs.add_unit("mixer", Mixer);
    let u_heater = fs.add_unit("heater", Heater { duty });
    let u_split = fs.add_unit(
        "split",
        Splitter {
            fraction: recycle_fraction,
        },
    );
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_heater);
    let heater_out = fs.add_stream(u_heater, blank.clone(), u_split);
    // Outlet order is positional: the splitter's first outlet takes the recycle fraction. This
    // is the loop's back edge, so it is the stream the solver tears; the mixer therefore runs
    // before the heater on every pass, with the tear's all-zero guess on the first.
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);
    let product = fs.add_stream(u_split, blank, u_product);

    let mut flowsheet = fs.validate().expect("the loop is wired correctly");
    let report = Solver::new(SolverConfig {
        max_iterations: 500,
        method,
        ..SolverConfig::default()
    })
    .solve(&mut flowsheet)?;

    Ok(Solved {
        flowsheet,
        report,
        mixer_out,
        heater_out,
        recycle,
        product,
    })
}

#[test]
fn the_product_carries_the_feed_enthalpy_plus_the_duty() {
    let r = demo::registry();
    let feed = demo::feed_stream(&r);
    let duty = 50_000.0;

    let s = solve(&feed, duty, RECYCLE, DirectSubstitution);

    assert_relative_eq!(
        s.flowsheet[s.product].enthalpy(&r),
        feed.enthalpy(&r) + duty,
        max_relative = 1e-6
    );
}

/// Solves a feed of chalcopyrite alone and checks every loop temperature against the hand balance.
/// Chalcopyrite is the demo's one constant-cp species, at 95.0 J/(mol·K) and 183.5 g/mol.
fn assert_hand_balance(recycle_fraction: f64, method: ConvergenceMethod) {
    let r = demo::registry();
    let feed = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);
    let c_feed = 100.0 * 95.0 / 183.5; // MJ/(h·K)
    let c_loop = c_feed / (1.0 - recycle_fraction);
    let duty = 2_000.0;

    let s = solve(&feed, duty, recycle_fraction, method);

    let hot = AMBIENT_K + duty / c_feed;
    for id in [s.heater_out, s.recycle, s.product] {
        assert_relative_eq!(s.flowsheet[id].temperature(), hot, max_relative = 1e-6);
    }
    assert_relative_eq!(
        s.flowsheet[s.mixer_out].temperature(),
        hot - duty / c_loop,
        max_relative = 1e-6
    );
}

#[test]
fn a_constant_heat_capacity_loop_settles_where_the_hand_balance_says() {
    assert_hand_balance(RECYCLE, DirectSubstitution);
}

#[test]
fn wegstein_extrapolating_temperature_still_settles_where_the_hand_balance_says() {
    assert_hand_balance(HIGH_RECYCLE, WEGSTEIN);
}

#[test]
fn a_heated_high_recycle_takes_171_passes_by_direct_substitution() {
    let r = demo::registry();
    let s = solve(
        &demo::feed_stream(&r),
        50_000.0,
        HIGH_RECYCLE,
        DirectSubstitution,
    );
    assert_eq!(s.report.iterations, 171);
}

#[test]
fn wegstein_cuts_a_heated_high_recycle_to_22_passes() {
    // The flows settle by pass 22. If Wegstein extrapolated only the flows, temperature would
    // keep contracting by the recycle fraction each pass and hold the solve open until pass 142.
    let r = demo::registry();
    let s = solve(&demo::feed_stream(&r), 50_000.0, HIGH_RECYCLE, WEGSTEIN);
    assert_eq!(s.report.iterations, 22);
}

#[test]
fn under_wegstein_the_duty_adds_no_passes_to_the_flows_alone() {
    // With no duty every stream stays at the feed temperature, so the pass count is the flows'.
    let r = demo::registry();
    let feed = demo::feed_stream(&r);

    let heated = solve(&feed, 50_000.0, HIGH_RECYCLE, WEGSTEIN);
    let isothermal = solve(&feed, 0.0, HIGH_RECYCLE, WEGSTEIN);

    assert_eq!(heated.report.iterations, isothermal.report.iterations);
}

#[test]
fn an_impossible_duty_fails_the_solve_and_names_the_unit() {
    // The reason `UnitOp::evaluate` returns a `Result`. Until it did, a duty out of a document
    // that no positive temperature could absorb panicked in the middle of a solve.
    //
    // -1e9 MJ/h is over a thousand times what it takes to bring this feed to 0 K, so the loop
    // has no steady state to converge to and no pass can get partway there.
    let r = demo::registry();

    // `let Err(e) = .. else` rather than `expect_err`, which would need `Solved: Debug`.
    let Err(e) = try_solve(&demo::feed_stream(&r), -1e9, RECYCLE, DirectSubstitution) else {
        panic!("no positive temperature absorbs -1e9 MJ/h");
    };

    let SolveError::Evaluation {
        name, iteration, ..
    } = &e
    else {
        panic!("expected an evaluation failure, got {e}");
    };
    assert_eq!(name, "heater");
    // Pass 1: the tear is the recycle (the back edge), so the mixer still runs before the
    // heater on every pass and the heater has the feed's flow to fail on immediately.
    assert_eq!(*iteration, 1);
    assert!(
        e.to_string()
            .starts_with("unit 'heater' failed on pass 1: "),
        "{e}"
    );
}

#[test]
fn a_duty_that_barely_works_still_solves() {
    // The other side of the line above: this duty takes the product to within 10 K of absolute
    // zero and still converges, so the error is not a conservative guard that rejects hard but
    // legal problems.
    //
    // Chalcopyrite alone, so cp is constant: C_feed = 100 * 95.0 / 183.5 MJ/(h·K), and cooling
    // the feed from 298.15 K to 10 K takes that times 288.15.
    let r = demo::registry();
    let feed = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);
    let c_feed = 100.0 * 95.0 / 183.5;

    let s = solve(
        &feed,
        -(AMBIENT_K - 10.0) * c_feed,
        RECYCLE,
        DirectSubstitution,
    );

    assert_relative_eq!(
        s.flowsheet[s.product].temperature(),
        10.0,
        max_relative = 1e-6
    );
}

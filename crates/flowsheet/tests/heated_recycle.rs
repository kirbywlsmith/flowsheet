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

use approx::assert_relative_eq;
use flowsheet::demo::{self, AMBIENT_K, AMBIENT_KPA};
use flowsheet::unit::{Feed, Heater, Mixer, Product, Splitter};
use flowsheet::{Flowsheet, Solver, Stream, StreamId, ValidFlowsheet};

/// The fraction of the heater outlet sent back round to the mixer.
const RECYCLE: f64 = 0.3;

struct Solved {
    flowsheet: ValidFlowsheet,
    mixer_out: StreamId,
    heater_out: StreamId,
    recycle: StreamId,
    product: StreamId,
}

fn solve(feed: &Stream, duty: f64) -> Solved {
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
    let u_split = fs.add_unit("split", Splitter { fraction: RECYCLE });
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    // The lowest-id stream inside the loop, so it is the tear: on the first pass the heater sees
    // its all-zero initial guess, which is the empty-inlet case `unit::heat` passes through.
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_heater);
    let heater_out = fs.add_stream(u_heater, blank.clone(), u_split);
    // Outlet order is positional: the splitter's first outlet takes `RECYCLE`.
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);
    let product = fs.add_stream(u_split, blank, u_product);

    let mut flowsheet = fs.validate().expect("the loop is wired correctly");
    Solver::default()
        .solve(&mut flowsheet)
        .expect("the loop converges");

    Solved {
        flowsheet,
        mixer_out,
        heater_out,
        recycle,
        product,
    }
}

#[test]
fn the_product_carries_the_feed_enthalpy_plus_the_duty() {
    let r = demo::registry();
    let feed = demo::feed_stream(&r);
    let duty = 50_000.0;

    let s = solve(&feed, duty);

    assert_relative_eq!(
        s.flowsheet[s.product].enthalpy(&r),
        feed.enthalpy(&r) + duty,
        max_relative = 1e-6
    );
}

#[test]
fn a_constant_heat_capacity_loop_settles_where_the_hand_balance_says() {
    // Chalcopyrite alone: the demo's one constant-cp species, at 95.0 J/(mol·K) and 183.5 g/mol.
    let r = demo::registry();
    let feed = Stream::from_flows(&r, vec![100.0, 0.0, 0.0], AMBIENT_K, AMBIENT_KPA);
    let c_feed = 100.0 * 95.0 / 183.5; // MJ/(h·K)
    let c_loop = c_feed / (1.0 - RECYCLE);
    let duty = 2_000.0;

    let s = solve(&feed, duty);

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

//! A pump or a compressor closing a recycle that loses pressure.
//!
//! ```text
//!   feed ──▶ Mixer ──▶ Splitter ──▶ Product
//!             ▲  (drop)   │ recycle
//!             └── Pump ◀──┘
//! ```
//!
//! Without the pump this loop has no steady state: the mixer takes the recycle's pressure, the
//! drop lowers it again, and every pass the loop sits lower than the last (`tests/pressure.rs`).
//! With a pump that puts back exactly the drop, the recycle comes round at the feed pressure and
//! the mixer sees one pressure on both inlets. A pump that puts back less still fails, on the
//! same pass a loop with the difference as its drop would; one that puts back more overshoots,
//! and the mixer takes the lower of its two inlets, which is now the feed.
//!
//! The energy side is the boundary balance from `tests/heated_recycle.rs` with the pump as the
//! heater: the product carries the feed's enthalpy plus whatever the pump added. For a pump that
//! is the friction, `W (1 - eta)`; for a compressor it is the whole shaft work, because an ideal
//! gas's enthalpy has no pressure term to hide any of it in.

use approx::assert_relative_eq;
use flowsheet::demo::{self, AMBIENT_K, AMBIENT_KPA};
use flowsheet::unit::{Compressor, Feed, Mixer, Product, Pump};
use flowsheet::{
    Flowsheet, Phase, Shomate, SolveError, Solver, SolverConfig, Species, SpeciesRegistry, Stream,
    StreamId, UnitId, ValidFlowsheet, report,
};

const FEED_KPA: f64 = 500.0;
const DROP: f64 = 10.0;
const RECYCLE: f64 = 0.3;

/// The demo slurry with densities, kg/m³, so a pump can find its volume.
fn slurry_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (species, density) in demo::registry().all().iter().zip([4200.0, 2650.0, 997.0]) {
        r.insert(Species {
            density: Some(density),
            ..species.clone()
        });
    }
    r
}

/// Methane and nitrogen as gases at constant cp, for the compressor.
fn gas_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (name, molar_mass, cp) in [("CH4", 16.043, 35.7), ("N2", 28.014, 29.1)] {
        r.insert(Species {
            name: name.into(),
            phase: Phase::Gas,
            molar_mass,
            shomate: Shomate::constant(cp),
            enthalpy_of_formation: None,
            density: None,
        });
    }
    r
}

struct Solved {
    flowsheet: ValidFlowsheet,
    machine: UnitId,
    mixer_out: StreamId,
    recycle: StreamId,
    product: StreamId,
}

/// The loop in the module docs, with `machine` on the recycle branch and `feed` at [`FEED_KPA`].
fn solve(
    registry: SpeciesRegistry,
    feed: Stream,
    machine: impl Into<Box<dyn flowsheet::UnitOp>>,
) -> Result<Solved, SolveError> {
    let blank = Stream::zeros(&registry, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(registry);
    let u_feed = fs.add_unit("feed", Feed { stream: feed });
    let u_mixer = fs.add_unit(
        "mixer",
        Mixer {
            pressure_drop: DROP,
        },
    );
    let u_split = fs.add_unit(
        "split",
        flowsheet::Splitter {
            fraction: RECYCLE,
            pressure_drop: 0.0,
        },
    );
    let u_machine = fs.add_unit("machine", machine);
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_split);
    fs.add_stream(u_split, blank.clone(), u_machine);
    let recycle = fs.add_stream(u_machine, blank.clone(), u_mixer);
    let product = fs.add_stream(u_split, blank, u_product);

    let mut flowsheet = fs.validate().expect("the loop is wired correctly");
    Solver::new(SolverConfig {
        max_iterations: 1000,
        ..SolverConfig::default()
    })
    .solve(&mut flowsheet)?;

    Ok(Solved {
        flowsheet,
        machine: u_machine,
        mixer_out,
        recycle,
        product,
    })
}

fn pumped(rise: f64, efficiency: f64) -> Result<Solved, SolveError> {
    let r = slurry_registry();
    let feed = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, FEED_KPA);
    solve(
        r,
        feed,
        Pump {
            pressure_rise: rise,
            efficiency,
        },
    )
}

fn compressed(rise: f64, efficiency: f64) -> Result<Solved, SolveError> {
    let r = gas_registry();
    let feed = Stream::from_flows(&r, vec![2.0, 70.0], AMBIENT_K, FEED_KPA);
    solve(
        r,
        feed,
        Compressor {
            pressure_rise: rise,
            efficiency,
        },
    )
}

#[test]
fn a_pump_that_puts_back_the_drop_holds_the_loop_at_the_feed_pressure() {
    let s = pumped(DROP, 0.7).expect("the pump closes the loop");

    assert_relative_eq!(s.flowsheet[s.recycle].pressure(), FEED_KPA);
    assert_relative_eq!(s.flowsheet[s.mixer_out].pressure(), FEED_KPA - DROP);
    assert_relative_eq!(s.flowsheet[s.product].pressure(), FEED_KPA - DROP);
}

#[test]
fn a_pump_that_overshoots_is_ignored_by_the_mixer() {
    // The recycle comes round above the feed, and the mixer takes the lower of the two.
    let s = pumped(2.0 * DROP, 0.7).expect("the pump more than closes the loop");

    assert_relative_eq!(s.flowsheet[s.recycle].pressure(), FEED_KPA + DROP);
    assert_relative_eq!(s.flowsheet[s.mixer_out].pressure(), FEED_KPA - DROP);
}

#[test]
fn a_pump_that_does_not_keep_up_still_has_no_steady_state() {
    // Half the drop back is a loop that falls by the other half every pass: the mixer's inlet
    // is 500 - 5 (k - 1) kPa on pass k, it takes 10 off, and on pass 99 that reaches 0 kPa. The
    // same arithmetic as `tests/pressure.rs`, where a bare 10 kPa loop fails on pass 50.
    let Err(e) = pumped(DROP / 2.0, 0.7) else {
        panic!("a pump that puts back half the drop does not close the loop");
    };

    let SolveError::Evaluation {
        name, iteration, ..
    } = &e
    else {
        panic!("expected the mixer to fail, got {e}");
    };
    assert_eq!(name, "mixer");
    assert_eq!(*iteration, 99);
}

#[test]
fn the_product_carries_the_feed_enthalpy_plus_the_heat_the_pump_wasted() {
    let r = slurry_registry();
    let feed = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, FEED_KPA);
    let s = pumped(DROP, 0.7).unwrap();

    // The pump's duty is the friction it turned into heat, `W (1 - eta)` on the recycle it
    // moved, and the boundary balance puts all of it in the product.
    let duty = report::duty(&s.flowsheet, s.machine);
    let recycle = &s.flowsheet[s.recycle];
    let work = flowsheet::unit::pump_work(&r, recycle, DROP, 0.7).unwrap();
    assert_relative_eq!(duty, work * 0.3, max_relative = 1e-6);
    assert!(duty > 0.0);
    assert_relative_eq!(
        s.flowsheet[s.product].enthalpy(&r),
        feed.enthalpy(&r) + duty,
        max_relative = 1e-6
    );
    assert!(s.flowsheet[s.product].temperature() > AMBIENT_K);
}

#[test]
fn an_ideal_pump_leaves_the_loop_at_the_feed_temperature() {
    let s = pumped(DROP, 1.0).unwrap();

    for stream in s.flowsheet.streams() {
        assert_relative_eq!(stream.temperature(), AMBIENT_K, epsilon = 1e-9);
    }
}

#[test]
fn a_compressor_closes_a_gas_loop_and_its_whole_work_reaches_the_product() {
    let r = gas_registry();
    let feed = Stream::from_flows(&r, vec![2.0, 70.0], AMBIENT_K, FEED_KPA);
    let s = compressed(DROP, 0.8).expect("the compressor closes the loop");

    assert_relative_eq!(s.flowsheet[s.recycle].pressure(), FEED_KPA);
    assert_relative_eq!(s.flowsheet[s.mixer_out].pressure(), FEED_KPA - DROP);

    // Unlike the pump, every joule of shaft work is enthalpy the gas carries away, so the
    // compressor's duty is its power and the product carries all of it.
    let duty = report::duty(&s.flowsheet, s.machine);
    assert!(duty > 0.0);
    assert_relative_eq!(
        s.flowsheet[s.product].enthalpy(&r),
        feed.enthalpy(&r) + duty,
        max_relative = 1e-6
    );
    assert!(s.flowsheet[s.recycle].temperature() > AMBIENT_K);
}

#[test]
fn a_compressor_that_does_not_keep_up_fails_the_same_way_a_pump_does() {
    let Err(e) = compressed(DROP / 2.0, 0.8) else {
        panic!("half the drop back does not close the loop");
    };
    let SolveError::Evaluation { name, .. } = &e else {
        panic!("expected the mixer to fail, got {e}");
    };
    assert_eq!(name, "mixer");
}

//! Pressure declared per unit and carried down the flowsheet.
//!
//! Nothing solves pressure. Each unit takes its `pressure_drop` off the pressure that reached
//! it, and a mixer starts from the lowest of its flowing inlets, so along a straight line the
//! drops simply add up. Around a recycle they do not add up to anything: the recycle re-enters
//! the mixer below the feed, so the mixer takes the recycle's pressure, the loop lowers it again,
//! and there is no steady state. That is what a pump in the loop is for (`tests/pumped_recycle.rs`),
//! and a loop with a drop and no pump has to fail rather than converge on a wrong number.

use approx::assert_relative_eq;
use flowsheet::demo::{self, AMBIENT_K};
use flowsheet::unit::{Feed, Heater, Mixer, Product, Splitter};
use flowsheet::{Flowsheet, SolveError, Solver, SolverConfig, Stream, StreamId, ValidFlowsheet};

const FEED_KPA: f64 = 500.0;

/// `feed -> heater -> splitter -> product`, with the splitter's second outlet also a product.
fn line(heater_drop: f64, splitter_drop: f64) -> (ValidFlowsheet, StreamId, StreamId) {
    let r = demo::registry();
    let blank = Stream::zeros(&r, AMBIENT_K, 101.325);
    let feed = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, FEED_KPA);

    let mut fs = Flowsheet::new(demo::registry());
    let u_feed = fs.add_unit("feed", Feed { stream: feed });
    let u_heater = fs.add_unit(
        "heater",
        Heater {
            duty: 1000.0,
            pressure_drop: heater_drop,
        },
    );
    let u_split = fs.add_unit(
        "split",
        Splitter {
            fraction: 0.3,
            pressure_drop: splitter_drop,
        },
    );
    let u_a = fs.add_unit("a", Product);
    let u_b = fs.add_unit("b", Product);

    fs.add_stream(u_feed, blank.clone(), u_heater);
    let heated = fs.add_stream(u_heater, blank.clone(), u_split);
    let a = fs.add_stream(u_split, blank.clone(), u_a);
    fs.add_stream(u_split, blank, u_b);

    let mut fs = fs.validate().expect("the line is wired correctly");
    Solver::default().solve(&mut fs).expect("a line converges");
    (fs, heated, a)
}

/// The demo recycle - `feed -> mixer -> splitter`, first outlet back to the mixer - with a drop
/// on the mixer, fed at [`FEED_KPA`].
fn loop_with(mixer_drop: f64) -> Result<(ValidFlowsheet, StreamId), SolveError> {
    let r = demo::registry();
    let blank = Stream::zeros(&r, AMBIENT_K, 101.325);
    let feed = Stream::from_flows(&r, vec![40.0, 360.0, 600.0], AMBIENT_K, FEED_KPA);

    let mut fs = Flowsheet::new(demo::registry());
    let u_feed = fs.add_unit("feed", Feed { stream: feed });
    let u_mixer = fs.add_unit(
        "mixer",
        Mixer {
            pressure_drop: mixer_drop,
        },
    );
    let u_split = fs.add_unit(
        "split",
        Splitter {
            fraction: 0.3,
            pressure_drop: 0.0,
        },
    );
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    fs.add_stream(u_mixer, blank.clone(), u_split);
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);
    fs.add_stream(u_split, blank, u_product);

    let mut fs = fs.validate().expect("the loop is wired correctly");
    Solver::new(SolverConfig {
        max_iterations: 1000,
        ..SolverConfig::default()
    })
    .solve(&mut fs)?;
    Ok((fs, recycle))
}

#[test]
fn drops_along_a_line_add_up() {
    let (fs, heated, a) = line(10.0, 5.0);

    assert_relative_eq!(fs[heated].pressure(), FEED_KPA - 10.0);
    assert_relative_eq!(fs[a].pressure(), FEED_KPA - 15.0);
}

#[test]
fn without_drops_every_stream_sits_at_the_feed_pressure() {
    let (fs, ..) = line(0.0, 0.0);
    for s in fs.streams() {
        assert_relative_eq!(s.pressure(), FEED_KPA);
    }
}

#[test]
fn a_recycle_without_drops_settles_at_the_feed_pressure() {
    // The tear starts as an empty placeholder at ambient. If the mixer let an empty inlet set its
    // pressure, the whole loop would sit at 101.325 kPa for good.
    let (fs, recycle) = loop_with(0.0).expect("no drop, so the loop converges");

    assert_relative_eq!(fs[recycle].pressure(), FEED_KPA);
    for s in fs.streams() {
        assert_relative_eq!(s.pressure(), FEED_KPA);
    }
}

#[test]
fn a_recycle_that_loses_pressure_fails_rather_than_converging_on_a_wrong_number() {
    // Every pass the mixer takes the recycle's pressure, which is last pass's minus the drop, so
    // the loop falls 10 kPa a pass. The flows settle by pass 17 or so; without pressure in the
    // convergence check the solve would report success there, at whatever pressure the loop had
    // reached. Instead it keeps going until the drop would take a stream to 0 kPa.
    let Err(e) = loop_with(10.0) else {
        panic!("a loop that loses pressure has no steady state");
    };

    let SolveError::Evaluation {
        name, iteration, ..
    } = &e
    else {
        panic!("expected the mixer to fail, got {e}");
    };
    assert_eq!(name, "mixer");
    // 500 kPa in, 10 kPa off per pass: the 50th pass asks for 0 kPa.
    assert_eq!(*iteration, 50);
    assert!(
        e.to_string()
            .starts_with("unit 'mixer' failed on pass 50: a pressure drop of 10 kPa"),
        "{e}"
    );
}

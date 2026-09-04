//! Convergence rate of direct substitution on a recycle loop.

use process_simulation::demo::{AMBIENT_K, AMBIENT_KPA, feed_stream, registry};
use process_simulation::flowsheet::Flowsheet;
use process_simulation::solver::{SolveReport, Solver, SolverConfig};
use process_simulation::stream::Stream;
use process_simulation::unit::UnitOp;

fn solve_recycle(fraction: f64, tolerance: f64) -> SolveReport {
    let r = registry();
    let feed = feed_stream(&r);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
    let mut fs = Flowsheet::new(r);

    let u_feed = fs.add_unit(UnitOp::Feed { stream: feed });
    let u_mixer = fs.add_unit(UnitOp::Mixer);
    let u_tank = fs.add_unit(UnitOp::Tank);
    let u_split = fs.add_unit(UnitOp::Splitter { fraction });
    let u_product = fs.add_unit(UnitOp::Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    fs.add_stream(u_mixer, blank.clone(), u_tank);
    fs.add_stream(u_tank, blank.clone(), u_split);
    fs.add_stream(u_split, blank.clone(), u_mixer); // recycle
    fs.add_stream(u_split, blank, u_product);

    let mut fs = fs.validate().expect("valid");
    Solver::new(SolverConfig {
        tolerance,
        max_iterations: 500,
    })
    .solve(&mut fs)
    .expect("converges")
}

#[test]
fn low_recycle_tight_tolerance_takes_18_passes() {
    assert_eq!(solve_recycle(0.3, 1e-9).iterations, 18);
}

#[test]
fn high_recycle_tight_tolerance_takes_171_passes() {
    assert_eq!(solve_recycle(0.9, 1e-9).iterations, 171);
}

#[test]
fn low_recycle_loose_tolerance_takes_12_passes() {
    assert_eq!(solve_recycle(0.3, 1e-6).iterations, 12);
}

#[test]
fn high_recycle_loose_tolerance_takes_106_passes() {
    assert_eq!(solve_recycle(0.9, 1e-6).iterations, 106);
}

//! Convergence rate of direct substitution on a recycle loop.

use process_simulation::demo::recycle_flowsheet;
use process_simulation::solver::{SolveReport, Solver, SolverConfig};

/// Solves the demo circuit at the given splitter `fraction`, to the given `tolerance`.
fn solve_recycle(fraction: f64, tolerance: f64) -> SolveReport {
    let mut fs = recycle_flowsheet(fraction)
        .validate()
        .expect("the demo circuit is well-formed at every fraction");

    Solver::new(SolverConfig {
        tolerance,
        max_iterations: 500,
    })
    .solve(&mut fs)
    .expect("one tear breaks the only loop")
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

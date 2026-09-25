//! Convergence rate of the tear-stream methods on a recycle loop.
//!
//! The rate is set by how much of the tear stream comes back on the next pass. With a
//! flotation cell in the loop that is `f * (1 - r)`, not `f`: whatever the cell floats off to
//! the concentrate never reaches the splitter, so it never returns. The slowest species
//! decides, and here that is the gangue at `r = 0.05` - so the loop contracts at `0.95 f`.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::{DirectSubstitution, Wegstein};
use flowsheet::demo::recycle_flowsheet;
use flowsheet::{ConvergenceMethod, SolveReport, Solver, SolverConfig};

const WEGSTEIN: ConvergenceMethod = Wegstein {
    q_min: -5.0,
    q_max: 0.0,
};

const WEGSTEIN_LOOSE_FLOOR: ConvergenceMethod = Wegstein {
    q_min: -20.0,
    q_max: 0.0,
};

/// Solves the demo circuit at the given splitter `fraction`, to the given `tolerance`.
fn solve_recycle(fraction: f64, tolerance: f64, method: ConvergenceMethod) -> SolveReport {
    let mut fs = recycle_flowsheet(fraction)
        .validate()
        .expect("the demo circuit is well-formed at every fraction");

    Solver::new(SolverConfig {
        tolerance,
        max_iterations: 500,
        method,
    })
    .solve(&mut fs)
    .expect("one tear breaks the only loop")
}

#[test]
fn low_recycle_tight_tolerance_takes_17_passes() {
    assert_eq!(solve_recycle(0.3, 1e-9, DirectSubstitution).iterations, 17);
}

#[test]
fn high_recycle_tight_tolerance_takes_119_passes() {
    assert_eq!(solve_recycle(0.9, 1e-9, DirectSubstitution).iterations, 119);
}

#[test]
fn low_recycle_loose_tolerance_takes_12_passes() {
    assert_eq!(solve_recycle(0.3, 1e-6, DirectSubstitution).iterations, 12);
}

#[test]
fn high_recycle_loose_tolerance_takes_75_passes() {
    assert_eq!(solve_recycle(0.9, 1e-6, DirectSubstitution).iterations, 75);
}

#[test]
fn wegstein_cuts_high_recycle_from_119_passes_to_11() {
    assert_eq!(solve_recycle(0.9, 1e-9, WEGSTEIN).iterations, 11);
}

#[test]
fn wegstein_cuts_low_recycle_from_17_passes_to_3() {
    assert_eq!(solve_recycle(0.3, 1e-9, WEGSTEIN).iterations, 3);
}

#[test]
fn an_unclamped_floor_solves_high_recycle_on_the_third_pass() {
    // `q` is computed per species, so each one gets its own extrapolation and all three land
    // on the answer together. What is left is rounding, not iteration error.
    let report = solve_recycle(0.9, 1e-9, WEGSTEIN_LOOSE_FLOOR);
    assert_eq!(report.iterations, 3);
    assert_relative_eq!(report.residual, 0.0, epsilon = 1e-15);
}

#[test]
fn wegstein_converges_inside_the_default_iteration_cap() {
    let mut fs = recycle_flowsheet(0.9)
        .validate()
        .expect("the demo circuit is well-formed at every fraction");

    let report = Solver::new(SolverConfig {
        method: WEGSTEIN,
        ..SolverConfig::default()
    })
    .solve(&mut fs)
    .expect("wegstein converges well inside the default 100 passes");

    assert_eq!(report.iterations, 11);
}

//! What [`Solver::solve_with`] reports while it runs.

use approx::assert_relative_eq;
use flowsheet::demo::recycle_flowsheet;
use flowsheet::{SolveError, SolveEvent, Solver, SolverConfig, UnitId};
use std::collections::HashSet;

#[test]
fn every_pass_is_reported_once_in_order_and_the_last_matches_the_report() {
    let mut fs = recycle_flowsheet(0.3).validate().expect("well-formed");
    let mut passes = Vec::new();

    let report = Solver::default()
        .solve_with(&mut fs, |event| {
            if let SolveEvent::PassCompleted {
                iteration,
                residual,
            } = event
            {
                passes.push((iteration, residual));
            }
        })
        .expect("converges");

    let iterations: Vec<usize> = passes.iter().map(|&(i, _)| i).collect();
    assert_eq!(iterations, (1..=report.iterations).collect::<Vec<_>>());
    assert_relative_eq!(passes.last().unwrap().1, report.residual);
}

#[test]
fn every_unit_is_reported_once_per_pass() {
    let mut fs = recycle_flowsheet(0.3).validate().expect("well-formed");
    let unit_count = fs.units().len();
    let mut evaluated = Vec::new();

    let report = Solver::default()
        .solve_with(&mut fs, |event| {
            if let SolveEvent::UnitEvaluated { unit, iteration } = event {
                evaluated.push((iteration, unit));
            }
        })
        .expect("converges");

    assert_eq!(evaluated.len(), unit_count * report.iterations);
    for pass in 1..=report.iterations {
        let units: HashSet<UnitId> = evaluated
            .iter()
            .filter(|&&(i, _)| i == pass)
            .map(|&(_, unit)| unit)
            .collect();
        assert_eq!(units.len(), unit_count, "pass {pass}");
    }
}

#[test]
fn a_solve_that_runs_out_of_passes_still_reports_all_of_them() {
    let mut fs = recycle_flowsheet(0.9).validate().expect("well-formed");
    let mut passes = 0;

    let result = Solver::new(SolverConfig {
        max_iterations: 5,
        ..SolverConfig::default()
    })
    .solve_with(&mut fs, |event| {
        if let SolveEvent::PassCompleted { .. } = event {
            passes += 1;
        }
    });

    assert!(matches!(
        result,
        Err(SolveError::NotConverged { iterations: 5, .. })
    ));
    assert_eq!(passes, 5);
}

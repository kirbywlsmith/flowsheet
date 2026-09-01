//! Solver data structures.

use crate::flowsheet::{UnitId, ValidFlowsheet};
use std::fmt;

/// Configures a [`Solver`].
#[derive(Debug)]
pub struct SolverConfig {
    /// The tolerance of what counts as convergence during [`Solver::solve`].
    ///
    /// If the maximum residual is less than or equal to the configured tolerance, convergence is achieved.
    pub tolerance: f64,
    /// The maximum number of passes [`Solver::solve`] makes before giving up.
    ///
    /// Exhausting this results in a [`SolveError::NotConverged`].
    pub max_iterations: usize,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            tolerance: 1e-9,
            max_iterations: 100,
        }
    }
}

/// Solves a [`ValidFlowsheet`].
#[derive(Debug, Default)]
pub struct Solver {
    config: SolverConfig,
}

/// A type of error encountered when solving a flowsheet.
#[derive(Debug)]
pub enum SolveError {
    /// The flowsheet contains a cycle.
    Cycle(Vec<UnitId>),
    /// The residual was still above tolerance after [`SolverConfig::max_iterations`] passes.
    NotConverged {
        /// How many passes were made before giving up.
        iterations: usize,
        /// The max residual on the final pass.
        residual: f64,
    },
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolveError::Cycle(units) => write!(
                f,
                "flowsheet contains a cycle through {} units",
                units.len()
            ),
            SolveError::NotConverged {
                iterations,
                residual,
            } => write!(
                f,
                "no convergence after {iterations} passes (residual {residual:e})"
            ),
        }
    }
}

impl std::error::Error for SolveError {}

/// The result of a converged [`Solver::solve`].
#[derive(Debug)]
pub struct SolveReport {
    /// How many passes were made.
    pub iterations: usize,
    /// The max residual on the final pass. Always within [`SolverConfig::tolerance`].
    pub residual: f64,
}

impl Solver {
    /// Initialises a new [`Solver`] with the specified config.
    pub fn new(config: SolverConfig) -> Self {
        Self { config }
    }

    /// Solves the specified flowsheet, writing the results into its streams.
    ///
    /// # Errors
    ///
    /// Returns [`SolveError::Cycle`] before any evaluation, and [`SolveError::NotConverged`] if
    /// the residual is still above tolerance after [`SolverConfig::max_iterations`] passes.
    pub fn solve(&self, flowsheet: &mut ValidFlowsheet) -> Result<SolveReport, SolveError> {
        let waves = flowsheet.evaluation_waves().map_err(SolveError::Cycle)?;

        let mut iterations = 0;
        let residual;

        loop {
            for wave in &waves {
                // TODO: make this parallel
                for &unit_id in wave {
                    flowsheet.evaluate_unit(unit_id);
                }
            }

            iterations += 1;

            // TODO: max over tear streams; 0.0 while acyclic
            let pass_residual = 0.0;

            if pass_residual <= self.config.tolerance {
                residual = pass_residual;
                break;
            }

            if iterations >= self.config.max_iterations {
                return Err(SolveError::NotConverged {
                    iterations,
                    residual: pass_residual,
                });
            }
        }

        Ok(SolveReport {
            iterations,
            residual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowsheet::Flowsheet;
    use crate::test_support::demo_registry;
    use crate::unit::UnitOp;

    /// `UnitId`'s field is private outside `flowsheet`, so ids have to come from a real
    /// flowsheet. The wiring is irrelevant here — only the count reaches the message.
    fn two_unit_ids() -> Vec<UnitId> {
        let mut fs = Flowsheet::new(demo_registry());
        vec![fs.add_unit(UnitOp::Mixer), fs.add_unit(UnitOp::Product)]
    }

    #[test]
    fn cycle_message_counts_the_units_it_could_not_order() {
        let e = SolveError::Cycle(two_unit_ids());
        assert_eq!(e.to_string(), "flowsheet contains a cycle through 2 units");
    }

    #[test]
    fn not_converged_message_prints_the_residual_in_scientific_notation() {
        let e = SolveError::NotConverged {
            iterations: 100,
            residual: 0.0015,
        };
        assert_eq!(
            e.to_string(),
            "no convergence after 100 passes (residual 1.5e-3)"
        );
    }
}

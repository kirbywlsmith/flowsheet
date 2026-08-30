//! Solver data structures.

use crate::flowsheet::{Flowsheet, FlowsheetError, UnitId};

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

/// Solves a [`Flowsheet`].
#[derive(Debug, Default)]
pub struct Solver {
    config: SolverConfig,
}

/// A type of error encountered when solving a flowsheet.
#[derive(Debug)]
pub enum SolveError {
    /// The provided flowsheet state is invalid.
    Invalid(Vec<FlowsheetError>),
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
    /// Returns [`SolveError::Invalid`] or [`SolveError::Cycle`] before any evaluation, and
    /// [`SolveError::NotConverged`] if the residual is still above tolerance after
    /// [`SolverConfig::max_iterations`] passes.
    pub fn solve(&self, flowsheet: &mut Flowsheet) -> Result<SolveReport, SolveError> {
        flowsheet.validate().map_err(SolveError::Invalid)?;
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

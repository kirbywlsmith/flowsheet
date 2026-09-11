//! Solver data structures.

use crate::flowsheet::{UnitId, ValidFlowsheet};
use crate::stream::Stream;
use std::fmt;

/// The convergence method used by the [`Solver`].
#[derive(Debug, Clone, Copy)]
pub enum ConvergenceMethod {
    /// Feeds the result of each pass straight back in as the next guess.
    DirectSubstitution,
    /// Extrapolates each tear component along the secant through the last two passes.
    ///
    /// The next guess is `q * old + (1 - q) * new`, with `q` re-estimated every pass
    /// and clamped to the bounds below.
    Wegstein {
        /// Lower clamp on `q`. Negative values accelerate; -5.0 is a common floor.
        q_min: f64,
        /// Upper clamp on `q`. 0.0 forbids damping, 0.9 allows it.
        q_max: f64,
    },
}

/// Configures a [`Solver`].
#[derive(Debug)]
pub struct SolverConfig {
    /// The tolerance of what counts as convergence during [`Solver::solve`].
    ///
    /// Every tear stream's flow residual ([`Stream::max_flow_residual`]) and temperature residual
    /// ([`Stream::temperature_residual`]) must be at or below it. Both are relative, so one
    /// tolerance serves both.
    pub tolerance: f64,
    /// The maximum number of passes [`Solver::solve`] makes before giving up.
    ///
    /// Exhausting this results in a [`SolveError::NotConverged`].
    pub max_iterations: usize,
    /// The method used to calculate the stream values for each solve iteration.
    pub method: ConvergenceMethod,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            tolerance: 1e-9,
            max_iterations: 100,
            method: ConvergenceMethod::DirectSubstitution,
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
    /// The tear set was too small to order the flowsheet, so the units listed - those still
    /// inside a loop, or downstream of one - were never reached.
    Untearable(Vec<UnitId>),
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
            SolveError::Untearable(units) => write!(
                f,
                "tear set too small to order {} units - interlocking loops need one tear each",
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
    /// Returns [`SolveError::Untearable`] before any evaluation, and [`SolveError::NotConverged`] if
    /// the residual is still above tolerance after [`SolverConfig::max_iterations`] passes.
    pub fn solve(&self, flowsheet: &mut ValidFlowsheet) -> Result<SolveReport, SolveError> {
        let tears = flowsheet.tear_streams();
        let waves = flowsheet
            .evaluation_waves_with_tears(&tears)
            .map_err(SolveError::Untearable)?;

        let mut iterations = 0;
        let residual;

        let mut tears_snapshot: Vec<Stream> = Vec::with_capacity(tears.len());
        let mut history: Option<PreviousPass> = None;

        loop {
            tears_snapshot.clear();
            tears_snapshot.extend(tears.iter().map(|&s| flowsheet[s].clone()));

            for wave in &waves {
                // TODO: make this parallel
                for &unit_id in wave {
                    flowsheet.evaluate_unit(unit_id);
                }
            }

            iterations += 1;

            let pass_residual = tears
                .iter()
                .zip(&tears_snapshot)
                .map(|(&s, old)| {
                    let new = &flowsheet[s];
                    nan_max(new.max_flow_residual(old), new.temperature_residual(old))
                })
                .fold(0.0, nan_max);

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

            if let ConvergenceMethod::Wegstein { q_min, q_max } = self.config.method {
                let results: Vec<Stream> = tears.iter().map(|&s| flowsheet[s].clone()).collect();

                if let Some(prev) = &history {
                    for (i, &s) in tears.iter().enumerate() {
                        flowsheet[s] = wegstein_step(
                            &prev.guesses[i],
                            &prev.results[i],
                            &tears_snapshot[i],
                            &results[i],
                            q_min,
                            q_max,
                        );
                    }
                }

                history = Some(PreviousPass {
                    guesses: tears_snapshot.clone(),
                    results,
                });
            }
        }

        Ok(SolveReport {
            iterations,
            residual,
        })
    }
}

/// The larger of `a` and `b`, or `NaN` if either is.
///
/// `f64::max` drops a `NaN` and returns the other number. Here that would let a pass that
/// produced garbage report itself converged.
fn nan_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// What Wegstein remembers between passes: the guess that went into the previous pass
/// and the result it produced, one entry per tear stream.
struct PreviousPass {
    guesses: Vec<Stream>,
    results: Vec<Stream>,
}

/// The Wegstein weight for one species, from the secant through two passes.
fn wegstein_q(dx: f64, dy: f64, q_min: f64, q_max: f64) -> f64 {
    if dx == 0.0 {
        return 0.0; // exact-zero guard on a division, not a float comparison
    }
    let slope = dy / dx;
    let denominator = slope - 1.0;
    if denominator == 0.0 {
        return 0.0; // slope of 1: the secant is parallel to y = x and never crosses it
    }
    (slope / denominator).clamp(q_min, q_max)
}

/// The next guess for one tear stream, extrapolated species by species.
///
/// `previous_*` are the pass before last; `guess` and `result` are the pass just finished.
/// Temperature and pressure come straight from `result`, so temperature always converges by
/// direct substitution. Around an adiabatic loop of mixers and splitters, temperature contracts
/// at roughly a heat-capacity-weighted average of the species' rates. Under direct substitution
/// the slowest species therefore still sets the pass count. Under Wegstein the flows can converge
/// faster than that, leaving temperature as the slowest part. Extend the acceleration to
/// temperature if a hot recycle ever shows it setting the pass count.
///
/// # Panics
/// If `q_min > q_max`, or either is `NaN`.
fn wegstein_step(
    previous_guess: &Stream,
    previous_result: &Stream,
    guess: &Stream,
    result: &Stream,
    q_min: f64,
    q_max: f64,
) -> Stream {
    let mut next = result.clone();
    for (k, flow) in next.flows_mut().iter_mut().enumerate() {
        let dx = guess.flows()[k] - previous_guess.flows()[k];
        let dy = result.flows()[k] - previous_result.flows()[k];
        let q = wegstein_q(dx, dy, q_min, q_max);
        *flow = q * guess.flows()[k] + (1.0 - q) * result.flows()[k];
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowsheet::Flowsheet;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use crate::unit::{Feed, Mixer, Product, Splitter};
    use approx::assert_relative_eq;

    /// `UnitId`'s field is private outside `flowsheet`, so ids have to come from a real
    /// flowsheet. The wiring is irrelevant here — only the count reaches the message.
    fn two_unit_ids() -> Vec<UnitId> {
        let mut fs = Flowsheet::new(demo_registry());
        vec![fs.add_unit("mixer", Mixer), fs.add_unit("product", Product)]
    }

    #[test]
    fn untearable_message_counts_the_units_it_could_not_order() {
        let e = SolveError::Untearable(two_unit_ids());
        assert_eq!(
            e.to_string(),
            "tear set too small to order 2 units - interlocking loops need one tear each"
        );
    }

    #[test]
    fn q_is_zero_when_the_guess_did_not_move() {
        // Nothing to draw a secant through, so fall back to direct substitution.
        assert_relative_eq!(wegstein_q(0.0, 5.0, -5.0, 0.0), 0.0);
    }

    #[test]
    fn q_is_zero_when_the_secant_is_parallel_to_the_fixed_point_line() {
        assert_relative_eq!(wegstein_q(2.0, 2.0, -5.0, 0.0), 0.0);
    }

    #[test]
    fn q_matches_the_recycle_fraction_it_came_from() {
        // A loop that returns `f` of what it is given has slope `f`, so q = f / (f - 1).
        assert_relative_eq!(
            wegstein_q(1.0, 0.3, -20.0, 0.0),
            -0.3 / 0.7,
            epsilon = 1e-12
        );
        assert_relative_eq!(wegstein_q(1.0, 0.9, -20.0, 0.0), -9.0, epsilon = 1e-12);
    }

    #[test]
    fn q_is_clamped_at_both_ends() {
        assert_relative_eq!(wegstein_q(1.0, 0.9, -5.0, 0.0), -5.0); // wants -9
        assert_relative_eq!(wegstein_q(1.0, 2.0, -5.0, 0.0), 0.0); // wants +2, damping forbidden
        assert_relative_eq!(wegstein_q(1.0, 2.0, -5.0, 0.9), 0.9); // damping allowed, still capped
    }

    #[test]
    fn a_zero_weight_reproduces_direct_substitution() {
        let r = demo_registry();
        let old = feed(&r);
        let new = Stream::from_flows(&r, vec![50.0, 300.0, 700.0], AMBIENT_K, AMBIENT_KPA);
        // Identical passes give dx = dy = 0, hence q = 0, hence next guess == new.
        let next = wegstein_step(&old, &new, &old, &new, -5.0, 0.0);
        assert!(next.flows_approx_eq(&new, 1e-12));
    }

    #[test]
    fn nan_max_keeps_the_nan_that_f64_max_would_drop() {
        assert_relative_eq!(f64::NAN.max(1.0), 1.0); // what std does
        assert!(nan_max(f64::NAN, 1.0).is_nan());
        assert!(nan_max(1.0, f64::NAN).is_nan());
        assert_relative_eq!(nan_max(1.0, 2.0), 2.0);
    }

    #[test]
    fn settled_flows_do_not_end_the_solve_while_temperature_is_still_moving() {
        //   feed (350 K) --> mixer --> splitter --> product
        //                      ^           |
        //                      +-----------+  half recycles
        //
        // Half recycling doubles the circulating load: 2F leaves the mixer, and F both recycles
        // and leaves as product. Every stream starts at exactly that, so the flow residual is zero
        // from the first pass. Only temperature is wrong: the feed is hot and the loop starts at
        // ambient. Judged on flows alone the solve would stop after one pass with the loop cold.
        let r = demo_registry();
        let mut hot = feed(&r);
        hot.set_temperature(350.0);

        let mut fs = Flowsheet::new(demo_registry());
        let u_feed = fs.add_unit("feed", Feed { stream: hot });
        let mixer = fs.add_unit("mixer", Mixer);
        let split = fs.add_unit("split", Splitter { fraction: 0.5 });
        let u_product = fs.add_unit("product", Product);
        fs.add_stream(u_feed, feed(&r), mixer);
        fs.add_stream(mixer, feed(&r).scaled(2.0), split);
        fs.add_stream(split, feed(&r), mixer);
        let product = fs.add_stream(split, feed(&r), u_product);

        let mut fs = fs.validate().expect("the loop is well-formed");
        let report = Solver::default()
            .solve(&mut fs)
            .expect("the loop converges");

        assert!(report.iterations > 1, "{report:?}");
        // One feed and no heat in or out, so at steady state the whole loop sits at 350 K.
        assert_relative_eq!(fs[product].temperature(), 350.0, max_relative = 1e-6);
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

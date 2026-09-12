//! `flowsheet` - load a flowsheet document, solve it, and print the result.

use clap::{Parser, ValueEnum};
use flowsheet::report;
use flowsheet::serial;
use flowsheet::{ConvergenceMethod, Flowsheet, FlowsheetError, SolveEvent, Solver, SolverConfig};
use indicatif::{ProgressBar, ProgressStyle};
use std::error::Error;
use std::fmt::Write;
use std::path::PathBuf;

/// Solve a steady-state flowsheet document.
#[derive(Parser)]
#[command(name = "flowsheet", version)]
struct Cli {
    /// The flowsheet document to solve.
    file: PathBuf,
    /// Print the solved document as JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// Relative residual every tear stream must reach to count as converged.
    #[arg(long, default_value_t = SolverConfig::default().tolerance)]
    tolerance: f64,
    /// Passes to make before giving up.
    #[arg(long, default_value_t = SolverConfig::default().max_iterations)]
    max_iterations: usize,
    /// How each pass's result becomes the next pass's guess.
    #[arg(long, value_enum, default_value_t = Method::Direct)]
    method: Method,
}

/// The `--method` values, one per [`ConvergenceMethod`].
///
/// A separate enum rather than `ConvergenceMethod` itself: `Wegstein` carries `q_min` and
/// `q_max`, and a `ValueEnum` derive needs fieldless variants to name on the command line. The
/// two numeric defaults above are read off `SolverConfig::default()` so they cannot drift; this
/// one is written out, because a bare `ConvergenceMethod` has no `Default` to read.
#[derive(Clone, Copy, ValueEnum)]
enum Method {
    /// Feed each pass's result straight back in as the next guess.
    Direct,
    /// Extrapolate each tear component along the secant through the last two passes.
    Wegstein,
}

impl From<Method> for ConvergenceMethod {
    fn from(method: Method) -> Self {
        match method {
            Method::Direct => ConvergenceMethod::DirectSubstitution,
            // The clamps stay library-only until something needs them. -5.0 is the conventional
            // floor and permits strong acceleration; 0.0 forbids damping.
            Method::Wegstein => ConvergenceMethod::Wegstein {
                q_min: -5.0,
                q_max: 0.0,
            },
        }
    }
}

fn main() {
    // `fn main() -> Result<..>` would print the error with `Debug`, not `Display`, so every
    // message these types carefully write would come out as a struct literal instead.
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    // `serde_json` names the line and column but not the file, so both of these say which.
    let text =
        std::fs::read_to_string(&cli.file).map_err(|e| format!("{}: {e}", cli.file.display()))?;
    let document: serial::Flowsheet =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", cli.file.display()))?;

    let mut solved = Flowsheet::try_from(document)
        .map_err(|e| format!("{}: {e}", cli.file.display()))?
        .validate()
        .map_err(describe_validation)?;

    // Drawn on stderr, and hidden when that is not a terminal, so `--json > solved.json` is clean.
    let progress = ProgressBar::new_spinner().with_style(
        ProgressStyle::with_template("{spinner} pass {pos}, residual {msg}")
            .expect("the template is valid"),
    );
    let solver = Solver::new(SolverConfig {
        tolerance: cli.tolerance,
        max_iterations: cli.max_iterations,
        method: cli.method.into(),
    });
    let result = solver.solve_with(&mut solved, |event| {
        if let SolveEvent::PassCompleted {
            iteration,
            residual,
        } = event
        {
            progress.set_message(format!("{residual:.1e}"));
            progress.set_position(iteration as u64);
        }
    });
    progress.finish_and_clear();
    let report = result?;

    if cli.json {
        let saved = serial::Flowsheet::from(&solved);
        println!("{}", serde_json::to_string_pretty(&saved)?);
    } else {
        print!("{}", report::table(&solved, &report));
    }

    Ok(())
}

/// Joins the errors `validate` collected into one message.
///
/// `Vec<FlowsheetError>` is not itself an error type, so `?` cannot box it. Collapsing the vector
/// into a `String` - which *does* convert into `Box<dyn Error>` - keeps all of them, and reporting
/// all of them is the whole reason `check` collects rather than stopping at the first.
fn describe_validation(errors: Vec<FlowsheetError>) -> String {
    let noun = if errors.len() == 1 { "error" } else { "errors" };
    let mut message = format!("{} validation {noun}:", errors.len());
    for e in errors {
        write!(message, "\n  {e}").expect("writing to a String is infallible");
    }
    message
}

// ============================================================================
// TARGET FLOWSHEET — "rougher flotation circuit with a scavenger recycle"
//
// Topology
// --------
//   Feed                     (source, 0 in / 1 out)
//     │ S0
//     ▼
//   Mixer  ◄─────────────┐   (2 in / 1 out)
//     │ S1                │
//     ▼                  │ S4   recycle ← TEAR STREAM
//   Flotation cell       │
//     ├── S2 ──▶ Concentrate   (1 in / 2 out)
//     │ S3               │
//     ▼                  │
//   Splitter ────────────┘   (1 in / 2 out)
//     │ S5
//     ▼
//   Tailings                 (sink, 1 in / 0 out)
//
// Streams: S0..S5. Units: U0..U5 in the order listed above.
// The Feed→Mixer→Flotation→Splitter→Mixer cycle is why a topological sort
// alone can't solve this — S4 must be guessed and iterated to convergence.
//
// Species (fixed list, index = SpeciesId)
// ---------------------------------------
//   0  CuFeS2(s)   chalcopyrite  — the valuable mineral
//   1  SiO2(s)     gangue        — the worthless rock
//   2  H2O(l)      water
//
// All flows are mass flow rates in t/h. Temperature is solved by an adiabatic
// energy balance, but every stream here enters at 25 °C, so every stream
// leaves at 25 °C as well. Pressure is carried and ignored.
//
// Unit models
// -----------
//   U0 Feed        outlet = fixed vector [40.0, 360.0, 600.0]
//   U1 Mixer       outlet[i] = sum of inlet[i] over all inlets
//                  outlet T  = whatever makes H(outlet) = sum of H(inlets)
//   U2 Flotation   param: recovery r = [0.85, 0.05, 0.30]
//                  concentrate[i] = inlet[i] * r[i]
//                  tails[i]       = inlet[i] * (1.0 - r[i])
//   U3 Concentrate accumulates inlet, no outlets
//   U4 Splitter    param: recycle_fraction f = 0.3
//                  recycle_outlet[i] = inlet[i] * f
//                  tailings_outlet[i] = inlet[i] * (1.0 - f)
//   U5 Tailings    accumulates inlet, no outlets
//
// Every unit must satisfy: sum(inlets) == sum(outlets), per species. With no
// heat in or out, the same holds for enthalpy.
//
// The flotation cell is what makes this a plant rather than a plumbing
// diagram. A splitter sends the same composition to both outlets, so no
// arrangement of splitters can concentrate anything; recovering each species
// at its own rate is the whole of mineral processing in one line of code.
//
// Expected converged solution (t/h)
// ---------------------------------
// Each species travels the loop independently, so one scalar equation per
// species is the whole answer. With F the feed, r the recovery and f the
// recycle fraction, the mixer outlet M satisfies M = F + f(1 - r)M, so
//
//   M = F / (1 - f(1 - r))
//
// and everything else follows. `flowsheet::demo::balance` is that formula,
// and the balance tests assert against it rather than against these numbers.
//
//                        CuFeS2     SiO2      H2O       total
//   S0 feed               40.000  360.000  600.000   1000.000
//   S1 mixer out          41.885  503.497  759.494   1304.875
//   S2 concentrate        35.602   25.175  227.848    288.625
//   S3 cell tails          6.283  478.322  531.646   1016.250
//   S4 recycle             1.885  143.497  159.494    304.875
//   S5 tailings            4.398  334.825  372.152    711.375
//
// Three invariants worth asserting in tests:
//   1. S2 + S5 == S0, per species. Nothing accumulates at steady state, so
//      whatever enters the plant must leave it by one of the two doors — the
//      recycle only inflates the INTERNAL flows.
//   2. The concentrate is upgraded: 4.00% CuFeS2 in the feed becomes 12.33%
//      in S2, and the tailings drop to 0.62%. Grade is the point.
//   3. Amplification is now per species: 1/(1 - f(1 - r)) is 1.047 for the
//      chalcopyrite that mostly floats out, but 1.399 for the gangue that
//      keeps going round. One number no longer describes the circuit.
//
// Convergence behaviour
// ---------------------
// Guess S4 = zeros, then direct substitution: the error in each species
// shrinks by f(1 - r) each pass, so the slowest species sets the rate. Here
// that is the gangue at 0.3 * 0.95 = 0.285, giving 17 iterations to 1e-9.
// Bump f to 0.9 and the factor is 0.855 and it takes 119 — that's the
// motivation for Wegstein, which does it in 11.
//
// Build order
// -----------
//   [ ] SpeciesId + species list, Stream with a dense Vec<f64> of flows
//   [ ] Mixer and Splitter as plain functions, unit-tested in isolation
//   [ ] Arena: Vec<UnitOp>, Vec<Stream>, UnitId/StreamId newtypes
//   [ ] Wire up the graph above by hand in a build_flowsheet() fn
//   [ ] Solve WITHOUT the recycle first (delete S4, Mixer takes only S0)
//       — this is acyclic, so a topological sort solves it in one pass
//   [ ] Add S4 back, detect the cycle, tear at S1, direct substitution
//   [ ] Wegstein acceleration, tolerance + max-iteration config
// ============================================================================

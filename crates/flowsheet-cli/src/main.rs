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

//! Rendering a solved flowsheet as a fixed-width table.
//!
//! This lives in the library rather than the binary because a stream's endpoints are only
//! recoverable from the units that list it, and those port vectors are crate-private. Keeping
//! the formatter here also keeps it free of any CLI dependency, so it survived the split into
//! the `flowsheet` library crate unchanged.

use crate::flowsheet::ValidFlowsheet;
use crate::solver::{SolveReport, nan_max};
use std::collections::HashMap;
use std::fmt::Write;

/// Leading indent on every line.
const INDENT: &str = "  ";
/// Gap between two columns.
const GAP: &str = "  ";
/// Decimal places on every flow.
const PRECISION: usize = 3;
/// Decimal places on the temperature column.
const TEMPERATURE_PRECISION: usize = 2;
/// The one column holding text rather than a number, so the one that is left-aligned.
const LABEL_COLUMN: usize = 1;

/// A `{from_unit}.{to_unit}` label for every stream, in [`crate::flowsheet::StreamId`] order.
///
/// Nothing forbids two streams between the same pair of units - a splitter routing both outlets
/// into one mixer is legal and balances - so a repeated label is suffixed `#2`, `#3`, and so on.
/// The first occurrence stays bare, which is why one running counter is enough and no counting
/// pre-pass is needed.
fn stream_labels(fs: &ValidFlowsheet) -> Vec<String> {
    // A stream's endpoints are whichever units list it, the same pairing `add_stream` recorded.
    let n = fs.streams().len();
    let (mut from, mut to) = (vec![""; n], vec![""; n]);
    for u in fs.units() {
        for &s in &u.outlets {
            from[s.as_usize()] = u.name.as_str();
        }
        for &s in &u.inlets {
            to[s.as_usize()] = u.name.as_str();
        }
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    (0..n)
        .map(|i| {
            let base = format!("{}.{}", from[i], to[i]);
            let count = seen.entry(base.clone()).or_default();
            *count += 1;
            if *count == 1 {
                base
            } else {
                format!("{base}#{count}")
            }
        })
        .collect()
}

/// The worst per-species disagreement between what enters the flowsheet and what leaves it.
///
/// A unit with no inlets is a source and a unit with no outlets is a sink, so the material that
/// entered is the sum of every source's outlets and the material that left is the sum of every
/// sink's inlets. Nothing accumulates at steady state, so the two must agree species by species:
/// on the demo circuit the concentrate and the tailings add back up to the feed, and a recycle
/// only inflates the flows in between. It is the first thing anyone from the domain checks, and
/// unlike the residual it is a statement about the answer rather than about how much the last
/// pass moved.
///
/// It does not reach zero on a torn flowsheet, and is not meant to. A solve stops once the tear
/// stream stops moving, so the tear still carries the error the next pass would have removed,
/// and that error is what arrives at the plant's doors: the demo circuit closes to 1.9e-10
/// against a residual of 5.4e-10. Reading it means comparing it to the tolerance, not to zero.
/// A flowsheet with no tear closes to rounding - every unit's arithmetic conserves mass.
///
/// Per species, not per total: a cell that moved 10 t/h of gangue into the copper column would
/// leave both totals untouched, and that is exactly the kind of wiring mistake this is for.
/// Normalised by the larger of the two totals, and `NaN`-propagating, the same convention as
/// [`crate::Stream::max_flow_residual`] - the two numbers print side by side in the footer of
/// [`table`], so they have to mean the same thing. A flowsheet with no sources and no sinks, or
/// one that has not been solved yet, has nothing flowing and reports `0.0`.
///
/// Note that this only sees the plant's two ends. A unit that loses mass in the middle of the
/// circuit shows up here only if the loss reaches a product; what checks every unit individually
/// is the arity and the op's own arithmetic.
pub fn imbalance(fs: &ValidFlowsheet) -> f64 {
    let n = fs.registry().len();
    let (mut entered, mut left) = (vec![0.0; n], vec![0.0; n]);

    for u in fs.units() {
        // A unit with neither inlets nor outlets takes the first arm and iterates over an empty
        // port list, so an isolated unit contributes to neither side. Every other unit is a
        // source, a sink, or in the middle - never two of them at once.
        let (ports, side) = if u.inlets.is_empty() {
            (&u.outlets, &mut entered)
        } else if u.outlets.is_empty() {
            (&u.inlets, &mut left)
        } else {
            continue;
        };
        for &s in ports {
            for (total, flow) in side.iter_mut().zip(fs[s].flows()) {
                *total += flow;
            }
        }
    }

    let scale = nan_max(entered.iter().sum(), left.iter().sum());
    if scale == 0.0 {
        return 0.0; // nothing entered and nothing left
    }

    entered
        .iter()
        .zip(&left)
        .map(|(a, b)| (a - b).abs() / scale)
        .fold(0.0, nan_max)
}

/// Renders every stream of a solved flowsheet as one row, followed by a convergence footer.
///
/// Flows are t/h and the last column is temperature in Kelvin. Pressure is not shown: nothing
/// solves it yet.
///
/// The leading `#` column is the stream's position in the `streams` array, which is its real
/// identity; the label beside it is for reading, and may be suffixed to break a tie.
///
/// ```text
///   #  stream                 CuFeS2     SiO2      H2O     total   T (K)
///   0  feed.mixer             40.000  360.000  600.000  1000.000  298.15
///   1  mixer.flotation        41.885  503.497  759.494  1304.875  298.15
///   2  flotation.concentrate  35.602   25.175  227.848   288.625  298.15
///      converged in 17 iterations, residual 5.4e-10, imbalance 1.9e-10
/// ```
///
/// The footer's `imbalance` is [`imbalance`]: how far the material entering the plant is from
/// the material leaving it, worst species.
pub fn table(fs: &ValidFlowsheet, report: &SolveReport) -> String {
    let mut header: Vec<String> = vec!["#".to_string(), "stream".to_string()];
    header.extend(fs.registry().all().iter().map(|s| s.name.clone()));
    header.push("total".to_string());
    header.push("T (K)".to_string());

    let rows: Vec<Vec<String>> = stream_labels(fs)
        .into_iter()
        .zip(fs.streams())
        .enumerate()
        .map(|(i, (label, s))| {
            let mut row = vec![i.to_string(), label];
            // The total is just one more numeric column, so it is appended to the flows and
            // formatted by the same rule.
            row.extend(
                s.flows()
                    .iter()
                    .copied()
                    .chain(std::iter::once(s.total()))
                    .map(|v| format!("{v:.PRECISION$}")),
            );
            row.push(format!("{:.TEMPERATURE_PRECISION$}", s.temperature()));
            row
        })
        .collect();

    let widths: Vec<usize> = (0..header.len())
        .map(|c| {
            std::iter::once(&header[c])
                .chain(rows.iter().map(|r| &r[c]))
                .map(String::len)
                .max()
                .expect("the header cell makes the iterator non-empty")
        })
        .collect();

    let mut out = String::new();
    write_row(&mut out, &header, &widths);
    for row in &rows {
        write_row(&mut out, row, &widths);
    }

    // The footer starts where the `stream` column does, so the index column reads as a gutter.
    let gutter = INDENT.len() + widths[0] + GAP.len();
    let plural = if report.iterations == 1 { "" } else { "s" };
    writeln!(
        out,
        "{:gutter$}converged in {} iteration{plural}, residual {:.1e}, imbalance {:.1e}",
        "",
        report.iterations,
        report.residual,
        imbalance(fs)
    )
    .expect("writing to a String is infallible");

    out
}

/// Writes one padded row. Only [`LABEL_COLUMN`] is left-aligned; the rest are numbers.
fn write_row(out: &mut String, cells: &[String], widths: &[usize]) {
    out.push_str(INDENT);
    for (c, (cell, &w)) in cells.iter().zip(widths).enumerate() {
        if c > 0 {
            out.push_str(GAP);
        }
        if c == LABEL_COLUMN {
            write!(out, "{cell:<w$}")
        } else {
            write!(out, "{cell:>w$}")
        }
        .expect("writing to a String is infallible");
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;
    use crate::flowsheet::Flowsheet;
    use crate::solver::{Solver, SolverConfig};
    use crate::stream::Stream;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use crate::unit::{Feed, Mixer, Product, Splitter, Tank};
    use approx::assert_relative_eq;

    /// A report is only ever an input to formatting, so the layout tests supply their own
    /// rather than depending on the last bits of a real solve.
    fn report(iterations: usize, residual: f64) -> SolveReport {
        SolveReport {
            iterations,
            residual,
        }
    }

    fn solved_demo() -> ValidFlowsheet {
        let mut fs = demo::build_flowsheet()
            .validate()
            .expect("the demo circuit is valid");
        Solver::default().solve(&mut fs).expect("it converges");
        fs
    }

    /// `feed -> split`, with BOTH splitter outlets running into one mixer. Legal, balances,
    /// and gives two streams the same `split.mixer` label.
    fn colliding_labels() -> ValidFlowsheet {
        let r = demo_registry();
        let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let feed_flows = feed(&r);
        let mut fs = Flowsheet::new(r);

        let u_feed = fs.add_unit("feed", Feed { stream: feed_flows });
        let u_split = fs.add_unit("split", Splitter { fraction: 0.3 });
        let u_mixer = fs.add_unit("mixer", Mixer);
        let u_product = fs.add_unit("product", Product);

        fs.add_stream(u_feed, blank.clone(), u_split);
        fs.add_stream(u_split, blank.clone(), u_mixer);
        fs.add_stream(u_split, blank.clone(), u_mixer);
        fs.add_stream(u_mixer, blank, u_product);

        fs.validate().expect("the circuit is wired correctly")
    }

    // ---- labels ----

    #[test]
    fn a_label_names_the_units_its_stream_connects() {
        assert_eq!(
            stream_labels(&solved_demo()),
            [
                "feed.mixer",
                "mixer.flotation",
                "flotation.concentrate",
                "flotation.split",
                "split.mixer",
                "split.tailings",
            ]
        );
    }

    #[test]
    fn a_repeated_label_is_numbered_from_the_second_occurrence() {
        // The first stays bare, so the suffix reads as "and another one of these".
        assert_eq!(
            stream_labels(&colliding_labels()),
            [
                "feed.split",
                "split.mixer",
                "split.mixer#2",
                "mixer.product"
            ]
        );
    }

    #[test]
    fn labels_stay_in_streams_array_order() {
        // The `#` column is the row's real identity, so label i must belong to stream i.
        let fs = colliding_labels();
        assert_eq!(stream_labels(&fs).len(), fs.streams().len());
    }

    // ---- balance ----

    /// `feed -> tank -> product`, with both streams written by hand rather than solved, so the
    /// two ends can be made to disagree. Nothing in the library will produce this - every op
    /// conserves mass - which is the point: the fixture has to lie to prove the check reads.
    fn leaking(entered: [f64; 3], left: [f64; 3]) -> ValidFlowsheet {
        let r = demo_registry();
        let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
        let at_inlet = Stream::from_flows(&r, entered.to_vec(), AMBIENT_K, AMBIENT_KPA);
        let at_outlet = Stream::from_flows(&r, left.to_vec(), AMBIENT_K, AMBIENT_KPA);
        let mut fs = Flowsheet::new(r);

        let u_feed = fs.add_unit("feed", Feed { stream: blank });
        let u_tank = fs.add_unit("tank", Tank);
        let u_product = fs.add_unit("product", Product);

        // The streams go in already carrying the numbers, so nothing here is ever solved -
        // a solve would overwrite both of them with a balanced pair.
        fs.add_stream(u_feed, at_inlet, u_tank);
        fs.add_stream(u_tank, at_outlet, u_product);

        fs.validate().expect("the chain is wired correctly")
    }

    #[test]
    fn the_demo_circuits_two_products_add_back_up_to_its_feed() {
        // S2 + S5 == S0, per species: the recycle inflates S1 and S3 and changes neither end
        // of the plant. Not *exactly* zero, and it cannot be - the solve stopped once the tear
        // stream moved by less than the tolerance, so the two ends still disagree by whatever
        // error was left in the tear. It comes out at 1.9e-10 against a residual of 5.4e-10,
        // which is the same quantity arriving at the plant's doors.
        let closure = imbalance(&solved_demo());
        assert!(closure <= SolverConfig::default().tolerance, "{closure:e}");
    }

    #[test]
    fn an_acyclic_circuit_closes_to_rounding_because_it_has_no_tear_to_leave_error_in() {
        // Splitting by 0.3 and 0.7 and adding the halves back together is not bit-exact, so
        // this is a few ULP rather than a hard zero - but it is 1e5 times tighter than the
        // demo circuit above, which is the difference between rounding and iteration error.
        let mut fs = colliding_labels();
        Solver::default().solve(&mut fs).expect("it is acyclic");
        assert_relative_eq!(imbalance(&fs), 0.0, epsilon = 1e-15);
    }

    #[test]
    fn an_unsolved_flowsheet_has_nothing_flowing_and_so_nothing_to_close() {
        assert_eq!(imbalance(&colliding_labels()), 0.0);
    }

    #[test]
    fn material_that_never_arrives_is_reported_against_the_larger_end() {
        // 100 t/h in, 90 t/h out. The scale is the bigger of the two totals, so 10 / 100.
        assert_relative_eq!(
            imbalance(&leaking([100.0, 0.0, 0.0], [90.0, 0.0, 0.0])),
            0.1
        );
    }

    #[test]
    fn a_species_swap_that_leaves_the_total_untouched_is_still_caught() {
        // Both ends carry 10 t/h, so a total-only check would call this closed. It is the
        // wiring mistake the per-species comparison exists for.
        assert_relative_eq!(imbalance(&leaking([10.0, 0.0, 0.0], [0.0, 10.0, 0.0])), 1.0);
    }

    #[test]
    fn a_nan_flow_does_not_report_a_closed_balance() {
        assert!(imbalance(&leaking([f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0])).is_nan());
    }

    // ---- layout ----

    #[test]
    fn every_column_is_padded_to_its_header_or_its_widest_value() {
        // `CuFeS2` is wider than any of its flows; `total` and `T (K)` are narrower than theirs.
        let text = table(&solved_demo(), &report(18, 5.4e-10));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[..7],
            [
                "  #  stream                 CuFeS2     SiO2      H2O     total   T (K)",
                "  0  feed.mixer             40.000  360.000  600.000  1000.000  298.15",
                "  1  mixer.flotation        41.885  503.497  759.494  1304.875  298.15",
                "  2  flotation.concentrate  35.602   25.175  227.848   288.625  298.15",
                "  3  flotation.split         6.283  478.322  531.646  1016.250  298.15",
                "  4  split.mixer             1.885  143.497  159.494   304.875  298.15",
                "  5  split.tailings          4.398  334.825  372.152   711.375  298.15",
            ]
        );
        // The iteration count and residual are supplied, so they are pinned; the imbalance is
        // computed from a real solve and its digits are float noise, so only its label is.
        assert!(
            lines[7].starts_with("     converged in 18 iterations, residual 5.4e-10, imbalance "),
            "{}",
            lines[7]
        );
    }

    #[test]
    fn the_footer_starts_where_the_stream_column_does() {
        let text = table(&solved_demo(), &report(18, 5.4e-10));
        let footer = text.lines().last().expect("there is a footer");
        let stream_column = text
            .lines()
            .next()
            .expect("there is a header")
            .find("stream")
            .expect("the header names the stream column");
        assert_eq!(footer.len() - footer.trim_start().len(), stream_column);
    }

    #[test]
    fn a_single_pass_is_reported_in_the_singular() {
        let text = table(&colliding_labels(), &report(1, 0.0));
        assert!(
            text.ends_with("converged in 1 iteration, residual 0.0e0, imbalance 0.0e0\n"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_flowsheet_still_renders_its_header() {
        let fs = Flowsheet::new(demo_registry())
            .validate()
            .expect("a flowsheet with no units is trivially valid");
        assert_eq!(
            table(&fs, &report(0, 0.0)),
            concat!(
                "  #  stream  CuFeS2  SiO2  H2O  total  T (K)\n",
                "     converged in 0 iterations, residual 0.0e0, imbalance 0.0e0\n",
            )
        );
    }

    #[test]
    fn no_row_carries_trailing_whitespace() {
        // The last column is right-aligned, so padding never reaches the end of a line.
        for line in table(&solved_demo(), &report(18, 5.4e-10)).lines() {
            assert_eq!(line, line.trim_end(), "trailing space on `{line}`");
        }
    }
}

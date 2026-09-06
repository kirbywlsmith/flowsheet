//! Rendering a solved flowsheet as a fixed-width table.
//!
//! This lives in the library rather than the binary because a stream's endpoints are only
//! recoverable from the units that list it, and those port vectors are crate-private. Keeping
//! the formatter here also keeps it free of any CLI dependency, so it survived the split into
//! the `flowsheet` library crate unchanged.

use crate::flowsheet::ValidFlowsheet;
use crate::solver::SolveReport;
use std::collections::HashMap;
use std::fmt::Write;

/// Leading indent on every line.
const INDENT: &str = "  ";
/// Gap between two columns.
const GAP: &str = "  ";
/// Decimal places on every flow.
const PRECISION: usize = 3;
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

/// Renders every stream of a solved flowsheet as one row, followed by a convergence footer.
///
/// The leading `#` column is the stream's position in the `streams` array, which is its real
/// identity; the label beside it is for reading, and may be suffixed to break a tie.
///
/// ```text
///   #  stream         CuFeS2     SiO2      H2O     total
///   0  feed.mixer     40.000  360.000  600.000  1000.000
///   1  mixer.tank     57.143  514.286  857.143  1428.571
///      converged in 18 iterations, residual 5.4e-10
/// ```
pub fn table(fs: &ValidFlowsheet, report: &SolveReport) -> String {
    let mut header: Vec<String> = vec!["#".to_string(), "stream".to_string()];
    header.extend(fs.registry().all().iter().map(|s| s.name.clone()));
    header.push("total".to_string());

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
        "{:gutter$}converged in {} iteration{plural}, residual {:.1e}",
        "", report.iterations, report.residual
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
    use crate::solver::Solver;
    use crate::stream::Stream;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use crate::unit::UnitOp;

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

        let u_feed = fs.add_unit("feed", UnitOp::Feed { stream: feed_flows });
        let u_split = fs.add_unit("split", UnitOp::Splitter { fraction: 0.3 });
        let u_mixer = fs.add_unit("mixer", UnitOp::Mixer);
        let u_product = fs.add_unit("product", UnitOp::Product);

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
                "mixer.tank",
                "tank.split",
                "split.mixer",
                "split.product",
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

    // ---- layout ----

    #[test]
    fn every_column_is_padded_to_its_header_or_its_widest_value() {
        // `CuFeS2` is wider than any of its flows; `total` is narrower than all of its.
        assert_eq!(
            table(&solved_demo(), &report(18, 5.4e-10)),
            concat!(
                "  #  stream         CuFeS2     SiO2      H2O     total\n",
                "  0  feed.mixer     40.000  360.000  600.000  1000.000\n",
                "  1  mixer.tank     57.143  514.286  857.143  1428.571\n",
                "  2  tank.split     57.143  514.286  857.143  1428.571\n",
                "  3  split.mixer    17.143  154.286  257.143   428.571\n",
                "  4  split.product  40.000  360.000  600.000  1000.000\n",
                "     converged in 18 iterations, residual 5.4e-10\n",
            )
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
            text.ends_with("converged in 1 iteration, residual 0.0e0\n"),
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
                "  #  stream  CuFeS2  SiO2  H2O  total\n",
                "     converged in 0 iterations, residual 0.0e0\n",
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

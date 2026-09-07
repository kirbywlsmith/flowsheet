//! How [`Solver::solve`] scales, along the two axes that decide it.
//!
//! - `solve/units` deliberately measures graph analysis and evaluation together. `solve` runs
//!   [`Flowsheet::tear_streams`] and [`Flowsheet::evaluation_waves_with_tears`] up front, both
//!   O(V+E), and a caller cannot opt out of either.
//! - `solve/tears` isolates the per-tear cost of a pass - the snapshot clone and residual
//!   comparison - because identical loops all converge in the same number of passes, whatever
//!   `k` is.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use flowsheet::demo::{AMBIENT_K, AMBIENT_KPA};
use flowsheet::unit::{Feed, Mixer, Product, Splitter, Tank};
use flowsheet::{Flowsheet, Solver, Stream, ValidFlowsheet, demo};

const RECYCLE_FRACTION: f64 = 0.3;
const UNIT_COUNTS: [usize; 4] = [8, 64, 512, 4096];
const LOOP_COUNTS: [usize; 5] = [1, 2, 4, 8, 16];

/// `Feed -> Tank * n -> Product`.
///
/// Acyclic, so the tear set is empty and a solve is exactly one pass whatever the streams
/// held beforehand.
fn chain(tanks: usize) -> ValidFlowsheet {
    let registry = demo::registry();
    let feed = demo::feed_stream(&registry);
    let blank = Stream::zeros(&registry, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(registry);

    let mut upstream = fs.add_unit("feed", Feed { stream: feed });
    for i in 0..tanks {
        let tank = fs.add_unit(format!("tank{i}"), Tank);
        fs.add_stream(upstream, blank.clone(), tank);
        upstream = tank;
    }
    let product = fs.add_unit("product", Product);
    fs.add_stream(upstream, blank, product);

    fs.validate().expect("the chain fixture is wired correctly")
}

/// `count` independent copies of one recycle loop, each its own two-unit component, so each
/// contributes exactly one tear.
///
/// ```text
///   Feed ──▶ Mixer ──▶ Splitter ──▶ Product
///              ▲           │
///              └───────────┘  (fraction, the tear)
/// ```
fn loops(count: usize) -> ValidFlowsheet {
    let registry = demo::registry();
    let feed = demo::feed_stream(&registry);
    let blank = Stream::zeros(&registry, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(registry);

    for i in 0..count {
        let u_feed = fs.add_unit(
            format!("feed{i}"),
            Feed {
                stream: feed.clone(),
            },
        );
        let u_mixer = fs.add_unit(format!("mixer{i}"), Mixer);
        let u_split = fs.add_unit(
            format!("split{i}"),
            Splitter {
                fraction: RECYCLE_FRACTION,
            },
        );
        let u_product = fs.add_unit(format!("product{i}"), Product);

        fs.add_stream(u_feed, blank.clone(), u_mixer);
        fs.add_stream(u_mixer, blank.clone(), u_split);
        let _recycle = fs.add_stream(u_split, blank.clone(), u_mixer);
        let _tailings = fs.add_stream(u_split, blank.clone(), u_product);
    }

    fs.validate().expect("the loops fixture is wired correctly")
}

fn solve_units(c: &mut Criterion) {
    let solver = Solver::default();
    let mut group = c.benchmark_group("solve/units");

    for tanks in UNIT_COUNTS {
        group.throughput(Throughput::Elements(tanks as u64));
        group.bench_with_input(BenchmarkId::from_parameter(tanks), &tanks, |b, &tanks| {
            // `solve` writes into the flowsheet, so a reused one is already converged and
            // would measure a single no-op pass. `LargeInput` keeps the live batch to a
            // thousandth of the iteration count; a 4096-unit flowsheet is about a megabyte.
            b.iter_batched_ref(
                || chain(tanks),
                |fs| solver.solve(fs).expect("an acyclic chain always converges"),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

fn solve_tears(c: &mut Criterion) {
    let solver = Solver::default();
    let mut group = c.benchmark_group("solve/tears");

    for count in LOOP_COUNTS {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter_batched_ref(
                || loops(count),
                |fs| {
                    solver
                        .solve(fs)
                        .expect("a 0.3 recycle converges well inside 100 passes")
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, solve_units, solve_tears);
criterion_main!(benches);

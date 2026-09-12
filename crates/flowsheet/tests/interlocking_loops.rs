//! A circuit whose loops interlock: two recycles in one component, sharing no stream.
//!
//!   feed -> bm -> bs -> a ---------> bm          (loop 1)
//!                 bs -> dm -> ds -> c -> dm      (loop 2)
//!                             ds -> bm           (loop 3, through both)
//!                             ds -> product
//!
//! No single stream lies on all three loops, so ordering it needs more than one tear. This is the
//! shape `tear_streams` tears in rounds for: one tear per component leaves a loop standing, and
//! the solve ends in `SolveError::Untearable`.

use approx::assert_relative_eq;
use flowsheet::demo::{feed_stream, registry};
use flowsheet::unit::{Feed, Mixer, Product, Splitter, SplitterN, Tank};
use flowsheet::{Flowsheet, SolveReport, Solver, Stream, ValidFlowsheet};

/// The circuit above, plus the id of the stream into the product.
fn circuit() -> (ValidFlowsheet, flowsheet::StreamId) {
    let r = registry();
    let blank = || Stream::zeros(&registry(), 298.15, 101.325);
    let mut fs = Flowsheet::new(registry());

    let u_feed = fs.add_unit(
        "feed",
        Feed {
            stream: feed_stream(&r),
        },
    );
    let bm = fs.add_unit("bm", Mixer);
    let bs = fs.add_unit("bs", Splitter { fraction: 0.5 });
    let a = fs.add_unit("a", Tank);
    let dm = fs.add_unit("dm", Mixer);
    let ds = fs.add_unit(
        "ds",
        SplitterN {
            ratios: vec![0.4, 0.3, 0.3],
        },
    );
    let c = fs.add_unit("c", Tank);
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank(), bm);
    fs.add_stream(bm, blank(), bs);
    fs.add_stream(bs, blank(), a);
    fs.add_stream(a, blank(), bm);
    fs.add_stream(bs, blank(), dm);
    fs.add_stream(dm, blank(), ds);
    fs.add_stream(ds, blank(), c);
    fs.add_stream(c, blank(), dm);
    fs.add_stream(ds, blank(), bm);
    let product = fs.add_stream(ds, blank(), u_product);

    (fs.validate().expect("the circuit is well-formed"), product)
}

fn solve(fs: &mut ValidFlowsheet) -> SolveReport {
    Solver::default()
        .solve(fs)
        .expect("two tears break both loops")
}

#[test]
fn it_takes_three_tears_where_two_would_do() {
    // Two tears are enough - a->bm with dm->ds, say, which between them touch all three loops.
    // The heuristic does not find that pair. Each round it cuts one back edge per component, and
    // the first round's lowest-id back edge is a->bm, which breaks only loop 1. Loop 2 costs a
    // second round and loop 3 a third.
    //
    // The extra tear costs one more stream in the solver's unknown vector. Finding the smallest
    // set is NP-hard, so ordering at all is what the heuristic promises.
    let (fs, _) = circuit();

    let tears = fs.tear_streams();

    assert_eq!(tears.len(), 3, "{tears:?}");
    assert!(fs.evaluation_waves_with_tears(&tears).is_ok());
}

#[test]
fn what_the_feed_brings_in_leaves_by_the_only_door() {
    // The single product is the only outlet, so at steady state it carries the whole feed -
    // whatever the two recycles do to the internal flows.
    let (mut fs, product) = circuit();

    solve(&mut fs);

    let feed = feed_stream(&registry());
    for (out, inn) in fs[product].flows().iter().zip(feed.flows()) {
        assert_relative_eq!(out, inn, max_relative = 1e-6);
    }
}

#[test]
fn both_recycles_inflate_the_internal_flows() {
    // bm carries the feed plus both returns, so it is strictly the larger of the two.
    let (mut fs, product) = circuit();

    solve(&mut fs);

    let internal: f64 = fs.streams()[1].flows().iter().sum();
    let out: f64 = fs[product].flows().iter().sum();
    assert!(internal > out, "{internal} vs {out}");
}

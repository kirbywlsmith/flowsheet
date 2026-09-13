//! Flowsheet graph functions.

use crate::flowsheet::{Flowsheet, StreamId, UnitId};

impl Flowsheet {
    /// The producing and consuming unit of every stream, indexed by [`StreamId`].
    ///
    /// Either end is `None` when nothing is wired to it. [`Flowsheet::check`] reports those, but
    /// the graph functions have to run on unvalidated flowsheets too.
    fn stream_ends(&self) -> (Vec<Option<UnitId>>, Vec<Option<UnitId>>) {
        let mut producer: Vec<Option<UnitId>> = vec![None; self.streams.len()];
        let mut consumer: Vec<Option<UnitId>> = vec![None; self.streams.len()];

        for (i, unit) in self.units.iter().enumerate() {
            let id = UnitId(i as u16);
            for &s in &unit.outlets {
                producer[s.as_usize()] = Some(id);
            }
            for &s in &unit.inlets {
                consumer[s.as_usize()] = Some(id);
            }
        }

        (producer, consumer)
    }

    /// The outgoing edges of every unit, indexed by [`UnitId`]: the stream, and the unit it feeds.
    ///
    /// A stream nothing consumes is not an edge. Two streams between the same pair of units stay
    /// two edges, because [`Flowsheet::evaluation_waves_with_tears`] counts inlet streams rather
    /// than neighbours - and because a tear names a stream, not a pair of units.
    fn edges(&self) -> Vec<Vec<(StreamId, UnitId)>> {
        let (_, consumer) = self.stream_ends();

        self.units
            .iter()
            .map(|unit| {
                unit.outlets
                    .iter()
                    .filter_map(|&s| Some((s, consumer[s.as_usize()]?)))
                    .collect()
            })
            .collect()
    }

    /// A `streams`-length mask of which streams are torn.
    fn torn_mask(&self, tears: &[StreamId]) -> Vec<bool> {
        let mut torn = vec![false; self.streams.len()];
        for &s in tears {
            torn[s.as_usize()] = true;
        }
        torn
    }

    /// The order a traversal visits units in: sources first, then everything else by id.
    ///
    /// Sources first is what makes the back edge of a loop come out as the recycle. Entering the
    /// demo circuit at the feed walks mixer, cell, splitter and finds the splitter's return to the
    /// mixer pointing back up the path; entering at the splitter would find the mixer's outlet
    /// instead, cutting the same loop at a stream whose zero first guess means a plant with no
    /// feed in it.
    fn visit_order(&self, torn: &[bool]) -> Vec<UnitId> {
        let ids = (0..self.units.len()).map(|i| UnitId(i as u16));
        let sources = ids.clone().filter(|u| {
            self.units[u.as_usize()]
                .inlets
                .iter()
                .all(|&s| torn[s.as_usize()])
        });

        sources.chain(ids).collect()
    }

    /// Picks the streams to drop from the evaluation order, for
    /// [`Flowsheet::evaluation_waves_with_tears`].
    ///
    /// A torn stream is not removed - only its ordering constraint is. Its consumer ends up
    /// evaluated first and so reads the previous pass's value, which is what the solver iterates
    /// on.
    ///
    /// Each round tears the back edge of every cyclic component - the edge the traversal found
    /// pointing back up its own path, which on a process flowsheet is the recycle - and then looks
    /// at what is left. One round is enough for a component holding a single loop; interlocking
    /// loops cost a round each, because no one stream lies on all of them.
    ///
    /// The result is not the smallest tear set possible. That is minimum feedback arc set, which
    /// is NP-hard.
    pub fn tear_streams(&self) -> Vec<StreamId> {
        let edges = self.edges();
        let mut torn = self.torn_mask(&[]);
        let mut tears = Vec::new();

        loop {
            let traversal = Tarjan::run(&edges, &torn, &self.visit_order(&torn));

            let mut component_of: Vec<usize> = vec![0; self.units.len()];
            for (c, component) in traversal.components.iter().enumerate() {
                for &unit in component {
                    component_of[unit.as_usize()] = c;
                }
            }

            // A back edge's ends are both inside one component, so its head names that component.
            let mut candidate: Vec<Option<StreamId>> = vec![None; traversal.components.len()];
            for &(stream, head) in &traversal.back_edges {
                let best = &mut candidate[component_of[head.as_usize()]];
                if best.is_none_or(|s| stream.as_usize() < s.as_usize()) {
                    *best = Some(stream);
                }
            }

            let mut round = Vec::new();
            for (c, _) in traversal
                .components
                .iter()
                .enumerate()
                // A singleton is only cyclic through a self-loop, which `check` rejects
                .filter(|(_, component)| component.len() > 1)
            {
                let Some(stream) = candidate[c] else {
                    debug_assert!(false, "a cyclic component always has a back edge");
                    continue;
                };
                torn[stream.as_usize()] = true;
                round.push(stream);
            }

            // Nothing cyclic left, or - only if the assertion above ever fires - nothing this
            // round could cut. Either way another round would repeat this one.
            if round.is_empty() {
                return tears;
            }

            tears.append(&mut round);
        }
    }

    /// Groups the flowsheet's units into topologically ordered evaluation waves.
    ///
    /// Wave `i` holds every unit whose inlets are all evaluated at wave `i - 1`.
    /// A wave's units are independent - they can be solved in parallel.
    ///
    /// # Errors
    ///
    /// Returns the units that could not be ordered - those inside a cycle or downstream of one.
    pub fn evaluation_waves(&self) -> Result<Vec<Vec<UnitId>>, Vec<UnitId>> {
        self.evaluation_waves_with_tears(&[])
    }

    /// [`Flowsheet::evaluation_waves`], ignoring the `tears` when ordering.
    ///
    /// This is what makes a cyclic flowsheet orderable: a torn stream stops counting as a
    /// dependency, so its consumer no longer waits on its producer.
    ///
    /// # Errors
    ///
    /// As [`Flowsheet::evaluation_waves`]. One tear per cyclic component is not always enough -
    /// a component of interlocking loops needs one per loop - so a short `tears` still leaves
    /// units unordered and reports them here.
    pub fn evaluation_waves_with_tears(
        &self,
        tears: &[StreamId],
    ) -> Result<Vec<Vec<UnitId>>, Vec<UnitId>> {
        let torn = self.torn_mask(tears);

        let edges = self.edges();

        // The unevaluated inlet stream count of each unit
        let mut in_degree: Vec<usize> = self
            .units
            .iter()
            .map(|u| u.inlets.iter().filter(|&&s| !torn[s.as_usize()]).count())
            .collect();

        let mut current: Vec<UnitId> = in_degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d == 0).then_some(UnitId(i as u16)))
            .collect();

        let mut waves: Vec<Vec<UnitId>> = Vec::new();

        while !current.is_empty() {
            let mut next = Vec::new();
            for &unit in &current {
                // A torn stream has to leave both halves of the count: if its consumer never waits
                // on it, its producer must not decrement it either, or the degree underflows.
                for &(stream, downstream) in &edges[unit.as_usize()] {
                    if torn[stream.as_usize()] {
                        continue;
                    }

                    let degree = &mut in_degree[downstream.as_usize()];
                    *degree -= 1;
                    if *degree == 0 {
                        next.push(downstream);
                    }
                }
            }

            waves.push(std::mem::replace(&mut current, next));
        }

        // Any leftover units with unevaluated inlet streams are a part of, or downstream of, a cycle
        let leftover: Vec<UnitId> = in_degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d > 0).then_some(UnitId(i as u16)))
            .collect();

        if leftover.is_empty() {
            Ok(waves)
        } else {
            Err(leftover)
        }
    }

    /// Groups the flowsheet's units into strongly connected components by Tarjan's algorithm.
    ///
    /// Components come back in reverse topological order - the most downstream first.
    pub fn components(&self) -> Vec<Vec<UnitId>> {
        let torn = self.torn_mask(&[]);
        Tarjan::run(&self.edges(), &torn, &self.visit_order(&torn)).components
    }
}

/// The scratch state of one run of Tarjan's algorithm.
struct Tarjan {
    /// Discovery order of each unit. `None` until it is first visited.
    index: Vec<Option<u32>>,
    /// The lowest discovery order reachable from each unit without leaving the current stack.
    lowlink: Vec<u32>,
    on_stack: Vec<bool>,
    /// Whether each unit is an ancestor of the unit being visited. Unlike `on_stack`, which holds
    /// the whole component being built, this holds only the path from the traversal's root.
    on_path: Vec<bool>,
    stack: Vec<UnitId>,
    next_index: u32,
    components: Vec<Vec<UnitId>>,
    /// Every edge found pointing back up the current path, as the stream and the unit it feeds.
    back_edges: Vec<(StreamId, UnitId)>,
}

impl Tarjan {
    /// Visits every unit in `order`, skipping torn edges, and returns the finished state.
    ///
    /// The traversal is depth-first, but on an explicit stack rather than by recursion, so its
    /// depth is bounded by the heap and not by the thread's stack - a single chain of tens of
    /// thousands of units is a legal flowsheet.
    fn run<'a>(edges: &'a [Vec<(StreamId, UnitId)>], torn: &[bool], order: &[UnitId]) -> Self {
        let mut tarjan = Self::new(edges.len());

        // One frame per unit on the current path: the unit, and the edges it has yet to look at.
        // The iterator borrows `edges`, not `call`, so taking an edge out of it ends the borrow of
        // the frame and leaves `call` free to push.
        let mut call: Vec<(UnitId, std::slice::Iter<'a, (StreamId, UnitId)>)> = Vec::new();

        for &root in order {
            if tarjan.index[root.as_usize()].is_some() {
                continue;
            }

            tarjan.enter(root);
            call.push((root, edges[root.as_usize()].iter()));

            while let Some((unit, rest)) = call.last_mut() {
                let u = unit.as_usize();

                let Some(&(stream, downstream)) = rest.next() else {
                    // Out of edges: the unit is done, and control returns to whoever reached it.
                    let unit = *unit;
                    call.pop();
                    tarjan.finish(unit);

                    // The line a recursive traversal runs right after its call returns: the parent
                    // inherits whatever the child could reach.
                    if let Some(&(parent, _)) = call.last() {
                        let p = parent.as_usize();
                        tarjan.lowlink[p] = tarjan.lowlink[p].min(tarjan.lowlink[u]);
                    }
                    continue;
                };

                if torn[stream.as_usize()] {
                    continue;
                }

                let d = downstream.as_usize();
                match tarjan.index[d] {
                    // Unvisited: descend. Its edges are looked at before this unit's next one.
                    None => {
                        tarjan.enter(downstream);
                        call.push((downstream, edges[d].iter()));
                    }
                    // Visited and still on the stack: an edge into the component being built.
                    Some(seen) if tarjan.on_stack[d] => {
                        tarjan.lowlink[u] = tarjan.lowlink[u].min(seen);
                        // The stack holds the whole component, so only `on_path` tells a back edge -
                        // a loop closing on an ancestor - from a cross edge into a sibling branch.
                        if tarjan.on_path[d] {
                            tarjan.back_edges.push((stream, downstream));
                        }
                    }
                    // Visited and already emitted: a cross edge into a finished component.
                    Some(_) => {}
                }
            }
        }

        tarjan
    }

    fn new(n: usize) -> Self {
        Self {
            index: vec![None; n],
            lowlink: vec![0; n],
            on_stack: vec![false; n],
            on_path: vec![false; n],
            stack: Vec::with_capacity(n),
            next_index: 0,
            components: Vec::new(),
            back_edges: Vec::new(),
        }
    }

    /// Discovers an unvisited unit: numbers it and puts it on both the stack and the path.
    fn enter(&mut self, unit: UnitId) {
        let u = unit.as_usize();
        let index = self.next_index;

        self.index[u] = Some(index);
        self.lowlink[u] = index;
        self.next_index += 1;
        self.stack.push(unit);
        self.on_stack[u] = true;
        self.on_path[u] = true;
    }

    /// Leaves a unit whose edges have all been looked at, emitting the component it roots, if any.
    fn finish(&mut self, unit: UnitId) {
        let u = unit.as_usize();

        self.on_path[u] = false;

        // Nothing under this unit reached above it, so it roots a component: everything pushed
        // since is part of it.
        if Some(self.lowlink[u]) == self.index[u] {
            let mut component = Vec::new();
            while let Some(popped) = self.stack.pop() {
                self.on_stack[popped.as_usize()] = false;
                component.push(popped);
                if popped == unit {
                    break;
                }
            }
            component.sort_unstable_by_key(|id| id.as_usize());
            self.components.push(component);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::species::SpeciesRegistry;
    use crate::stream::Stream;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use crate::unit::{Feed, Mixer, Product, Splitter, SplitterN, Tank};

    /// A placeholder stream value - the solver overwrites these.
    fn blank(registry: &SpeciesRegistry) -> Stream {
        Stream::zeros(registry, AMBIENT_K, AMBIENT_KPA)
    }

    #[test]
    fn edges_repeat_a_unit_reached_by_more_than_one_stream() {
        // Both splitter outlets land on the same mixer. The duplicate is load-bearing:
        // `evaluation_waves` decrements in-degree once per stream, not once per neighbour.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
        let u_split = fs.add_unit("split", Splitter { fraction: 0.5 });
        let u_mix = fs.add_unit("mix", Mixer);
        let u_product = fs.add_unit("product", Product);

        fs.add_stream(u_feed, blank(&r), u_split);
        fs.add_stream(u_split, blank(&r), u_mix);
        fs.add_stream(u_split, blank(&r), u_mix);
        fs.add_stream(u_mix, blank(&r), u_product);

        assert_eq!(
            fs.edges(),
            vec![
                vec![(StreamId(0), u_split)],
                vec![(StreamId(1), u_mix), (StreamId(2), u_mix)],
                vec![(StreamId(3), u_product)],
                Vec::new(),
            ]
        );
        assert!(fs.evaluation_waves().is_ok());
    }

    #[test]
    fn empty_flowsheet_has_no_components() {
        let fs = Flowsheet::new(demo_registry());
        assert_eq!(fs.components(), Vec::<Vec<UnitId>>::new());
    }

    #[test]
    fn every_unit_of_an_acyclic_chain_is_its_own_component() {
        // feed -> tank -> product, no loop anywhere.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
        let u_tank = fs.add_unit("tank", Tank);
        let u_product = fs.add_unit("product", Product);

        fs.add_stream(u_feed, blank(&r), u_tank);
        fs.add_stream(u_tank, blank(&r), u_product);

        // Reverse topological: the product is the sink, so it comes out first.
        assert_eq!(
            fs.components(),
            vec![vec![u_product], vec![u_tank], vec![u_feed]]
        );
    }

    #[test]
    fn a_recycle_collapses_into_one_component_in_reverse_topological_order() {
        // The target circuit: S4 sends the splitter's 0.3 side back to the mixer, so the
        // mixer, flotation cell and splitter can only be solved together.
        let fs = crate::demo::build_flowsheet();

        let components = fs.components();

        // Both products are sinks, so both come out before the loop that feeds them.
        assert_eq!(components.len(), 4);
        assert_eq!(components[0], vec![UnitId(3)], "the concentrate is a sink");
        assert_eq!(components[1], vec![UnitId(5)], "the tailings are a sink");
        assert_eq!(
            components[2],
            vec![UnitId(1), UnitId(2), UnitId(4)],
            "mixer, flotation and splitter form the loop"
        );
        assert_eq!(components[3], vec![UnitId(0)], "the feed is the source");

        // Reversing gives the order the solver evaluates in: source first.
        let mut order = components;
        order.reverse();
        assert_eq!(order[0], vec![UnitId(0)]);
    }

    #[test]
    fn two_independent_loops_stay_separate_components() {
        // feed -> split -> {loop A, loop B}, each a mixer/tank/bleed triangle feeding itself.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
        let u_split = fs.add_unit("split", Splitter { fraction: 0.5 });
        fs.add_stream(u_feed, blank(&r), u_split);

        let mut loops = Vec::new();
        for i in 0..2 {
            let u_mix = fs.add_unit(format!("mix{i}"), Mixer);
            let u_tank = fs.add_unit(format!("tank{i}"), Tank);
            let u_bleed = fs.add_unit(format!("bleed{i}"), Splitter { fraction: 0.4 });
            let u_product = fs.add_unit(format!("product{i}"), Product);

            fs.add_stream(u_split, blank(&r), u_mix);
            fs.add_stream(u_mix, blank(&r), u_tank);
            fs.add_stream(u_tank, blank(&r), u_bleed);
            fs.add_stream(u_bleed, blank(&r), u_mix); // the recycle
            fs.add_stream(u_bleed, blank(&r), u_product);

            loops.push(vec![u_mix, u_tank, u_bleed]);
        }

        let components = fs.components();

        assert_eq!(fs.check(), Vec::new(), "the fixture is wired correctly");
        assert!(components.contains(&loops[0]));
        assert!(components.contains(&loops[1]));
        assert_eq!(
            components.iter().filter(|c| c.len() > 1).count(),
            2,
            "one component per loop, never merged"
        );
    }

    #[test]
    fn tearing_the_recycle_circuit_makes_it_orderable() {
        // Untorn this is the `recycle_circuit_reports_the_units_it_could_not_order` case.
        // S4 (splitter -> mixer) is the tear, so the mixer runs first on last pass's recycle and
        // the rest of the circuit falls into process order behind it.
        let fs = crate::demo::build_flowsheet();

        let waves = fs
            .evaluation_waves_with_tears(&fs.tear_streams())
            .expect("one tear breaks the only loop");

        assert_eq!(
            waves,
            vec![
                vec![UnitId(0)],            // feed
                vec![UnitId(1)],            // mixer
                vec![UnitId(2)],            // flotation
                vec![UnitId(3), UnitId(4)], // concentrate, splitter
                vec![UnitId(5)],            // tailings
            ]
        );
    }

    #[test]
    fn tearing_a_stream_outside_the_loop_leaves_the_loop() {
        // S0 is the feed into the mixer. Cutting it drops the mixer's wait on the feed but not on
        // the splitter, so the loop survives - and both products are dragged down with it, being
        // downstream of units that never run.
        let fs = crate::demo::build_flowsheet();

        assert_eq!(
            fs.evaluation_waves_with_tears(&[StreamId(0)]),
            Err(vec![UnitId(1), UnitId(2), UnitId(3), UnitId(4), UnitId(5)])
        );
    }

    #[test]
    fn tearing_nothing_matches_the_untorn_ordering() {
        let fs = crate::demo::build_flowsheet();

        assert_eq!(fs.evaluation_waves_with_tears(&[]), fs.evaluation_waves());
    }

    #[test]
    fn an_acyclic_flowsheet_has_nothing_to_tear() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
        let u_tank = fs.add_unit("tank", Tank);
        let u_product = fs.add_unit("product", Product);

        fs.add_stream(u_feed, blank(&r), u_tank);
        fs.add_stream(u_tank, blank(&r), u_product);

        assert_eq!(fs.tear_streams(), Vec::new());
    }

    #[test]
    fn the_recycle_circuit_tears_the_recycle() {
        // All three of S1, S3 and S4 sit inside the loop and any one of them breaks it. The
        // traversal enters at the feed and walks mixer, cell, splitter, so the one pointing back
        // up that path is S4, the recycle - the only one of the three whose zero first guess
        // describes a real state of the plant, namely the first pass with nothing recycled yet.
        //
        // S0, the feed into the mixer, crosses into the loop from outside. Cutting it would leave
        // the loop intact, and a crossing edge is never a back edge.
        let fs = crate::demo::build_flowsheet();

        assert_eq!(fs.tear_streams(), vec![StreamId(4)]);
    }

    /// Two loops in one component, sharing no stream: an inner loop around `bm`, another around
    /// `dm`, and a third path that runs `bm -> dm -> bm`. No single stream sits on all three, so
    /// one tear cannot order it.
    ///
    ///   feed -> bm -> bs -> a --> bm        (loop 1)
    ///                 bs -> dm -> ds -> c -> dm   (loop 2)
    ///                             ds -> bm        (loop 3, through both)
    ///                             ds -> product
    fn interlocking_loops() -> Flowsheet {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
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

        fs.add_stream(u_feed, blank(&r), bm);
        fs.add_stream(bm, blank(&r), bs);
        fs.add_stream(bs, blank(&r), a);
        fs.add_stream(a, blank(&r), bm);
        fs.add_stream(bs, blank(&r), dm);
        fs.add_stream(dm, blank(&r), ds);
        fs.add_stream(ds, blank(&r), c);
        fs.add_stream(c, blank(&r), dm);
        fs.add_stream(ds, blank(&r), bm);
        fs.add_stream(ds, blank(&r), u_product);

        fs
    }

    #[test]
    fn interlocking_loops_are_torn_until_the_flowsheet_orders() {
        let fs = interlocking_loops();
        assert_eq!(fs.check(), Vec::new(), "the fixture is wired correctly");

        let tears = fs.tear_streams();

        assert!(
            tears.len() > 1,
            "one tear cannot break both loops: {tears:?}"
        );
        assert!(fs.evaluation_waves_with_tears(&tears).is_ok());
    }

    #[test]
    fn two_independent_loops_are_torn_once_each() {
        // Same fixture as `two_independent_loops_stay_separate_components`. Each loop holds three
        // internal streams, and the one torn is the bleed's return to its mixer - S4 and S9. The
        // streams from the splitter into each loop (S1, S6) cross in from outside and are never
        // back edges.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit("feed", Feed { stream: feed(&r) });
        let u_split = fs.add_unit("split", Splitter { fraction: 0.5 });
        fs.add_stream(u_feed, blank(&r), u_split);

        for i in 0..2 {
            let u_mix = fs.add_unit(format!("mix{i}"), Mixer);
            let u_tank = fs.add_unit(format!("tank{i}"), Tank);
            let u_bleed = fs.add_unit(format!("bleed{i}"), Splitter { fraction: 0.4 });
            let u_product = fs.add_unit(format!("product{i}"), Product);

            fs.add_stream(u_split, blank(&r), u_mix);
            fs.add_stream(u_mix, blank(&r), u_tank);
            fs.add_stream(u_tank, blank(&r), u_bleed);
            fs.add_stream(u_bleed, blank(&r), u_mix); // the recycle
            fs.add_stream(u_bleed, blank(&r), u_product);
        }

        // Component order is Tarjan's, so sort before comparing.
        let mut tears = fs.tear_streams();
        tears.sort_unstable_by_key(|s| s.as_usize());

        assert_eq!(tears, vec![StreamId(4), StreamId(9)]);
    }

    /// Deeper than any stack a traversal that recursed once per unit could fit in.
    const DEEP: u16 = 50_000;

    /// `DEEP` tanks, each feeding the next, and if `closed` the last feeding the first.
    ///
    /// Nothing here is validated - no feed, no product - because the graph functions never ask.
    /// Empty names and an empty registry keep it cheap, as in `units_filled_to_the_id_space`.
    fn deep_chain(closed: bool) -> Flowsheet {
        let empty = SpeciesRegistry::default();
        let mut fs = Flowsheet::new(SpeciesRegistry::default());

        let units: Vec<UnitId> = (0..DEEP).map(|_| fs.add_unit("", Tank)).collect();
        for pair in units.windows(2) {
            fs.add_stream(pair[0], blank(&empty), pair[1]);
        }
        if closed {
            fs.add_stream(units[units.len() - 1], blank(&empty), units[0]);
        }

        fs
    }

    /// Runs `f` on a thread with a small stack of known size.
    ///
    /// A test thread's default stack is 2 MiB and a debug frame is larger than a release one, so
    /// without this whether the old recursion overflowed would depend on the build. A stack
    /// overflow aborts the whole test binary rather than failing one test, so there is no
    /// `should_panic` equivalent to assert it with.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .expect("the thread spawns")
            .join()
            .expect("the traversal does not panic")
    }

    #[test]
    fn a_chain_deeper_than_the_stack_is_one_component_per_unit() {
        let fs = deep_chain(false);

        let components = on_small_stack(move || fs.components());

        assert_eq!(components.len(), DEEP as usize);
        // Reverse topological: the far end of the chain is the sink, so it comes out first.
        assert_eq!(components[0], vec![UnitId(DEEP - 1)]);
    }

    #[test]
    fn a_loop_deeper_than_the_stack_is_torn_once_at_its_back_edge() {
        // Every unit's lowlink has to travel back up all the way from the last unit, which is the
        // half of Tarjan an explicit stack moves around.
        let fs = deep_chain(true);

        let tears = on_small_stack(move || fs.tear_streams());

        assert_eq!(tears, vec![StreamId(DEEP - 1)]);
    }
}

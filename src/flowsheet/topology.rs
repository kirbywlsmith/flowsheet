//! Flowsheet graph functions.

use crate::flowsheet::{Flowsheet, UnitId};

impl Flowsheet {
    /// The downstream units of every unit, indexed by [`UnitId`].
    fn successors(&self) -> Vec<Vec<UnitId>> {
        // The consuming unit of each stream
        let mut consumer: Vec<Option<UnitId>> = vec![None; self.streams.len()];
        for (i, unit) in self.units.iter().enumerate() {
            for &s in &unit.inlets {
                consumer[s.as_usize()] = Some(UnitId(i as u16));
            }
        }

        self.units
            .iter()
            .map(|unit| {
                unit.outlets
                    .iter()
                    .filter_map(|&s| consumer[s.as_usize()])
                    .collect()
            })
            .collect()
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
        let successors = self.successors();

        // The unevaluated inlet stream count of each unit
        let mut in_degree: Vec<usize> = self.units.iter().map(|u| u.inlets.len()).collect();

        let mut current: Vec<UnitId> = in_degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d == 0).then_some(UnitId(i as u16)))
            .collect();

        let mut waves: Vec<Vec<UnitId>> = Vec::new();

        while !current.is_empty() {
            let mut next = Vec::new();
            for &unit in &current {
                for &downstream in &successors[unit.as_usize()] {
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
    ///
    /// Note that the traversal recurses, so it is bounded by the stack.
    pub fn components(&self) -> Vec<Vec<UnitId>> {
        let successors = self.successors();
        let mut tarjan = Tarjan::new(&successors);

        for i in 0..self.units.len() {
            if tarjan.index[i].is_none() {
                tarjan.visit(UnitId(i as u16));
            }
        }

        tarjan.components
    }
}

/// The scratch state of one run of Tarjan's algorithm.
struct Tarjan<'a> {
    successors: &'a [Vec<UnitId>],
    /// Discovery order of each unit. `None` until it is first visited.
    index: Vec<Option<u32>>,
    /// The lowest discovery order reachable from each unit without leaving the current stack.
    lowlink: Vec<u32>,
    on_stack: Vec<bool>,
    stack: Vec<UnitId>,
    next_index: u32,
    components: Vec<Vec<UnitId>>,
}

impl<'a> Tarjan<'a> {
    fn new(successors: &'a [Vec<UnitId>]) -> Self {
        let n = successors.len();
        Self {
            successors,
            index: vec![None; n],
            lowlink: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::with_capacity(n),
            next_index: 0,
            components: Vec::new(),
        }
    }

    /// Visits one unvisited unit, emitting every component rooted at or below it.
    fn visit(&mut self, unit: UnitId) {
        let u = unit.as_usize();
        let index = self.next_index;

        self.index[u] = Some(index);
        self.lowlink[u] = index;
        self.next_index += 1;
        self.stack.push(unit);
        self.on_stack[u] = true;

        // `successors` is a shared reference, so copying it out of `self` detaches it from the
        // borrow - otherwise iterating it would hold `self` immutably across the `&mut self` call.
        let successors = self.successors;
        for &downstream in &successors[u] {
            let d = downstream.as_usize();
            match self.index[d] {
                // Unvisited: recurse, then inherit whatever it could reach.
                None => {
                    self.visit(downstream);
                    self.lowlink[u] = self.lowlink[u].min(self.lowlink[d]);
                }
                // Visited and still on the stack: a back edge into the component being built.
                Some(seen) if self.on_stack[d] => {
                    self.lowlink[u] = self.lowlink[u].min(seen);
                }
                // Visited and already emitted: a cross edge into a finished component.
                Some(_) => {}
            }
        }

        // Nothing under this unit reached above it, so it roots a component: everything pushed
        // since is part of it.
        if self.lowlink[u] == index {
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
    use crate::unit::UnitOp;

    /// A placeholder stream value - the solver overwrites these.
    fn blank(registry: &SpeciesRegistry) -> Stream {
        Stream::zeros(registry, AMBIENT_K, AMBIENT_KPA)
    }

    #[test]
    fn successors_repeat_a_unit_reached_by_more_than_one_stream() {
        // Both splitter outlets land on the same mixer. The duplicate is load-bearing:
        // `evaluation_waves` decrements in-degree once per stream, not once per neighbour.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.5 });
        let u_mix = fs.add_unit(UnitOp::Mixer);
        let u_product = fs.add_unit(UnitOp::Product);

        fs.add_stream(u_feed, blank(&r), u_split);
        fs.add_stream(u_split, blank(&r), u_mix);
        fs.add_stream(u_split, blank(&r), u_mix);
        fs.add_stream(u_mix, blank(&r), u_product);

        assert_eq!(
            fs.successors(),
            vec![
                vec![u_split],
                vec![u_mix, u_mix],
                vec![u_product],
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

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_tank = fs.add_unit(UnitOp::Tank);
        let u_product = fs.add_unit(UnitOp::Product);

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
        // mixer, tank and splitter can only be solved together.
        let fs = crate::demo::build_flowsheet();

        let components = fs.components();

        assert_eq!(components.len(), 3);
        assert_eq!(components[0].len(), 1, "the product is a sink");
        assert_eq!(
            components[1],
            vec![UnitId(1), UnitId(2), UnitId(3)],
            "mixer, tank and splitter form the loop"
        );
        assert_eq!(components[2], vec![UnitId(0)], "the feed is the source");

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

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.5 });
        fs.add_stream(u_feed, blank(&r), u_split);

        let mut loops = Vec::new();
        for _ in 0..2 {
            let u_mix = fs.add_unit(UnitOp::Mixer);
            let u_tank = fs.add_unit(UnitOp::Tank);
            let u_bleed = fs.add_unit(UnitOp::Splitter { fraction: 0.4 });
            let u_product = fs.add_unit(UnitOp::Product);

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
}

//! Flowsheet data structures.

use crate::species::SpeciesRegistry;
use crate::stream::Stream;
use crate::units::{mix, split, split_n};

/// Used to index a [`Flowsheet`]'s units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitId(u16);

impl UnitId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Used to index a [`Flowsheet`]'s streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(u16);

impl StreamId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// The different types of supported unit operations and their parameters.
#[derive(Debug, Clone)]
pub enum UnitOp {
    /// One outlet.
    Feed {
        /// The outlet [`Stream`].
        stream: Stream,
    },
    /// Combines all inlets into one outlet.
    Mixer,
    /// One inlet, two outlets: `fraction` and `1.0 - fraction`.
    Splitter {
        /// The `fraction` to pass to [`split`]
        fraction: f64,
    },
    /// One inlet, one outlet per ratio.
    SplitterN {
        /// The `ratios` to pass to [`split_n`]
        ratios: Vec<f64>,
    },
    /// One inlet, one outlet
    Tank,
    /// One inlet
    Product,
}

impl UnitOp {
    /// Evaluates a unit operation's outlet [`Stream`]s.
    pub fn evaluate(&self, inlets: &[&Stream]) -> Vec<Stream> {
        match self {
            UnitOp::Feed { stream } => vec![stream.clone()],
            UnitOp::Mixer => {
                vec![mix(inlets.iter().copied()).expect("mixer needs at least one inlet")]
            }
            UnitOp::Tank => vec![inlets[0].clone()],
            UnitOp::Splitter { fraction } => {
                let (a, b) = split(inlets[0], *fraction);
                vec![a, b]
            }
            UnitOp::SplitterN { ratios } => split_n(inlets[0], ratios),
            UnitOp::Product => vec![],
        }
    }
}

/// A distinct section of a system that takes inlet [`Stream`]s and performs a [`UnitOp`] to produce outlet streams.
#[derive(Debug)]
pub struct Unit {
    op: UnitOp,
    inlets: Vec<StreamId>,
    outlets: Vec<StreamId>,
}

/// Represents the [`Unit`]s, [`SpeciesRegistry`] and [`Stream`]s that make up a process.
#[derive(Debug)]
pub struct Flowsheet {
    registry: SpeciesRegistry,
    units: Vec<Unit>,
    streams: Vec<Stream>,
}

/// A type of validation error returned during [`Flowsheet::validate`].
#[derive(Debug)]
pub enum FlowsheetError {
    /// A unit contains an unexpected number of inlet streams.
    WrongInletCount {
        /// The specific unit.
        unit: UnitId,
        /// The expected number of inlet streams.
        expected: usize,
        /// The actual number of inlet streams.
        found: usize,
    },
    /// A unit contains an unexpected number of outlet streams.
    WrongOutletCount {
        /// The specific unit.
        unit: UnitId,
        /// The expected number of outlet streams.
        expected: usize,
        /// The actual number of outlet streams.
        found: usize,
    },
    /// A stream is missing a source and/or target unit.
    DanglingStream {
        /// The specific stream.
        stream: StreamId,
    },
}

impl Flowsheet {
    /// Creates a new [`Flowsheet`].
    pub fn new(registry: SpeciesRegistry) -> Self {
        Self {
            registry,
            units: Vec::new(),
            streams: Vec::new(),
        }
    }

    /// Returns the flowsheet's [`SpeciesRegistry`].
    pub fn registry(&self) -> &SpeciesRegistry {
        &self.registry
    }

    /// Adds a [`Unit`] of the specified [`UnitOp`], with no inlets or outlets.
    pub fn add_unit(&mut self, op: UnitOp) -> UnitId {
        self.units.push(Unit {
            op,
            inlets: Vec::new(),
            outlets: Vec::new(),
        });
        UnitId((self.units.len() - 1) as u16)
    }

    /// Connects `start` to `end` with the given [`Stream`].
    ///
    /// Note that order of a unit's inlets and outlets is determined by the way this is called.
    pub fn add_stream(&mut self, start: UnitId, stream: Stream, end: UnitId) -> StreamId {
        self.streams.push(stream);
        let stream_id = StreamId((self.streams.len() - 1) as u16);
        self.units[start.as_usize()].outlets.push(stream_id);
        self.units[end.as_usize()].inlets.push(stream_id);
        stream_id
    }

    // TODO: when implementing the solver, have validate return flowsheet errors OR a ValidFlowsheet wrapper. Only expose solve() on the ValidFlowsheet
    /// Validates the current state of the flowsheet.
    pub fn validate(&self) -> Result<(), Vec<FlowsheetError>> {
        // TODO: implement this
        Ok(())
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
        // The consuming unit of each stream
        let mut consumer: Vec<Option<UnitId>> = vec![None; self.streams.len()];
        for (i, unit) in self.units.iter().enumerate() {
            for &s in &unit.inlets {
                consumer[s.as_usize()] = Some(UnitId(i as u16));
            }
        }

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
                for &s in &self.units[unit.as_usize()].outlets {
                    let Some(downstream) = consumer[s.as_usize()] else {
                        continue;
                    };
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

    /// Evaluates one unit, writing its results into the unit's outlet streams.
    pub(crate) fn evaluate_unit(&mut self, id: UnitId) {
        let Flowsheet { units, streams, .. } = self;
        let unit = &units[id.as_usize()];

        let outputs = {
            let inlets: Vec<&Stream> = unit
                .inlets
                .iter()
                .map(|&s| &streams[s.as_usize()])
                .collect();

            unit.op.evaluate(&inlets)
        };

        debug_assert_eq!(
            outputs.len(),
            unit.outlets.len(),
            "unit {id:?} outlet count mismatch during evaluation"
        );
        for (&s, out) in unit.outlets.iter().zip(outputs) {
            streams[s.as_usize()] = out;
        }
    }
}

impl std::ops::Index<StreamId> for Flowsheet {
    type Output = Stream;
    fn index(&self, id: StreamId) -> &Stream {
        &self.streams[id.as_usize()]
    }
}

impl std::ops::IndexMut<StreamId> for Flowsheet {
    fn index_mut(&mut self, id: StreamId) -> &mut Stream {
        &mut self.streams[id.as_usize()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{AMBIENT_K, AMBIENT_KPA, demo_registry, feed};
    use approx::assert_relative_eq;

    /// A placeholder stream value — the solver overwrites these.
    fn blank(registry: &SpeciesRegistry) -> Stream {
        Stream::zeros(registry, AMBIENT_K, AMBIENT_KPA)
    }

    // ---- arena ----

    #[test]
    fn add_unit_returns_sequential_ids() {
        let mut fs = Flowsheet::new(demo_registry());
        let a = fs.add_unit(UnitOp::Mixer);
        let b = fs.add_unit(UnitOp::Tank);
        assert_eq!(a.as_usize(), 0);
        assert_eq!(b.as_usize(), 1);
        assert_eq!(fs.units.len(), 2);
    }

    #[test]
    fn add_stream_wires_the_outlet_on_start_and_the_inlet_on_end() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let tank = fs.add_unit(UnitOp::Tank);
        let product = fs.add_unit(UnitOp::Product);

        let s = fs.add_stream(tank, blank(&r), product);

        assert_eq!(fs.units[tank.as_usize()].outlets, vec![s]);
        assert!(fs.units[tank.as_usize()].inlets.is_empty());
        assert_eq!(fs.units[product.as_usize()].inlets, vec![s]);
        assert!(fs.units[product.as_usize()].outlets.is_empty());
    }

    #[test]
    fn outlets_are_recorded_in_call_order() {
        // Pins the documented behaviour: the FIRST outlet added to a splitter
        // receives `fraction`, the second `1.0 - fraction`. Swapping the two
        // add_stream calls silently changes which stream is the recycle.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let splitter = fs.add_unit(UnitOp::Splitter { fraction: 0.3 });
        let first = fs.add_unit(UnitOp::Product);
        let second = fs.add_unit(UnitOp::Product);

        let s_a = fs.add_stream(splitter, blank(&r), first);
        let s_b = fs.add_stream(splitter, blank(&r), second);

        assert_eq!(fs.units[splitter.as_usize()].outlets, vec![s_a, s_b]);
    }

    #[test]
    fn streams_can_be_read_and_written_by_id() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let tank = fs.add_unit(UnitOp::Tank);
        let product = fs.add_unit(UnitOp::Product);
        let s = fs.add_stream(tank, blank(&r), product);

        assert_relative_eq!(fs[s].total(), 0.0);
        fs[s] = feed(&r);
        assert_relative_eq!(fs[s].total(), 1000.0);
    }

    // ---- evaluate ----

    #[test]
    fn feed_emits_its_stream_and_ignores_inlets() {
        let r = demo_registry();
        let op = UnitOp::Feed { stream: feed(&r) };

        let outs = op.evaluate(&[]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1000.0);
    }

    #[test]
    fn mixer_sums_all_inlets_into_one_outlet() {
        let r = demo_registry();
        let a = feed(&r);
        let b = Stream::from_flows(&r, vec![10.0, 20.0, 30.0], AMBIENT_K, AMBIENT_KPA);

        let outs = UnitOp::Mixer.evaluate(&[&a, &b]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1060.0, max_relative = 1e-12);
    }

    #[test]
    fn tank_passes_its_inlet_straight_through() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Tank.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 1);
        assert!(inlet.flows_approx_eq(&outs[0], 1e-12));
    }

    #[test]
    fn splitter_returns_the_fraction_side_first() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Splitter { fraction: 0.3 }.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 2);
        assert_relative_eq!(outs[0].total(), 300.0, max_relative = 1e-12);
        assert_relative_eq!(outs[1].total(), 700.0, max_relative = 1e-12);
    }

    #[test]
    fn splitter_n_returns_one_outlet_per_ratio() {
        let r = demo_registry();
        let inlet = feed(&r);
        let op = UnitOp::SplitterN {
            ratios: vec![1.0, 1.0, 2.0],
        };

        let outs = op.evaluate(&[&inlet]);

        assert_eq!(outs.len(), 3);
        assert_relative_eq!(outs[0].total(), 250.0, max_relative = 1e-12);
        assert_relative_eq!(outs[2].total(), 500.0, max_relative = 1e-12);
    }

    #[test]
    fn product_consumes_its_inlet_and_emits_nothing() {
        let r = demo_registry();
        let outs = UnitOp::Product.evaluate(&[&feed(&r)]);
        assert!(outs.is_empty());
    }

    // ---- the target circuit ----

    #[test]
    fn target_recycle_circuit_wires_up_correctly() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_mixer = fs.add_unit(UnitOp::Mixer);
        let u_tank = fs.add_unit(UnitOp::Tank);
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.3 });
        let u_product = fs.add_unit(UnitOp::Product);

        let s0 = fs.add_stream(u_feed, blank(&r), u_mixer);
        let s1 = fs.add_stream(u_mixer, blank(&r), u_tank);
        let s2 = fs.add_stream(u_tank, blank(&r), u_split);
        // Recycle added FIRST so it takes the 0.3 side of the splitter.
        let s4 = fs.add_stream(u_split, blank(&r), u_mixer);
        let s3 = fs.add_stream(u_split, blank(&r), u_product);

        assert_eq!(fs.units.len(), 5);
        assert_eq!(fs.streams.len(), 5);

        assert!(fs.units[u_feed.as_usize()].inlets.is_empty());
        assert_eq!(fs.units[u_feed.as_usize()].outlets, vec![s0]);

        assert_eq!(fs.units[u_mixer.as_usize()].inlets, vec![s0, s4]);
        assert_eq!(fs.units[u_mixer.as_usize()].outlets, vec![s1]);

        assert_eq!(fs.units[u_tank.as_usize()].inlets, vec![s1]);
        assert_eq!(fs.units[u_tank.as_usize()].outlets, vec![s2]);

        assert_eq!(fs.units[u_split.as_usize()].inlets, vec![s2]);
        assert_eq!(fs.units[u_split.as_usize()].outlets, vec![s4, s3]);

        assert_eq!(fs.units[u_product.as_usize()].inlets, vec![s3]);
        assert!(fs.units[u_product.as_usize()].outlets.is_empty());
    }

    // ---- topological sort ----

    #[test]
    fn empty_flowsheet_has_no_waves() {
        let fs = Flowsheet::new(demo_registry());
        assert_eq!(fs.evaluation_waves().unwrap(), Vec::<Vec<UnitId>>::new());
    }

    #[test]
    fn acyclic_circuit_sorts_into_one_wave_per_depth() {
        // The demo circuit is the target flowsheet with the recycle not yet wired:
        // feed -> mixer -> tank -> splitter -> {bleed, product}.
        let fs = crate::demo::build_flowsheet();

        let waves = fs.evaluation_waves().expect("demo circuit is acyclic");

        assert_eq!(
            waves,
            vec![
                vec![UnitId(0)], // feed
                vec![UnitId(1)], // mixer
                vec![UnitId(2)], // tank
                vec![UnitId(3)], // splitter
                // Both splitter outlets clear at once — nothing orders them
                // relative to each other.
                vec![UnitId(4), UnitId(5)],
            ]
        );
    }

    #[test]
    fn parallel_branches_share_a_wave_and_the_merge_waits_for_both() {
        // feed -> splitter -> {tank_a, tank_b} -> mixer -> product
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.5 });
        let u_a = fs.add_unit(UnitOp::Tank);
        let u_b = fs.add_unit(UnitOp::Tank);
        let u_mix = fs.add_unit(UnitOp::Mixer);
        let u_product = fs.add_unit(UnitOp::Product);

        fs.add_stream(u_feed, blank(&r), u_split);
        fs.add_stream(u_split, blank(&r), u_a);
        fs.add_stream(u_split, blank(&r), u_b);
        fs.add_stream(u_a, blank(&r), u_mix);
        fs.add_stream(u_b, blank(&r), u_mix);
        fs.add_stream(u_mix, blank(&r), u_product);

        let waves = fs.evaluation_waves().expect("diamond is acyclic");

        assert_eq!(
            waves,
            vec![
                vec![u_feed],
                vec![u_split],
                vec![u_a, u_b], // independent — the point of emitting waves
                vec![u_mix],    // waits for BOTH branches, not just the first
                vec![u_product],
            ]
        );
    }

    #[test]
    fn recycle_circuit_reports_the_units_it_could_not_order() {
        // The full target circuit. S4 closes the loop, so no topological order exists
        // and the solver has to tear the recycle instead.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_mixer = fs.add_unit(UnitOp::Mixer);
        let u_tank = fs.add_unit(UnitOp::Tank);
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.3 });
        let u_product = fs.add_unit(UnitOp::Product);

        fs.add_stream(u_feed, blank(&r), u_mixer);
        fs.add_stream(u_mixer, blank(&r), u_tank);
        fs.add_stream(u_tank, blank(&r), u_split);
        fs.add_stream(u_split, blank(&r), u_mixer); // S4, the recycle
        fs.add_stream(u_split, blank(&r), u_product);

        let leftover = fs
            .evaluation_waves()
            .expect_err("recycle circuit is cyclic");

        // The feed still sorts; everything from the mixer onwards is blocked, either
        // by the cycle itself or by sitting downstream of it.
        assert_eq!(leftover, vec![u_mixer, u_tank, u_split, u_product]);
    }
}

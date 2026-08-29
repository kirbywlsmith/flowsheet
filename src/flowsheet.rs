use crate::stream::Stream;
use crate::units::{mix, split, split_n};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitId(u16);

impl UnitId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(u16);

impl StreamId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
pub enum UnitOp {
    /// One outlet
    Feed { stream: Stream },
    /// Combines all inlets into one outlet.
    Mixer,
    /// One inlet, two outlets: `fraction` and `1.0 - fraction`.
    Splitter { fraction: f64 },
    /// One inlet, one outlet per ratio.
    SplitterN { ratios: Vec<f64> },
    /// One inlet, one outlet
    Tank,
    /// One inlet
    Product,
}

impl UnitOp {
    pub fn evaluate(&self, inlets: &[Stream]) -> Vec<Stream> {
        match self {
            UnitOp::Feed { stream } => vec![stream.clone()],
            UnitOp::Mixer => vec![mix(inlets).expect("mixer needs at least one inlet")],
            UnitOp::Tank => vec![inlets[0].clone()],
            UnitOp::Splitter { fraction } => {
                let (a, b) = split(&inlets[0], *fraction);
                vec![a, b]
            }
            UnitOp::SplitterN { ratios } => split_n(&inlets[0], ratios),
            UnitOp::Product => vec![],
        }
    }
}

#[derive(Debug)]
pub struct Unit {
    op: UnitOp,
    inlets: Vec<StreamId>,
    outlets: Vec<StreamId>,
}

#[derive(Debug, Default)]
pub struct Flowsheet {
    units: Vec<Unit>,
    streams: Vec<Stream>,
}

#[derive(Debug)]
pub enum FlowsheetError {
    WrongInletCount {
        unit: UnitId,
        expected: usize,
        found: usize,
    },
    WrongOutletCount {
        unit: UnitId,
        expected: usize,
        found: usize,
    },
    DanglingStream {
        stream: StreamId,
    },
}

impl Flowsheet {
    /// Adds a unit of the specified type, with no inlets or outlets.
    pub fn add_unit(&mut self, op: UnitOp) -> UnitId {
        self.units.push(Unit {
            op,
            inlets: Vec::new(),
            outlets: Vec::new(),
        });
        UnitId((self.units.len() - 1) as u16)
    }
    
    /// Connects `start` to `end` with a new stream.
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
    pub fn validate(&self) -> Result<(), Vec<FlowsheetError>> {
        todo!()
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

    /// A placeholder stream value — the solver overwrites these during a solve.
    fn blank(registry: &crate::species::SpeciesRegistry) -> Stream {
        Stream::zeros(registry, AMBIENT_K, AMBIENT_KPA)
    }

    // ---- arena ----

    #[test]
    fn add_unit_returns_sequential_ids() {
        let mut fs = Flowsheet::default();
        let a = fs.add_unit(UnitOp::Mixer);
        let b = fs.add_unit(UnitOp::Tank);
        assert_eq!(a.as_usize(), 0);
        assert_eq!(b.as_usize(), 1);
        assert_eq!(fs.units.len(), 2);
    }

    #[test]
    fn add_stream_wires_the_outlet_on_start_and_the_inlet_on_end() {
        let r = demo_registry();
        let mut fs = Flowsheet::default();
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
        let mut fs = Flowsheet::default();
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
        let mut fs = Flowsheet::default();
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

        let outs = UnitOp::Mixer.evaluate(&[a, b]);

        assert_eq!(outs.len(), 1);
        assert_relative_eq!(outs[0].total(), 1060.0, max_relative = 1e-12);
    }

    #[test]
    fn tank_passes_its_inlet_straight_through() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Tank.evaluate(std::slice::from_ref(&inlet));

        assert_eq!(outs.len(), 1);
        assert!(inlet.flows_approx_eq(&outs[0], 1e-12));
    }

    #[test]
    fn splitter_returns_the_fraction_side_first() {
        let r = demo_registry();
        let inlet = feed(&r);

        let outs = UnitOp::Splitter { fraction: 0.3 }.evaluate(std::slice::from_ref(&inlet));

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

        let outs = op.evaluate(std::slice::from_ref(&inlet));

        assert_eq!(outs.len(), 3);
        assert_relative_eq!(outs[0].total(), 250.0, max_relative = 1e-12);
        assert_relative_eq!(outs[2].total(), 500.0, max_relative = 1e-12);
    }

    #[test]
    fn product_consumes_its_inlet_and_emits_nothing() {
        let r = demo_registry();
        let outs = UnitOp::Product.evaluate(std::slice::from_ref(&feed(&r)));
        assert!(outs.is_empty());
    }

    // ---- the target circuit ----

    #[test]
    fn target_recycle_circuit_wires_up_correctly() {
        let r = demo_registry();
        let mut fs = Flowsheet::default();

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
}

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

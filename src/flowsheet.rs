//! Flowsheet data structures.

mod topology;

use crate::species::SpeciesRegistry;
use crate::stream::Stream;
use crate::unit::{Unit, UnitOp};
use std::fmt;

/// Used to index a [`Flowsheet`]'s units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitId(u16);

impl UnitId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for UnitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Represents the [`Unit`]s, [`SpeciesRegistry`] and [`Stream`]s that make up a process.
#[derive(Debug)]
pub struct Flowsheet {
    registry: SpeciesRegistry,
    units: Vec<Unit>,
    streams: Vec<Stream>,
}

/// A type of validation error returned during [`Flowsheet::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowsheetError {
    /// A unit contains an unexpected number of inlet streams.
    WrongInletCount {
        /// The specific unit.
        unit: UnitId,
        /// The fewest inlet streams the unit's [`UnitOp`] accepts.
        min: usize,
        /// The most inlet streams the unit's [`UnitOp`] accepts. `None` is unbounded.
        max: Option<usize>,
        /// The actual number of inlet streams.
        found: usize,
    },
    /// A unit contains an unexpected number of outlet streams.
    WrongOutletCount {
        /// The specific unit.
        unit: UnitId,
        /// The fewest outlet streams the unit's [`UnitOp`] produces.
        min: usize,
        /// The most outlet streams the unit's [`UnitOp`] produces. `None` is unbounded.
        max: Option<usize>,
        /// The actual number of outlet streams.
        found: usize,
    },
    /// A stream leaves a unit and comes straight back into it.
    SelfLoop {
        /// The specific unit.
        unit: UnitId,
        /// The specific stream.
        stream: StreamId,
    },
}

fn write_arity(
    f: &mut fmt::Formatter<'_>,
    unit: UnitId,
    port: &str,
    min: usize,
    max: Option<usize>,
    found: usize,
) -> fmt::Result {
    match max {
        Some(max) => write!(f, "unit {unit} has {found} {port}, expected {min} to {max}"),
        None => write!(f, "unit {unit} has {found} {port}, expected at least {min}"),
    }
}

impl fmt::Display for FlowsheetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowsheetError::WrongInletCount {
                unit,
                max,
                min,
                found,
            } => write_arity(f, *unit, "inlets", *min, *max, *found),
            FlowsheetError::WrongOutletCount {
                unit,
                max,
                min,
                found,
            } => write_arity(f, *unit, "outlets", *min, *max, *found),
            FlowsheetError::SelfLoop { unit, stream } => {
                write!(f, "stream {stream} leaves unit {unit} and returns to it")
            }
        }
    }
}

impl std::error::Error for FlowsheetError {}

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

    /// Collects all structural problems in the flowsheet. An empty [`Vec`] means it is valid.
    pub fn check(&self) -> Vec<FlowsheetError> {
        let mut errors = Vec::new();

        for (i, unit) in self.units.iter().enumerate() {
            let id = UnitId(i as u16);

            let inlets = unit.op.inlet_arity();
            if !inlets.permits(unit.inlets.len()) {
                errors.push(FlowsheetError::WrongInletCount {
                    unit: id,
                    min: inlets.min,
                    max: inlets.max,
                    found: unit.inlets.len(),
                });
            }

            let outlets = unit.op.outlet_arity();
            if !outlets.permits(unit.outlets.len()) {
                errors.push(FlowsheetError::WrongOutletCount {
                    unit: id,
                    min: outlets.min,
                    max: outlets.max,
                    found: unit.outlets.len(),
                });
            }

            for &s in &unit.outlets {
                if unit.inlets.contains(&s) {
                    errors.push(FlowsheetError::SelfLoop {
                        unit: id,
                        stream: s,
                    });
                }
            }
        }

        errors
    }

    /// Consumes the flowsheet and returns a [`ValidFlowsheet`] if it is valid.
    ///
    /// # Errors
    ///
    /// Returns every [`FlowsheetError`] found by [`Flowsheet::check`].
    pub fn validate(self) -> Result<ValidFlowsheet, Vec<FlowsheetError>> {
        let errors = self.check();
        if errors.is_empty() {
            Ok(ValidFlowsheet(self))
        } else {
            Err(errors)
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

/// A [`Flowsheet`] that has passed [`Flowsheet::validate`].
#[derive(Debug)]
pub struct ValidFlowsheet(Flowsheet);

impl ValidFlowsheet {
    /// Returns the wrapped [`Flowsheet`].
    pub fn into_inner(self) -> Flowsheet {
        self.0
    }

    /// Evaluates one unit, writing its results into the unit's outlet streams.
    pub(crate) fn evaluate_unit(&mut self, id: UnitId) {
        self.0.evaluate_unit(id);
    }
}

/// Read-only access to everything on [`Flowsheet`].
impl std::ops::Deref for ValidFlowsheet {
    type Target = Flowsheet;
    fn deref(&self) -> &Flowsheet {
        &self.0
    }
}

impl std::ops::Index<StreamId> for ValidFlowsheet {
    type Output = Stream;
    fn index(&self, id: StreamId) -> &Stream {
        &self.0[id]
    }
}

impl std::ops::IndexMut<StreamId> for ValidFlowsheet {
    fn index_mut(&mut self, id: StreamId) -> &mut Stream {
        &mut self.0[id]
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

    fn acyclic_flowsheet() -> Flowsheet {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());

        let u_feed = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let u_mixer = fs.add_unit(UnitOp::Mixer);
        let u_tank = fs.add_unit(UnitOp::Tank);
        let u_split = fs.add_unit(UnitOp::Splitter { fraction: 0.3 });
        let u_bleed = fs.add_unit(UnitOp::Product);
        let u_product = fs.add_unit(UnitOp::Product);

        fs.add_stream(u_feed, blank(&r), u_mixer);
        fs.add_stream(u_mixer, blank(&r), u_tank);
        fs.add_stream(u_tank, blank(&r), u_split);
        fs.add_stream(u_split, blank(&r), u_bleed);
        fs.add_stream(u_split, blank(&r), u_product);

        fs
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

    // ---- validation ----

    #[test]
    fn a_correctly_wired_flowsheet_has_no_errors() {
        assert_eq!(crate::demo::build_flowsheet().check(), Vec::new());
    }

    #[test]
    fn an_unwired_unit_is_reported_on_both_sides() {
        // A lone tank has neither the inlet nor the outlet it needs — one error each,
        // because `check` never stops at the first problem.
        let mut fs = Flowsheet::new(demo_registry());
        let tank = fs.add_unit(UnitOp::Tank);

        assert_eq!(
            fs.check(),
            vec![
                FlowsheetError::WrongInletCount {
                    unit: tank,
                    min: 1,
                    max: Some(1),
                    found: 0,
                },
                FlowsheetError::WrongOutletCount {
                    unit: tank,
                    min: 1,
                    max: Some(1),
                    found: 0,
                },
            ]
        );
    }

    #[test]
    fn a_mixer_accepts_any_number_of_inlets_but_not_zero() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let mixer = fs.add_unit(UnitOp::Mixer);
        let product = fs.add_unit(UnitOp::Product);
        fs.add_stream(mixer, blank(&r), product);

        // No inlets yet — the variadic side still has a floor of one.
        assert!(matches!(
            fs.check().as_slice(),
            [FlowsheetError::WrongInletCount { unit, .. }] if *unit == mixer
        ));

        for _ in 0..3 {
            let source = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
            fs.add_stream(source, blank(&r), mixer);
        }

        assert_eq!(fs.check(), Vec::new());
    }

    #[test]
    fn a_splitter_is_invalid_until_its_second_outlet_is_wired() {
        // The point the incremental design turns on: a half-built unit is *temporarily*
        // wrong, which is exactly why arity cannot be checked inside `add_stream`.
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let source = fs.add_unit(UnitOp::Feed { stream: feed(&r) });
        let splitter = fs.add_unit(UnitOp::Splitter { fraction: 0.3 });
        let first = fs.add_unit(UnitOp::Product);

        fs.add_stream(source, blank(&r), splitter);
        fs.add_stream(splitter, blank(&r), first);

        assert_eq!(
            fs.check(),
            vec![FlowsheetError::WrongOutletCount {
                unit: splitter,
                min: 2,
                max: Some(2),
                found: 1,
            }]
        );

        let second = fs.add_unit(UnitOp::Product);
        fs.add_stream(splitter, blank(&r), second);
        assert_eq!(fs.check(), Vec::new());
    }

    #[test]
    fn a_stream_from_a_unit_back_into_itself_is_a_self_loop() {
        let r = demo_registry();
        let mut fs = Flowsheet::new(demo_registry());
        let tank = fs.add_unit(UnitOp::Tank);

        let s = fs.add_stream(tank, blank(&r), tank);

        assert_eq!(
            fs.check(),
            vec![FlowsheetError::SelfLoop {
                unit: tank,
                stream: s,
            }]
        );
    }

    #[test]
    fn validate_hands_back_a_solvable_flowsheet() {
        let valid = acyclic_flowsheet()
            .validate()
            .expect("the chain is correctly wired");

        // Deref gives read-only access to the wrapped flowsheet.
        assert_eq!(valid.units.len(), 6);
        assert!(valid.evaluation_waves().is_ok());
    }

    #[test]
    fn validate_reports_every_error_at_once() {
        // Two units, each broken on both sides: four errors from one call.
        let mut fs = Flowsheet::new(demo_registry());
        fs.add_unit(UnitOp::Tank);
        fs.add_unit(UnitOp::Splitter { fraction: 0.3 });

        let errors = fs.validate().expect_err("neither unit is wired");

        assert_eq!(errors.len(), 4);
    }

    // ---- topological sort ----

    #[test]
    fn empty_flowsheet_has_no_waves() {
        let fs = Flowsheet::new(demo_registry());
        assert_eq!(fs.evaluation_waves().unwrap(), Vec::<Vec<UnitId>>::new());
    }

    #[test]
    fn acyclic_circuit_sorts_into_one_wave_per_depth() {
        let fs = acyclic_flowsheet();

        let waves = fs.evaluation_waves().expect("the chain is acyclic");

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

    #[test]
    fn self_loop_names_the_stream() {
        let e = FlowsheetError::SelfLoop {
            unit: UnitId(2),
            stream: StreamId(5),
        };
        assert_eq!(e.to_string(), "stream 5 leaves unit 2 and returns to it");
    }

    #[test]
    fn arity_message_names_both_bounds_when_the_maximum_is_finite() {
        let e = FlowsheetError::WrongOutletCount {
            unit: UnitId(1),
            min: 2,
            max: Some(2),
            found: 3,
        };
        assert_eq!(e.to_string(), "unit 1 has 3 outlets, expected 2 to 2");
    }

    #[test]
    fn arity_message_reports_an_unbounded_maximum_as_a_floor() {
        let e = FlowsheetError::WrongInletCount {
            unit: UnitId(0),
            min: 1,
            max: None,
            found: 0,
        };
        assert_eq!(e.to_string(), "unit 0 has 0 inlets, expected at least 1");
    }
}

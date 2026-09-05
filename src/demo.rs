//! The worked example flowsheet from the project README — a simple recycle circuit.
//!
//! Kept in the library (rather than `examples/`) so `main.rs`, integration tests,
//! and benchmarks can all build the same flowsheet.

use crate::flowsheet::{Flowsheet, StreamId};
use crate::species::{Phase, Species, SpeciesRegistry};
use crate::stream::Stream;
use crate::unit::UnitOp;

/// 25 °C in Kelvin.
pub const AMBIENT_K: f64 = 298.15;
/// 1 atm in kPa.
pub const AMBIENT_KPA: f64 = 101.325;

/// The three [`Species`] of the demo circuit: the valuable mineral, the gangue, and water.
pub fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    r.insert(Species {
        name: "CuFeS2".into(),
        phase: Phase::Solid,
        molar_mass: 183.5,
    });
    r.insert(Species {
        name: "SiO2".into(),
        phase: Phase::Solid,
        molar_mass: 60.08,
    });
    r.insert(Species {
        name: "H2O".into(),
        phase: Phase::Liquid,
        molar_mass: 18.015,
    });
    r
}

/// The plant feed S0 — 40 / 360 / 600 t/h, 1000 t/h total.
pub fn feed_stream(registry: &SpeciesRegistry) -> Stream {
    Stream::from_flows(registry, vec![40.0, 360.0, 600.0], AMBIENT_K, AMBIENT_KPA)
}

/// Builds a demo [`Flowsheet`].
pub fn build_flowsheet() -> Flowsheet {
    recycle_flowsheet(0.3)
}

/// Builds a recycle circuit with a configurable splitter `fraction`.
pub fn recycle_flowsheet(fraction: f64) -> Flowsheet {
    recycle_circuit(fraction).flowsheet
}

/// The demo recycle circuit, together with the [`StreamId`]s `add_stream` handed back.
///
/// A built [`Flowsheet`] has no way to recover which id belongs to which connection, so a
/// test that wants to assert on a particular stream has to keep the ids from build time.
///
/// ```text
///   Feed ──S0──▶ Mixer ──S1──▶ Tank ──S2──▶ Splitter ──S4──┐  recycle (f)
///                  ▲                            │          │
///                  └────────────────────────────┼──────────┘
///                                               └──S3──▶ Product (1 - f)
/// ```
#[derive(Debug)]
pub struct RecycleCircuit {
    /// The wired-but-unvalidated flowsheet. Call [`Flowsheet::validate`] before solving.
    pub flowsheet: Flowsheet,
    /// S0 — the plant feed entering the mixer.
    pub feed: StreamId,
    /// S1 — mixer outlet, the circulating load. Carries `1 / (1 - fraction)` of the feed.
    pub mixer_out: StreamId,
    /// S2 — tank outlet, identical to S1 because the tank is a pass-through.
    pub tank_out: StreamId,
    /// S4 — the recycle back to the mixer, and the stream the solver tears.
    pub recycle: StreamId,
    /// S3 — the product leaving the plant. Equals the feed at steady state.
    pub product: StreamId,
}

/// Builds the recycle circuit with a configurable splitter `fraction`, keeping its stream ids.
pub fn recycle_circuit(fraction: f64) -> RecycleCircuit {
    let r = registry();

    // Built while `r` is still owned here, before it moves into the flowsheet.
    let feed_flows = feed_stream(&r);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(r);

    let u_feed = fs.add_unit(UnitOp::Feed { stream: feed_flows });
    let u_mixer = fs.add_unit(UnitOp::Mixer);
    let feed = fs.add_stream(u_feed, blank.clone(), u_mixer);

    let u_tank = fs.add_unit(UnitOp::Tank);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_tank);

    let u_split = fs.add_unit(UnitOp::Splitter { fraction });
    let tank_out = fs.add_stream(u_tank, blank.clone(), u_split);

    // Outlet order is positional: the first stream out of the splitter gets `fraction`.
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);

    let u_product = fs.add_unit(UnitOp::Product);
    let product = fs.add_stream(u_split, blank, u_product);

    RecycleCircuit {
        flowsheet: fs,
        feed,
        mixer_out,
        tank_out,
        recycle,
        product,
    }
}

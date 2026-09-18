//! The demo flowsheet — a flotation circuit with a recycle.
//!
//! Kept in the library (rather than `examples/`) so `main.rs`, integration tests,
//! and benchmarks can all build the same flowsheet.

use crate::flowsheet::{Flowsheet, StreamId};
use crate::species::{Phase, Species, SpeciesRegistry};
use crate::stream::Stream;
use crate::thermo::{Antoine, Shomate};
use crate::unit::{Feed, Flotation, Mixer, Product, Splitter};

/// 25 °C in Kelvin.
pub const AMBIENT_K: f64 = 298.15;
/// 1 atm in kPa.
pub const AMBIENT_KPA: f64 = 101.325;

/// The rougher cell's recovery to concentrate, in [`crate::species::SpeciesId`] order.
///
/// Most of the chalcopyrite floats, almost none of the gangue, and the water splits somewhere
/// in between - it is carried over mechanically rather than floated.
pub const ROUGHER_RECOVERY: [f64; 3] = [0.85, 0.05, 0.30];

/// Liquid water, fitted over 298-500 K. NIST-JANAF (Chase, 1998), via the NIST Chemistry WebBook.
pub const WATER_CP: Shomate = Shomate {
    a: -203.6060,
    b: 1523.290,
    c: -3196.413,
    d: 2474.455,
    e: 3.855326,
};

/// Alpha quartz, fitted over 298-847 K. NIST-JANAF (Chase, 1998), via the NIST Chemistry WebBook.
pub const QUARTZ_CP: Shomate = Shomate {
    a: -6.076591,
    b: 251.6755,
    c: -324.7964,
    d: 168.5604,
    e: 0.002548,
};

/// Chalcopyrite, as an estimated constant 95.0 J/(mol·K).
///
/// NIST publishes no Shomate fit for chalcopyrite, so this is the Neumann-Kopp rule: a solid's
/// heat capacity is roughly the sum of its elements'. One Cu, one Fe and two S at 298.15 K, from
/// their NIST-JANAF fits, give 24.47 + 25.10 + 2 × 22.70 = 94.97. It is held constant rather than
/// summed term by term, because the rule is not precise enough to justify a polynomial. Sulfur's
/// fit also stops at its melting point, 388 K.
pub const CHALCOPYRITE_CP: Shomate = Shomate::constant(95.0);

/// Water's vapour pressure over 344-373 K, from the NIST Chemistry WebBook (Stull, 1947), with
/// NIST's `A` of 5.08354 in bar raised by 2 for kPa.
///
/// Not part of the demo registry - nothing in the circuit evaporates - but the fit the crate's
/// steam-table tests are written against.
pub const WATER_VAPOUR_PRESSURE: Antoine = Antoine {
    a: 7.08354,
    b: 1663.125,
    c: -45.622,
};

/// The three [`Species`] of the demo circuit: the valuable mineral, the gangue, and water.
pub fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    r.insert(Species {
        name: "CuFeS2".into(),
        phase: Phase::Solid,
        molar_mass: 183.5,
        shomate: CHALCOPYRITE_CP,
        enthalpy_of_formation: None,
        density: None,
        vapour_pressure: None,
    });
    r.insert(Species {
        name: "SiO2".into(),
        phase: Phase::Solid,
        molar_mass: 60.08,
        shomate: QUARTZ_CP,
        enthalpy_of_formation: None,
        density: None,
        vapour_pressure: None,
    });
    r.insert(Species {
        name: "H2O".into(),
        phase: Phase::Liquid,
        molar_mass: 18.015,
        shomate: WATER_CP,
        enthalpy_of_formation: None,
        density: None,
        vapour_pressure: None,
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

/// Builds the flotation circuit with a configurable splitter `fraction`.
pub fn recycle_flowsheet(fraction: f64) -> Flowsheet {
    recycle_circuit(fraction).flowsheet
}

/// The demo flotation circuit, together with the [`StreamId`]s `add_stream` handed back.
///
/// A built [`Flowsheet`] has no way to recover which id belongs to which connection, so a
/// test that wants to assert on a particular stream has to keep the ids from build time.
///
/// ```text
///   Feed ──S0──▶ Mixer ──S1──▶ Flotation ──S2──▶ Concentrate
///                  ▲               │
///                  │              S3 (tails)
///                  │               ▼
///                  └────S4────  Splitter ──S5──▶ Tailings
///                    recycle (f)            (1 - f)
/// ```
///
/// The recycle is what makes this worth solving: the tails that would otherwise leave are sent
/// back to the cell for another chance to float, so the circuit recovers more of the mineral
/// than one pass could.
#[derive(Debug)]
pub struct RecycleCircuit {
    /// The wired-but-unvalidated flowsheet. Call [`Flowsheet::validate`] before solving.
    pub flowsheet: Flowsheet,
    /// S0 — the plant feed entering the mixer.
    pub feed: StreamId,
    /// S1 — mixer outlet, the circulating load entering the cell.
    pub mixer_out: StreamId,
    /// S2 — the flotation concentrate, upgraded in the valuable species.
    pub concentrate: StreamId,
    /// S3 — the flotation tails, on their way to the splitter.
    pub tails: StreamId,
    /// S4 — the recycle back to the mixer, and the stream the solver tears.
    pub recycle: StreamId,
    /// S5 — the final tailings leaving the plant.
    pub tailings: StreamId,
}

/// Builds the flotation circuit with a configurable splitter `fraction`, keeping its stream ids.
pub fn recycle_circuit(fraction: f64) -> RecycleCircuit {
    let r = registry();

    // Built while `r` is still owned here, before it moves into the flowsheet.
    let feed_flows = feed_stream(&r);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(r);

    let u_feed = fs.add_unit("feed", Feed { stream: feed_flows });
    let u_mixer = fs.add_unit("mixer", Mixer::default());
    let feed = fs.add_stream(u_feed, blank.clone(), u_mixer);

    let u_cell = fs.add_unit(
        "flotation",
        Flotation {
            recovery: ROUGHER_RECOVERY.to_vec(),
            pressure_drop: 0.0,
        },
    );
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_cell);

    // Outlet order is positional: the cell's first outlet is the concentrate.
    let u_concentrate = fs.add_unit("concentrate", Product);
    let concentrate = fs.add_stream(u_cell, blank.clone(), u_concentrate);

    let u_split = fs.add_unit(
        "split",
        Splitter {
            fraction,
            pressure_drop: 0.0,
        },
    );
    let tails = fs.add_stream(u_cell, blank.clone(), u_split);

    // And the splitter's first outlet gets `fraction`.
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);

    let u_tailings = fs.add_unit("tailings", Product);
    let tailings = fs.add_stream(u_split, blank, u_tailings);

    RecycleCircuit {
        flowsheet: fs,
        feed,
        mixer_out,
        concentrate,
        tails,
        recycle,
        tailings,
    }
}

/// The exact steady-state solution of [`recycle_circuit`], species by species.
///
/// Each species travels the loop independently, so a scalar balance per species is enough:
/// with `F` the feed, `r` the recovery and `f` the recycle fraction, the mixer outlet `M`
/// satisfies `M = F + f(1 - r)M`, giving `M = F / (1 - f(1 - r))`. Everything else follows.
///
/// This is what the tests assert against, and the reason the circuit stays hand-checkable
/// even though the flotation cell has made the arithmetic species-dependent.
#[derive(Debug, Clone, Copy)]
pub struct Balance {
    /// The circulating load entering the cell.
    pub mixer_out: f64,
    /// What reports to the concentrate.
    pub concentrate: f64,
    /// What leaves the cell in the tails.
    pub tails: f64,
    /// What the splitter sends back to the mixer.
    pub recycle: f64,
    /// What leaves the plant in the tailings.
    pub tailings: f64,
}

/// Solves [`Balance`] by hand for one species.
///
/// # Panics
/// If `f * (1.0 - recovery)` is 1.0 - a circuit that recycles everything never converges.
pub fn balance(feed: f64, recovery: f64, fraction: f64) -> Balance {
    let denominator = 1.0 - fraction * (1.0 - recovery);
    assert!(
        denominator > 0.0,
        "a circuit that recycles all of its tails has no steady state"
    );

    let mixer_out = feed / denominator;
    let tails = mixer_out * (1.0 - recovery);

    Balance {
        mixer_out,
        concentrate: mixer_out * recovery,
        tails,
        recycle: tails * fraction,
        tailings: tails * (1.0 - fraction),
    }
}

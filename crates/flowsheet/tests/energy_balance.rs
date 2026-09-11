//! Steady-state energy balance across a flotation circuit fed at two temperatures.
//!
//! ```text
//!   hot water ──┐
//!               ▼
//!   cold ore ──▶ Mixer ──▶ Flotation ──▶ Concentrate
//!                 ▲            │ tails
//!                 │            ▼
//!                 └───────── Splitter ──▶ Tailings
//! ```
//!
//! Nothing here adds or removes heat, and every operation after the mixer keeps its inlet's
//! temperature. So at steady state every internal stream sits at one temperature: the one the two
//! feeds would reach if mixed on their own. The recycle enters the mixer at that temperature and
//! leaves it at that temperature, so it cancels out of the balance. That keeps the answer
//! hand-checkable, the way `demo::balance` does for mass.
//!
//! 1. With constant heat capacities that temperature has a closed form, and the solve reaches it.
//! 2. With the demo's Shomate fits it has none, but it is still exactly what
//!    [`flowsheet::unit::mix`] gives for the two feeds alone.
//! 3. Enthalpy in equals enthalpy out: the two feeds carry in as much as the two products carry
//!    out.

use approx::assert_relative_eq;
use flowsheet::demo::{self, AMBIENT_K, AMBIENT_KPA, ROUGHER_RECOVERY};
use flowsheet::unit::{self, Feed, Flotation, Mixer, Product, Splitter};
use flowsheet::{
    Flowsheet, Phase, Shomate, Solver, Species, SpeciesRegistry, Stream, StreamId, ValidFlowsheet,
};

/// Process water at 60 °C.
const HOT_FLOWS: [f64; 3] = [0.0, 0.0, 400.0];
const HOT_K: f64 = 333.15;
/// The demo's plant feed, at 25 °C.
const COLD_FLOWS: [f64; 3] = [40.0, 360.0, 600.0];

/// Per species, in `SpeciesId` order: molar mass (g/mol) and a round, constant heat capacity
/// (J/(mol·K)) close to each one's value at 25 °C.
const MOLAR_MASS: [f64; 3] = [183.5, 60.08, 18.015];
const CONSTANT_CP: [f64; 3] = [95.0, 44.6, 75.3];

/// A solved circuit, with the feeds it was given and the ids of the streams the tests read.
struct Solved {
    flowsheet: ValidFlowsheet,
    hot: Stream,
    cold: Stream,
    /// Every stream downstream of the mixer, recycle included.
    internal: Vec<StreamId>,
    concentrate: StreamId,
    tailings: StreamId,
}

/// The demo's three species, with the heat capacities held constant.
fn constant_cp_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    let species = [
        ("CuFeS2", Phase::Solid),
        ("SiO2", Phase::Solid),
        ("H2O", Phase::Liquid),
    ];
    for (i, (name, phase)) in species.into_iter().enumerate() {
        r.insert(Species {
            name: name.into(),
            phase,
            molar_mass: MOLAR_MASS[i],
            shomate: Shomate::constant(CONSTANT_CP[i]),
        });
    }
    r
}

/// Builds and solves the circuit over species from `registry`.
///
/// Takes a constructor rather than a registry because `Flowsheet::new` consumes one, and
/// `SpeciesRegistry` is deliberately not `Clone`, and the feed streams need one of their own.
fn solve(registry: fn() -> SpeciesRegistry) -> Solved {
    let r = registry();
    let hot = Stream::from_flows(&r, HOT_FLOWS.to_vec(), HOT_K, AMBIENT_KPA);
    let cold = Stream::from_flows(&r, COLD_FLOWS.to_vec(), AMBIENT_K, AMBIENT_KPA);
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);

    let mut fs = Flowsheet::new(registry());
    let u_hot = fs.add_unit(
        "hot",
        Feed {
            stream: hot.clone(),
        },
    );
    let u_cold = fs.add_unit(
        "cold",
        Feed {
            stream: cold.clone(),
        },
    );
    let u_mixer = fs.add_unit("mixer", Mixer);
    let u_cell = fs.add_unit(
        "flotation",
        Flotation {
            recovery: ROUGHER_RECOVERY.to_vec(),
        },
    );
    let u_concentrate = fs.add_unit("concentrate", Product);
    let u_split = fs.add_unit("split", Splitter { fraction: 0.3 });
    let u_tailings = fs.add_unit("tailings", Product);

    fs.add_stream(u_hot, blank.clone(), u_mixer);
    fs.add_stream(u_cold, blank.clone(), u_mixer);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_cell);
    // Outlet order is positional: the cell's first outlet is the concentrate.
    let concentrate = fs.add_stream(u_cell, blank.clone(), u_concentrate);
    let tails = fs.add_stream(u_cell, blank.clone(), u_split);
    let recycle = fs.add_stream(u_split, blank.clone(), u_mixer);
    let tailings = fs.add_stream(u_split, blank, u_tailings);

    let mut flowsheet = fs.validate().expect("the circuit is wired correctly");
    Solver::default()
        .solve(&mut flowsheet)
        .expect("the circuit converges");

    Solved {
        flowsheet,
        hot,
        cold,
        internal: vec![mixer_out, concentrate, tails, recycle, tailings],
        concentrate,
        tailings,
    }
}

#[test]
fn constant_heat_capacities_settle_at_the_weighted_mean_of_the_feeds() {
    let s = solve(constant_cp_registry);

    // Worked by hand, without the library: each feed's heat capacity flow in MJ/(h·K) is
    // sum(t/h * kJ/(kg·K)), and a species' kJ/(kg·K) is its J/(mol·K) over its g/mol. With cp
    // constant, enthalpy is linear in temperature and the weighted mean, about 311 K, is exact.
    let heat_capacity = |flows: [f64; 3]| -> f64 {
        (0..3)
            .map(|i| flows[i] * CONSTANT_CP[i] / MOLAR_MASS[i])
            .sum()
    };
    let (c_hot, c_cold) = (heat_capacity(HOT_FLOWS), heat_capacity(COLD_FLOWS));
    let expected = (c_hot * HOT_K + c_cold * AMBIENT_K) / (c_hot + c_cold);

    for &id in &s.internal {
        assert_relative_eq!(s.flowsheet[id].temperature(), expected, max_relative = 1e-6);
    }
}

#[test]
fn the_recycle_does_not_shift_the_temperature_the_feeds_mix_to() {
    let s = solve(demo::registry);
    let r = s.flowsheet.registry();

    // The Shomate fits have no closed form, but the recycle cancels out of the balance, so the
    // loop has to settle wherever the two feeds would mix with nothing else in the mixer.
    let expected = unit::mix(r, [&s.hot, &s.cold])
        .expect("two inlets")
        .temperature();
    assert!(AMBIENT_K < expected && expected < HOT_K, "{expected}");

    for &id in &s.internal {
        assert_relative_eq!(s.flowsheet[id].temperature(), expected, max_relative = 1e-6);
    }
}

#[test]
fn enthalpy_in_equals_enthalpy_out() {
    let s = solve(demo::registry);
    let r = s.flowsheet.registry();

    let entering = s.hot.enthalpy(r) + s.cold.enthalpy(r);
    let leaving = s.flowsheet[s.concentrate].enthalpy(r) + s.flowsheet[s.tailings].enthalpy(r);

    assert!(entering > 0.0, "the hot feed carries heat above 25 °C");
    assert_relative_eq!(leaving, entering, max_relative = 1e-6);
}

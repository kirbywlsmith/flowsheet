//! Steady-state solves through a conversion reactor, checked against the extent worked by hand.
//!
//! ```text
//!   feed ──▶ Mixer ──▶ Reactor ──▶ Splitter ──▶ Product
//!             ▲                       │ recycle
//!             └───────────────────────┘
//! ```
//!
//! The reaction is `CH4 + 2 O2 -> CO2 + 2 H2O` at conversion `X`, limited by the methane. Methane
//! is the only species whose balance is closed by the reactor alone, and it travels the loop the
//! way a species travels the flotation circuit in `demo::balance`, with `X` in place of the
//! recovery: `A_mix = F_A + f(1 - X) A_mix`, so `A_mix = F_A / (1 - f(1 - X))`.
//!
//! Every other species follows from the boundary. All of the reactor's outlet eventually leaves
//! through the product, so the product carries the feed plus whatever the reactor made or used,
//! and the reactor burned `X * A_mix` t/h of methane: `P_i = F_i + nu_i * (X * A_mix / M_A) * M_i`.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::{self, DirectSubstitution, Wegstein};
use flowsheet::demo::{AMBIENT_K, AMBIENT_KPA};
use flowsheet::unit::{Feed, Mixer, Product, Splitter};
use flowsheet::{
    ConversionReactor, Flowsheet, Phase, Reaction, Shomate, Solver, SolverConfig, Species,
    SpeciesRegistry, Stream, StreamId, ValidFlowsheet,
};

/// CH4, O2, CO2 and H2O to three decimals. Both sides of the equation sum to 80.039 g/mol, so the
/// reactor's mass correction is 1 to rounding and the hand formula needs no `k`.
const MOLAR_MASS: [f64; 4] = [16.043, 31.998, 44.009, 18.015];

const STOICHIOMETRY: [f64; 4] = [-1.0, -2.0, 1.0, 2.0];

/// 10 t/h of methane in 60 t/h of oxygen. Burning all of it needs 39.9 t/h of oxygen.
const FEED: [f64; 4] = [10.0, 60.0, 0.0, 0.0];

const CONVERSION: f64 = 0.9;

fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (name, molar_mass) in ["CH4", "O2", "CO2", "H2O"].into_iter().zip(MOLAR_MASS) {
        r.insert(Species {
            name: name.into(),
            phase: Phase::Gas,
            molar_mass,
            shomate: Shomate::constant(35.0),
            enthalpy_of_formation: None,
        });
    }
    r
}

struct Solved {
    flowsheet: ValidFlowsheet,
    mixer_out: StreamId,
    product: StreamId,
}

/// Builds and solves the loop at recycle fraction `fraction`. A fraction of 0 is the plain
/// `feed -> reactor -> product` chain with an empty recycle hanging off it.
fn solve(fraction: f64, method: ConvergenceMethod) -> Solved {
    let r = registry();
    let blank = Stream::zeros(&r, AMBIENT_K, AMBIENT_KPA);
    let reaction = Reaction {
        stoichiometry: STOICHIOMETRY.to_vec(),
        limiting: r.find("CH4", Phase::Gas).expect("declared above"),
        conversion: CONVERSION,
    };

    let mut fs = Flowsheet::new(registry());
    let u_feed = fs.add_unit(
        "feed",
        Feed {
            stream: Stream::from_flows(&r, FEED.to_vec(), AMBIENT_K, AMBIENT_KPA),
        },
    );
    let u_mixer = fs.add_unit("mixer", Mixer);
    let u_reactor = fs.add_unit("reactor", ConversionReactor { reaction });
    let u_split = fs.add_unit("split", Splitter { fraction });
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    let mixer_out = fs.add_stream(u_mixer, blank.clone(), u_reactor);
    fs.add_stream(u_reactor, blank.clone(), u_split);
    fs.add_stream(u_split, blank.clone(), u_mixer); // first outlet recycles
    let product = fs.add_stream(u_split, blank, u_product);

    let mut flowsheet = fs.validate().expect("the loop is wired correctly");
    Solver::new(SolverConfig {
        tolerance: 1e-12,
        max_iterations: 1000,
        method,
    })
    .solve(&mut flowsheet)
    .unwrap_or_else(|e| panic!("f = {fraction} under {method:?}: {e}"));

    Solved {
        flowsheet,
        mixer_out,
        product,
    }
}

/// Asserts the mixer outlet's methane and every product flow against the hand balance.
fn assert_hand_balance(fraction: f64, method: ConvergenceMethod) {
    let s = solve(fraction, method);
    let what = format!("f = {fraction} under {method:?}");

    let methane_mix = FEED[0] / (1.0 - fraction * (1.0 - CONVERSION));
    assert_relative_eq!(
        s.flowsheet[s.mixer_out].flows()[0],
        methane_mix,
        max_relative = 1e-9
    );

    // Mmol/h: the methane burned each hour, over its coefficient of 1.
    let extent = CONVERSION * methane_mix / MOLAR_MASS[0];
    let product = s.flowsheet[s.product].flows();
    for i in 0..4 {
        let expected = FEED[i] + STOICHIOMETRY[i] * extent * MOLAR_MASS[i];
        assert!(
            (product[i] - expected).abs() <= 1e-9 * expected.max(1.0),
            "{what}: species {i} should leave at {expected}, got {}",
            product[i]
        );
    }
    assert_relative_eq!(
        s.flowsheet[s.product].total(),
        FEED.iter().sum::<f64>(),
        max_relative = 1e-9
    );
}

const WEGSTEIN: ConvergenceMethod = Wegstein {
    q_min: -5.0,
    q_max: 0.0,
};

#[test]
fn a_reactor_with_no_recycle_burns_the_fraction_it_was_asked_to() {
    assert_hand_balance(0.0, DirectSubstitution);
}

#[test]
fn a_reactor_inside_a_recycle_settles_where_the_hand_balance_says() {
    for fraction in [0.3, 0.9] {
        assert_hand_balance(fraction, DirectSubstitution);
    }
}

#[test]
fn wegstein_through_a_reactor_settles_where_the_hand_balance_says() {
    // Wegstein extrapolates every tear flow with no floor, and the reactor couples the species
    // through its extent, so an early pass could undershoot the oxygen and fail the solve.
    for fraction in [0.3, 0.9] {
        assert_hand_balance(fraction, WEGSTEIN);
    }
}

#[test]
fn the_product_leaves_at_the_feed_temperature_whatever_the_reaction_released() {
    // Isothermal, and so wrong for combustion: see the reactor energy balance item in TODO.md.
    let s = solve(0.3, DirectSubstitution);
    assert_eq!(s.flowsheet[s.product].temperature(), AMBIENT_K);
}

//! Adiabatic conversion reactors, checked against enthalpy balances worked by hand.
//!
//! Once every participant has an enthalpy of formation, the heat of reaction is not a parameter
//! anywhere: it is the difference between the outlet's absolute enthalpy and the inlet's. An
//! adiabatic reactor holds that difference at zero, so every temperature below follows from one
//! equation, per species `i` with molar flow `n_i` (Mmol/h), `h_f` in kJ/mol and constant `cp` in
//! J/(mol·K):
//!
//! ```text
//! H(n, T) = sum n_i * (h_f,i + cp_i * (T - 298.15) / 1000)     GJ/h
//! ```
//!
//! That is GJ/h, not MJ/h: Mmol/h times kJ/mol is 10^6 × 10^3 J/h. The library never sees the
//! factor, because it works per kilogram, but a hand balance in molar units has to.

use approx::assert_relative_eq;
use flowsheet::ConvergenceMethod::DirectSubstitution;
use flowsheet::demo::AMBIENT_KPA;
use flowsheet::unit::{Feed, Mixer, Product, Splitter};
use flowsheet::{
    ConversionReactor, Flowsheet, Phase, Reaction, ReactorEnergy, Shomate, Solver, SolverConfig,
    Species, SpeciesRegistry, Stream, StreamId, ValidFlowsheet,
};

const REFERENCE_K: f64 = 298.15;

// ---- combustion ----

/// CH4, O2, CO2, H2O and N2. Molar masses to three decimals, so the combustion equation closes
/// exactly and the reactor's mass correction is 1.
const MOLAR_MASS: [f64; 5] = [16.043, 31.998, 44.009, 18.015, 28.014];
/// Constant heat capacities near 25 °C, J/(mol·K). Constant, so the hand balance is linear in T.
const CP: [f64; 5] = [35.7, 29.4, 37.1, 33.6, 29.1];
/// NIST standard enthalpies of formation as gases, kJ/mol. Nitrogen only rides along, so it has
/// none, which is allowed for a species the reaction does not touch.
const FORMATION: [Option<f64>; 5] = [Some(-74.87), Some(0.0), Some(-393.52), Some(-241.83), None];
const STOICHIOMETRY: [f64; 5] = [-1.0, -2.0, 1.0, 2.0, 0.0];

/// 2 t/h of methane in air-like excess: enough oxygen and nitrogen to keep the flame near 1300 K.
const FEED: [f64; 5] = [2.0, 20.0, 0.0, 0.0, 70.0];
const FEED_K: f64 = 400.0;
const CONVERSION: f64 = 0.9;

fn combustion_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (i, name) in ["CH4", "O2", "CO2", "H2O", "N2"].into_iter().enumerate() {
        r.insert(Species {
            name: name.into(),
            phase: Phase::Gas,
            molar_mass: MOLAR_MASS[i],
            shomate: Shomate::constant(CP[i]),
            enthalpy_of_formation: FORMATION[i],
        });
    }
    r
}

/// Molar flows, Mmol/h, of the feed after `extent` Mmol/h of methane has burned.
fn burned(extent: f64) -> [f64; 5] {
    std::array::from_fn(|i| FEED[i] / MOLAR_MASS[i] + STOICHIOMETRY[i] * extent)
}

/// `H(n, T)` from the module docs, GJ/h.
fn enthalpy(n: [f64; 5], t: f64) -> f64 {
    (0..5)
        .map(|i| n[i] * (FORMATION[i].unwrap_or(0.0) + CP[i] * (t - REFERENCE_K) / 1000.0))
        .sum()
}

/// The temperature at which molar flows `n` carry `h` GJ/h: `H(n, T) = h`, solved for `T`.
fn temperature_holding(n: [f64; 5], h: f64) -> f64 {
    let formation = enthalpy(n, REFERENCE_K);
    let heat_capacity: f64 = (0..5).map(|i| n[i] * CP[i] / 1000.0).sum();
    REFERENCE_K + (h - formation) / heat_capacity
}

/// The feed and the reactor that burns it.
fn burner(r: &SpeciesRegistry) -> (Stream, ConversionReactor) {
    let feed = Stream::from_flows(r, FEED.to_vec(), FEED_K, AMBIENT_KPA);
    let reactor = ConversionReactor {
        reactions: vec![Reaction {
            stoichiometry: STOICHIOMETRY.to_vec(),
            limiting: r.find("CH4", Phase::Gas).expect("declared above"),
            conversion: CONVERSION,
        }],
        energy: ReactorEnergy::Adiabatic,
        pressure_drop: 0.0,
    };
    (feed, reactor)
}

struct Solved {
    flowsheet: ValidFlowsheet,
    reactor_out: StreamId,
    product: StreamId,
}

/// `feed -> mixer -> reactor -> splitter -> product`, recycling `fraction` of the reactor outlet.
/// A fraction of 0 leaves the recycle empty.
fn solve_burner(fraction: f64) -> Solved {
    let r = combustion_registry();
    let blank = Stream::zeros(&r, FEED_K, AMBIENT_KPA);
    let (feed, reactor) = burner(&r);

    let mut fs = Flowsheet::new(combustion_registry());
    let u_feed = fs.add_unit("feed", Feed { stream: feed });
    let u_mixer = fs.add_unit("mixer", Mixer::default());
    let u_reactor = fs.add_unit("reactor", reactor);
    let u_split = fs.add_unit(
        "split",
        Splitter {
            fraction,
            pressure_drop: 0.0,
        },
    );
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_mixer);
    fs.add_stream(u_mixer, blank.clone(), u_reactor);
    let reactor_out = fs.add_stream(u_reactor, blank.clone(), u_split);
    fs.add_stream(u_split, blank.clone(), u_mixer); // first outlet recycles
    let product = fs.add_stream(u_split, blank, u_product);

    let mut flowsheet = fs.validate().expect("the loop is wired correctly");
    Solver::new(SolverConfig {
        tolerance: 1e-12,
        max_iterations: 1000,
        method: DirectSubstitution,
    })
    .solve(&mut flowsheet)
    .unwrap_or_else(|e| panic!("f = {fraction}: {e}"));

    Solved {
        flowsheet,
        reactor_out,
        product,
    }
}

#[test]
fn an_exothermic_reaction_raises_its_own_outlet_to_the_hand_balance_temperature() {
    let s = solve_burner(0.0);

    // 90% of the methane burns, and the products hold what the feed held.
    let extent = CONVERSION * FEED[0] / MOLAR_MASS[0];
    let expected = temperature_holding(burned(extent), enthalpy(burned(0.0), FEED_K));

    let actual = s.flowsheet[s.reactor_out].temperature();
    assert!(expected > FEED_K + 500.0, "{expected}");
    assert_relative_eq!(actual, expected, max_relative = 1e-9);
}

#[test]
fn an_adiabatic_reactor_inside_a_recycle_settles_where_the_hand_balance_says() {
    // No heat crosses the boundary, so the product holds the feed's enthalpy whatever the loop
    // does inside it. The methane balance round the loop is the one in `conversion_reactor.rs`:
    // `A_mix = F_A / (1 - f(1 - X))`, and the reactor burns `X * A_mix` of it.
    let fraction = 0.5;
    let s = solve_burner(fraction);

    let methane_mix = FEED[0] / MOLAR_MASS[0] / (1.0 - fraction * (1.0 - CONVERSION));
    let extent = CONVERSION * methane_mix;
    let expected = temperature_holding(burned(extent), enthalpy(burned(0.0), FEED_K));

    let product = &s.flowsheet[s.product];
    assert_relative_eq!(product.temperature(), expected, max_relative = 1e-9);
    assert_relative_eq!(
        product.flows()[0] / (1.0 - fraction),
        s.flowsheet[s.reactor_out].flows()[0],
        max_relative = 1e-9
    );
}

// ---- a cycle of reactions ----

/// n-butane and isobutane: isomers, so one molar mass and a mass correction of exactly 1.
const BUTANE_MOLAR_MASS: f64 = 58.122;

/// Illustrative Shomate fits, not NIST's: temperature-dependent on purpose, so that returning to
/// the feed temperature proves the basis rather than the arithmetic of a constant cp.
const N_BUTANE_CP: Shomate = Shomate {
    a: 20.0,
    b: 250.0,
    c: -100.0,
    d: 10.0,
    e: 0.1,
};
const ISOBUTANE_CP: Shomate = Shomate {
    a: 15.0,
    b: 270.0,
    c: -120.0,
    d: 15.0,
    e: 0.2,
};

fn butane_registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    // Approximately NIST's gas-phase formation enthalpies: isomerising n-butane releases 8.6 kJ/mol.
    for (name, shomate, h_f) in [
        ("n-C4H10", N_BUTANE_CP, -125.6),
        ("i-C4H10", ISOBUTANE_CP, -134.2),
    ] {
        r.insert(Species {
            name: name.into(),
            phase: Phase::Gas,
            molar_mass: BUTANE_MOLAR_MASS,
            shomate,
            enthalpy_of_formation: Some(h_f),
        });
    }
    r
}

#[test]
fn a_cycle_of_reactions_returns_to_the_temperature_it_started_at() {
    // n-butane -> isobutane -> n-butane, both adiabatic. The second reactor converts everything
    // the first made back, so the product has the feed's composition. Enthalpy is a state
    // function, so it must have the feed's temperature too - which only holds if the two formation
    // enthalpies and both heat capacity integrals share one reference.
    let r = butane_registry();
    let n = r.find("n-C4H10", Phase::Gas).unwrap();
    let iso = r.find("i-C4H10", Phase::Gas).unwrap();
    let feed_k = 350.0;
    let feed = Stream::from_flows(&r, vec![10.0, 0.0], feed_k, AMBIENT_KPA);
    let blank = Stream::zeros(&r, feed_k, AMBIENT_KPA);

    let mut fs = Flowsheet::new(butane_registry());
    let u_feed = fs.add_unit("feed", Feed { stream: feed });
    let u_forward = fs.add_unit(
        "forward",
        ConversionReactor {
            reactions: vec![Reaction {
                stoichiometry: vec![-1.0, 1.0],
                limiting: n,
                conversion: 0.7,
            }],
            energy: ReactorEnergy::Adiabatic,
            pressure_drop: 0.0,
        },
    );
    let u_back = fs.add_unit(
        "back",
        ConversionReactor {
            reactions: vec![Reaction {
                stoichiometry: vec![1.0, -1.0],
                limiting: iso,
                conversion: 1.0,
            }],
            energy: ReactorEnergy::Adiabatic,
            pressure_drop: 0.0,
        },
    );
    let u_product = fs.add_unit("product", Product);

    fs.add_stream(u_feed, blank.clone(), u_forward);
    let middle = fs.add_stream(u_forward, blank.clone(), u_back);
    let out = fs.add_stream(u_back, blank.clone(), u_product);

    let mut fs = fs.validate().expect("the chain is wired correctly");
    Solver::default().solve(&mut fs).expect("it is acyclic");

    // The forward step is exothermic, so the cycle is not trivially isothermal.
    assert!(
        fs[middle].temperature() > feed_k + 1.0,
        "{}",
        fs[middle].temperature()
    );

    assert_relative_eq!(fs[out].flows()[0], 10.0, max_relative = 1e-12);
    assert_eq!(fs[out].flows()[1], 0.0);
    assert_relative_eq!(fs[out].temperature(), feed_k, max_relative = 1e-9);
}

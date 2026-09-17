//! A conversion reactor running several reactions in declared order, checked against extents
//! worked by hand.
//!
//! Each reaction runs on what the one before it left, so its conversion is a fraction of the
//! limiting reactant at that point. With methane `n` Mmol/h in, and two reactions both limited by
//! it at conversions `X1` then `X2`:
//!
//! ```text
//! extent_1 = X1 * n
//! extent_2 = X2 * (1 - X1) * n       not X2 * n: the first reaction already took its share
//! ```

use approx::assert_relative_eq;
use flowsheet::demo::AMBIENT_KPA;
use flowsheet::unit::{Feed, Product};
use flowsheet::{
    ConversionReactor, Flowsheet, Phase, Reaction, ReactorEnergy, Shomate, SolveError, Solver,
    Species, SpeciesRegistry, Stream, StreamId, ValidFlowsheet,
};

/// CH4, O2, CO, CO2, H2O and N2. Molar masses to three decimals, so every equation below closes
/// exactly and the reactor's mass correction is 1.
const NAMES: [&str; 6] = ["CH4", "O2", "CO", "CO2", "H2O", "N2"];
const MOLAR_MASS: [f64; 6] = [16.043, 31.998, 28.010, 44.009, 18.015, 28.014];
/// Constant heat capacities near 25 °C, J/(mol·K).
const CP: [f64; 6] = [35.7, 29.4, 29.1, 37.1, 33.6, 29.1];
/// NIST standard enthalpies of formation as gases, kJ/mol. Nitrogen only rides along.
const FORMATION: [Option<f64>; 6] = [
    Some(-74.87),
    Some(0.0),
    Some(-110.53),
    Some(-393.52),
    Some(-241.83),
    None,
];

const CH4: usize = 0;
const O2: usize = 1;
const CO: usize = 2;
const CO2: usize = 3;

/// `CH4 + 2 O2 -> CO2 + 2 H2O`
const FULL: [f64; 6] = [-1.0, -2.0, 0.0, 1.0, 2.0, 0.0];
/// `CH4 + 1.5 O2 -> CO + 2 H2O`
const PARTIAL: [f64; 6] = [-1.0, -1.5, 1.0, 0.0, 2.0, 0.0];
/// `CO + 0.5 O2 -> CO2`
const SHIFT: [f64; 6] = [0.0, -0.5, -1.0, 1.0, 0.0, 0.0];

/// 8 t/h of methane - half a megamole an hour - in plenty of oxygen and nitrogen.
const FEED: [f64; 6] = [8.0215, 60.0, 0.0, 0.0, 0.0, 200.0];
const FEED_K: f64 = 500.0;

fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for i in 0..NAMES.len() {
        r.insert(Species {
            name: NAMES[i].into(),
            phase: Phase::Gas,
            molar_mass: MOLAR_MASS[i],
            shomate: Shomate::constant(CP[i]),
            enthalpy_of_formation: FORMATION[i],
        });
    }
    r
}

fn reaction(r: &SpeciesRegistry, stoichiometry: [f64; 6], limiting: &str, x: f64) -> Reaction {
    Reaction {
        stoichiometry: stoichiometry.to_vec(),
        limiting: r.find(limiting, Phase::Gas).expect("declared above"),
        conversion: x,
    }
}

/// `feed -> reactor -> ... -> reactor -> product`, one reactor per entry of `stages`, solved.
/// Returns the flowsheet and the stream into the product.
fn solve(
    feed: [f64; 6],
    energy: ReactorEnergy,
    stages: impl Fn(&SpeciesRegistry) -> Vec<Vec<Reaction>>,
) -> Result<(ValidFlowsheet, StreamId), SolveError> {
    let r = registry();
    let blank = Stream::zeros(&r, FEED_K, AMBIENT_KPA);
    let stages = stages(&r);

    let mut fs = Flowsheet::new(registry());
    let mut upstream = fs.add_unit(
        "feed",
        Feed {
            stream: Stream::from_flows(&r, feed.to_vec(), FEED_K, AMBIENT_KPA),
        },
    );
    for (i, reactions) in stages.into_iter().enumerate() {
        let reactor = fs.add_unit(
            format!("reactor {i}"),
            ConversionReactor { reactions, energy },
        );
        fs.add_stream(upstream, blank.clone(), reactor);
        upstream = reactor;
    }
    let product = fs.add_unit("product", Product);
    let out = fs.add_stream(upstream, blank, product);

    let mut fs = fs.validate().expect("the chain is wired correctly");
    Solver::default().solve(&mut fs)?;
    Ok((fs, out))
}

/// Molar flows, Mmol/h, of `stream`.
fn moles(fs: &ValidFlowsheet, stream: StreamId) -> [f64; 6] {
    std::array::from_fn(|i| fs[stream].flows()[i] / MOLAR_MASS[i])
}

/// Molar flows of the feed after `extents[k]` Mmol/h of `reactions[k]`.
fn after(extents: &[(f64, [f64; 6])]) -> [f64; 6] {
    std::array::from_fn(|i| {
        FEED[i] / MOLAR_MASS[i] + extents.iter().map(|(x, nu)| x * nu[i]).sum::<f64>()
    })
}

fn assert_moles(actual: [f64; 6], expected: [f64; 6]) {
    for i in 0..6 {
        assert!(
            (actual[i] - expected[i]).abs() <= 1e-9 * expected[i].abs().max(1.0),
            "{}: expected {} Mmol/h, got {}",
            NAMES[i],
            expected[i],
            actual[i]
        );
    }
}

#[test]
fn two_reactions_competing_for_methane_share_it_in_declared_order() {
    let (fs, out) = solve(FEED, ReactorEnergy::Isothermal, |r| {
        vec![vec![
            reaction(r, FULL, "CH4", 0.6),
            reaction(r, PARTIAL, "CH4", 0.5),
        ]]
    })
    .unwrap();

    let n = FEED[CH4] / MOLAR_MASS[CH4];
    let expected = after(&[(0.6 * n, FULL), (0.5 * 0.4 * n, PARTIAL)]);
    assert_moles(moles(&fs, out), expected);

    // 60% then half of the remaining 40%: 80% burned, never more than arrived.
    assert_relative_eq!(moles(&fs, out)[CH4], 0.2 * n, max_relative = 1e-12);
}

#[test]
fn swapping_the_order_of_competing_reactions_changes_the_outlet() {
    let full_first = |r: &SpeciesRegistry| {
        vec![vec![
            reaction(r, FULL, "CH4", 0.6),
            reaction(r, PARTIAL, "CH4", 0.5),
        ]]
    };
    let partial_first = |r: &SpeciesRegistry| {
        vec![vec![
            reaction(r, PARTIAL, "CH4", 0.5),
            reaction(r, FULL, "CH4", 0.6),
        ]]
    };
    let (a, a_out) = solve(FEED, ReactorEnergy::Isothermal, full_first).unwrap();
    let (b, b_out) = solve(FEED, ReactorEnergy::Isothermal, partial_first).unwrap();

    // Either way 80% of the methane burns, because 1 - 0.4 * 0.5 = 1 - 0.5 * 0.4. How it splits
    // between CO and CO2 does not: whichever runs first takes the larger share.
    let n = FEED[CH4] / MOLAR_MASS[CH4];
    assert_relative_eq!(
        moles(&a, a_out)[CH4],
        moles(&b, b_out)[CH4],
        max_relative = 1e-12
    );
    assert_relative_eq!(moles(&a, a_out)[CO], 0.2 * n, max_relative = 1e-12);
    assert_relative_eq!(moles(&b, b_out)[CO], 0.5 * n, max_relative = 1e-12);
    assert_relative_eq!(moles(&b, b_out)[CO2], 0.6 * 0.5 * n, max_relative = 1e-12);
}

#[test]
fn a_later_reaction_consumes_what_an_earlier_one_made() {
    // Partial combustion makes CO, and the shift burns 80% of *that* - there is no CO at the
    // inlet, so read against the inlet this reaction would do nothing.
    let (fs, out) = solve(FEED, ReactorEnergy::Isothermal, |r| {
        vec![vec![
            reaction(r, PARTIAL, "CH4", 0.9),
            reaction(r, SHIFT, "CO", 0.8),
        ]]
    })
    .unwrap();

    let made = 0.9 * FEED[CH4] / MOLAR_MASS[CH4];
    let expected = after(&[(made, PARTIAL), (0.8 * made, SHIFT)]);
    assert_moles(moles(&fs, out), expected);
    assert_relative_eq!(moles(&fs, out)[CO], 0.2 * made, max_relative = 1e-9);
}

#[test]
fn one_adiabatic_reactor_with_two_reactions_matches_two_reactors_in_series() {
    let together = |r: &SpeciesRegistry| {
        vec![vec![
            reaction(r, PARTIAL, "CH4", 0.9),
            reaction(r, SHIFT, "CO", 0.8),
        ]]
    };
    let apart = |r: &SpeciesRegistry| {
        vec![
            vec![reaction(r, PARTIAL, "CH4", 0.9)],
            vec![reaction(r, SHIFT, "CO", 0.8)],
        ]
    };
    let (a, a_out) = solve(FEED, ReactorEnergy::Adiabatic, together).unwrap();
    let (b, b_out) = solve(FEED, ReactorEnergy::Adiabatic, apart).unwrap();

    // Enthalpy is a state function, so solving for temperature once at the end lands where
    // solving after each reaction does.
    assert!(a[a_out].temperature() > FEED_K + 500.0);
    assert_relative_eq!(
        a[a_out].temperature(),
        b[b_out].temperature(),
        max_relative = 1e-9
    );
    assert_moles(moles(&a, a_out), moles(&b, b_out));
}

#[test]
fn a_later_reaction_short_of_a_reactant_names_itself() {
    // 0.5 Mmol/h of methane and 0.9 of oxygen. Burning half the methane takes 2 * 0.25 = 0.5 of
    // the oxygen, leaving 0.4 for a second reaction that asks for 2 * 0.25 = 0.5 to burn the rest.
    let mut feed = FEED;
    feed[O2] = 0.9 * MOLAR_MASS[O2];
    let e = solve(feed, ReactorEnergy::Isothermal, |r| {
        vec![vec![
            reaction(r, FULL, "CH4", 0.5),
            reaction(r, FULL, "CH4", 1.0),
        ]]
    })
    .map(|_| ())
    .expect_err("the second reaction runs out of oxygen");

    let message = e.to_string();
    assert!(
        message.contains("reaction 1: the reaction needs more O2"),
        "{message}"
    );
}

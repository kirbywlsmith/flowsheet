//! Flash drums, isothermal and adiabatic, checked against answers worked by hand.
//!
//! Humid nitrogen is the case with a closed form. Nitrogen dissolves in nothing, so whatever
//! water stays liquid is pure, and Raoult's law puts the water in the vapour at `Psat / P`. The
//! steam the nitrogen leaves with is therefore
//!
//! ```text
//! n_steam = n_N2 * Psat / (P - Psat)
//! ```
//!
//! as long as some water stays liquid, however much there is.

use approx::assert_relative_eq;
use flowsheet::demo::AMBIENT_KPA;
use flowsheet::species::latent_heat;
use flowsheet::unit::{Feed, Mixer, Product, Splitter};
use flowsheet::{
    ConvergenceMethod, Flash, FlashEnergy, Flowsheet, Phase, Solver, SolverConfig, Species,
    SpeciesRegistry, Stream, StreamId, UnitId, ValidFlowsheet, report,
};

const FLASH_K: f64 = 350.0;
const WATER_T_PER_H: f64 = 100.0;
const NITROGEN_T_PER_H: f64 = 28.0;

/// Liquid water, steam and nitrogen, as shipped.
fn registry() -> SpeciesRegistry {
    let mut r = SpeciesRegistry::default();
    for (name, phase) in [
        ("H2O", Phase::Liquid),
        ("H2O", Phase::Gas),
        ("N2", Phase::Gas),
    ] {
        r.insert(species(name, phase));
    }
    r
}

fn species(name: &str, phase: Phase) -> Species {
    flowsheet::library::find(name, phase)
        .expect("shipped species missing")
        .species
        .clone()
}

/// `n_steam` from the module docs, in t/h.
fn saturated_steam() -> f64 {
    saturated_steam_at(FLASH_K)
}

/// `n_steam` from the module docs at `temperature`, in t/h.
fn saturated_steam_at(temperature: f64) -> f64 {
    let water = species("H2O", Phase::Liquid);
    let psat = water.vapour_pressure.unwrap().vapour_pressure(temperature);
    let nitrogen = NITROGEN_T_PER_H / species("N2", Phase::Gas).molar_mass;
    nitrogen * psat / (AMBIENT_KPA - psat) * water.molar_mass
}

/// The humid feed at `temperature`.
fn feed_at(r: &SpeciesRegistry, temperature: f64) -> Stream {
    Stream::from_flows(
        r,
        vec![WATER_T_PER_H, 0.0, NITROGEN_T_PER_H],
        temperature,
        AMBIENT_KPA,
    )
}

/// A drum at one atmosphere and `FLASH_K`.
fn isothermal() -> Flash {
    Flash {
        pressure: AMBIENT_KPA,
        energy: FlashEnergy::Temperature(FLASH_K),
    }
}

/// A drum at one atmosphere with no heat in or out.
fn adiabatic() -> Flash {
    Flash {
        pressure: AMBIENT_KPA,
        energy: FlashEnergy::Duty(0.0),
    }
}

/// The adiabatic feed's temperature: hot enough that dry nitrogen strips a good deal of steam
/// from it, and cools it doing so.
const HOT_K: f64 = 360.0;

struct Solved {
    fs: ValidFlowsheet,
    drum: UnitId,
    vapour: StreamId,
    liquid: StreamId,
    passes: usize,
}

/// Feed -> flash -> two products.
fn once_through(feed_k: f64, drum: Flash) -> Solved {
    let r = registry();
    let feed_stream = feed_at(&r, feed_k);
    let zeros = Stream::zeros(&r, feed_k, AMBIENT_KPA);
    let mut fs = Flowsheet::new(r);
    let f = fs.add_unit(
        "feed",
        Feed {
            stream: feed_stream.clone(),
        },
    );
    let d = fs.add_unit("drum", drum);
    let v = fs.add_unit("vapour", Product);
    let l = fs.add_unit("liquid", Product);
    fs.add_stream(f, feed_stream, d);
    let vapour = fs.add_stream(d, zeros.clone(), v);
    let liquid = fs.add_stream(d, zeros, l);

    let mut fs = fs.validate().unwrap();
    let passes = Solver::default().solve(&mut fs).unwrap().iterations;
    Solved {
        fs,
        drum: d,
        vapour,
        liquid,
        passes,
    }
}

#[test]
fn nitrogen_leaves_saturated_with_water() {
    let Solved {
        fs, vapour, liquid, ..
    } = once_through(FLASH_K, isothermal());

    assert_relative_eq!(
        fs[vapour].flows()[1],
        saturated_steam(),
        max_relative = 1e-10
    );
    assert_eq!(fs[vapour].flows()[2], NITROGEN_T_PER_H);
    assert_relative_eq!(
        fs[liquid].flows()[0],
        WATER_T_PER_H - saturated_steam(),
        max_relative = 1e-10
    );
}

#[test]
fn the_drum_takes_in_the_latent_heat_of_what_it_evaporates() {
    // The feed is already at the drum's temperature, so no sensible heat moves, and the whole
    // duty is the evaporation. Mmol/h times kJ/mol is GJ/h, hence the 1000.
    let Solved { fs, drum, .. } = once_through(FLASH_K, isothermal());
    let evaporated = saturated_steam() / species("H2O", Phase::Gas).molar_mass;
    let latent = latent_heat(
        &species("H2O", Phase::Liquid),
        &species("H2O", Phase::Gas),
        FLASH_K,
    );

    assert_relative_eq!(
        report::duty(&fs, drum),
        1000.0 * evaporated * latent,
        max_relative = 1e-9
    );
}

#[test]
fn a_flash_balances_on_total_mass() {
    // Water changes species id as it evaporates, so the per-species check would fail; the flash
    // says so and the flowsheet falls back to total mass, which closes.
    let Solved { fs, .. } = once_through(FLASH_K, isothermal());
    assert!(report::imbalance(&fs) < 1e-14, "{}", report::imbalance(&fs));
}

#[test]
fn an_adiabatic_drum_takes_in_nothing_and_lands_on_an_equilibrium() {
    let Solved {
        fs, drum, vapour, ..
    } = once_through(HOT_K, adiabatic());
    let t = fs[vapour].temperature();

    assert!(t < HOT_K, "evaporation should cool the drum, got {t} K");
    // Zero against the feed's absolute enthalpy flow, about 1.6e6 MJ/h.
    let duty = report::duty(&fs, drum);
    assert!(duty.abs() < 1e-6, "{duty} MJ/h");
    assert_relative_eq!(
        fs[vapour].flows()[1],
        saturated_steam_at(t),
        max_relative = 1e-10
    );
}

/// Direct substitution, or Wegstein with the CLI's clamps.
const METHODS: [ConvergenceMethod; 2] = [
    ConvergenceMethod::DirectSubstitution,
    ConvergenceMethod::Wegstein {
        q_min: -5.0,
        q_max: 0.0,
    },
];

/// Feed -> mixer -> flash, with `fraction` of the drum's liquid recycled to the mixer.
///
/// ```text
///   Feed ──▶ Mixer ──▶ Flash ──vapour──▶ Product
///              ▲         │
///              │       liquid
///              │         ▼
///              └─────  Splitter ──▶ Product
/// ```
fn recycled(feed_k: f64, drum: Flash, fraction: f64, method: ConvergenceMethod) -> Solved {
    let r = registry();
    let feed_stream = feed_at(&r, feed_k);
    let zeros = Stream::zeros(&r, feed_k, AMBIENT_KPA);
    let mut fs = Flowsheet::new(r);
    let f = fs.add_unit(
        "feed",
        Feed {
            stream: feed_stream.clone(),
        },
    );
    let m = fs.add_unit("mixer", Mixer::default());
    let d = fs.add_unit("drum", drum);
    let s = fs.add_unit(
        "splitter",
        Splitter {
            fraction,
            pressure_drop: 0.0,
        },
    );
    let v = fs.add_unit("vapour", Product);
    let l = fs.add_unit("liquid", Product);

    fs.add_stream(f, feed_stream, m);
    fs.add_stream(m, zeros.clone(), d);
    let vapour = fs.add_stream(d, zeros.clone(), v);
    fs.add_stream(d, zeros.clone(), s);
    fs.add_stream(s, zeros.clone(), m);
    let liquid = fs.add_stream(s, zeros, l);

    let mut fs = fs.validate().unwrap();
    // 500 passes, past the default 100 that direct substitution overruns at a recycle of 0.9,
    // the same way the demo circuit does.
    let passes = Solver::new(SolverConfig {
        max_iterations: 500,
        method,
        ..SolverConfig::default()
    })
    .solve(&mut fs)
    .unwrap()
    .iterations;
    Solved {
        fs,
        drum: d,
        vapour,
        liquid,
        passes,
    }
}

#[test]
fn recycling_the_liquid_changes_nothing_at_the_plant_boundary() {
    // The nitrogen alone sets how much steam leaves, and the recycled water is already at the
    // drum's temperature and pressure, so the products are the once-through ones whatever the
    // recycle fraction - the flash's counterpart of `demo::balance`.
    for fraction in [0.3, 0.9] {
        let Solved {
            fs, vapour, liquid, ..
        } = recycled(FLASH_K, isothermal(), fraction, METHODS[0]);
        assert_relative_eq!(
            fs[vapour].flows()[1],
            saturated_steam(),
            max_relative = 1e-8
        );
        assert_relative_eq!(
            fs[liquid].flows()[0],
            WATER_T_PER_H - saturated_steam(),
            max_relative = 1e-8
        );
    }
}

/// The TODO item's test: Rachford-Rice at 1e-12, the drum's temperature search at 1e-9 K and
/// the solver at 1e-9 relative, nested three deep, and the outer loop still converges to the
/// answer the inner two give on their own.
///
/// The answer is the once-through drum's. Its liquid is pure water at the drum's temperature,
/// so recycling some of it adds water that the drum hands straight back at the same state: the
/// nitrogen still carries the same steam, and every extra tonne of recycle leaves as liquid.
#[test]
fn an_adiabatic_drum_on_a_recycle_converges_to_the_once_through_answer() {
    let alone = once_through(HOT_K, adiabatic());
    let steam = alone.fs[alone.vapour].flows()[1];
    let water = alone.fs[alone.liquid].flows()[0];
    let t = alone.fs[alone.vapour].temperature();

    // Direct substitution takes the isothermal loop's 18 and 176 to the pass: the drum's own
    // search converges far inside the solver's tolerance, so it adds nothing the outer loop sees.
    for (method, counts) in METHODS.into_iter().zip([[18, 176], [3, 22]]) {
        for (fraction, expected) in [0.3, 0.9].into_iter().zip(counts) {
            let Solved {
                fs,
                vapour,
                liquid,
                passes,
                ..
            } = recycled(HOT_K, adiabatic(), fraction, method);
            let case = format!("{method:?} at {fraction}");
            assert_eq!(passes, expected, "{case}");

            assert_relative_eq!(fs[vapour].flows()[1], steam, max_relative = 1e-8);
            assert_relative_eq!(fs[liquid].flows()[0], water, max_relative = 1e-8);
            let drum_k = fs[vapour].temperature();
            assert!(
                (drum_k - t).abs() < 1e-6,
                "{case}: {drum_k} K against {t} K"
            );
            assert!(report::imbalance(&fs) < 1e-8, "{case}");

            // Nothing heats or cools anywhere, so the products carry the feed's enthalpy.
            let r = fs.registry();
            let fed = feed_at(r, HOT_K).enthalpy(r);
            let out = fs[vapour].enthalpy(r) + fs[liquid].enthalpy(r);
            assert_relative_eq!(out, fed, max_relative = 1e-8);
        }
    }
}

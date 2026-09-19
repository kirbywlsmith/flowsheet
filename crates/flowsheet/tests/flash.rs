//! Isothermal flash drums, checked against answers worked by hand.
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
    Flash, Flowsheet, Phase, Solver, SolverConfig, Species, SpeciesRegistry, Stream, StreamId,
    UnitId, ValidFlowsheet, report,
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
    let water = species("H2O", Phase::Liquid);
    let psat = water.vapour_pressure.unwrap().vapour_pressure(FLASH_K);
    let nitrogen = NITROGEN_T_PER_H / species("N2", Phase::Gas).molar_mass;
    nitrogen * psat / (AMBIENT_KPA - psat) * water.molar_mass
}

fn feed(r: &SpeciesRegistry) -> Stream {
    Stream::from_flows(
        r,
        vec![WATER_T_PER_H, 0.0, NITROGEN_T_PER_H],
        FLASH_K,
        AMBIENT_KPA,
    )
}

fn drum() -> Flash {
    Flash {
        temperature: FLASH_K,
        pressure: AMBIENT_KPA,
    }
}

struct OnceThrough {
    fs: ValidFlowsheet,
    drum: UnitId,
    vapour: StreamId,
    liquid: StreamId,
}

/// Feed -> flash -> two products.
fn once_through() -> OnceThrough {
    let r = registry();
    let feed_stream = feed(&r);
    let mut fs = Flowsheet::new(r);
    let f = fs.add_unit(
        "feed",
        Feed {
            stream: feed_stream.clone(),
        },
    );
    let drum = fs.add_unit("drum", drum());
    let v = fs.add_unit("vapour", Product);
    let l = fs.add_unit("liquid", Product);
    fs.add_stream(f, feed_stream, drum);
    let zeros = Stream::zeros(fs.registry(), FLASH_K, AMBIENT_KPA);
    let vapour = fs.add_stream(drum, zeros.clone(), v);
    let liquid = fs.add_stream(drum, zeros, l);

    let mut fs = fs.validate().unwrap();
    Solver::default().solve(&mut fs).unwrap();
    OnceThrough {
        fs,
        drum,
        vapour,
        liquid,
    }
}

#[test]
fn nitrogen_leaves_saturated_with_water() {
    let OnceThrough {
        fs, vapour, liquid, ..
    } = once_through();

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
    let OnceThrough { fs, drum, .. } = once_through();
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
    let OnceThrough { fs, .. } = once_through();
    assert!(report::imbalance(&fs) < 1e-14, "{}", report::imbalance(&fs));
}

/// Feed -> mixer -> flash, with `fraction` of the drum's liquid recycled to the mixer.
///
/// ```text
///   Feed ──▶ Mixer ──▶ Flash ──vapour──▶ Product
///              ▲         │
///              │       liquid
///              │         ▼
///              └─────  Splitter ──▶ Product
/// ```
fn recycled(fraction: f64) -> (ValidFlowsheet, StreamId, StreamId) {
    let r = registry();
    let feed_stream = feed(&r);
    let zeros = Stream::zeros(&r, FLASH_K, AMBIENT_KPA);
    let mut fs = Flowsheet::new(r);
    let f = fs.add_unit(
        "feed",
        Feed {
            stream: feed_stream.clone(),
        },
    );
    let m = fs.add_unit("mixer", Mixer::default());
    let d = fs.add_unit("drum", drum());
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
    // 500 passes: direct substitution takes 18 at a recycle of 0.3 and 176 at 0.9, past the
    // default 100, the same way the demo circuit does.
    Solver::new(SolverConfig {
        max_iterations: 500,
        ..SolverConfig::default()
    })
    .solve(&mut fs)
    .unwrap();
    (fs, vapour, liquid)
}

#[test]
fn recycling_the_liquid_changes_nothing_at_the_plant_boundary() {
    // The nitrogen alone sets how much steam leaves, and the recycled water is already at the
    // drum's temperature and pressure, so the products are the once-through ones whatever the
    // recycle fraction - the flash's counterpart of `demo::balance`.
    for fraction in [0.3, 0.9] {
        let (fs, vapour, liquid) = recycled(fraction);
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

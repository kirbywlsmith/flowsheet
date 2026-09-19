//! The shipped species library: some twenty common species with their properties, so a
//! document can write `{ "name": "H2O", "phase": "Liquid" }` and leave the NIST lookup to the
//! crate.
//!
//! The data is `data/species.json`, compiled into the library with [`include_str!`] (an embedded
//! resource, in .NET terms) and parsed once, on first use, behind a [`LazyLock`] (`Lazy<T>`).
//! Every entry names its source, so a wrong number can be traced back to the page it came from.
//!
//! A document's entry is resolved against this by [`crate::serial::Species::resolve`]. An entry
//! that leaves out `molar_mass` or `shomate` is a reference: the library supplies every property
//! it does not give, and one it does give wins. An entry that gives both is taken as written. A
//! species that is not here has to be written out in full, exactly as every document did before
//! the library existed.

use crate::species::{Phase, Species};
use crate::thermo::{Antoine, Shomate};
use serde::Deserialize;
use std::sync::LazyLock;

/// One shipped species and where its numbers came from.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The species, complete: every entry carries a molar mass and a heat capacity, and the
    /// optional properties are whatever the source had.
    pub species: Species,
    /// Where the numbers came from - the NIST WebBook page and the fit's temperature range, or
    /// the estimate and how it was made.
    pub source: String,
}

/// One record of `data/species.json`. Its own struct rather than [`Species`] with a `source`
/// flattened beside it, because `flatten` and `deny_unknown_fields` cannot share a struct - the
/// same wall `serial::Op` hit - and a typo in the shipped file must be an error.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    name: String,
    phase: Phase,
    molar_mass: f64,
    shomate: Shomate,
    #[serde(default)]
    enthalpy_of_formation: Option<f64>,
    #[serde(default)]
    density: Option<f64>,
    #[serde(default)]
    vapour_pressure: Option<Antoine>,
    source: String,
}

impl From<Record> for Entry {
    fn from(r: Record) -> Self {
        Entry {
            species: Species {
                name: r.name,
                phase: r.phase,
                molar_mass: r.molar_mass,
                shomate: r.shomate,
                enthalpy_of_formation: r.enthalpy_of_formation,
                density: r.density,
                vapour_pressure: r.vapour_pressure,
            },
            source: r.source,
        }
    }
}

/// The file, as shipped.
const DATA: &str = include_str!("../data/species.json");

static ENTRIES: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    // A panic, not a `Result`: the file is part of the crate, so a bad one is a bug here and not
    // user input, and the tests below parse it on every run.
    let records: Vec<Record> =
        serde_json::from_str(DATA).expect("the shipped species library parses");
    records.into_iter().map(Entry::from).collect()
});

/// Every shipped species, in the order of the file.
pub fn entries() -> &'static [Entry] {
    &ENTRIES
}

/// The shipped entry for `name` in `phase`, if there is one. A linear scan, like
/// [`crate::SpeciesRegistry::find`]: a few dozen entries, and a document resolves each of its
/// species once.
pub fn find(name: &str, phase: Phase) -> Option<&'static Entry> {
    entries()
        .iter()
        .find(|e| e.species.name == name && e.species.phase == phase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{QUARTZ_CP, WATER_CP, WATER_VAPOUR_PRESSURE};
    use crate::serial;
    use crate::thermo::REFERENCE_K;
    use std::collections::HashSet;

    #[test]
    fn every_entry_cites_its_source() {
        for e in entries() {
            assert!(
                !e.source.trim().is_empty(),
                "{} ({:?}) has no source",
                e.species.name,
                e.species.phase
            );
        }
    }

    #[test]
    fn no_name_and_phase_is_shipped_twice() {
        let mut seen = HashSet::new();
        for e in entries() {
            assert!(
                seen.insert((e.species.name.clone(), e.species.phase)),
                "{} ({:?}) is shipped twice",
                e.species.name,
                e.species.phase
            );
        }
    }

    #[test]
    fn every_entry_passes_the_checks_the_loader_makes() {
        // Written out in full, so nothing here depends on the library resolving itself: the
        // same molar mass, heat capacity, density and vapour pressure checks an inline species
        // gets, applied to each shipped one.
        for e in entries() {
            let doc = serial::Flowsheet {
                species: vec![serial::Species::from(&e.species)],
                units: vec![],
                streams: vec![],
            };
            crate::Flowsheet::try_from(doc).unwrap_or_else(|err| {
                panic!(
                    "{} ({:?}) does not load: {err}",
                    e.species.name, e.species.phase
                )
            });
        }
    }

    #[test]
    fn every_heat_capacity_is_positive_across_the_reference_and_boiling_range() {
        // The loader checks 298.15 K alone. A fit copied with a wrong sign somewhere else in its
        // range would pass that and give a stream a negative heat capacity later.
        for e in entries() {
            for t in [REFERENCE_K, 350.0, 400.0] {
                assert!(
                    e.species.shomate.heat_capacity(t) > 0.0,
                    "{} ({:?}) has cp <= 0 at {t} K",
                    e.species.name,
                    e.species.phase
                );
            }
        }
    }

    #[test]
    fn the_demo_constants_and_the_library_agree() {
        // `demo` keeps its own copies because `thermo`'s tests are written against them; this
        // is what stops the two drifting.
        let as_json = |v: &dyn erased::Serialize| v.to_json();
        let water = find("H2O", Phase::Liquid).expect("water is shipped");
        assert_eq!(as_json(&water.species.shomate), as_json(&WATER_CP));
        assert_eq!(
            as_json(&water.species.vapour_pressure),
            as_json(&Some(WATER_VAPOUR_PRESSURE))
        );
        let quartz = find("SiO2", Phase::Solid).expect("quartz is shipped");
        assert_eq!(as_json(&quartz.species.shomate), as_json(&QUARTZ_CP));
    }

    /// `Shomate` and `Antoine` deliberately have no `PartialEq` - nothing in the crate compares
    /// floats with `==` - so the test above compares their JSON instead.
    mod erased {
        pub trait Serialize {
            fn to_json(&self) -> String;
        }
        impl<T: serde::Serialize> Serialize for T {
            fn to_json(&self) -> String {
                serde_json::to_string(self).unwrap()
            }
        }
    }

    /// NIST's normal boiling points, K: the averages on each species' phase-change page, which
    /// come from data independent of the Antoine fits shipped beside them.
    const NORMAL_BOILING_POINTS: [(&str, f64); 7] = [
        ("H2O", 373.17),
        ("CH3OH", 337.8),
        ("C2H5OH", 351.5),
        ("CH3COCH3", 329.3),
        ("C6H6", 353.3),
        ("C7H8", 383.8),
        ("C6H14", 341.9),
    ];

    #[test]
    fn every_vapour_pressure_fit_boils_where_nist_says_at_one_atmosphere() {
        // What catches an Antoine `A` copied in bar rather than kPa: water's boiling point would
        // move from 373 K to 587 K. Each fit is used near the top of its range here, and every
        // range shipped covers the normal boiling point.
        for e in entries() {
            let Some(antoine) = e.species.vapour_pressure else {
                continue;
            };
            let (_, expected) = NORMAL_BOILING_POINTS
                .iter()
                .find(|(name, _)| *name == e.species.name)
                .unwrap_or_else(|| {
                    panic!(
                        "{} ships a vapour pressure but has no boiling point to check it by",
                        e.species.name
                    )
                });
            let boils = antoine.boiling_point(101.325);
            assert!(
                (boils - expected).abs() <= 1.0,
                "{} boils at {boils:.2} K by its fit, {expected} K by NIST",
                e.species.name
            );
        }
    }

    #[test]
    fn the_shipped_water_phases_give_the_steam_table_latent_heat() {
        // 40.65 kJ/mol at 373.15 K. The shipped steam fit is NIST's 500-1700 K one extrapolated
        // down, so this is also the test that the extrapolation is harmless where it is used.
        let liquid = &find("H2O", Phase::Liquid).unwrap().species;
        let gas = &find("H2O", Phase::Gas).unwrap().species;
        let latent = crate::species::latent_heat(liquid, gas, 373.15);
        assert!((latent - 40.65).abs() / 40.65 <= 1e-2, "{latent} kJ/mol");
    }

    #[test]
    fn find_is_by_name_and_phase() {
        assert!(find("H2O", Phase::Liquid).is_some());
        assert!(find("H2O", Phase::Solid).is_none());
        assert!(find("unobtainium", Phase::Solid).is_none());
    }

    /// The README's table of shipped species, rendered from the library, so the two cannot
    /// drift: the README carries the rows between two comment markers, and this is what those
    /// rows must be.
    fn readme_table() -> String {
        let mut out = String::from(
            "| Name | Phase | Molar mass (g/mol) | Also carries |\n\
             |------|-------|--------------------|--------------|\n",
        );
        for e in entries() {
            let s = &e.species;
            let carries: Vec<&str> = [
                (s.enthalpy_of_formation.is_some(), "formation enthalpy"),
                (s.density.is_some(), "density"),
                (s.vapour_pressure.is_some(), "vapour pressure"),
            ]
            .into_iter()
            .filter_map(|(has, what)| has.then_some(what))
            .collect();
            out.push_str(&format!(
                "| `{}` | {:?} | {} | {} |\n",
                s.name,
                s.phase,
                s.molar_mass,
                carries.join(", ")
            ));
        }
        out
    }

    #[test]
    fn the_readme_lists_exactly_the_shipped_species() {
        let readme =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
                .expect("the README is two directories up from the package");
        let start = readme
            .find("<!-- library:start -->\n")
            .expect("the README has a library:start marker")
            + "<!-- library:start -->\n".len();
        let end = readme
            .find("<!-- library:end -->")
            .expect("the README has a library:end marker");
        assert_eq!(
            &readme[start..end],
            readme_table(),
            "the README's shipped-species table is out of date; paste this in:\n{}",
            readme_table()
        );
    }
}

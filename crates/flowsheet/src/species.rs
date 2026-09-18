//! Species data structures.

use crate::thermo::Shomate;
use serde::{Deserialize, Serialize};

/// A distinct form in which matter can exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    /// A state of matter in which atoms are closely packed.
    Solid,
    /// A state of matter with a definite volume but no fixed shape.
    Liquid,
    /// A state of matter with neither fixed volume nor fixed shape.
    Gas,
}

impl Phase {
    /// Every phase, in declaration order.
    pub const ALL: [Phase; 3] = [Phase::Solid, Phase::Liquid, Phase::Gas];

    /// The IUPAC state symbol in brackets, as it follows a name in a species key: `(s)`, `(l)`
    /// or `(g)`.
    pub fn suffix(self) -> &'static str {
        match self {
            Phase::Solid => "(s)",
            Phase::Liquid => "(l)",
            Phase::Gas => "(g)",
        }
    }

    /// Splits a trailing phase suffix off `key`, so `"H2O(g)"` is `("H2O", Phase::Gas)`.
    ///
    /// `None` for a key without one - which includes `"Fe(OH)3"` and `"Quartz(150um)"`: only
    /// the three state symbols count.
    pub fn split_suffix(key: &str) -> Option<(&str, Phase)> {
        Phase::ALL
            .into_iter()
            .find_map(|phase| key.strip_suffix(phase.suffix()).map(|name| (name, phase)))
    }
}

/// Used to index a [`SpeciesRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeciesId(u16);

impl SpeciesId {
    /// Returns the inner value as `usize`.
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for SpeciesId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A species is a distinct particle type within a system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Species {
    /// The name of the species.
    ///
    /// e.g. H2O
    pub name: String,
    /// The phase of the species.
    ///
    /// e.g. [`Phase::Liquid`]
    pub phase: Phase,
    /// The molar mass of the species, in g/mol.
    ///
    /// e.g. `18.015`
    pub molar_mass: f64,
    /// Heat capacity coefficients, per mole.
    ///
    /// e.g. `Shomate::constant(75.3)` for liquid water near 25 °C
    pub shomate: Shomate,
    /// Standard enthalpy of formation at [`crate::thermo::REFERENCE_K`], in kJ/mol - the `H` that
    /// NIST publishes beside the Shomate coefficients. It depends on phase: liquid water is
    /// -285.83 and steam -241.83.
    ///
    /// `None` for a species no reaction touches. Its zero point is the same on both sides of every
    /// balance it enters and cancels, so there is nothing to look up for a plant that only mixes,
    /// splits and heats. A species that *is* destroyed has to carry one.
    ///
    /// e.g. `Some(-285.83)` for liquid water
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enthalpy_of_formation: Option<f64>,
    /// Density, in kg/m³, held constant: the one property that turns a mass flow into a volume.
    ///
    /// `None` for a species nothing pumps. Only a [`crate::unit::Pump`] asks - its work is the
    /// volume it moves times the pressure it adds - so a flotation plant with no pump need not
    /// look up chalcopyrite's, the same allowance as `enthalpy_of_formation`. A species that
    /// *does* flow through a pump has to carry one, and the pump says so if it does not.
    ///
    /// e.g. `Some(997.0)` for liquid water at 25 °C
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<f64>,
}

impl Species {
    /// Specific heat capacity at `temperature` (K), in kJ/(kg·K).
    pub fn heat_capacity(&self, temperature: f64) -> f64 {
        // J/(mol·K) over g/mol is J/(g·K), which is already kJ/(kg·K).
        self.shomate.heat_capacity(temperature) / self.molar_mass
    }

    /// Specific enthalpy at `temperature` (K), in kJ/kg: the enthalpy of formation plus the
    /// sensible heat gained from [`crate::thermo::REFERENCE_K`].
    ///
    /// Absolute when [`Species::enthalpy_of_formation`] is set, so a reaction's heat shows up as
    /// the difference between its products and reactants. Without one this is the sensible heat
    /// alone, exactly as before: `0.0 + x` is `x`, bit for bit.
    pub fn enthalpy(&self, temperature: f64) -> f64 {
        let formation = self.enthalpy_of_formation.unwrap_or(0.0);
        // kJ/mol over g/mol is kJ/g, and a kilogram is a thousand grams.
        1000.0 * (formation + self.shomate.enthalpy(temperature)) / self.molar_mass
    }

    /// Specific entropy at `temperature` (K) relative to [`crate::thermo::REFERENCE_K`], in
    /// kJ/(kg·K). A difference only, like [`crate::thermo::Shomate::entropy`] it is built on.
    pub fn entropy(&self, temperature: f64) -> f64 {
        // J/(mol·K) over g/mol is J/(g·K), which is already kJ/(kg·K) - the same arithmetic as
        // `heat_capacity`.
        self.shomate.entropy(temperature) / self.molar_mass
    }
}

/// Contains a specific set of [`Species`].
#[derive(Debug, Default)]
pub struct SpeciesRegistry {
    species: Vec<Species>,
}

impl SpeciesRegistry {
    /// Inserts `s` into the registry if it doesn't already exist.
    ///
    /// # Panics
    /// If `s` is new and the registry already holds every species a [`SpeciesId`] can address
    /// (65,536 of them), since the cast below would otherwise wrap the new id round to 0. The
    /// check sits in the `None` arm on purpose: a duplicate hands back an id that already exists
    /// and allocates nothing, so a full registry can still answer for what it already has.
    pub fn insert(&mut self, s: Species) -> SpeciesId {
        // TODO: should handle duplicate insert with differing molar masses
        match self.find(&s.name, s.phase) {
            None => {
                crate::assert_id_space(self.species.len(), "species");
                self.species.push(s);
                SpeciesId((self.species.len() - 1) as u16)
            }
            Some(id) => id,
        }
    }

    /// Returns an optional reference to the specified [`Species`].
    pub fn get(&self, id: SpeciesId) -> Option<&Species> {
        self.species.get(id.as_usize())
    }

    /// Searches for a species by `name` and `phase`, returning its optional [`SpeciesId`].
    pub fn find(&self, name: &str, phase: Phase) -> Option<SpeciesId> {
        self.species
            .iter()
            .position(|s| s.name == name && s.phase == phase)
            .map(|i| SpeciesId(i as u16))
    }

    /// The key a document uses for `id`: the bare name when no other species shares it, and the
    /// name with its phase suffix when one does - `"H2O"` alone, but `"H2O(l)"` beside
    /// `"H2O(g)"`.
    ///
    /// Bare wherever it can be, so a single-phase document reads exactly as it did before keys
    /// could carry a phase.
    pub fn key(&self, id: SpeciesId) -> String {
        let species = &self[id];
        if self.shares_name(&species.name) {
            format!("{}{}", species.name, species.phase.suffix())
        } else {
            species.name.clone()
        }
    }

    /// Every [`SpeciesRegistry::key`], in [`SpeciesId`] order.
    pub fn keys(&self) -> Vec<String> {
        (0..self.species.len())
            .map(|i| self.key(SpeciesId(i as u16)))
            .collect()
    }

    /// Resolves a document key back to a species: either a bare name only one species has, or a
    /// name with a phase suffix.
    ///
    /// `None` for an unknown key, and for a bare name two phases share, since it cannot say which
    /// one it means. [`SpeciesRegistry::shares_name`] tells those two cases apart.
    ///
    /// The two readings never compete for one key as long as no name itself ends in a suffix,
    /// which loading and [`crate::flowsheet::Flowsheet::check`] both enforce.
    pub fn resolve(&self, key: &str) -> Option<SpeciesId> {
        let mut named = self
            .species
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == key);
        if let (Some((i, _)), None) = (named.next(), named.next()) {
            return Some(SpeciesId(i as u16));
        }
        let (name, phase) = Phase::split_suffix(key)?;
        self.find(name, phase)
    }

    /// Returns `true` if more than one species is called `name` - the one case where a bare name
    /// is ambiguous.
    pub fn shares_name(&self, name: &str) -> bool {
        self.species
            .iter()
            .filter(|s| s.name == name)
            .nth(1)
            .is_some()
    }

    /// Returns the number of species in the registry.
    pub fn len(&self) -> usize {
        self.species.len()
    }

    /// Returns `true` if the registry contains no species.
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }

    /// Every [`Species`], in [`SpeciesId`] order.
    pub fn all(&self) -> &[Species] {
        &self.species
    }
}

impl std::ops::Index<SpeciesId> for SpeciesRegistry {
    type Output = Species;
    fn index(&self, id: SpeciesId) -> &Species {
        &self.species[id.as_usize()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{QUARTZ_CP, WATER_CP};
    use crate::thermo::REFERENCE_K;
    use approx::assert_relative_eq;

    fn water() -> Species {
        Species {
            name: "H2O".into(),
            phase: Phase::Liquid,
            molar_mass: 18.015,
            shomate: WATER_CP,
            enthalpy_of_formation: None,
            density: None,
        }
    }

    #[test]
    fn insert_returns_sequential_ids() {
        let mut reg = SpeciesRegistry::default();
        let a = reg.insert(water());
        let b = reg.insert(Species {
            name: "SiO2".into(),
            phase: Phase::Solid,
            molar_mass: 60.08,
            shomate: QUARTZ_CP,
            enthalpy_of_formation: None,
            density: None,
        });
        assert_eq!(a.as_usize(), 0);
        assert_eq!(b.as_usize(), 1);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn a_suffix_is_one_of_the_three_state_symbols_and_nothing_else() {
        assert_eq!(Phase::split_suffix("H2O(g)"), Some(("H2O", Phase::Gas)));
        assert_eq!(Phase::split_suffix("NaCl(s)"), Some(("NaCl", Phase::Solid)));
        assert_eq!(Phase::split_suffix("H2O(l)"), Some(("H2O", Phase::Liquid)));
        assert_eq!(Phase::split_suffix("H2O"), None);
        assert_eq!(Phase::split_suffix("Fe(OH)3"), None);
        assert_eq!(Phase::split_suffix("Quartz(150um)"), None);
    }

    #[test]
    fn a_name_only_one_phase_uses_is_keyed_bare() {
        let mut reg = SpeciesRegistry::default();
        let id = reg.insert(water());
        assert_eq!(reg.key(id), "H2O");
        assert_eq!(reg.resolve("H2O"), Some(id));
        // The suffixed form is still accepted, just never written.
        assert_eq!(reg.resolve("H2O(l)"), Some(id));
        assert_eq!(reg.resolve("H2O(g)"), None);
    }

    #[test]
    fn a_name_two_phases_share_is_keyed_with_a_suffix() {
        let mut reg = SpeciesRegistry::default();
        let liquid = reg.insert(water());
        let gas = reg.insert(Species {
            phase: Phase::Gas,
            ..water()
        });
        assert_eq!(reg.keys(), vec!["H2O(l)", "H2O(g)"]);
        assert_eq!(reg.resolve("H2O(l)"), Some(liquid));
        assert_eq!(reg.resolve("H2O(g)"), Some(gas));
        assert_eq!(reg.resolve("H2O"), None);
        assert!(reg.shares_name("H2O"));
    }

    #[test]
    fn insert_dedupes_on_name_and_phase() {
        let mut reg = SpeciesRegistry::default();
        let a = reg.insert(water());
        let b = reg.insert(water());
        assert_eq!(a, b);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn same_name_different_phase_is_a_distinct_species() {
        let mut reg = SpeciesRegistry::default();
        let liquid = reg.insert(water());
        let gas = reg.insert(Species {
            phase: Phase::Gas,
            ..water()
        });
        assert_ne!(liquid, gas);
        assert_eq!(reg[gas].phase, Phase::Gas);
    }

    #[test]
    fn get_is_none_for_unknown_and_find_round_trips() {
        let mut reg = SpeciesRegistry::default();
        let id = reg.insert(water());
        assert_eq!(reg.find("H2O", Phase::Liquid), Some(id));
        assert_eq!(reg.find("H2O", Phase::Gas), None);
        assert_eq!(reg[id].molar_mass, 18.015);
    }

    #[test]
    fn the_mass_basis_divides_the_molar_properties_by_molar_mass() {
        // 18.015 J/(mol·K) over 18.015 g/mol is exactly 1 J/(g·K), which is 1 kJ/(kg·K)...
        let s = Species {
            shomate: Shomate::constant(18.015),
            ..water()
        };
        assert_relative_eq!(s.heat_capacity(350.0), 1.0, max_relative = 1e-12);
        // ...so 10 K above the reference is 10 kJ/kg.
        assert_relative_eq!(s.enthalpy(REFERENCE_K + 10.0), 10.0, max_relative = 1e-12);
    }

    #[test]
    fn an_enthalpy_of_formation_offsets_enthalpy_and_leaves_heat_capacity_alone() {
        let plain = water();
        let formed = Species {
            enthalpy_of_formation: Some(-285.83),
            density: None,
            ..water()
        };

        // At the reference temperature there is no sensible heat, so all that is left is the
        // formation enthalpy per kg: -285.83 kJ/mol over 18.015 g/mol, times 1000 g/kg.
        assert_relative_eq!(
            formed.enthalpy(REFERENCE_K),
            -285_830.0 / 18.015,
            max_relative = 1e-12
        );

        // Away from it, the offset is the same constant, and the slope does not see it.
        for temperature in [300.0, 350.0, 450.0] {
            assert_relative_eq!(
                formed.enthalpy(temperature) - plain.enthalpy(temperature),
                -285_830.0 / 18.015,
                max_relative = 1e-9
            );
            assert_eq!(
                formed.heat_capacity(temperature),
                plain.heat_capacity(temperature)
            );
        }
    }

    #[test]
    fn liquid_water_takes_4_18_kilojoules_per_kilogram_kelvin() {
        assert_relative_eq!(
            water().heat_capacity(REFERENCE_K),
            4.18,
            max_relative = 2e-3
        );
    }

    /// A registry holding `n` distinct species, filled by pushing rather than by `insert`.
    ///
    /// `insert` runs `find` first, and `find` is a linear scan, so filling 65,536 species through
    /// the public path would be O(n^2) - roughly two billion comparisons. Pushing straight into
    /// the vector reaches the same state, and the names stay distinct so that the one `insert`
    /// each test does still takes the branch it is meant to.
    fn filled_to(n: usize) -> SpeciesRegistry {
        SpeciesRegistry {
            species: (0..n)
                .map(|i| Species {
                    name: format!("s{i}"),
                    phase: Phase::Solid,
                    molar_mass: 1.0,
                    shomate: Shomate::constant(1.0),
                    enthalpy_of_formation: None,
                    density: None,
                })
                .collect(),
        }
    }

    #[test]
    #[should_panic(expected = "65537 species exceeds the 65536 a u16 id can address")]
    fn one_species_past_the_u16_id_space_panics() {
        // `water` is liquid H2O and every filler species is a solid, so `find` misses and the
        // insert takes the `None` arm where the new id would be allocated.
        filled_to(crate::MAX_IDS).insert(water());
    }

    #[test]
    fn a_full_registry_still_answers_for_a_species_it_already_holds() {
        // The guard sits in the `None` arm, so a duplicate costs no id and must not panic even
        // when there is no id left to hand out.
        let mut registry = filled_to(crate::MAX_IDS);
        let duplicate = Species {
            name: "s0".into(),
            phase: Phase::Solid,
            molar_mass: 1.0,
            shomate: Shomate::constant(1.0),
            enthalpy_of_formation: None,
            density: None,
        };
        assert_eq!(registry.insert(duplicate).as_usize(), 0);
    }
}

//! Heat capacity and enthalpy, from the Shomate equation.
//!
//! Everything in this module is per mole, because that is how the coefficients are published.
//! [`Species::heat_capacity`] and [`Species::enthalpy`] convert to the per-kilogram basis the
//! rest of the crate works in.
//!
//! [`Species::heat_capacity`]: crate::species::Species::heat_capacity
//! [`Species::enthalpy`]: crate::species::Species::enthalpy

use serde::{Deserialize, Serialize};

/// The temperature every enthalpy is measured from, in Kelvin (25 °C).
///
/// Sensible heat is measured from here, and [`crate::Species::enthalpy_of_formation`] is quoted at
/// it, so the two add to an absolute enthalpy. It is the temperature NIST publishes both against.
pub const REFERENCE_K: f64 = 298.15;

/// The molar gas constant, in J/(mol·K). CODATA 2018, which made it exact.
///
/// What an ideal gas's entropy gains per mole for a fall in pressure: `R ln(P1 / P2)`. That is
/// the one place pressure enters the thermodynamics, and it is why a compressor can be built
/// from the Shomate fit alone (see [`crate::unit::compress`]).
pub const GAS_CONSTANT: f64 = 8.314_462_618;

/// Heat capacity coefficients for the Shomate equation, as published by the NIST Chemistry
/// WebBook.
///
/// With `t = T / 1000`, and `T` in Kelvin:
///
/// ```text
/// cp(T) = A + B·t + C·t² + D·t³ + E/t²    J/(mol·K)
/// ```
///
/// NIST publishes `F`, `G` and `H` as well, and none of them is stored here. `H` *is* the standard
/// enthalpy of formation, and it lives on [`crate::Species::enthalpy_of_formation`] instead.
/// `F - H` only makes the antiderivative vanish at [`REFERENCE_K`], which [`Shomate::enthalpy`]
/// already does exactly by subtracting it there; storing `F` would be a second, rounded source of
/// the same number. `G` is for entropy. A document that pastes any of the three in is rejected for
/// its unknown fields rather than having them silently ignored.
///
/// A constant heat capacity is the special case `B = C = D = E = 0`, built by
/// [`Shomate::constant`]. A zero term means the term is absent, the same way a zero flow means the
/// species is absent, so a document may leave zero terms out and saving always does.
///
/// Coefficients are fitted over a stated temperature range, which is not stored: nothing checks
/// that a stream stays inside it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shomate {
    /// The constant term, in J/(mol·K).
    pub a: f64,
    /// The coefficient of `t`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub b: f64,
    /// The coefficient of `t²`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub c: f64,
    /// The coefficient of `t³`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub d: f64,
    /// The coefficient of `1/t²`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub e: f64,
}

/// `skip_serializing_if` hands its predicate a reference, hence `&f64`.
fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl Shomate {
    /// A heat capacity that does not vary with temperature, in J/(mol·K).
    pub const fn constant(cp: f64) -> Self {
        Self {
            a: cp,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
        }
    }

    /// Heat capacity at `temperature` (K), in J/(mol·K).
    pub fn heat_capacity(&self, temperature: f64) -> f64 {
        let t = temperature / 1000.0;
        self.a + self.b * t + self.c * t.powi(2) + self.d * t.powi(3) + self.e / t.powi(2)
    }

    /// Enthalpy at `temperature` (K) relative to [`REFERENCE_K`], in kJ/mol.
    pub fn enthalpy(&self, temperature: f64) -> f64 {
        self.antiderivative(temperature / 1000.0) - self.antiderivative(REFERENCE_K / 1000.0)
    }

    /// Entropy at `temperature` (K) relative to [`REFERENCE_K`], in J/(mol·K): the integral of
    /// `cp / T`, at constant pressure.
    ///
    /// NIST's `G` is the constant that makes this absolute, and it is not stored, so this is a
    /// difference only. That is all an isentropic path needs: it asks where the stream's entropy
    /// returns to what it was, and the offset is the same at both ends. Pressure is not in here.
    /// An ideal gas gains `R ln(P1 / P2)` per mole for a fall in pressure, and that term is the
    /// caller's ([`crate::unit::compress`]), because the coefficients know nothing about it.
    pub fn entropy(&self, temperature: f64) -> f64 {
        self.entropy_antiderivative(temperature / 1000.0)
            - self.entropy_antiderivative(REFERENCE_K / 1000.0)
    }

    /// An antiderivative of [`Shomate::heat_capacity`] with respect to `t`, in kJ/mol.
    ///
    /// Integrating over `t` rather than `T` is what turns J into kJ: `dT = 1000 dt`, so the
    /// factor of 1000 is absorbed by the substitution instead of appearing as a conversion.
    fn antiderivative(&self, t: f64) -> f64 {
        self.a * t + self.b * t.powi(2) / 2.0 + self.c * t.powi(3) / 3.0 + self.d * t.powi(4) / 4.0
            - self.e / t
    }

    /// An antiderivative of `cp / t` with respect to `t`, in J/(mol·K).
    ///
    /// No factor of 1000 this time: `dT / T` is `dt / t`, since the scale cancels. The Shomate
    /// entropy equation NIST publishes is this plus `G`.
    fn entropy_antiderivative(&self, t: f64) -> f64 {
        self.a * t.ln() + self.b * t + self.c * t.powi(2) / 2.0 + self.d * t.powi(3) / 3.0
            - self.e / (2.0 * t.powi(2))
    }
}

/// Vapour pressure from the Antoine equation, in the crate's units:
///
/// ```text
/// log10(P) = a - b / (c + T)    P in kPa, T in K
/// ```
///
/// NIST publishes Antoine coefficients in **bar**, so their `A` needs `+2` on the way into a
/// document (`log10(100)`); `B` and `C` carry over unchanged. Written in kPa here rather than
/// converted in code, so that the one system of units the crate uses holds on the wire too.
///
/// Like [`Shomate`], a fit over a stated temperature range that is not stored, and nothing checks
/// that a temperature stays inside it. Water alone has three NIST fits between 255 K and 573 K.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Antoine {
    /// The constant term: `log10` of a pressure in kPa.
    pub a: f64,
    /// The temperature scale, in K. Positive, so that vapour pressure rises with temperature.
    pub b: f64,
    /// The temperature offset, in K. Negative for every published fit in Kelvin.
    pub c: f64,
}

impl Antoine {
    /// The vapour pressure at `temperature` (K), in kPa.
    ///
    /// Zero at and below `T = -c`. The equation has a pole there: as `T` falls towards it the
    /// pressure falls to zero, and past it `c + T` turns negative and the formula climbs back to
    /// an absurd pressure - water's `c` of -45.6 K would boil it at 45 K. Zero is the equation's
    /// own limit from above, so the extension keeps vapour pressure rising with temperature
    /// everywhere, which a search over temperature, like the adiabatic flash's, depends on.
    pub fn vapour_pressure(&self, temperature: f64) -> f64 {
        if self.c + temperature <= 0.0 {
            return 0.0;
        }
        10f64.powf(self.a - self.b / (self.c + temperature))
    }

    /// The temperature (K) at which the vapour pressure is `pressure` (kPa): the boiling point
    /// at that pressure. The closed-form inverse of [`Antoine::vapour_pressure`], so no
    /// iteration is needed.
    pub fn boiling_point(&self, pressure: f64) -> f64 {
        self.b / (self.a - pressure.log10()) - self.c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{QUARTZ_CP, WATER_CP, WATER_VAPOUR_PRESSURE};
    use approx::assert_relative_eq;

    #[test]
    fn water_boils_at_one_atmosphere() {
        // The steam table's 101.325 kPa at 373.15 K, from NIST's 344-373 K fit.
        assert_relative_eq!(
            WATER_VAPOUR_PRESSURE.vapour_pressure(373.15),
            101.325,
            max_relative = 5e-3
        );
    }

    #[test]
    fn vapour_pressure_is_zero_below_the_pole_rather_than_absurd() {
        // Water's pole is at 45.622 K. Just above it the pressure is vanishingly small; below it
        // the raw formula would give 10^79 kPa at 22 K.
        assert!(WATER_VAPOUR_PRESSURE.vapour_pressure(46.0) < 1e-300);
        assert_eq!(WATER_VAPOUR_PRESSURE.vapour_pressure(45.622), 0.0);
        assert_eq!(WATER_VAPOUR_PRESSURE.vapour_pressure(22.0), 0.0);
    }

    #[test]
    fn boiling_point_inverts_vapour_pressure() {
        for temperature in [350.0, 365.0, 373.15] {
            let pressure = WATER_VAPOUR_PRESSURE.vapour_pressure(temperature);
            assert_relative_eq!(
                WATER_VAPOUR_PRESSURE.boiling_point(pressure),
                temperature,
                max_relative = 1e-12
            );
        }
    }

    #[test]
    fn vapour_pressure_rises_with_temperature() {
        assert!(
            WATER_VAPOUR_PRESSURE.vapour_pressure(360.0)
                > WATER_VAPOUR_PRESSURE.vapour_pressure(350.0)
        );
    }

    #[test]
    fn a_fourth_antoine_coefficient_is_rejected() {
        let e =
            serde_json::from_str::<Antoine>(r#"{ "a": 7.08, "b": 1663.1, "c": -45.6, "d": 1.0 }"#)
                .unwrap_err();
        assert!(e.to_string().contains("unknown field `d`"), "{e}");
    }

    #[test]
    fn enthalpy_is_zero_at_the_reference_temperature() {
        assert_relative_eq!(WATER_CP.enthalpy(REFERENCE_K), 0.0);
        assert_relative_eq!(QUARTZ_CP.enthalpy(REFERENCE_K), 0.0);
    }

    #[test]
    fn the_published_coefficients_reproduce_the_textbook_heat_capacities() {
        // 75.3 J/(mol·K) is the familiar 4.18 kJ/(kg·K) of water, per mole instead of per kg.
        assert_relative_eq!(
            WATER_CP.heat_capacity(REFERENCE_K),
            75.3,
            max_relative = 2e-3
        );
        assert_relative_eq!(
            QUARTZ_CP.heat_capacity(REFERENCE_K),
            44.6,
            max_relative = 2e-3
        );
    }

    #[test]
    fn a_constant_heat_capacity_gives_an_enthalpy_linear_in_temperature() {
        let cp = Shomate::constant(75.0);
        assert_relative_eq!(cp.heat_capacity(500.0), 75.0);
        // 75 J/(mol·K) across 100 K is 7500 J/mol, which is 7.5 kJ/mol.
        assert_relative_eq!(cp.enthalpy(REFERENCE_K + 100.0), 7.5, max_relative = 1e-12);
    }

    #[test]
    fn enthalpy_is_the_integral_of_heat_capacity() {
        // A central difference of h gives back cp. The 1000 is the kJ-to-J step the
        // antiderivative gets for free by integrating over `t`. Get it wrong and this is 1000x off.
        let dt = 1e-3;
        for cp in [WATER_CP, QUARTZ_CP] {
            for temperature in [300.0, 350.0, 450.0] {
                let slope =
                    (cp.enthalpy(temperature + dt) - cp.enthalpy(temperature - dt)) / (2.0 * dt);
                assert_relative_eq!(
                    1000.0 * slope,
                    cp.heat_capacity(temperature),
                    max_relative = 1e-6
                );
            }
        }
    }

    #[test]
    fn entropy_is_zero_at_the_reference_temperature() {
        assert_relative_eq!(WATER_CP.entropy(REFERENCE_K), 0.0);
        assert_relative_eq!(QUARTZ_CP.entropy(REFERENCE_K), 0.0);
    }

    #[test]
    fn a_constant_heat_capacity_gives_an_entropy_logarithmic_in_temperature() {
        let cp = Shomate::constant(75.0);
        assert_relative_eq!(
            cp.entropy(2.0 * REFERENCE_K),
            75.0 * 2.0_f64.ln(),
            max_relative = 1e-12
        );
    }

    #[test]
    fn entropy_is_the_integral_of_heat_capacity_over_temperature() {
        // The same central difference as for enthalpy, and this time no 1000: `dT / T` is
        // scale-free, so the antiderivative over `t` is already in J/(mol·K).
        let dt = 1e-3;
        for cp in [WATER_CP, QUARTZ_CP] {
            for temperature in [300.0, 350.0, 450.0] {
                let slope =
                    (cp.entropy(temperature + dt) - cp.entropy(temperature - dt)) / (2.0 * dt);
                assert_relative_eq!(
                    slope,
                    cp.heat_capacity(temperature) / temperature,
                    max_relative = 1e-6
                );
            }
        }
    }

    #[test]
    fn zero_terms_are_left_off_the_wire_and_load_back_as_zero() {
        let json = serde_json::to_string(&Shomate::constant(75.3)).unwrap();
        assert_eq!(json, r#"{"a":75.3}"#);

        let back: Shomate = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn nist_f_g_and_h_are_rejected_rather_than_ignored() {
        let e = serde_json::from_str::<Shomate>(r#"{ "a": 75.3, "h": -285.8304 }"#).unwrap_err();
        assert!(e.to_string().contains("unknown field `h`"), "{e}");
    }
}

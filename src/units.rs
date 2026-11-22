//! Physical quantities.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub};

/// Active power in kilowatts. Positive is consumption (Bezug), negative is feed-in.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Kw(pub f64);

impl Kw {
    /// Zero power.
    pub const ZERO: Kw = Kw(0.0);

    /// The larger of two powers.
    #[must_use]
    pub fn max(self, other: Kw) -> Kw {
        Kw(self.0.max(other.0))
    }

    /// The smaller of two powers.
    #[must_use]
    pub fn min(self, other: Kw) -> Kw {
        Kw(self.0.min(other.0))
    }

    /// Absolute value.
    #[must_use]
    pub fn abs(self) -> Kw {
        Kw(self.0.abs())
    }

    /// Rounded to watts, which is the resolution every record is written in.
    #[must_use]
    pub fn round_w(self) -> Kw {
        Kw((self.0 * 1000.0).round() / 1000.0)
    }
}

impl fmt::Display for Kw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = f.precision().unwrap_or(1);
        write!(f, "{:.*} kW", p, self.0)
    }
}

impl Add for Kw {
    type Output = Kw;
    fn add(self, rhs: Kw) -> Kw {
        Kw(self.0 + rhs.0)
    }
}

impl AddAssign for Kw {
    fn add_assign(&mut self, rhs: Kw) {
        self.0 += rhs.0;
    }
}

impl Sub for Kw {
    type Output = Kw;
    fn sub(self, rhs: Kw) -> Kw {
        Kw(self.0 - rhs.0)
    }
}

impl Neg for Kw {
    type Output = Kw;
    fn neg(self) -> Kw {
        Kw(-self.0)
    }
}

impl Mul<f64> for Kw {
    type Output = Kw;
    fn mul(self, rhs: f64) -> Kw {
        Kw(self.0 * rhs)
    }
}

impl std::iter::Sum for Kw {
    fn sum<I: Iterator<Item = Kw>>(iter: I) -> Kw {
        Kw(iter.map(|k| k.0).sum())
    }
}

/// Voltage in per unit of the nominal voltage (1.0 = 230/400 V).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PerUnit(pub f64);

impl fmt::Display for PerUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} p.u.", self.0)
    }
}

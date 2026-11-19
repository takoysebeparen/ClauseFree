//! Deterministic fixed-point number used for every non-integer game value.
//!
//! Floating point is not allowed in the simulation (results would differ between machines). `Fixed` stores a value
//! as a signed 64-bit integer scaled by [`Fixed::SCALE`] (5 decimal digits).
//!
//! Source: the community wiki ("Data structures", section on variables) describes the original's numbers as a signed
//! 32-bit whole part plus a fractional part from 0 to 99 999, i.e. five decimal digits. Another wiki statement says
//! modifiers support at most 3 decimals. Five digits covers both; the exact precision and rounding rules of the
//! original are **not yet measured** (see docs/CANON.md). Keep the scale a single constant so it can be changed.
//!
//! Arithmetic saturates instead of overflowing, so malformed data can never cause a panic.

use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

/// Fixed-point number with five decimal digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fixed(i64);

/// Why a decimal text could not be read as a [`Fixed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseFixedError {
    Empty,
    InvalidCharacter,
    OutOfRange,
}

impl fmt::Display for ParseFixedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseFixedError::Empty => "empty number",
            ParseFixedError::InvalidCharacter => "not a decimal number",
            ParseFixedError::OutOfRange => "number out of range",
        })
    }
}

impl std::error::Error for ParseFixedError {}

impl Fixed {
    /// Number of decimal digits kept.
    pub const DECIMALS: u32 = 5;
    /// Raw units per whole number.
    pub const SCALE: i64 = 100_000;
    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(Self::SCALE);
    pub const MAX: Fixed = Fixed(i64::MAX);
    pub const MIN: Fixed = Fixed(i64::MIN);
    /// Largest whole part accepted when parsing: the original keeps a signed 32-bit whole part.
    pub const PARSE_LIMIT: i64 = i32::MAX as i64;

    pub const fn from_raw(raw: i64) -> Fixed {
        Fixed(raw)
    }

    pub const fn raw(self) -> i64 {
        self.0
    }

    pub const fn from_int(value: i64) -> Fixed {
        Fixed(value.saturating_mul(Self::SCALE))
    }

    /// Whole part, rounded toward zero.
    pub const fn trunc(self) -> i64 {
        self.0 / Self::SCALE
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn abs(self) -> Fixed {
        Fixed(self.0.saturating_abs())
    }

    /// Quotient, rounded toward zero. `None` when dividing by zero.
    pub fn checked_div(self, other: Fixed) -> Option<Fixed> {
        if other.0 == 0 {
            return None;
        }
        let wide = i128::from(self.0) * i128::from(Self::SCALE) / i128::from(other.0);
        Some(Fixed(saturate(wide)))
    }

    /// Parses decimal text as written in game files: `12`, `-0.75`, `.5`, `5.`, `+3`.
    /// Digits beyond the fifth decimal are dropped (rounding toward zero; the original's rule is unmeasured).
    /// The whole part must fit the original's signed 32-bit range.
    pub fn parse(text: &str) -> Result<Fixed, ParseFixedError> {
        let (negative, body) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
        if whole.is_empty() && frac.is_empty() {
            return Err(ParseFixedError::Empty);
        }
        if !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseFixedError::InvalidCharacter);
        }
        let whole_value: i64 = if whole.is_empty() {
            0
        } else {
            // More than 10 digits cannot fit the 32-bit range anyway; avoid overflow while parsing.
            if whole.trim_start_matches('0').len() > 10 {
                return Err(ParseFixedError::OutOfRange);
            }
            whole.parse().map_err(|_| ParseFixedError::OutOfRange)?
        };
        if whole_value > Self::PARSE_LIMIT {
            return Err(ParseFixedError::OutOfRange);
        }
        let mut frac_value: i64 = 0;
        let mut digits = 0;
        for b in frac.bytes().take(Self::DECIMALS as usize) {
            frac_value = frac_value * 10 + i64::from(b - b'0');
            digits += 1;
        }
        while digits < Self::DECIMALS {
            frac_value *= 10;
            digits += 1;
        }
        let raw = whole_value * Self::SCALE + frac_value;
        Ok(Fixed(if negative { -raw } else { raw }))
    }
}

fn saturate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

impl Add for Fixed {
    type Output = Fixed;
    fn add(self, other: Fixed) -> Fixed {
        Fixed(self.0.saturating_add(other.0))
    }
}

impl Sub for Fixed {
    type Output = Fixed;
    fn sub(self, other: Fixed) -> Fixed {
        Fixed(self.0.saturating_sub(other.0))
    }
}

impl Mul for Fixed {
    type Output = Fixed;
    /// Product, rounded toward zero.
    fn mul(self, other: Fixed) -> Fixed {
        let wide = i128::from(self.0) * i128::from(other.0) / i128::from(Self::SCALE);
        Fixed(saturate(wide))
    }
}

impl AddAssign for Fixed {
    fn add_assign(&mut self, other: Fixed) {
        *self = *self + other;
    }
}

impl SubAssign for Fixed {
    fn sub_assign(&mut self, other: Fixed) {
        *self = *self - other;
    }
}

impl Neg for Fixed {
    type Output = Fixed;
    fn neg(self) -> Fixed {
        Fixed(self.0.saturating_neg())
    }
}

impl fmt::Display for Fixed {
    /// Shortest exact decimal form: `1.5`, `-0.75`, `3`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw = i128::from(self.0);
        let sign = if raw < 0 { "-" } else { "" };
        let abs = raw.abs();
        let scale = i128::from(Self::SCALE);
        let (whole, frac) = (abs / scale, abs % scale);
        if frac == 0 {
            return write!(f, "{sign}{whole}");
        }
        let digits = format!("{:0width$}", frac, width = Self::DECIMALS as usize);
        write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fixed({self})")
    }
}

impl std::str::FromStr for Fixed {
    type Err = ParseFixedError;
    fn from_str(s: &str) -> Result<Fixed, ParseFixedError> {
        Fixed::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(text: &str) -> Fixed {
        Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parses_the_forms_found_in_game_files() {
        assert_eq!(f("12").raw(), 1_200_000);
        assert_eq!(f("-0.75").raw(), -75_000);
        assert_eq!(f(".5").raw(), 50_000);
        assert_eq!(f("5.").raw(), 500_000);
        assert_eq!(f("+3").raw(), 300_000);
        assert_eq!(f("0.00001").raw(), 1);
        assert_eq!(f("0.123456789").raw(), 12_345, "digits beyond the fifth are dropped");
        assert_eq!(f("-0.000009").raw(), 0, "rounds toward zero");
        assert_eq!(f("2147483647").trunc(), 2_147_483_647);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert_eq!(Fixed::parse(""), Err(ParseFixedError::Empty));
        assert_eq!(Fixed::parse("-"), Err(ParseFixedError::Empty));
        assert_eq!(Fixed::parse("."), Err(ParseFixedError::Empty));
        assert_eq!(Fixed::parse("1.2.3"), Err(ParseFixedError::InvalidCharacter));
        assert_eq!(Fixed::parse("1e5"), Err(ParseFixedError::InvalidCharacter));
        assert_eq!(Fixed::parse("yes"), Err(ParseFixedError::InvalidCharacter));
        assert_eq!(Fixed::parse("--1"), Err(ParseFixedError::InvalidCharacter));
        assert_eq!(Fixed::parse("2147483648"), Err(ParseFixedError::OutOfRange));
        assert_eq!(Fixed::parse("99999999999999999999999"), Err(ParseFixedError::OutOfRange));
        assert_eq!(f("0000000000000000005").trunc(), 5, "leading zeros are fine");
    }

    #[test]
    fn arithmetic_is_exact_and_saturating() {
        assert_eq!(f("0.1") + f("0.2"), f("0.3"), "no floating point drift");
        assert_eq!(f("1.5") * f("-2"), f("-3"));
        assert_eq!(f("0.33333") * f("3"), f("0.99999"));
        assert_eq!(f("1").checked_div(f("3")), Some(f("0.33333")));
        assert_eq!(f("1").checked_div(Fixed::ZERO), None);
        assert_eq!(Fixed::MAX + Fixed::ONE, Fixed::MAX);
        assert_eq!(Fixed::MIN - Fixed::ONE, Fixed::MIN);
        assert_eq!(-Fixed::MIN, Fixed::MAX);
        assert_eq!(Fixed::MAX * Fixed::MAX, Fixed::MAX);
        assert_eq!(f("-2.5").abs(), f("2.5"));
        assert_eq!(f("-2.7").trunc(), -2);
        assert_eq!(Fixed::from_int(4), f("4"));
    }

    #[test]
    fn display_round_trips() {
        for text in ["0", "1.5", "-0.75", "3", "0.00001", "-12.34567", "2147483647"] {
            assert_eq!(f(text).to_string(), text);
        }
        assert_eq!(format!("{:?}", f("0.5")), "Fixed(0.5)");
        assert_eq!(Fixed::MIN.to_string(), "-92233720368547.75808");
    }
}

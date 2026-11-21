//! Game time: an absolute hour counter and the calendar that maps it to dates.
//!
//! The simulation advances in whole hours. A [`GameTime`] is the number of hours since `0.1.1` hour 0, so time is a
//! single integer that orders, subtracts and hashes trivially; dates are only a view of it.
//!
//! **Calendar (guess, see `docs/CANON.md`, "Calendar"):** every year has 365 days and February always has 28. This is
//! a community claim about all Clausewitz games, not yet measured in HOI4 itself. No date in the base game's data uses
//! 29 February, which is consistent with it but does not prove it.

use std::fmt;

use data::Date;

/// Hours in a day.
pub const HOURS_PER_DAY: i64 = 24;
/// Days in every year of the calendar (no leap years; see the module documentation).
pub const DAYS_PER_YEAR: i64 = 365;
/// Hours in every year.
pub const HOURS_PER_YEAR: i64 = DAYS_PER_YEAR * HOURS_PER_DAY;

/// Days of each month, January first.
const MONTH_DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Absolute game time in hours since `0.1.1` hour 0. Negative values are years before 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GameTime(pub i64);

/// Why a [`Date`] is not a valid point of the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    /// The month is not 1..=12.
    Month(Date),
    /// The day does not exist in that month (for example 29 February, or 31 April).
    Day(Date),
    /// The hour is not 0..=23.
    Hour(Date),
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, d) = match self {
            TimeError::Month(d) => ("month", d),
            TimeError::Day(d) => ("day", d),
            TimeError::Hour(d) => ("hour", d),
        };
        write!(f, "invalid {what} in date {}.{}.{}.{}", d.year, d.month, d.day, d.hour)
    }
}

impl std::error::Error for TimeError {}

/// Number of days in `month` (1..=12); 0 for an invalid month.
pub fn days_in_month(month: u8) -> u8 {
    match month {
        1..=12 => MONTH_DAYS[usize::from(month - 1)],
        _ => 0,
    }
}

impl GameTime {
    /// Converts a calendar date. Rejects dates that do not exist in the calendar instead of normalising them.
    pub fn from_date(date: Date) -> Result<GameTime, TimeError> {
        if !(1..=12).contains(&date.month) {
            return Err(TimeError::Month(date));
        }
        if date.day == 0 || date.day > days_in_month(date.month) {
            return Err(TimeError::Day(date));
        }
        if i64::from(date.hour) >= HOURS_PER_DAY {
            return Err(TimeError::Hour(date));
        }
        let days_before_month: i64 = MONTH_DAYS[..usize::from(date.month - 1)].iter().map(|d| i64::from(*d)).sum();
        let day_of_year = days_before_month + i64::from(date.day) - 1;
        let days = i64::from(date.year) * DAYS_PER_YEAR + day_of_year;
        Ok(GameTime(days * HOURS_PER_DAY + i64::from(date.hour)))
    }

    /// The calendar date of this hour. Years outside the `i32` range saturate.
    pub fn to_date(self) -> Date {
        let hour = self.0.rem_euclid(HOURS_PER_DAY);
        let days = self.0.div_euclid(HOURS_PER_DAY);
        let year = days.div_euclid(DAYS_PER_YEAR);
        let mut day_of_year = days.rem_euclid(DAYS_PER_YEAR);
        let mut month = 1u8;
        for len in MONTH_DAYS {
            if day_of_year < i64::from(len) {
                break;
            }
            day_of_year -= i64::from(len);
            month += 1;
        }
        Date {
            year: year.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            month,
            // Both values are bounded by the calendar (day < 31, hour < 24), so the casts cannot truncate.
            day: (day_of_year + 1) as u8,
            hour: hour as u8,
        }
    }

    /// Hour of the day, 0..=23.
    pub fn hour(self) -> u8 {
        self.0.rem_euclid(HOURS_PER_DAY) as u8
    }

    /// The time `hours` later (saturating at the ends of the range).
    pub fn plus_hours(self, hours: i64) -> GameTime {
        GameTime(self.0.saturating_add(hours))
    }

    /// Which periodic pulses start at this hour.
    pub fn pulses(self) -> Pulses {
        let date = self.to_date();
        let day = date.hour == 0;
        let month = day && date.day == 1;
        let year = month && date.month == 1;
        Pulses { day, month, year }
    }
}

impl fmt::Display for GameTime {
    /// The script notation `Y.M.D.H`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.to_date();
        write!(f, "{}.{}.{}.{}", d.year, d.month, d.day, d.hour)
    }
}

/// The periodic pulses that fire at one hour, in addition to the hourly one.
///
/// **Guess (see `docs/CANON.md`, "Clock pulses"):** a day starts at hour 0 and a month at hour 0 of its first day.
/// When the original runs its daily and monthly work is not yet measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pulses {
    pub day: bool,
    pub month: bool,
    pub year: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u8, d: u8, h: u8) -> Date {
        Date { year: y, month: m, day: d, hour: h }
    }

    fn time(y: i32, m: u8, d: u8, h: u8) -> GameTime {
        GameTime::from_date(date(y, m, d, h)).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn round_trip_every_hour_of_a_year() {
        let start = time(1936, 1, 1, 0);
        for h in 0..HOURS_PER_YEAR + 48 {
            let t = start.plus_hours(h);
            assert_eq!(GameTime::from_date(t.to_date()), Ok(t), "{t}");
        }
        assert_eq!(time(1937, 1, 1, 0).0 - start.0, HOURS_PER_YEAR);
    }

    #[test]
    fn month_and_year_boundaries() {
        assert_eq!(time(1936, 2, 28, 23).plus_hours(1).to_date(), date(1936, 3, 1, 0));
        assert_eq!(time(1936, 12, 31, 23).plus_hours(1).to_date(), date(1937, 1, 1, 0));
        assert_eq!(time(1936, 4, 30, 23).plus_hours(1).to_date(), date(1936, 5, 1, 0));
    }

    #[test]
    fn negative_years() {
        let t = time(-1, 12, 31, 23);
        assert_eq!(t.0, -1);
        assert_eq!(t.to_date(), date(-1, 12, 31, 23));
        assert_eq!(t.plus_hours(1), GameTime(0));
    }

    #[test]
    fn invalid_dates_are_rejected() {
        assert_eq!(GameTime::from_date(date(1936, 2, 29, 0)), Err(TimeError::Day(date(1936, 2, 29, 0))));
        assert!(matches!(GameTime::from_date(date(1936, 4, 31, 0)), Err(TimeError::Day(_))));
        assert!(matches!(GameTime::from_date(date(1936, 1, 0, 0)), Err(TimeError::Day(_))));
        assert!(matches!(GameTime::from_date(date(1936, 13, 1, 0)), Err(TimeError::Month(_))));
        assert!(matches!(GameTime::from_date(date(1936, 1, 1, 24)), Err(TimeError::Hour(_))));
    }

    #[test]
    fn pulses() {
        assert_eq!(time(1936, 1, 1, 12).pulses(), Pulses::default());
        assert_eq!(time(1936, 1, 2, 0).pulses(), Pulses { day: true, month: false, year: false });
        assert_eq!(time(1936, 2, 1, 0).pulses(), Pulses { day: true, month: true, year: false });
        assert_eq!(time(1937, 1, 1, 0).pulses(), Pulses { day: true, month: true, year: true });
    }

    #[test]
    fn display() {
        assert_eq!(time(1936, 1, 1, 12).to_string(), "1936.1.1.12");
    }
}

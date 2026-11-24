//! Wall-clock time (Bongo-Kart #182, ADR 0045 §1): what
//! [`Platform::unix_seconds`](crate::Platform::unix_seconds) returns and the
//! calendar arithmetic around it, without a C library.
//!
//! [`Platform::now_micros`](crate::Platform::now_micros) is monotonic from an
//! arbitrary start: right for timing, useless for "today's challenge". The
//! wall clock is the opposite: it can jump (the player sets it), so it is for
//! dates, never for timing, and never inside a simulation that a replay or a
//! netplay peer must repeat (read it once, keep the value in the game's state
//! or its recording).
//!
//! Where it comes from, per target:
//!
//! - PC: the system clock (UTC); `TRINO_UNIX_TIME=<seconds>` pins it for
//!   tests and recordings.
//! - Web: `Date.now()` (UTC); `?unix=<seconds>` pins it.
//! - 3DS: the console's clock, which the player sets to their *local* time
//!   and which has no time zone: the value is that local time counted as if
//!   it were UTC, so a day starts at the player's midnight.
//! - N64: the cartridge's Joybus real-time clock when there is one (an
//!   emulator or a flash cart that provides it; the ROM header declares it),
//!   local time like the 3DS's; `None` without one.

/// Seconds in a day.
pub const DAY: u64 = 86_400;

/// A date and time of day in the proleptic Gregorian calendar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Civil {
    pub year: u16,
    /// 1..=12.
    pub month: u8,
    /// 1..=31.
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
    /// 0 = Sunday ..= 6 = Saturday.
    pub weekday: u8,
}

/// Days from 1970-01-01 to `year-month-day` (Howard Hinnant's
/// `days_from_civil`); negative before 1970.
pub const fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since 1970-01-01 00:00 for a calendar time; `None` before 1970 or
/// for a field out of range (month 0, day 32, hour 24, …).
pub const fn unix_from_civil(c: &Civil) -> Option<u64> {
    if c.month < 1 || c.month > 12 || c.day < 1 || c.day > 31 || c.hour > 23 || c.min > 59 {
        return None;
    }
    if c.sec > 60 {
        return None;
    }
    let days = days_from_civil(c.year as i32, c.month as u32, c.day as u32);
    if days < 0 {
        return None;
    }
    Some(days as u64 * DAY + c.hour as u64 * 3600 + c.min as u64 * 60 + c.sec as u64)
}

/// The calendar time of `secs` seconds since 1970 (Hinnant's
/// `civil_from_days`).
pub const fn civil_from_unix(secs: u64) -> Civil {
    let days = (secs / DAY) as i64;
    let rem = secs % DAY;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    Civil {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: (rem / 3600) as u8,
        min: (rem / 60 % 60) as u8,
        sec: (rem % 60) as u8,
        // 1970-01-01 was a Thursday.
        weekday: ((days + 4) % 7) as u8,
    }
}

/// The day number of `secs` (days since 1970-01-01): a daily challenge's
/// seed, the same for every player on the same date.
pub const fn day_index(secs: u64) -> u32 {
    (secs / DAY) as u32
}

/// A pinned clock that keeps running (the hosts' `TRINO_UNIX_TIME` /
/// `?unix=`): the pinned value plus the monotonic time since it was pinned.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PinnedClock {
    base_secs: u64,
    since_us: u64,
}

impl PinnedClock {
    /// The clock reads `secs` at monotonic time `now_us`.
    pub const fn new(secs: u64, now_us: u64) -> PinnedClock {
        PinnedClock {
            base_secs: secs,
            since_us: now_us,
        }
    }

    pub const fn read(&self, now_us: u64) -> u64 {
        self.base_secs + now_us.saturating_sub(self.since_us) / 1_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_and_unix_round_trip_across_leap_years_and_eras() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        let c = civil_from_unix(0);
        assert_eq!((c.year, c.month, c.day, c.weekday), (1970, 1, 1, 4));
        // 2026-11-24 12:34:56 UTC, a Tuesday.
        let t = 1_795_523_696;
        let c = civil_from_unix(t);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.min, c.sec, c.weekday),
            (2026, 11, 24, 12, 34, 56, 2)
        );
        assert_eq!(unix_from_civil(&c), Some(t));
        // Leap days and the century rules.
        for (y, m, d) in [(2000, 2, 29), (2024, 2, 29), (2100, 3, 1), (1999, 12, 31)] {
            let c = Civil {
                year: y,
                month: m,
                day: d,
                ..Civil::default()
            };
            let s = unix_from_civil(&c).unwrap();
            let back = civil_from_unix(s);
            assert_eq!((back.year, back.month, back.day), (y, m, d));
        }
        // Every day for 300 years.
        let mut prev = None;
        for day in 0..(300 * 366) {
            let c = civil_from_unix(day * DAY + 3661);
            assert_eq!(unix_from_civil(&c), Some(day * DAY + 3661));
            if let Some(p) = prev {
                assert_eq!(c.weekday, (p + 1) % 7);
            }
            prev = Some(c.weekday);
        }
    }

    #[test]
    fn bad_fields_and_dates_before_1970_are_refused() {
        let ok = Civil {
            year: 2026,
            month: 1,
            day: 1,
            ..Civil::default()
        };
        assert!(unix_from_civil(&ok).is_some());
        for bad in [
            Civil { month: 0, ..ok },
            Civil { month: 13, ..ok },
            Civil { day: 0, ..ok },
            Civil { day: 32, ..ok },
            Civil { hour: 24, ..ok },
            Civil { min: 60, ..ok },
            Civil { sec: 61, ..ok },
            Civil { year: 1969, ..ok },
        ] {
            assert_eq!(unix_from_civil(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_pinned_clock_runs_and_days_index_from_1970() {
        let c = PinnedClock::new(1_000, 5_000_000);
        assert_eq!(c.read(5_000_000), 1_000);
        assert_eq!(c.read(7_999_999), 1_002);
        assert_eq!(c.read(0), 1_000);
        assert_eq!(day_index(DAY * 3 - 1), 2);
        assert_eq!(day_index(DAY * 3), 3);
    }
}

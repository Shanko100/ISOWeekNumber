//! Civil-date arithmetic: proleptic Gregorian days, ISO-8601 weeks and the
//! clipboard formats. Pure functions, no I/O.

/// A calendar date. Ordering is chronological.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        _ => 28,
    }
}

impl Date {
    /// A valid date, or None.
    pub fn new(year: i32, month: u32, day: u32) -> Option<Date> {
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
            return None;
        }
        if day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    pub fn to_days(self) -> i64 {
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_days(days: i64) -> Date {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Date { year, month, day }
    }

    pub fn add_days(self, days: i64) -> Date {
        Date::from_days(self.to_days() + days)
    }

    /// Monday = 0 ... Sunday = 6, as Python's date.weekday().
    pub fn weekday(self) -> u32 {
        // 1970-01-01 was a Thursday (3).
        (self.to_days() + 3).rem_euclid(7) as u32
    }

    /// (ISO year, ISO week), as Python's date.isocalendar()[:2].
    pub fn iso_week(self) -> (i32, u32) {
        let thursday = self.add_days(3 - i64::from(self.weekday()));
        let jan1 = Date {
            year: thursday.year,
            month: 1,
            day: 1,
        };
        let week = (thursday.to_days() - jan1.to_days()) / 7 + 1;
        (thursday.year, week as u32)
    }

    pub fn first_of_month(self) -> Date {
        Date { day: 1, ..self }
    }

    /// The first of the month `delta` months from this date's month.
    pub fn shift_months(self, delta: i32) -> Date {
        let absolute = self.year * 12 + self.month as i32 - 1 + delta;
        Date {
            year: absolute.div_euclid(12),
            month: absolute.rem_euclid(12) as u32 + 1,
            day: 1,
        }
    }
}

/// `dd-mm-yyyy`, the copied date format.
pub fn format_gui_date(date: Date) -> String {
    format!("{:02}-{:02}-{}", date.day, date.month, date.year)
}

/// `yyyy CWww`, the copied week format; the year is the ISO week-year.
pub fn format_iso_week(iso_year: i32, iso_week: u32) -> String {
    format!("{iso_year} CW{iso_week:02}")
}

pub fn format_calendar_week(date: Date) -> String {
    let (year, week) = date.iso_week();
    format_iso_week(year, week)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip() {
        for days in -800_000..800_000i64 {
            if days % 997 != 0 {
                continue;
            }
            assert_eq!(Date::from_days(days).to_days(), days);
        }
        assert_eq!(Date::new(1970, 1, 1).unwrap().to_days(), 0);
        assert_eq!(Date::new(2000, 3, 1).unwrap().to_days(), 11_017);
    }

    #[test]
    fn iso_weeks() {
        let d = |y, m, dd| Date::new(y, m, dd).unwrap();
        assert_eq!(d(2021, 1, 1).iso_week(), (2020, 53));
        assert_eq!(d(2026, 10, 24).iso_week(), (2026, 43));
        assert_eq!(d(2024, 12, 30).iso_week(), (2025, 1));
        assert_eq!(d(2008, 12, 29).iso_week(), (2009, 1));
        assert_eq!(d(2010, 1, 3).iso_week(), (2009, 53));
        assert_eq!(d(2026, 10, 24).weekday(), 5);
        assert_eq!(format_calendar_week(d(2021, 1, 1)), "2020 CW53");
        assert_eq!(format_gui_date(d(2026, 3, 5)), "05-03-2026");
    }

    #[test]
    fn month_shift() {
        let d = Date::new(2026, 12, 1).unwrap();
        assert_eq!(d.shift_months(1), Date::new(2027, 1, 1).unwrap());
        assert_eq!(d.shift_months(-12), Date::new(2025, 12, 1).unwrap());
    }
}

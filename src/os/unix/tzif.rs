//! Time zones from the system's IANA database (TZif files, RFC 8536): the
//! transition table, and the POSIX TZ rule in the file's footer for the
//! years after it. Files are read whole, bounded in size and validated;
//! anything malformed is rejected rather than guessed at.

use crate::dates::{self, Date};
use std::path::{Path, PathBuf};

/// Largest TZif file accepted (the biggest in tzdata is under 4 KiB).
const MAX_TZIF: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalType {
    utoff: i32,
    dst: bool,
}

/// When a POSIX rule switches: day selector and seconds after local midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Day {
    /// Jn: 1..=365, February 29 never counted.
    Julian1(u32),
    /// n: 0..=365, February 29 counted in leap years.
    Julian0(u32),
    /// Mm.w.d: month, week 1..=5 (5 = last), weekday 0 = Sunday.
    Month(u32, u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rule {
    day: Day,
    secs: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posix {
    std: i32,
    dst: Option<(i32, Rule, Rule)>,
}

#[derive(Clone, Debug)]
pub struct Zone {
    times: Vec<i64>,
    idx: Vec<u8>,
    types: Vec<LocalType>,
    footer: Option<Posix>,
}

/// The directories holding the database, in order of preference.
fn zoneinfo_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("TZDIR").map(PathBuf::from)
        && d.is_absolute()
    {
        dirs.push(d);
    }
    for d in [
        "/usr/share/zoneinfo",
        "/var/db/timezone/zoneinfo",
        "/usr/lib/zoneinfo",
        "/usr/share/lib/zoneinfo",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

/// A zone name like Europe/Amsterdam: letters, digits and - + _ / only,
/// no empty or dot components (so no path escapes).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('/').all(|c| {
            !c.is_empty()
                && !c.starts_with('.')
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-+_".contains(&b))
        })
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut data = Vec::new();
    f.take(MAX_TZIF + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= MAX_TZIF).then_some(data)
}

impl Zone {
    /// The zone called `name` in the system database.
    pub fn load(name: &str) -> Option<Zone> {
        if !valid_name(name) {
            return None;
        }
        zoneinfo_dirs()
            .iter()
            .find_map(|d| read_bounded(&d.join(name)).and_then(|b| Zone::parse(&b)))
    }

    /// The machine's own zone and its IANA name when known: $TZ, then
    /// /etc/localtime (whose link target names the zone).
    pub fn local() -> Option<(Option<String>, Zone)> {
        if let Ok(tz) = std::env::var("TZ") {
            let name = tz.strip_prefix(':').unwrap_or(&tz);
            if let Some(z) = Zone::load(name) {
                return Some((Some(name.to_string()), z));
            }
            if let Some(p) = Posix::parse(name) {
                return Some((
                    None,
                    Zone {
                        times: Vec::new(),
                        idx: Vec::new(),
                        types: Vec::new(),
                        footer: Some(p),
                    },
                ));
            }
        }
        let link = Path::new("/etc/localtime");
        let name = std::fs::read_link(link)
            .ok()
            .and_then(|t| zone_name_from_path(&t))
            .or_else(|| {
                std::fs::read_to_string("/etc/timezone")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| valid_name(s))
            });
        let zone = read_bounded(link)
            .and_then(|b| Zone::parse(&b))
            .or_else(|| name.as_deref().and_then(Zone::load))?;
        Some((name, zone))
    }

    pub fn parse(b: &[u8]) -> Option<Zone> {
        let (v1, version) = header(b)?;
        let v1_len = v1.data_len(4)?;
        if version >= b'2' {
            let rest = b.get(44 + v1_len..)?;
            let (h2, _) = header(rest)?;
            let body = rest.get(44..44 + h2.data_len(8)?)?;
            let mut zone = h2.parse_body(body, 8)?;
            let footer = rest.get(44 + h2.data_len(8)?..)?;
            if footer.first() == Some(&b'\n') {
                let end = footer[1..].iter().position(|&c| c == b'\n')?;
                let text = std::str::from_utf8(&footer[1..1 + end]).ok()?;
                zone.footer = if text.is_empty() {
                    None
                } else {
                    Some(Posix::parse(text)?)
                };
            }
            Some(zone)
        } else {
            v1.parse_body(b.get(44..44 + v1_len)?, 4)
        }
    }

    /// UTC offset in seconds at `t` (seconds since the epoch, UTC).
    pub fn offset_at(&self, t: i64) -> i32 {
        if let Some(p) = &self.footer
            && self.times.last().is_none_or(|&last| t > last)
        {
            return p.offset_at(t);
        }
        match self.times.partition_point(|&x| x <= t) {
            0 => self.first_type().utoff,
            i => self.types[usize::from(self.idx[i - 1])].utoff,
        }
    }

    /// The zone's standard (non-daylight) offset today, in minutes.
    pub fn standard_offset_minutes(&self) -> i32 {
        if let Some(p) = &self.footer {
            return p.std / 60;
        }
        self.idx
            .iter()
            .rev()
            .map(|&i| self.types[usize::from(i)])
            .find(|t| !t.dst)
            .unwrap_or_else(|| self.first_type())
            .utoff
            / 60
    }

    fn first_type(&self) -> LocalType {
        self.types
            .iter()
            .copied()
            .find(|t| !t.dst)
            .or_else(|| self.types.first().copied())
            .unwrap_or(LocalType {
                utoff: 0,
                dst: false,
            })
    }

    /// Whether two zones keep the same clock: the same offset at a set of
    /// instants spread over two years around `t`.
    pub fn same_clock(&self, other: &Zone, t: i64) -> bool {
        (0..48).all(|k| {
            let at = t + (k - 12) * 15 * 86_400 + 3_600;
            self.offset_at(at) == other.offset_at(at)
        })
    }
}

/// "…/zoneinfo/Europe/Amsterdam" -> "Europe/Amsterdam".
pub fn zone_name_from_path(p: &Path) -> Option<String> {
    let s = p.to_string_lossy();
    let at = s.find("zoneinfo/")?;
    let name = &s[at + "zoneinfo/".len()..];
    let name = name
        .strip_prefix("posix/")
        .or_else(|| name.strip_prefix("right/"))
        .unwrap_or(name);
    valid_name(name).then(|| name.to_string())
}

struct Header {
    isut: usize,
    isstd: usize,
    leap: usize,
    time: usize,
    typ: usize,
    chars: usize,
}

fn header(b: &[u8]) -> Option<(Header, u8)> {
    if b.get(..4)? != b"TZif" {
        return None;
    }
    let version = *b.get(4)?;
    let n = |i: usize| -> Option<usize> {
        Some(u32::from_be_bytes(b.get(20 + 4 * i..24 + 4 * i)?.try_into().ok()?) as usize)
    };
    let h = Header {
        isut: n(0)?,
        isstd: n(1)?,
        leap: n(2)?,
        time: n(3)?,
        typ: n(4)?,
        chars: n(5)?,
    };
    if h.typ == 0 || h.typ > 256 || h.time > 10_000 || h.chars > 1024 || h.leap > 1000 {
        return None;
    }
    if (h.isut != 0 && h.isut != h.typ) || (h.isstd != 0 && h.isstd != h.typ) {
        return None;
    }
    Some((h, version))
}

impl Header {
    fn data_len(&self, tsize: usize) -> Option<usize> {
        Some(
            self.time * tsize
                + self.time
                + self.typ * 6
                + self.chars
                + self.leap * (tsize + 4)
                + self.isstd
                + self.isut,
        )
    }

    fn parse_body(&self, b: &[u8], tsize: usize) -> Option<Zone> {
        let mut times = Vec::with_capacity(self.time);
        for i in 0..self.time {
            let c = b.get(i * tsize..(i + 1) * tsize)?;
            let t = if tsize == 8 {
                i64::from_be_bytes(c.try_into().ok()?)
            } else {
                i64::from(i32::from_be_bytes(c.try_into().ok()?))
            };
            if times.last().is_some_and(|&p| t <= p) {
                return None;
            }
            times.push(t);
        }
        let at = self.time * tsize;
        let idx = b.get(at..at + self.time)?.to_vec();
        if idx.iter().any(|&i| usize::from(i) >= self.typ) {
            return None;
        }
        let at = at + self.time;
        let mut types = Vec::with_capacity(self.typ);
        for i in 0..self.typ {
            let e = b.get(at + 6 * i..at + 6 * i + 6)?;
            let utoff = i32::from_be_bytes(e[..4].try_into().ok()?);
            if !(-89_999..=93_599).contains(&utoff) {
                return None;
            }
            types.push(LocalType {
                utoff,
                dst: e[4] != 0,
            });
        }
        Some(Zone {
            times,
            idx,
            types,
            footer: None,
        })
    }
}

impl Posix {
    /// A POSIX TZ string such as "CET-1CEST,M3.5.0,M10.5.0/3".
    pub fn parse(s: &str) -> Option<Posix> {
        let mut p = Parser {
            b: s.as_bytes(),
            i: 0,
        };
        p.name()?;
        let std = -p.offset()?;
        if p.done() {
            return Some(Posix { std, dst: None });
        }
        p.name()?;
        let dst = if p.peek() != Some(b',') && !p.done() {
            -p.offset()?
        } else {
            std + 3600
        };
        let (start, end) = if p.done() {
            // No rule: the US rules POSIX implementations default to.
            (
                Rule {
                    day: Day::Month(3, 2, 0),
                    secs: 7200,
                },
                Rule {
                    day: Day::Month(11, 1, 0),
                    secs: 7200,
                },
            )
        } else {
            p.expect(b',')?;
            let a = p.rule()?;
            p.expect(b',')?;
            let b = p.rule()?;
            (a, b)
        };
        p.done().then_some(Posix {
            std,
            dst: Some((dst, start, end)),
        })
    }

    pub fn offset_at(&self, t: i64) -> i32 {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let year = Date::from_days((t + i64::from(self.std)).div_euclid(86_400)).year;
        // Check the neighbouring years too: a southern-hemisphere summer
        // spans New Year.
        for y in [year - 1, year, year + 1] {
            let s = start.utc(y, self.std);
            let e = end.utc(y, dst);
            if s < e {
                if (s..e).contains(&t) {
                    return dst;
                }
            } else {
                let next_e = end.utc(y + 1, dst);
                if (s..next_e).contains(&t) {
                    return dst;
                }
            }
        }
        self.std
    }
}

impl Rule {
    /// The switch in `year` as UTC seconds, given the offset in force before it.
    fn utc(self, year: i32, before: i32) -> i64 {
        let jan1 = Date {
            year,
            month: 1,
            day: 1,
        }
        .to_days();
        let day = match self.day {
            Day::Julian1(n) => {
                let leap_skip = i64::from(dates::is_leap(year) && n >= 60);
                jan1 + i64::from(n) - 1 + leap_skip
            }
            Day::Julian0(n) => jan1 + i64::from(n),
            Day::Month(m, w, d) => {
                let first = Date {
                    year,
                    month: m,
                    day: 1,
                };
                // Python-style weekday: Monday 0 .. Sunday 6; POSIX: Sunday 0.
                let first_wd = (first.weekday() + 1) % 7;
                let mut dom = 1 + (d + 7 - first_wd) % 7 + 7 * (w - 1);
                let last = dates::days_in_month(year, m);
                while dom > last {
                    dom -= 7;
                }
                first.to_days() + i64::from(dom) - 1
            }
        };
        day * 86_400 + i64::from(self.secs) - i64::from(before)
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn done(&self) -> bool {
        self.i >= self.b.len()
    }
    fn expect(&mut self, c: u8) -> Option<()> {
        (self.peek()? == c).then(|| self.i += 1)
    }
    fn name(&mut self) -> Option<()> {
        let start = self.i;
        if self.peek() == Some(b'<') {
            self.i += 1;
            while self.peek()? != b'>' {
                self.i += 1;
            }
            self.i += 1;
            return (self.i - start >= 5).then_some(());
        }
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - start >= 3).then_some(())
    }
    fn number(&mut self) -> Option<i32> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start || self.i - start > 3 {
            return None;
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
    /// [+-]hh[:mm[:ss]] in seconds (hours up to 167 for rule times).
    fn hms(&mut self) -> Option<i32> {
        let sign = match self.peek() {
            Some(b'-') => {
                self.i += 1;
                -1
            }
            Some(b'+') => {
                self.i += 1;
                1
            }
            _ => 1,
        };
        let h = self.number()?;
        let mut secs = h * 3600;
        for unit in [60, 1] {
            if self.peek() == Some(b':') {
                self.i += 1;
                let v = self.number()?;
                if v > 59 {
                    return None;
                }
                secs += v * unit;
            } else {
                break;
            }
        }
        (h <= 167).then_some(sign * secs)
    }
    fn offset(&mut self) -> Option<i32> {
        let v = self.hms()?;
        (v.abs() <= 24 * 3600 + 59 * 60).then_some(v)
    }
    fn rule(&mut self) -> Option<Rule> {
        let day = match self.peek()? {
            b'J' => {
                self.i += 1;
                let n = self.number()? as u32;
                (1..=365).contains(&n).then_some(Day::Julian1(n))?
            }
            b'M' => {
                self.i += 1;
                let m = self.number()? as u32;
                self.expect(b'.')?;
                let w = self.number()? as u32;
                self.expect(b'.')?;
                let d = self.number()? as u32;
                ((1..=12).contains(&m) && (1..=5).contains(&w) && d <= 6)
                    .then_some(Day::Month(m, w, d))?
            }
            _ => {
                let n = self.number()? as u32;
                (n <= 365).then_some(Day::Julian0(n))?
            }
        };
        let secs = if self.peek() == Some(b'/') {
            self.i += 1;
            self.hms()?
        } else {
            7200
        };
        Some(Rule { day, secs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: i64) -> i64 {
        Date::new(y, m, d).unwrap().to_days() * 86_400 + h * 3600
    }

    #[test]
    fn posix_rules() {
        let cet = Posix::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(cet.offset_at(utc(2026, 1, 15, 12)), 3600);
        assert_eq!(cet.offset_at(utc(2026, 7, 1, 12)), 7200);
        // 2026-03-29 01:00 UTC is the switch.
        assert_eq!(cet.offset_at(utc(2026, 3, 29, 1) - 1), 3600);
        assert_eq!(cet.offset_at(utc(2026, 3, 29, 1)), 7200);
        // 2026-10-25 01:00 UTC switches back.
        assert_eq!(cet.offset_at(utc(2026, 10, 25, 1) - 1), 7200);
        assert_eq!(cet.offset_at(utc(2026, 10, 25, 1)), 3600);
        let aus = Posix::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(aus.offset_at(utc(2026, 1, 10, 0)), 11 * 3600);
        assert_eq!(aus.offset_at(utc(2026, 6, 10, 0)), 10 * 3600);
        assert_eq!(aus.offset_at(utc(2026, 12, 10, 0)), 11 * 3600);
        let india = Posix::parse("<+0530>-5:30").unwrap();
        assert_eq!(india.offset_at(0), 19_800);
        let ny = Posix::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(ny.offset_at(utc(2026, 3, 8, 7)), -4 * 3600);
        assert_eq!(ny.offset_at(utc(2026, 3, 8, 7) - 1), -5 * 3600);
        assert_eq!(Posix::parse("<-03>3").unwrap().offset_at(0), -3 * 3600);
        assert!(Posix::parse("X").is_none());
        assert!(Posix::parse("CET-1CEST,M13.5.0,M10.5.0").is_none());
        assert!(Posix::parse("CET-1CEST,M3.5.0").is_none());
    }

    /// A hand-built v2 file: one transition, two types, a footer.
    fn sample() -> Vec<u8> {
        let mut f = Vec::new();
        let head = |f: &mut Vec<u8>, time: u32, typ: u32, chars: u32| {
            f.extend_from_slice(b"TZif2");
            f.extend_from_slice(&[0; 15]);
            for n in [0, 0, 0, time, typ, chars] {
                f.extend_from_slice(&u32::to_be_bytes(n));
            }
        };
        head(&mut f, 0, 1, 4);
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        f.extend_from_slice(b"UTC\0");
        head(&mut f, 1, 2, 8);
        f.extend_from_slice(&i64::to_be_bytes(1_000_000));
        f.push(1);
        f.extend_from_slice(&i32::to_be_bytes(3600));
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&i32::to_be_bytes(7200));
        f.extend_from_slice(&[1, 4]);
        f.extend_from_slice(b"CET\0CEST");
        f.extend_from_slice(b"\nCET-1CEST,M3.5.0,M10.5.0/3\n");
        f
    }

    #[test]
    fn tzif_v2() {
        let z = Zone::parse(&sample()).unwrap();
        assert_eq!(z.offset_at(0), 3600);
        assert_eq!(z.offset_at(1_000_000), 7200);
        assert_eq!(z.offset_at(1_000_001), 3600);
        assert_eq!(z.offset_at(utc(2026, 1, 1, 0)), 3600);
        assert_eq!(z.offset_at(utc(2026, 7, 1, 0)), 7200);
        assert_eq!(z.standard_offset_minutes(), 60);
        let mut bad = sample();
        bad[0] = b'X';
        assert!(Zone::parse(&bad).is_none());
        assert!(Zone::parse(&sample()[..60]).is_none());
    }

    #[test]
    fn names() {
        assert!(valid_name("Europe/Amsterdam"));
        assert!(valid_name("Etc/GMT+5"));
        assert!(!valid_name("../etc/passwd"));
        assert!(!valid_name("Europe//X"));
        assert!(!valid_name(""));
        assert_eq!(
            zone_name_from_path(Path::new("/usr/share/zoneinfo/America/New_York")).as_deref(),
            Some("America/New_York")
        );
        assert_eq!(
            zone_name_from_path(Path::new("../usr/share/zoneinfo/posix/Asia/Tokyo")).as_deref(),
            Some("Asia/Tokyo")
        );
    }

    /// The system database, where one is installed (CI runners have it).
    #[test]
    fn system_database() {
        let Some(z) = Zone::load("Europe/Amsterdam") else {
            return;
        };
        assert_eq!(z.offset_at(utc(2026, 1, 15, 12)), 3600);
        assert_eq!(z.offset_at(utc(2026, 7, 15, 12)), 7200);
        assert_eq!(z.standard_offset_minutes(), 60);
        let berlin = Zone::load("Europe/Berlin").unwrap();
        assert!(z.same_clock(&berlin, utc(2026, 6, 1, 0)));
        let london = Zone::load("Europe/London").unwrap();
        assert!(!z.same_clock(&london, utc(2026, 6, 1, 0)));
        let kolkata = Zone::load("Asia/Kolkata").unwrap();
        assert_eq!(kolkata.offset_at(utc(2026, 6, 1, 0)), 19_800);
    }
}

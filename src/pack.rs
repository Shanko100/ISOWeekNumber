//! Readers for the binary tables build.rs embeds: varint cursor, inflate,
//! and the holiday and time-zone decoders. Every read is bounds-checked and
//! every count capped; a damaged table yields None, never a panic.

/// Embedded at build time from data/holiday_data.jsonl.
pub static HOLIDAYS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/holidays.bin"));
/// Embedded at build time from data/timezone_data.json.
pub static TIMEZONES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/timezones.bin"));

pub struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Cursor { b, i: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    pub fn uvar(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    /// A count, refused above `max`.
    pub fn count(&mut self, max: usize) -> Option<usize> {
        let v = self.uvar()?;
        (v <= max as u64).then_some(v as usize)
    }
    pub fn svar(&mut self) -> Option<i64> {
        let v = self.uvar()?;
        Some((v >> 1) as i64 ^ -((v & 1) as i64))
    }
    pub fn string(&mut self, max_bytes: usize) -> Option<&'a str> {
        let n = self.count(max_bytes)?;
        std::str::from_utf8(self.bytes(n)?).ok()
    }
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.i..]
    }
}

/// Inflate raw deflate data whose inflated size is known; refuses anything
/// that inflates to a different size or past `max`.
pub fn inflate(data: &[u8], raw_len: usize, max: usize) -> Option<Vec<u8>> {
    if raw_len > max {
        return None;
    }
    let out = miniz_oxide::inflate::decompress_to_vec_with_limit(data, raw_len).ok()?;
    (out.len() == raw_len).then_some(out)
}

// ---------------------------------------------------------------------------
// Holidays
// ---------------------------------------------------------------------------

pub const MIN_HOLIDAY_YEAR: i32 = 2000;
pub const MAX_HOLIDAY_YEAR: i32 = 2050;
const MAX_COUNTRIES: usize = 300;
const MAX_TABLE_BYTES: usize = 512 * 1024;

pub struct CountryEntry {
    pub code: [u8; 2],
    pub name: &'static str,
    offset: usize,
    len: usize,
    raw_len: usize,
}

impl CountryEntry {
    pub fn code_str(&self) -> &str {
        std::str::from_utf8(&self.code).unwrap_or("")
    }
}

/// The country index (code, English name), in code order. Borrowed from the
/// embedded table: nothing is allocated but the list itself.
pub fn holiday_index() -> Option<(Vec<CountryEntry>, &'static [u8])> {
    let mut c = Cursor::new(HOLIDAYS);
    if c.bytes(4)? != b"WNH1" {
        return None;
    }
    let n = c.count(MAX_COUNTRIES)?;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        let code: [u8; 2] = c.bytes(2)?.try_into().ok()?;
        if !code.iter().all(u8::is_ascii_uppercase) {
            return None;
        }
        let name = c.string(320)?;
        let offset = c.count(HOLIDAYS.len())?;
        let len = c.count(HOLIDAYS.len())?;
        let raw_len = c.count(MAX_TABLE_BYTES)?;
        list.push(CountryEntry {
            code,
            name,
            offset,
            len,
            raw_len,
        });
    }
    Some((list, c.rest()))
}

/// One year of one country: (day of year, holiday names).
pub type YearTable = Vec<(u16, Vec<String>)>;

/// The years `keep` of one country, decoded from its compressed table.
pub fn holiday_years(
    entry: &CountryEntry,
    chunks: &[u8],
    keep: &[i32],
) -> Option<Vec<(i32, YearTable)>> {
    let data = chunks.get(entry.offset..entry.offset.checked_add(entry.len)?)?;
    let raw = inflate(data, entry.raw_len, MAX_TABLE_BYTES)?;
    let mut c = Cursor::new(&raw);
    let name_count = c.count(4096)?;
    let mut names = Vec::with_capacity(name_count);
    for _ in 0..name_count {
        let n = c.string(240 * 4)?;
        if n.is_empty() || n.chars().count() > 240 {
            return None;
        }
        names.push(n);
    }
    let mut out = Vec::new();
    for year in MIN_HOLIDAY_YEAR..=MAX_HOLIDAY_YEAR {
        let days = c.count(366)?;
        let wanted = keep.contains(&year);
        let mut table = Vec::new();
        let mut doy = 0u64;
        for _ in 0..days {
            doy += c.uvar()?;
            if doy > 365 {
                return None;
            }
            let k = c.count(16)?;
            if k == 0 {
                return None;
            }
            let mut list = Vec::new();
            for _ in 0..k {
                let idx = c.count(name_count.checked_sub(1)?)?;
                if wanted {
                    list.push(names[idx].to_string());
                }
            }
            if wanted {
                table.push((doy as u16, list));
            }
        }
        if wanted {
            out.push((year, table));
        }
    }
    c.rest().is_empty().then_some(out)
}

// ---------------------------------------------------------------------------
// Time zones
// ---------------------------------------------------------------------------

const MAX_ZONES: usize = 400;
const MAX_BANDS: usize = 200;
const MAX_RINGS: usize = 8000;
const MAX_RING_POINTS: usize = 4000;
const MAX_TOTAL_POINTS: usize = 400_000;
const MAX_TZ_PART_BYTES: usize = 4 * 1024 * 1024;

pub struct ZoneRow {
    pub key: String,
    pub label: String,
    pub city: String,
    /// The IANA zone with the same clock (the macOS and Linux editions
    /// convert with it).
    #[cfg_attr(windows, allow(dead_code))]
    pub iana: String,
    pub lat: f64,
    pub lon: f64,
    /// Index into the map's bands, or None.
    pub band: Option<usize>,
}

/// Flat (lon, lat) points in degrees.
pub type Ring = Vec<(f32, f32)>;

pub struct Band {
    pub utc: f64,
    pub sea: Vec<Ring>,
    pub land: Vec<Ring>,
    pub tint: usize,
}

pub struct Map {
    pub bands: Vec<Band>,
    pub borders: Vec<Ring>,
}

fn tz_parts() -> Option<[Vec<u8>; 2]> {
    let mut c = Cursor::new(TIMEZONES);
    if c.bytes(4)? != b"WNT1" {
        return None;
    }
    let mut part = || -> Option<Vec<u8>> {
        let len = c.count(MAX_TZ_PART_BYTES)?;
        let raw_len = c.count(MAX_TZ_PART_BYTES)?;
        inflate(c.bytes(len)?, raw_len, MAX_TZ_PART_BYTES)
    };
    let zones = part()?;
    let map = part()?;
    Some([zones, map])
}

/// The listed zones, unfiltered and unsorted (tz.rs keeps the ones Windows
/// knows and sorts them).
pub fn zone_rows() -> Option<Vec<ZoneRow>> {
    let [raw, _] = tz_parts()?;
    let mut c = Cursor::new(&raw);
    let n = c.count(MAX_ZONES)?;
    let mut rows = Vec::with_capacity(n);
    for _ in 0..n {
        let key: String = c.string(127 * 4)?.chars().take(127).collect();
        let label: String = c.string(1024)?.chars().take(80).collect();
        let city: String = c.string(1024)?.chars().take(40).collect();
        let iana: String = c.string(64)?.to_string();
        let lat = c.svar()? as f64 / 100.0;
        let lon = c.svar()? as f64 / 100.0;
        let band = c.count(MAX_BANDS)?.checked_sub(1);
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) || key.is_empty() {
            continue;
        }
        rows.push(ZoneRow {
            key,
            label,
            city,
            iana,
            lat,
            lon,
            band,
        });
    }
    Some(rows)
}

fn rings(c: &mut Cursor<'_>, budget: &mut usize) -> Option<Vec<Ring>> {
    let n = c.count(MAX_RINGS)?;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        let points = c.count(MAX_RING_POINTS)?;
        *budget = budget.checked_sub(points)?;
        let (mut x, mut y) = (0i64, 0i64);
        let mut ring = Vec::with_capacity(points);
        for _ in 0..points {
            x += c.svar()?;
            y += c.svar()?;
            if x.abs() > 18000 || y.abs() > 9000 {
                return None;
            }
            ring.push((x as f32 / 100.0, y as f32 / 100.0));
        }
        list.push(ring);
    }
    Some(list)
}

/// Bands (west to east) and border lines.
pub fn map() -> Option<Map> {
    let [_, raw] = tz_parts()?;
    let mut c = Cursor::new(&raw);
    let mut budget = MAX_TOTAL_POINTS;
    let n = c.count(MAX_BANDS)?;
    let mut bands = Vec::with_capacity(n);
    for tint in 0..n {
        let utc = (c.svar()? as f64 / 100.0).clamp(-24.0, 24.0);
        let sea = rings(&mut c, &mut budget)?;
        let land = rings(&mut c, &mut budget)?;
        bands.push(Band {
            utc,
            sea,
            land,
            tint: tint % 5,
        });
    }
    let borders = rings(&mut c, &mut budget)?;
    Some(Map { bands, borders })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tables_decode() {
        let (list, chunks) = holiday_index().expect("index");
        assert!(list.len() >= 200);
        let nl = list.iter().find(|c| &c.code == b"NL").expect("NL");
        assert_eq!(nl.name, "Netherlands");
        let years = holiday_years(nl, chunks, &[2026]).expect("NL years");
        let (_, table) = &years[0];
        // King's Day 2026-04-27 is day 116 (0-based).
        assert!(
            table
                .iter()
                .any(|(d, n)| *d == 116 && n.iter().any(|s| s.contains("King")))
        );
        for c in &list {
            assert!(
                holiday_years(c, chunks, &[2000, 2050]).is_some(),
                "{}",
                c.code_str()
            );
        }
        let zones = zone_rows().expect("zones");
        assert_eq!(zones.len(), 63);
        let m = map().expect("map");
        assert_eq!(m.bands.len(), 40);
        assert!(
            zones
                .iter()
                .all(|z| z.band.is_none_or(|b| b < m.bands.len()))
        );
    }
}

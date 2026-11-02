//! Time zones: Windows owns the rules (EnumDynamicTimeZoneInformation and
//! SystemTimeToTzSpecificLocalTimeEx, i.e. dynamic DST); the embedded table
//! supplies the dropdown rows and the map. Nothing here is kept between
//! window openings.

use crate::dates::Date;
use crate::os::{self, TzInfo, WallTime};
use crate::pack::{self, ZoneRow};

/// Every zone this system can convert with.
pub fn system_zones() -> Vec<TzInfo> {
    let mut zones = os::enum_time_zones();
    if zones.is_empty() {
        zones.extend(os::current_time_zone());
    }
    zones
}

pub fn find<'a>(zones: &'a [TzInfo], key: &str) -> Option<&'a TzInfo> {
    zones.iter().find(|z| z.key() == key)
}

/// The machine's own zone, if this system can convert with it.
pub fn system_zone_key(zones: &[TzInfo]) -> Option<String> {
    let key = os::current_time_zone()?.key();
    find(zones, &key).map(|_| key)
}

/// The zone to display: the saved one when Windows knows it, else the
/// machine's own.
pub fn resolve_zone_key(saved: Option<&str>, zones: &[TzInfo]) -> Option<String> {
    match saved {
        Some(k) if find(zones, k).is_some() => Some(k.to_string()),
        _ => system_zone_key(zones),
    }
}

fn minutes(t: WallTime) -> i64 {
    let days = Date::new(t.year, t.month, t.day).map_or(0, Date::to_days);
    days * 1440 + i64::from(t.hour) * 60 + i64::from(t.minute)
}

/// Wall-clock time and UTC offset (minutes) from one UTC sample.
pub fn now_and_offset(zone: &TzInfo) -> Option<(WallTime, i32)> {
    let mut utc = os::utc_now();
    utc.millis = 0;
    let local = os::utc_to_zone(zone, utc)?;
    let diff_seconds =
        (minutes(local) - minutes(utc)) * 60 + i64::from(local.second) - i64::from(utc.second);
    Some((local, (diff_seconds as f64 / 60.0).round() as i32))
}

/// Full form for the dropdown: UTC+01:00 (None: UTC).
pub fn format_offset(minutes: Option<i32>) -> String {
    let Some(m) = minutes else {
        return "UTC".into();
    };
    let sign = if m < 0 { '-' } else { '+' };
    let a = m.abs();
    format!("UTC{sign}{:02}:{:02}", a / 60, a % 60)
}

/// Compact form for the calendar footer: UTC+2, UTC+5:30, UTC.
pub fn format_offset_short(minutes: i32) -> String {
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let (h, r) = (minutes.abs() / 60, minutes.abs() % 60);
    if r == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{r:02}")
    }
}

/// The listed zones Windows knows, sorted by standard offset, then label.
pub fn listed_zones(zones: &[TzInfo]) -> Vec<ZoneRow> {
    let mut rows: Vec<(i32, ZoneRow)> = Vec::new();
    for row in pack::zone_rows().unwrap_or_default() {
        if rows.iter().any(|(_, r)| r.key == row.key) {
            continue;
        }
        if let Some(info) = find(zones, &row.key) {
            rows.push((info.standard_offset(), row));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    rows.into_iter().map(|(_, r)| r).collect()
}

// ---------------------------------------------------------------------------
// Map projection: plate carree with the polar caps cropped
// ---------------------------------------------------------------------------

pub const LAT_MAX: f64 = 84.0;
pub const LAT_MIN: f64 = -58.0;
const LAT_MID: f64 = (LAT_MAX + LAT_MIN) / 2.0;
const LAT_SPAN: f64 = (LAT_MAX - LAT_MIN) / 2.0;
/// Width / height of the full map.
pub const ASPECT: f64 = 360.0 / (LAT_MAX - LAT_MIN);

/// (lon, lat) -> (x, y) in -1..1, y north.
pub fn project(lon: f64, lat: f64) -> (f64, f64) {
    (
        (lon / 180.0).clamp(-1.0, 1.0),
        ((lat - LAT_MID) / LAT_SPAN).clamp(-1.0, 1.0),
    )
}

pub fn unproject(nx: f64, ny: f64) -> Option<(f64, f64)> {
    (nx.abs() <= 1.0 + 1e-9 && ny.abs() <= 1.0 + 1e-9)
        .then_some((nx * 180.0, LAT_MID + ny * LAT_SPAN))
}

fn great_circle_km(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = p2 - p1;
    let dlam = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlam / 2.0).sin().powi(2);
    12742.0 * a.sqrt().min(1.0).asin()
}

fn point_in_ring(lon: f64, lat: f64, ring: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (x1, y1) = (f64::from(ring[i].0), f64::from(ring[i].1));
        let (x2, y2) = (
            f64::from(ring[(i + 1) % n].0),
            f64::from(ring[(i + 1) % n].1),
        );
        if (y1 > lat) != (y2 > lat) {
            let crossing = x1 + (lat - y1) * (x2 - x1) / (y2 - y1);
            if lon < crossing {
                inside = !inside;
            }
        }
    }
    inside
}

/// Index of the band containing a point (its sea rings cover the land).
pub fn band_at(map: &pack::Map, lon: f64, lat: f64) -> Option<usize> {
    map.bands
        .iter()
        .position(|b| b.sea.iter().any(|r| point_in_ring(lon, lat, r)))
}

/// The closest listed city inside the clicked band, else the closest one.
pub fn nearest_zone<'a>(
    rows: &'a [ZoneRow],
    map: &pack::Map,
    lon: f64,
    lat: f64,
) -> Option<&'a ZoneRow> {
    let band = band_at(map, lon, lat);
    let dist = |z: &&ZoneRow| great_circle_km(lon, lat, z.lon, z.lat);
    let in_band: Vec<&ZoneRow> = rows
        .iter()
        .filter(|z| band.is_some() && z.band == band)
        .collect();
    let pool: Vec<&ZoneRow> = if in_band.is_empty() {
        rows.iter().collect()
    } else {
        in_band
    };
    pool.into_iter().min_by(|a, b| dist(a).total_cmp(&dist(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets() {
        assert_eq!(format_offset(Some(60)), "UTC+01:00");
        assert_eq!(format_offset(Some(-570)), "UTC-09:30");
        assert_eq!(format_offset_short(330), "UTC+5:30");
        assert_eq!(format_offset_short(-120), "UTC-2");
        assert_eq!(format_offset_short(0), "UTC");
        let (x, y) = project(-180.0, LAT_MAX);
        assert_eq!((x, y), (-1.0, 1.0));
        let (lon, lat) = unproject(0.5, 0.0).unwrap();
        assert!((lon - 90.0).abs() < 1e-9 && (lat - 13.0).abs() < 1e-9);
    }
}

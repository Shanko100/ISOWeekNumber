//! Build step, run by cargo before compiling the app:
//!
//! 1. Converts the bundled holiday and time-zone data in assets/ into
//!    compact binary tables, each part
//!    deflate-compressed, which the app embeds with include_bytes!. Only the
//!    parts a window needs are inflated, and only while it is open.
//! 2. Composes the Legal notices text (legal/, the repository's LICENSE,
//!    the license texts in legal/licenses/ and the exact versions built
//!    from), deflated the same way, for About -> Legal notices.
//! 3. Writes the Windows resources -- manifest, icon, version information --
//!    as a .res file and hands it to the MSVC linker. No resource compiler,
//!    no extra build crates.
//!
//! The inputs are part of this repository; the build fails on anything that
//! does not match the expected schema, so the app's decoder can stay small.

#[path = "src/json.rs"]
#[allow(dead_code)]
mod json;

use json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;

const APP_VERSION: (u16, u16) = (1, 0);
const APP_EDITION: &str = "Workplace Edition";
const APP_COPYRIGHT: &str = "Shanko100";

const HOLIDAY_DATA: &str = "assets/holiday_data.jsonl";
const TIMEZONE_DATA: &str = "assets/timezone_data.json";
const APP_ICON: &str = "assets/app.ico";
/// The Legal notices window's sources (see legal_text()).
const LEGAL_NOTICES: &str = "legal/LEGAL_NOTICES.md";
const THIRD_PARTY_NOTICES: &str = "legal/THIRD_PARTY_NOTICES.md";
const LICENSES_DIR: &str = "legal/licenses";
const REPO_LICENSE: &str = "LICENSE";
/// The app refuses a legal text larger than this (src/legal.rs).
const MAX_LEGAL_BYTES: usize = 1024 * 1024;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    for f in [
        HOLIDAY_DATA,
        TIMEZONE_DATA,
        APP_ICON,
        "app.manifest",
        "src/json.rs",
        LEGAL_NOTICES,
        THIRD_PARTY_NOTICES,
        LICENSES_DIR,
        REPO_LICENSE,
        "rust-toolchain.toml",
        "Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }
    let legal = legal_text();
    let mut packed = b"WNL1".to_vec();
    uvar(&mut packed, legal.len() as u64);
    packed.extend_from_slice(&deflate(legal.as_bytes()));
    fs::write(out.join("legal.bin"), packed).expect("write legal.bin");
    // The same text as a plain file, for the Linux packages' documentation
    // (packaging/linux/make-packages.sh); it is not part of the program.
    fs::write(out.join("legal.txt"), &legal).expect("write legal.txt");
    // "Holidays last updated on ...", from the data's own metadata.
    println!("cargo:rustc-env=WN_HOLIDAYS_UPDATED={}", holidays_updated());
    fs::write(out.join("holidays.bin"), pack_holidays()).expect("write holidays.bin");
    fs::write(out.join("timezones.bin"), pack_timezones()).expect("write timezones.bin");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let res = out.join("weeknumber.res");
        fs::write(&res, resources()).expect("write weeknumber.res");
        if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // link.exe takes compiled resource files as ordinary inputs.
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Binary writer helpers (varints; the app's src/pack.rs reads them)
// ---------------------------------------------------------------------------

fn uvar(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn svar(out: &mut Vec<u8>, v: i64) {
    uvar(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn string(out: &mut Vec<u8>, s: &str) {
    uvar(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn deflate(raw: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(raw, 10)
}

/// A coordinate in hundredths of a degree; the data carry two decimals.
fn centi(v: f64) -> i64 {
    let c = (v * 100.0).round();
    assert!(
        (c - v * 100.0).abs() < 1e-6,
        "{v} has more than two decimals"
    );
    c as i64
}

// ---------------------------------------------------------------------------
// Holidays
// ---------------------------------------------------------------------------

/// Layout (all integers LEB128 varints):
///   "WNH1", country count, then per country (sorted by code):
///     code (2 ASCII bytes), name, chunk offset, chunk length, inflated length
///   then the chunks. A chunk inflates to:
///     name count, names; then for each year 2000..=2050: date count, and per
///     date: day-of-year delta from the previous date (first: from 0), name
///     count, name indices.
fn pack_holidays() -> Vec<u8> {
    let text = fs::read_to_string(HOLIDAY_DATA).expect("read holiday data");
    let mut lines = text.split('\n');
    let index = json::parse(lines.next().expect("index line")).expect("index JSON");
    let countries = index
        .get("COUNTRIES")
        .and_then(Value::as_array)
        .expect("COUNTRIES");
    let mut names_by_code = std::collections::BTreeMap::new();
    for c in countries {
        let row = c.as_array().expect("country row");
        let code = row[0].as_str().expect("code").to_string();
        let name = row[1].as_str().expect("name").trim().to_string();
        assert!(
            code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase()),
            "bad code {code}"
        );
        assert!(
            (2..=80).contains(&name.chars().count()),
            "bad name for {code}"
        );
        names_by_code.insert(code, name);
    }
    let mut chunks: Vec<(String, Vec<u8>, usize)> = Vec::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let table = json::parse(line).expect("country line JSON");
        let code = table
            .get("code")
            .and_then(Value::as_str)
            .expect("code")
            .to_string();
        assert!(
            names_by_code.contains_key(&code),
            "{code} missing from the index"
        );
        let years = table
            .get("years")
            .and_then(Value::as_object)
            .expect("years");
        let mut names: Vec<String> = Vec::new();
        let mut body = Vec::new();
        for year in 2000..=2050 {
            let days = years
                .iter()
                .find(|(k, _)| k == &year.to_string())
                .and_then(|(_, v)| v.as_object())
                .unwrap_or_else(|| panic!("{code} lacks {year}"));
            let mut entries: Vec<(u32, Vec<usize>)> = Vec::new();
            for (date, value) in days {
                let (y, m, d) = (&date[0..4], &date[5..7], &date[8..10]);
                assert_eq!(y.parse::<i32>().unwrap(), year, "{date} outside {year}");
                let doy = day_of_year(year, m.parse().unwrap(), d.parse().unwrap());
                let raw: Vec<&str> = match value {
                    Value::Str(s) => vec![s.as_str()],
                    Value::Array(a) => a.iter().map(|v| v.as_str().expect("name")).collect(),
                    _ => panic!("bad names for {code} {date}"),
                };
                let mut idx = Vec::new();
                for n in raw {
                    let n = n.trim();
                    assert!((1..=240).contains(&n.chars().count()));
                    let i = names.iter().position(|x| x == n).unwrap_or_else(|| {
                        names.push(n.to_string());
                        names.len() - 1
                    });
                    if !idx.contains(&i) {
                        idx.push(i);
                    }
                }
                assert!((1..=16).contains(&idx.len()));
                entries.push((doy, idx));
            }
            entries.sort();
            assert!(entries.len() <= 366);
            uvar(&mut body, entries.len() as u64);
            let mut prev = 0;
            for (doy, idx) in entries {
                uvar(&mut body, u64::from(doy - prev));
                prev = doy;
                uvar(&mut body, idx.len() as u64);
                for i in idx {
                    uvar(&mut body, i as u64);
                }
            }
        }
        let mut raw = Vec::new();
        uvar(&mut raw, names.len() as u64);
        for n in &names {
            string(&mut raw, n);
        }
        raw.extend_from_slice(&body);
        let len = raw.len();
        chunks.push((code, deflate(&raw), len));
    }
    chunks.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        chunks.len(),
        names_by_code.len(),
        "index and tables disagree"
    );

    let mut out = b"WNH1".to_vec();
    uvar(&mut out, chunks.len() as u64);
    let mut offset = 0usize;
    for (code, data, raw_len) in &chunks {
        out.extend_from_slice(code.as_bytes());
        string(&mut out, &names_by_code[code]);
        uvar(&mut out, offset as u64);
        uvar(&mut out, data.len() as u64);
        uvar(&mut out, *raw_len as u64);
        offset += data.len();
    }
    for (_, data, _) in &chunks {
        out.extend_from_slice(data);
    }
    out
}

/// The day the holiday data was generated (METADATA.provenance.generated_utc,
/// "2026-10-17T16:50:56+00:00") as "17 Oct 2026".
fn holidays_updated() -> String {
    let text = fs::read_to_string(HOLIDAY_DATA).expect("read holiday data");
    let first = text.split('\n').next().expect("index line");
    let index = json::parse(first).expect("index JSON");
    let stamp = index
        .get("METADATA")
        .and_then(|m| m.get("provenance"))
        .and_then(|p| p.get("generated_utc"))
        .and_then(Value::as_str)
        .expect("METADATA.provenance.generated_utc");
    let date = stamp.get(..10).expect("generated_utc date");
    let (y, m, d) = (&date[0..4], &date[5..7], &date[8..10]);
    let (year, month, day): (i32, u32, u32) = (
        y.parse().expect("year"),
        m.parse().expect("month"),
        d.parse().expect("day"),
    );
    assert!(
        &date[4..5] == "-" && &date[7..8] == "-",
        "generated_utc {stamp}"
    );
    day_of_year(year, month, day); // validates the date
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{day} {} {year}", MONTHS[month as usize - 1])
}

fn day_of_year(year: i32, month: u32, day: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    assert!((1..=12).contains(&month) && day >= 1 && day <= days[month as usize - 1]);
    days[..month as usize - 1].iter().sum::<u32>() + day - 1
}

// ---------------------------------------------------------------------------
// Time zones and the map
// ---------------------------------------------------------------------------

/// Layout: "WNT1", zones chunk (offset 0), map chunk; each preceded by its
/// compressed and inflated lengths.
///   zones: count, then per zone: key, label, city, lat, lon (centi-degrees,
///          zig-zag), band index + 1 (0 = none).
///   map:   band count, per band: utc in hundredths of an hour, sea rings,
///          land rings; then border lines. A ring list is: count, then per
///          ring: point count and zig-zag deltas of centi-degree lon, lat.
fn pack_timezones() -> Vec<u8> {
    let text = fs::read_to_string(TIMEZONE_DATA).expect("read time-zone data");
    let root = json::parse(&text).expect("time-zone JSON");
    let zones = root.get("zones").and_then(Value::as_array).expect("zones");
    let bands = root.get("bands").and_then(Value::as_array).expect("bands");
    let borders = root
        .get("borders")
        .and_then(Value::as_array)
        .expect("borders");

    let mut z = Vec::new();
    uvar(&mut z, zones.len() as u64);
    for zone in zones {
        let s = |k: &str| {
            zone.get(k)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("zone {k}"))
        };
        let f = |k: &str| {
            zone.get(k)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("zone {k}"))
        };
        let label = s("label");
        let city = zone
            .get("city")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map_or_else(
                || label.split(',').next().unwrap_or(label).trim().to_string(),
                str::to_string,
            );
        string(&mut z, s("key"));
        string(&mut z, label);
        string(&mut z, &city);
        let iana = s("iana");
        assert!(
            !iana.is_empty()
                && iana.len() <= 64
                && iana.split('/').all(|c| !c.is_empty()
                    && !c.starts_with('.')
                    && c.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-+_".contains(&b))),
            "zone iana {iana}"
        );
        string(&mut z, iana);
        svar(&mut z, centi(f("lat")));
        svar(&mut z, centi(f("lon")));
        let band = zone
            .get("band")
            .and_then(Value::as_i64)
            .filter(|b| (0..bands.len() as i64).contains(b));
        uvar(&mut z, band.map_or(0, |b| b as u64 + 1));
    }

    let rings = |out: &mut Vec<u8>, list: &Value| {
        let list = list.as_array().expect("ring list");
        uvar(out, list.len() as u64);
        for ring in list {
            let values = ring.as_array().expect("ring");
            assert!(values.len() % 2 == 0);
            uvar(out, (values.len() / 2) as u64);
            let (mut px, mut py) = (0i64, 0i64);
            for pair in values.as_chunks::<2>().0 {
                let (x, y) = (
                    centi(pair[0].as_f64().unwrap()),
                    centi(pair[1].as_f64().unwrap()),
                );
                assert!(x.abs() <= 18000 && y.abs() <= 9000);
                svar(out, x - px);
                svar(out, y - py);
                (px, py) = (x, y);
            }
        }
    };
    let mut m = Vec::new();
    uvar(&mut m, bands.len() as u64);
    for band in bands {
        svar(
            &mut m,
            centi(band.get("utc").and_then(Value::as_f64).expect("utc")),
        );
        rings(&mut m, band.get("sea").expect("sea"));
        rings(&mut m, band.get("land").expect("land"));
    }
    rings(&mut m, &Value::Array(borders.to_vec()));

    let mut out = b"WNT1".to_vec();
    for raw in [z, m] {
        let packed = deflate(&raw);
        uvar(&mut out, packed.len() as u64);
        uvar(&mut out, raw.len() as u64);
        out.extend_from_slice(&packed);
    }
    out
}

// ---------------------------------------------------------------------------
// Legal notices
// ---------------------------------------------------------------------------

/// One titled section: the title between two rule lines, which the Legal
/// notices window shows as a heading.
fn section(title: &str, text: &str) -> String {
    let rule = "=".repeat(76);
    format!("{rule}\n{title}\n{rule}\n\n{}\n\n\n", text.trim())
}

/// A text file as the window shows it: UTF-8 without BOM, LF line ends,
/// tabs expanded, and no other control characters (the build fails on one).
fn read_text(path: &str) -> String {
    let raw = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let text = String::from_utf8(raw).unwrap_or_else(|_| panic!("{path} is not UTF-8"));
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut out = String::new();
    for line in text.split('\n') {
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let n = 8 - col % 8;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else {
                assert!(!ch.is_control(), "{path} holds a control character");
                out.push(ch);
                col += 1;
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

/// A Markdown document without its "# " title line.
fn strip_title(text: &str) -> &str {
    match text.split_once('\n') {
        Some((first, rest)) if first.starts_with("# ") => rest,
        _ => text,
    }
}

/// The toolchain pin and every crate in Cargo.lock with its version.
fn build_environment(os: &str) -> String {
    let toolchain = read_text("rust-toolchain.toml");
    let channel = toolchain
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .map(|v| v.trim_matches('"').to_string())
        .expect("channel in rust-toolchain.toml");
    let target = env::var("TARGET").expect("TARGET");
    let profile = env::var("PROFILE").expect("PROFILE");
    let lock = read_text("Cargo.lock");
    let mut crates = Vec::new();
    let mut name = None;
    for line in lock.lines() {
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let (Some(v), Some(n)) = (line.strip_prefix("version = "), name.take())
            && n != "weeknumber"
            // Off Windows only the inflate crates are linked.
            && (os == "windows" || n == "miniz_oxide" || n == "adler2")
        {
            crates.push(format!("{n}=={}", v.trim_matches('"')));
        }
    }
    crates.sort();
    format!(
        "Rust {channel} ({target}), {profile} profile, Cargo.lock as committed\n\n{}",
        crates.join("\n")
    )
}

/// The target platform: (os as cfg names it, platform name, architecture name).
fn platform() -> (String, &'static str, &'static str) {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let name = match os.as_str() {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        _ => "this platform",
    };
    let arch_name = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => "Apple silicon",
        (_, "aarch64") => "ARM64",
        (_, "x86_64") => "x64",
        _ => "other",
    };
    (os, name, arch_name)
}

/// Keep the blocks of a notices file meant for `os`: a block starts with a
/// line `<!-- only: windows, linux -->` and ends with `<!-- end -->`;
/// other `<!-- ... -->` comments (one or more lines) are dropped.
fn for_platform(text: &str, os: &str) -> String {
    let mut out = String::new();
    let mut keep = true;
    let mut in_comment = false;
    for line in text.lines() {
        let t = line.trim();
        if in_comment {
            in_comment = !t.ends_with("-->");
            continue;
        }
        if let Some(list) = t
            .strip_prefix("<!-- only:")
            .and_then(|r| r.strip_suffix("-->"))
        {
            keep = list.split(',').any(|p| p.trim() == os);
            continue;
        }
        if t == "<!-- end -->" {
            keep = true;
            continue;
        }
        if t.starts_with("<!--") {
            in_comment = !t.ends_with("-->");
            continue;
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Blank lines left where blocks were removed collapse to one.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// Whether a license text in legal/licenses belongs in this build.
fn license_applies(name: &str, os: &str) -> bool {
    match name {
        "windows-rs-MIT.txt" | "Rust-standard-library.txt" => os == "windows",
        "Rust-standard-library-unix.txt" => os == "macos" || os == "linux",
        "musl-MIT.txt" => {
            os == "linux" && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl")
        }
        _ => true,
    }
}

fn legal_text() -> String {
    let (major, minor) = APP_VERSION;
    let (os, platform, arch) = platform();
    let title = format!(
        "WeekNumber {APP_EDITION} {major}.{minor} (Rust) for {platform} ({arch}) - legal notices and licenses"
    );
    let mut out = format!(
        "{title}\n{}\n\n\
         The legal notices come first: license and warranty, trademarks,\n\
         privacy and encryption. Then WeekNumber's MIT license, the third-party\n\
         components this build includes with their licenses, and the exact\n\
         versions it was built from.\n\n\n",
        "=".repeat(title.chars().count())
    );
    out += &section(
        "Legal notices",
        &for_platform(strip_title(&read_text(LEGAL_NOTICES)), &os),
    );
    out += &section("WeekNumber: LICENSE", &read_text(REPO_LICENSE));
    out += &section(
        "Third-party notices",
        &for_platform(strip_title(&read_text(THIRD_PARTY_NOTICES)), &os),
    );
    let mut files: Vec<String> = fs::read_dir(LICENSES_DIR)
        .expect("read legal/licenses")
        .map(|e| {
            e.expect("licenses entry")
                .file_name()
                .into_string()
                .expect("name")
        })
        .filter(|n| license_applies(n, &os))
        .collect();
    files.sort_by_key(|n| n.to_lowercase());
    assert!(!files.is_empty(), "no license texts");
    for name in files {
        out += &section(
            &format!("licenses/{name}"),
            &read_text(&format!("{LICENSES_DIR}/{name}")),
        );
    }
    out += &section("Build environment", &build_environment(&os));
    let out = out.trim_end().to_string() + "\n";
    assert!(out.len() <= MAX_LEGAL_BYTES, "legal text too large");
    out
}

// ---------------------------------------------------------------------------
// Resources (.res)
// ---------------------------------------------------------------------------

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const LANG_EN_US: u16 = 0x0409;

fn align4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn res_entry(out: &mut Vec<u8>, kind: u16, id: u16, lang: u16, flags: u16, data: &[u8]) {
    let mut header = Vec::new();
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    header.extend_from_slice(&32u32.to_le_bytes()); // header size, fixed below
    for v in [0xFFFF, kind, 0xFFFF, id] {
        header.extend_from_slice(&u16::to_le_bytes(v));
    }
    header.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&lang.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // Version
    header.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    let size = header.len() as u32;
    header[4..8].copy_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    align4(out);
}

fn resources() -> Vec<u8> {
    let mut out = Vec::new();
    res_entry(&mut out, 0, 0, 0, 0, &[]); // the .res file's leading empty entry

    let manifest = fs::read("app.manifest").expect("read app.manifest");
    res_entry(&mut out, RT_MANIFEST, 1, LANG_EN_US, 0x0030, &manifest);

    // The icon: every frame of app.ico as RT_ICON, plus the group directory.
    let ico = fs::read(APP_ICON).expect("read app.ico");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    assert!(
        u16::from_le_bytes([ico[2], ico[3]]) == 1 && count > 0,
        "not an icon"
    );
    let mut group = Vec::new();
    for v in [0u16, 1, count as u16] {
        group.extend_from_slice(&v.to_le_bytes());
    }
    for i in 0..count {
        let e = &ico[6 + 16 * i..22 + 16 * i];
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        res_entry(
            &mut out,
            RT_ICON,
            (i + 1) as u16,
            LANG_EN_US,
            0x1010,
            &ico[offset..offset + size],
        );
        group.extend_from_slice(&e[..12]); // width .. bytes in resource
        group.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }
    res_entry(&mut out, RT_GROUP_ICON, 1, LANG_EN_US, 0x1030, &group);

    res_entry(&mut out, RT_VERSION, 1, LANG_EN_US, 0x0030, &version_info());
    out
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// One VS_VERSIONINFO node: length, value length, type, key, value, children.
fn node(key: &str, value: &[u8], value_units: usize, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0u8; 6];
    v[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    v.extend_from_slice(&utf16z(key));
    align4(&mut v);
    v[2..4].copy_from_slice(&(value_units as u16).to_le_bytes());
    v.extend_from_slice(value);
    for child in children {
        align4(&mut v);
        v.extend_from_slice(child);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

fn version_info() -> Vec<u8> {
    let (major, minor) = APP_VERSION;
    let quad = format!("{major}.{minor}.0.0");
    let product = format!("WeekNumber {APP_EDITION}");
    let strings: Vec<Vec<u8>> = [
        (
            "FileDescription",
            format!("WeekNumber {APP_EDITION} - ISO week number in the system tray"),
        ),
        ("FileVersion", quad.clone()),
        ("InternalName", "WeekNumber".to_string()),
        ("LegalCopyright", APP_COPYRIGHT.to_string()),
        ("OriginalFilename", "WeekNumber.exe".to_string()),
        ("ProductName", product),
        ("ProductVersion", quad),
    ]
    .iter()
    .map(|(k, v)| {
        let value = utf16z(v);
        node(k, &value, value.len() / 2, true, &[])
    })
    .collect();
    let table = node("040904B0", &[], 0, true, &strings);
    let sfi = node("StringFileInfo", &[], 0, true, &[table]);
    let translation = [0x09u8, 0x04, 0xB0, 0x04];
    let var = node("Translation", &translation, 4, false, &[]);
    let vfi = node("VarFileInfo", &[], 0, true, &[var]);

    // VS_FIXEDFILEINFO
    let ms = (u32::from(major) << 16) | u32::from(minor);
    let mut fixed = Vec::new();
    for v in [
        0xFEEF04BDu32,
        0x0001_0000,
        ms,
        0,
        ms,
        0,
        0x3F,
        0,
        0x0004_0004,
        1,
        0,
        0,
        0,
    ] {
        fixed.extend_from_slice(&v.to_le_bytes());
    }
    node("VS_VERSION_INFO", &fixed, fixed.len(), false, &[sfi, vfi])
}

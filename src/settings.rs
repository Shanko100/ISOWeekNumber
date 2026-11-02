//! settings.json in the settings folder (%LOCALAPPDATA%\WeekNumber on
//! Windows, see os::settings_dir): a flat JSON object, read defensively and
//! written atomically.

use crate::json::{self, Flat, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Largest settings file read, as in the Python version.
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
/// Bold digits by default on Windows (the taskbar's look) and macOS; on
/// Linux the panel's own text is regular weight, so Bold Text starts off
/// there and stays an option.
const DEFAULT_BOLD: bool = !cfg!(target_os = "linux");
/// DYNAMIC_TIME_ZONE_INFORMATION.TimeZoneKeyName holds 127 characters.
pub const MAX_KEY_CHARS: usize = 127;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub selected_color: String,
    pub is_bold: bool,
    pub holiday_primary: Option<String>,
    pub holiday_secondary: Option<String>,
    pub dark_mode: bool,
    pub startup_initialized: bool,
    pub timezone_enabled: bool,
    pub timezone_key: Option<String>,
    /// Whether a settings file was found, readable or not.
    pub file_existed: bool,
    dir: PathBuf,
}

/// Exactly `#` and six hex digits (upper-cased), anything else white.
pub fn normalize_hex(value: &str) -> String {
    parse_hex(value).unwrap_or_else(|| "#FFFFFF".to_string())
}

/// A typed colour as `#RRGGBB`, or None.
pub fn parse_hex(value: &str) -> Option<String> {
    let v = value.trim();
    let digits = v.strip_prefix('#').unwrap_or(v);
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(format!("#{}", digits.to_ascii_uppercase()))
    } else {
        None
    }
}

pub fn hex_to_rgb(value: &str) -> (u8, u8, u8) {
    let v = normalize_hex(value);
    let byte = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).unwrap_or(255);
    (byte(1), byte(3), byte(5))
}

fn country_value(value: Option<&Value>) -> Option<String> {
    // Python: a str of length 2 (in characters).
    let s = value?.as_str()?;
    (s.chars().count() == 2).then(|| s.to_string())
}

impl Settings {
    fn defaults(dir: PathBuf) -> Settings {
        Settings {
            selected_color: "#FFFFFF".into(),
            is_bold: DEFAULT_BOLD,
            holiday_primary: None,
            holiday_secondary: None,
            dark_mode: false,
            startup_initialized: false,
            timezone_enabled: false,
            timezone_key: None,
            file_existed: false,
            dir,
        }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// Load from `dir`; any problem leaves the defaults in place.
    pub fn load(dir: PathBuf) -> Settings {
        let mut s = Settings::defaults(dir);
        let path = s.file();
        s.file_existed = path.exists();
        let Some(raw) = read_bounded(&path).and_then(|t| json::parse(&t)) else {
            return s;
        };
        if raw.as_object().is_none() {
            return s;
        }
        if let Some(color) = raw.get("SelectedColor").and_then(Value::as_str) {
            s.selected_color = normalize_hex(color);
        }
        if let Some(b) = raw.get("IsBold").and_then(Value::as_bool) {
            s.is_bold = b;
        }
        // New keys win; a legacy HolidayCountry migrates to primary.
        let primary = raw
            .get("HolidayCountryPrimary")
            .or_else(|| raw.get("HolidayCountry"));
        s.holiday_primary = country_value(primary);
        s.holiday_secondary = country_value(raw.get("HolidayCountrySecondary"));
        for (key, field) in [
            ("DarkMode", &mut s.dark_mode),
            ("StartupInitialized", &mut s.startup_initialized),
            ("TimeZoneEnabled", &mut s.timezone_enabled),
        ] {
            if let Some(b) = raw.get(key).and_then(Value::as_bool) {
                *field = b;
            }
        }
        match raw.get("TimeZoneKey") {
            None | Some(Value::Null) => s.timezone_key = None,
            Some(Value::Str(k)) if k.chars().count() <= MAX_KEY_CHARS => {
                s.timezone_key = Some(k.clone())
            }
            _ => {}
        }
        s
    }

    /// The file's text: same keys, order and layout as the Python version.
    pub fn to_json(&self) -> String {
        fn opt(v: &Option<String>) -> Flat<'_> {
            v.as_deref().map_or(Flat::Null, Flat::Str)
        }
        json::write_flat_object(&[
            ("SelectedColor", Flat::Str(&self.selected_color)),
            ("IsBold", Flat::Bool(self.is_bold)),
            ("HolidayCountry", opt(&self.holiday_primary)),
            ("HolidayCountryPrimary", opt(&self.holiday_primary)),
            ("HolidayCountrySecondary", opt(&self.holiday_secondary)),
            ("DarkMode", Flat::Bool(self.dark_mode)),
            ("StartupInitialized", Flat::Bool(self.startup_initialized)),
            ("TimeZoneEnabled", Flat::Bool(self.timezone_enabled)),
            ("TimeZoneKey", opt(&self.timezone_key)),
        ])
    }

    /// Write through a new file in the same folder, flushed to disk, then
    /// renamed over the old one, so a crash never leaves half a file.
    pub fn save(&self) -> bool {
        if fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let text = self.to_json();
        let Some((mut file, tmp)) = create_new_file(&self.dir) else {
            return false;
        };
        let ok = file.write_all(text.as_bytes()).is_ok() && file.sync_all().is_ok();
        drop(file);
        if ok && fs::rename(&tmp, self.file()).is_ok() {
            return true;
        }
        let _ = fs::remove_file(&tmp);
        false
    }

    pub fn set_color(&mut self, color: &str) {
        self.selected_color = normalize_hex(color);
    }

    pub fn mark_startup_initialized(&mut self) {
        self.startup_initialized = true;
        self.save();
    }

    /// Drop saved countries that are no longer offered, or duplicated.
    pub fn validate_holiday_countries(&mut self, valid: &dyn Fn(&str) -> bool) {
        let mut primary = self.holiday_primary.clone();
        let mut secondary = self.holiday_secondary.clone();
        if primary.as_deref().is_some_and(|c| !valid(c)) {
            primary = None;
        }
        if secondary.as_deref().is_some_and(|c| !valid(c)) {
            secondary = None;
        }
        if secondary.is_some() && secondary == primary {
            secondary = None;
        }
        if primary != self.holiday_primary || secondary != self.holiday_secondary {
            self.holiday_primary = primary;
            self.holiday_secondary = secondary;
            self.save();
        }
    }

    /// Drop a saved zone this system no longer knows.
    pub fn validate_timezone_key(&mut self, known: &dyn Fn(&str) -> bool) {
        if self.timezone_key.as_deref().is_some_and(|k| !known(k)) {
            self.timezone_key = None;
            self.save();
        }
    }
}

/// The file's text if it is a regular file of at most 64 KB of UTF-8.
fn read_bounded(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_SETTINGS_BYTES {
        return None;
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    // One byte more than the cap: a file that grew while read is rejected.
    Read::by_ref(&mut file)
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_SETTINGS_BYTES || buf.len() as u64 != meta.len() {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// A file under a fresh name in `dir` that did not exist before.
fn create_new_file(dir: &Path) -> Option<(fs::File, PathBuf)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "settings-{:x}{:08x}{:04x}.json.tmp",
            std::process::id(),
            nanos,
            n
        );
        let path = dir.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Some((file, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex() {
        assert_eq!(normalize_hex(" ff00aa "), "#FF00AA");
        assert_eq!(normalize_hex("#1_2345"), "#FFFFFF");
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(hex_to_rgb("#0A0B0C"), (10, 11, 12));
    }

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("wn-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut s = Settings::load(dir.clone());
        assert!(!s.file_existed);
        s.selected_color = "#123ABC".into();
        s.holiday_primary = Some("NL".into());
        s.timezone_key = Some("W. Europe Standard Time".into());
        assert!(s.save());
        let t = Settings::load(dir.clone());
        assert!(t.file_existed);
        assert_eq!(t.to_json(), s.to_json());
        let bold = format!("\"IsBold\": {DEFAULT_BOLD},");
        assert!(t.to_json().starts_with(&format!(
            "{{\n  \"SelectedColor\": \"#123ABC\",\n  {bold}\n  \"HolidayCountry\": \"NL\""
        )));
        fs::write(
            dir.join("settings.json"),
            "{\"HolidayCountry\": \"DE\", \"IsBold\": 1}",
        )
        .unwrap();
        let u = Settings::load(dir.clone());
        assert_eq!(u.holiday_primary.as_deref(), Some("DE"));
        // A value of the wrong type leaves the default.
        assert_eq!(u.is_bold, DEFAULT_BOLD);
        let _ = fs::remove_dir_all(&dir);
    }
}

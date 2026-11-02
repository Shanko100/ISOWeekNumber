//! Run at Startup: the HKCU Run value "WeekNumber".
//!
//! * The Run value is the single source of truth. It is ticked only when the
//!   value is a REG_SZ that starts this very exe and Task Manager has not
//!   disabled it (StartupApproved\Run, bit 0 of the first byte).
//! * StartupApproved is written only on an explicit enable, so a "Disabled"
//!   choice made in Task Manager is respected.
//! * Refused while the exe sits where others can swap it (Downloads,
//!   Desktop, temporary, synced, network or removable locations), and for
//!   commands of MAX_PATH characters or more, which Windows ignores.
//! * At launch the value is repointed only when the program it names no
//!   longer exists (the app moved) -- never to another copy, never when
//!   disabled. The Startup-folder shortcut an earlier Rust build wrote is
//!   moved over when it starts this app, with its Task Manager state.

use crate::os::{self, Approved, KnownFolder};
use std::path::{Path, PathBuf};

/// Our own Run value (the settings folder uses the same name).
const RUN_VALUE: &str = "WeekNumber";
/// The Startup-folder shortcut of the earlier Rust build; migrated likewise.
const LEGACY_LNK_NAME: &str = "WeekNumber.lnk";
/// Windows ignores Run commands of MAX_PATH characters or more.
const MAX_RUN_COMMAND: usize = 259;
/// StartupApproved data for "enabled": 0x02, then a zero FILETIME.
const APPROVED_ENABLED: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn move_hint() -> String {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    format!(
        " Move WeekNumber to {pf}\\WeekNumber (needs administrator rights), or to \
         %LOCALAPPDATA%\\Programs\\WeekNumber if you can't install software, then turn \
         Run at Startup on again."
    )
}

/// This exe's canonical path.
fn this_exe() -> Option<PathBuf> {
    os::exe_path().and_then(|p| os::final_path(&p))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The deepest folder in `roots` containing `path` (whole components,
/// case-insensitive, both sides canonical where they exist).
fn containing_root<'a>(path: &Path, roots: &'a [(PathBuf, &'static str)]) -> Option<&'a str> {
    let target = os::final_path(path).unwrap_or_else(|| path.to_path_buf());
    let t = target.to_string_lossy().to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for (root, label) in roots {
        let r = os::final_path(root).unwrap_or_else(|| root.clone());
        let mut r = r.to_string_lossy().to_lowercase();
        if r.is_empty() {
            continue;
        }
        if !r.ends_with('\\') {
            r.push('\\');
        }
        let inside = t.starts_with(&r) || format!("{t}\\") == r;
        if inside && best.is_none_or(|(len, _)| r.len() > len) {
            best = Some((r.len(), label));
        }
    }
    best.map(|(_, l)| l)
}

fn risky_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    if let Some(t) = os::temp_path() {
        roots.push((t, "a temporary folder"));
    }
    for var in ["TEMP", "TMP", "TMPDIR"] {
        if let Ok(v) = std::env::var(var)
            && Path::new(&v).is_absolute()
        {
            roots.push((PathBuf::from(v), "a temporary folder"));
        }
    }
    for (f, label) in [
        (KnownFolder::Downloads, "your Downloads folder"),
        (KnownFolder::Desktop, "your Desktop"),
        (KnownFolder::Public, "the shared Public folder"),
        (KnownFolder::OneDrive, "a OneDrive folder"),
    ] {
        if let Some(p) = os::known_folder(f) {
            roots.push((p, label));
        }
    }
    for var in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            roots.push((PathBuf::from(v), "a OneDrive folder"));
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = PathBuf::from(home);
        for (sub, label) in [
            ("Downloads", "your Downloads folder"),
            ("Desktop", "your Desktop"),
            ("Dropbox", "a Dropbox folder"),
            ("Google Drive", "a Google Drive folder"),
        ] {
            roots.push((home.join(sub), label));
        }
    }
    roots.push((
        PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())),
        "ProgramData, which every user can write to",
    ));
    roots.push((os::windows_dir().join("Temp"), "the Windows Temp folder"));
    roots
}

/// Why autostart from `exe` is refused, or None. Fails closed.
fn location_problem(exe: Option<&Path>) -> Option<String> {
    let Some(exe) = exe else {
        return Some("WeekNumber could not determine where it is installed.".into());
    };
    let text = exe.to_string_lossy();
    if text.starts_with("\\\\") {
        return Some(format!("WeekNumber is on a network share.{}", move_hint()));
    }
    let root = exe.ancestors().last().unwrap_or(exe);
    match os::drive_type(root) {
        os::DRIVE_REMOTE_ => {
            return Some(format!("WeekNumber is on a network drive.{}", move_hint()));
        }
        os::DRIVE_REMOVABLE_ | os::DRIVE_CDROM_ => {
            return Some(format!(
                "WeekNumber is on a removable drive.{}",
                move_hint()
            ));
        }
        _ => {}
    }
    containing_root(exe, &risky_roots())
        .map(|label| format!("WeekNumber is in {label}.{}", move_hint()))
}

fn approved_disabled(data: Option<Vec<u8>>) -> bool {
    data.is_some_and(|d| d.first().is_some_and(|b| b & 0x01 != 0))
}

fn quote_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The paths in a Run command: every double-quoted token, or the first
/// bare token of an unquoted command.
fn command_paths(command: &str) -> Vec<String> {
    let quoted: Vec<String> = command
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    if !quoted.is_empty() {
        return quoted;
    }
    command
        .split_whitespace()
        .take(1)
        .map(str::to_string)
        .collect()
}

/// Whether a Run command starts this exe.
fn points_to(command: Option<&str>, exe: &Path) -> bool {
    command
        .and_then(|c| command_paths(c).into_iter().next())
        .is_some_and(|p| same_path(Path::new(&p), exe))
}

pub fn is_enabled() -> bool {
    let Some(exe) = this_exe() else {
        return false;
    };
    points_to(os::run_get(RUN_VALUE).as_deref(), &exe)
        && !approved_disabled(os::approved_get(Approved::Run, RUN_VALUE))
}

/// Turn startup on or off; Err carries the message to show.
pub fn set(enable: bool) -> Result<(), String> {
    const REFUSED: &str = "Windows refused to update the startup entry.";
    if !enable {
        return if os::run_delete(RUN_VALUE) {
            Ok(())
        } else {
            Err(REFUSED.into())
        };
    }
    let exe = this_exe();
    if let Some(problem) = location_problem(exe.as_deref()) {
        return Err(problem);
    }
    let exe = exe.ok_or("WeekNumber could not determine where it is installed.")?;
    let command = quote_command(&exe);
    if command.chars().count() > MAX_RUN_COMMAND {
        return Err(
            "WeekNumber's install path is too long for Windows to start it \
                    automatically. Move it to a shorter path, such as \
                    C:\\Program Files\\WeekNumber (needs administrator rights) or \
                    %LOCALAPPDATA%\\Programs\\WeekNumber."
                .into(),
        );
    }
    if !os::run_set(RUN_VALUE, &command) {
        return Err(REFUSED.into());
    }
    // Only an explicit enable clears a Task Manager "disabled" mark.
    os::approved_set(Approved::Run, RUN_VALUE, &APPROVED_ENABLED);
    Ok(())
}

/// First run: start with autostart off.
pub fn disable_on_first_run() {
    let _ = set(false);
}

/// At launch: adopt an earlier build's Startup-folder shortcut that starts
/// this app, then repair our value after the app moved. Never repoints to a different
/// copy and never re-enables an entry the user disabled.
pub fn refresh_at_launch() {
    let Some(exe) = this_exe() else {
        return;
    };
    migrate_legacy_shortcut(&exe);
    let command = quote_command(&exe);
    let Some(current) = os::run_get(RUN_VALUE) else {
        return;
    };
    if current == command {
        return;
    }
    let registered = command_paths(&current).pop().unwrap_or_default();
    if !registered.is_empty() && Path::new(&registered).exists() {
        return; // another copy: leave it
    }
    if approved_disabled(os::approved_get(Approved::Run, RUN_VALUE)) {
        return;
    }
    if location_problem(Some(&exe)).is_some() || command.chars().count() > MAX_RUN_COMMAND {
        return;
    }
    os::run_set(RUN_VALUE, &command);
}

/// The earlier Rust build started from a Startup-folder shortcut. When that
/// shortcut starts this exe it becomes our Run value (if there is none yet),
/// keeping its Task Manager state, and is deleted, so the app never starts
/// twice. A shortcut to anything else is left alone.
fn migrate_legacy_shortcut(exe: &Path) {
    let Some(lnk) = os::known_folder(KnownFolder::Startup).map(|d| d.join(LEGACY_LNK_NAME)) else {
        return;
    };
    if !lnk.is_file() {
        return;
    }
    let target = os::read_shortcut(&lnk).and_then(|t| os::final_path(&t));
    if !target.is_some_and(|t| same_path(&t, exe)) {
        return;
    }
    if os::run_get(RUN_VALUE).is_none() {
        let command = quote_command(exe);
        if command.chars().count() > MAX_RUN_COMMAND || !os::run_set(RUN_VALUE, &command) {
            return;
        }
        let state = os::approved_get(Approved::StartupFolder, LEGACY_LNK_NAME)
            .unwrap_or_else(|| APPROVED_ENABLED.to_vec());
        os::approved_set(Approved::Run, RUN_VALUE, &state);
    }
    if std::fs::remove_file(&lnk).is_ok() {
        os::approved_delete(Approved::StartupFolder, LEGACY_LNK_NAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_paths() {
        assert_eq!(command_paths("\"C:\\A B\\W.exe\""), vec!["C:\\A B\\W.exe"]);
        assert_eq!(command_paths("C:\\W.exe --x"), vec!["C:\\W.exe"]);
        assert_eq!(command_paths("\"p.exe\" \"s.py\""), vec!["p.exe", "s.py"]);
        assert!(points_to(
            Some("\"C:\\Apps\\WeekNumber.exe\""),
            Path::new("c:\\apps\\weeknumber.exe")
        ));
        assert!(!points_to(
            Some("\"C:\\Other\\WeekNumber.exe\""),
            Path::new("C:\\Apps\\WeekNumber.exe")
        ));
        assert!(!points_to(None, Path::new("C:\\Apps\\WeekNumber.exe")));
        assert!(approved_disabled(Some(vec![3, 0])) && !approved_disabled(Some(vec![2, 0])));
        assert!(!approved_disabled(None));
    }
}

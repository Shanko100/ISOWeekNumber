//! Run at Startup on Linux: an XDG autostart entry,
//! ~/.config/autostart/weeknumber.desktop, which every freedesktop desktop
//! (GNOME, KDE Plasma, Xfce, Cinnamon, MATE, LXQt, Budgie) starts at login
//! as the user, with no privileges and no background service.
//!
//! * The entry is ours only while its Exec line starts this very program,
//!   and the desktop has not disabled it (Hidden=true or
//!   X-GNOME-Autostart-enabled=false).
//! * Refused while the program sits where someone else could swap it: in a
//!   temporary, download or desktop folder, on removable or network media,
//!   or in a folder (or file) that other users may write to.
//! * At launch the entry is repointed only when the program it names no
//!   longer exists (the app moved), never to another copy.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "weeknumber.desktop";

fn autostart_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("autostart"))
}

fn entry_path() -> Option<PathBuf> {
    autostart_dir().map(|d| d.join(FILE_NAME))
}

fn this_exe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
}

/// Exec value for a program path (Desktop Entry spec: quoted, with ", `,
/// $ and \ escaped inside the quotes, and % doubled).
pub fn exec_line(exe: &Path) -> String {
    let mut s = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                s.push('\\');
                s.push(c);
            }
            '%' => s.push_str("%%"),
            _ => s.push(c),
        }
    }
    s.push('"');
    s
}

/// The program an Exec value starts (first argument, unquoted).
pub fn exec_program(exec: &str) -> Option<String> {
    let exec = exec.trim();
    if let Some(rest) = exec.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?),
                '"' => return Some(out.replace("%%", "%")),
                _ => out.push(c),
            }
        }
        None
    } else {
        exec.split_whitespace().next().map(|p| p.replace("%%", "%"))
    }
}

/// The desktop entry this app writes.
pub fn entry_text(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=WeekNumber\n\
         Comment=ISO week number in the system tray\n\
         Exec={}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-after=panel\n",
        exec_line(exe)
    )
}

struct Entry {
    program: Option<String>,
    disabled: bool,
}

fn parse_entry(text: &str) -> Entry {
    let mut program = None;
    let mut disabled = false;
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match (k.trim(), v.trim()) {
            ("Exec", v) => program = exec_program(v),
            ("Hidden", "true") | ("X-GNOME-Autostart-enabled", "false") => disabled = true,
            _ => {}
        }
    }
    Entry { program, disabled }
}

fn read_entry() -> Option<Entry> {
    let p = entry_path()?;
    let meta = std::fs::symlink_metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    Some(parse_entry(&std::fs::read_to_string(p).ok()?))
}

pub fn is_enabled() -> bool {
    let Some(exe) = this_exe() else {
        return false;
    };
    read_entry()
        .is_some_and(|e| !e.disabled && e.program.as_deref().is_some_and(|p| Path::new(p) == exe))
}

/// Folders a program must not start from: (folder, how to name it).
fn risky_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots: Vec<(PathBuf, &'static str)> = Vec::new();
    for t in ["/tmp", "/var/tmp", "/dev/shm", "/run/user"] {
        roots.push((PathBuf::from(t), "a temporary folder"));
    }
    if let Some(t) = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        roots.push((t, "a temporary folder"));
    }
    for m in ["/media", "/run/media", "/mnt"] {
        roots.push((PathBuf::from(m), "a removable or mounted drive"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let dirs = user_dirs(&home);
        let pick =
            |key: &str, default: &str| dirs.get(key).cloned().unwrap_or_else(|| home.join(default));
        roots.push((
            pick("XDG_DOWNLOAD_DIR", "Downloads"),
            "your Downloads folder",
        ));
        roots.push((pick("XDG_DESKTOP_DIR", "Desktop"), "your Desktop"));
        roots.push((
            pick("XDG_PUBLICSHARE_DIR", "Public"),
            "the shared Public folder",
        ));
        for (sub, label) in [
            ("Dropbox", "a Dropbox folder"),
            ("Nextcloud", "a Nextcloud folder"),
            ("OneDrive", "a OneDrive folder"),
            ("Google Drive", "a Google Drive folder"),
            (".cache", "a cache folder"),
        ] {
            roots.push((home.join(sub), label));
        }
    }
    roots
}

/// ~/.config/user-dirs.dirs: XDG_*_DIR="$HOME/..." entries.
fn user_dirs(home: &Path) -> std::collections::HashMap<String, PathBuf> {
    let mut map = std::collections::HashMap::new();
    let file = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("user-dirs.dirs");
    let Ok(text) = std::fs::read_to_string(file) else {
        return map;
    };
    for line in text.lines().take(64) {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        let path = if let Some(rest) = v.strip_prefix("$HOME/") {
            home.join(rest)
        } else if v.starts_with('/') {
            PathBuf::from(v)
        } else {
            continue;
        };
        map.insert(k.trim().to_string(), path);
    }
    map
}

/// The filesystem type of the mount holding `path` (/proc/self/mountinfo).
fn mount_type(path: &Path, mountinfo: &str) -> Option<String> {
    let p = path.to_string_lossy();
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let (Some(mount), Some(fstype)) = (fields.get(4), fields.get(sep + 1)) else {
            continue;
        };
        let mount = mount.replace("\\040", " ");
        let inside = mount == "/"
            || p == mount
            || p.starts_with(&format!("{}/", mount.trim_end_matches('/')));
        if inside && best.as_ref().is_none_or(|(l, _)| mount.len() > *l) {
            best = Some((mount.len(), (*fstype).to_string()));
        }
    }
    best.map(|(_, t)| t)
}

fn is_network_fs(t: &str) -> bool {
    matches!(
        t,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "smbfs"
            | "9p"
            | "afs"
            | "ncpfs"
            | "davfs"
            | "ceph"
            | "glusterfs"
            | "fuse.sshfs"
            | "fuse.rclone"
            | "fuse.gvfsd-fuse"
            | "fuse.s3fs"
            | "fuse.davfs2"
    ) || t.starts_with("fuse.") && t.contains("ssh")
}

/// Why the program may not start at login from `exe`, or None.
pub fn location_problem(exe: Option<&Path>) -> Option<String> {
    let hint =
        " Move WeekNumber to ~/.local/bin or /usr/local/bin, then turn Run at Startup on again.";
    let Some(exe) = exe else {
        return Some("WeekNumber could not determine where it is installed.".into());
    };
    let s = exe.to_string_lossy();
    if s.chars().any(char::is_control) {
        return Some(format!(
            "WeekNumber's path contains characters a desktop entry cannot hold.{hint}"
        ));
    }
    let mut label = None;
    let mut depth = 0;
    for (root, l) in risky_roots() {
        let r = std::fs::canonicalize(&root).unwrap_or(root);
        if exe.starts_with(&r) && r.components().count() > depth {
            depth = r.components().count();
            label = Some(l);
        }
    }
    if let Some(l) = label {
        return Some(format!("WeekNumber can't start at login from {l}.{hint}"));
    }
    if let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo")
        && mount_type(exe, &info).is_some_and(|t| is_network_fs(&t))
    {
        return Some(format!(
            "WeekNumber can't start at login from a network folder.{hint}"
        ));
    }
    if let Some(p) = writable_by_others(exe) {
        return Some(format!(
            "WeekNumber can't start at login while other users can change {}.{hint}",
            p.display()
        ));
    }
    None
}

/// The program, or a folder above it, that is owned by someone other than
/// this user or root, or writable by group or others (sticky folders
/// excepted) -- anyone who can write there could swap the program.
fn writable_by_others(exe: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let uid = crate::os::unix::sys::uid();
    let mut p = Some(exe);
    while let Some(cur) = p {
        let meta = std::fs::metadata(cur).ok()?;
        let mode = meta.permissions().mode();
        let sticky_dir = meta.is_dir() && mode & 0o1000 != 0 && meta.uid() == 0;
        if (meta.uid() != uid && meta.uid() != 0) || (mode & 0o022 != 0 && !sticky_dir) {
            return Some(cur.to_path_buf());
        }
        p = cur.parent();
    }
    None
}

/// Turn startup on or off; Err carries the message to show.
pub fn set(enable: bool) -> Result<(), String> {
    const REFUSED: &str = "WeekNumber could not update its autostart entry.";
    let path = entry_path().ok_or(REFUSED)?;
    if !enable {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(REFUSED.into()),
        };
    }
    let exe = this_exe();
    if let Some(problem) = location_problem(exe.as_deref()) {
        return Err(problem);
    }
    let exe = exe.ok_or(REFUSED)?;
    write_entry(&path, &entry_text(&exe)).map_err(|_| REFUSED.to_string())
}

/// Write via a temporary file and rename, so a crash never leaves half an entry.
fn write_entry(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{FILE_NAME}.{}", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// After installing: an enabled entry that starts `from` now starts `to`.
pub fn repoint(from: &Path, to: &Path) {
    let (Some(entry), Some(path)) = (read_entry(), entry_path()) else {
        return;
    };
    if entry.disabled || entry.program.as_deref().map(Path::new) != Some(from) {
        return;
    }
    if location_problem(Some(to)).is_none() {
        let _ = write_entry(&path, &entry_text(to));
    }
}

/// On uninstall: remove the entry if it starts `program`.
pub fn remove_if_starts(program: &Path) {
    if read_entry().is_some_and(|e| e.program.as_deref().map(Path::new) == Some(program))
        && let Some(path) = entry_path()
    {
        let _ = std::fs::remove_file(path);
    }
}

/// First run: start with autostart off.
pub fn disable_on_first_run() {
    let _ = set(false);
}

/// At launch: repair our entry after the app moved; never repoint it to a
/// different copy, never re-enable one the desktop disabled.
pub fn refresh_at_launch() {
    let (Some(exe), Some(entry), Some(path)) = (this_exe(), read_entry(), entry_path()) else {
        return;
    };
    if entry.disabled {
        return;
    }
    let Some(program) = entry.program else { return };
    if Path::new(&program) == exe || Path::new(&program).exists() {
        return;
    }
    if location_problem(Some(&exe)).is_none() {
        let _ = write_entry(&path, &entry_text(&exe));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting_round_trips() {
        for p in [
            "/usr/local/bin/WeekNumber",
            "/home/a b/My \"App\" $x/100%/WeekNumber",
        ] {
            let line = exec_line(Path::new(p));
            assert!(line.starts_with('"') && line.ends_with('"'));
            assert_eq!(exec_program(&line).as_deref(), Some(p));
        }
        assert_eq!(
            exec_program("/usr/bin/x --flag").as_deref(),
            Some("/usr/bin/x")
        );
        assert_eq!(exec_program("\"unterminated"), None);
    }

    #[test]
    fn entry_parsing() {
        let e = parse_entry(&entry_text(Path::new("/opt/WeekNumber")));
        assert_eq!(e.program.as_deref(), Some("/opt/WeekNumber"));
        assert!(!e.disabled);
        let off = parse_entry("[Desktop Entry]\nExec=/x\nX-GNOME-Autostart-enabled=false\n");
        assert!(off.disabled);
        let other = parse_entry("[Other]\nExec=/evil\n[Desktop Entry]\nHidden=true\n");
        assert!(other.program.is_none() && other.disabled);
    }

    #[test]
    fn mounts() {
        let info = "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n\
                    40 22 0:50 / /mnt/share rw - cifs //srv/share rw\n\
                    41 22 0:51 / /home/u/remote rw - fuse.sshfs u@h: rw\n";
        assert_eq!(
            mount_type(Path::new("/usr/bin/x"), info).as_deref(),
            Some("ext4")
        );
        assert_eq!(
            mount_type(Path::new("/mnt/share/x"), info).as_deref(),
            Some("cifs")
        );
        assert_eq!(
            mount_type(Path::new("/mnt/shared/x"), info).as_deref(),
            Some("ext4")
        );
        assert!(is_network_fs("fuse.sshfs") && is_network_fs("nfs4") && !is_network_fs("ext4"));
    }

    #[test]
    fn risky_locations() {
        assert!(location_problem(None).is_some());
        assert!(
            location_problem(Some(Path::new("/tmp/WeekNumber")))
                .is_some_and(|m| m.contains("temporary"))
        );
        assert!(
            location_problem(Some(Path::new("/media/usb/WeekNumber")))
                .is_some_and(|m| m.contains("removable"))
        );
        if let Some(home) = std::env::var_os("HOME") {
            let d = PathBuf::from(home).join("Downloads/WeekNumber");
            assert!(location_problem(Some(&d)).is_some_and(|m| m.contains("Downloads")));
        }
        // A world-writable folder is refused, the same folder locked down is not.
        let base = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("wn-auto-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let exe = base.join("WeekNumber");
        std::fs::write(&exe, b"").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(writable_by_others(&exe).is_some_and(|p| p == base));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            writable_by_others(&exe).filter(|p| p.starts_with(&base)),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn atomic_write() {
        let dir = std::env::temp_dir().join(format!("wn-entry-{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        write_entry(&path, "a").unwrap();
        write_entry(&path, "b").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}

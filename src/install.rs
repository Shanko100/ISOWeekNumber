//! Linux: install the running copy for this user, and be its own AppImage.
//!
//! * Install: the program goes to ~/.local/bin/weeknumber, a menu entry to
//!   ~/.local/share/applications and the icon, at every size the app ships,
//!   to the hicolor theme there -- nothing outside the home folder, no root,
//!   no helper processes. Offered when the app starts from anywhere else
//!   (a double-clicked AppImage in Downloads), or with `--install`.
//! * AppImage: the executable is self-contained, so the AppImage is the
//!   executable itself with the AppImage magic bytes and a small SquashFS
//!   (menu entry and icon) appended for AppImage tools. No FUSE runtime is
//!   involved; the runtime options those tools use (`--appimage-extract`,
//!   `--appimage-offset`, ...) are answered here.

use crate::startup;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "io.github.shanko100.weeknumber";
/// Name of the installed program in ~/.local/bin.
const BIN_NAME: &str = "weeknumber";
/// Icon sizes of the hicolor theme the app's icon ships in.
const ICON_SIDES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// The settings-folder file that remembers "Not Now".
const DECLINED: &str = "install-declined";

/// Where an installation goes.
struct Places {
    /// ~/.local/bin/weeknumber
    program: PathBuf,
    /// $XDG_DATA_HOME, ~/.local/share by default.
    data: PathBuf,
}

impl Places {
    fn of_user() -> Option<Places> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())?;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        Some(Places {
            program: home.join(".local/bin").join(BIN_NAME),
            data,
        })
    }

    fn desktop(&self) -> PathBuf {
        self.data
            .join("applications")
            .join(format!("{APP_ID}.desktop"))
    }

    fn icon(&self, side: u32) -> PathBuf {
        self.data
            .join(format!("icons/hicolor/{side}x{side}/apps"))
            .join(format!("{APP_ID}.png"))
    }
}

/// Where the installed program lives.
pub fn installed_path() -> Option<PathBuf> {
    Places::of_user().map(|p| p.program)
}

fn this_exe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| fs::canonicalize(p).ok())
}

/// The menu entry; `exec` is the program it starts.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=WeekNumber\n\
         GenericName=Week Number\n\
         Comment=The ISO week number in the system tray\n\
         Exec={exec}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         Keywords=week;ISO;calendar;date;holiday;time zone;\n\
         StartupWMClass=weeknumber\n\
         StartupNotify=false\n"
    )
}

/// The app icon's PNG frame of exactly `side` px, as shipped.
fn icon_png(side: u32) -> Option<&'static [u8]> {
    crate::png::ico_frames(crate::os::unix::APP_ICO)
        .into_iter()
        .find(|(s, _)| *s == side)
        .map(|(_, png)| png)
}

/// Replace `path` with `data` through a new file in the same folder, so a
/// crash or a symlink planted at `path` can't redirect the write.
fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or(std::io::ErrorKind::NotFound)?;
    let tmp = dir.join(format!(
        ".{}.{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Whether the running program is the installed one.
pub fn is_installed_copy() -> bool {
    this_exe().is_some_and(|e| Some(e) == installed_path())
}

/// The running program as a plain executable: for the AppImage, the part
/// before the appended SquashFS, without the AppImage magic.
fn own_program() -> Option<Vec<u8>> {
    let exe = this_exe()?;
    Some(plain_program(fs::read(exe).ok()?))
}

fn plain_program(mut data: Vec<u8>) -> Vec<u8> {
    if let Some(end) = appimage_offset(&data) {
        data.truncate(end);
        data[8..11].fill(0);
    }
    data
}

const INSTALL_FAILED: &str = "WeekNumber could not be installed in your home folder.";

/// Copy the running program to ~/.local/bin and add the menu entry and the
/// icons. Err carries the message to show.
pub fn install() -> Result<PathBuf, String> {
    let places = Places::of_user().ok_or(INSTALL_FAILED)?;
    let exe = this_exe().ok_or(INSTALL_FAILED)?;
    let program = if exe == places.program {
        None
    } else {
        Some(own_program().ok_or(INSTALL_FAILED)?)
    };
    install_into(&places, program.as_deref())?;
    // Run at Startup follows the program to its new place.
    startup::repoint(&exe, &places.program);
    Ok(places.program)
}

/// Write the program (the executable without the AppImage payload; None
/// keeps the one there), the icons and the menu entry.
fn install_into(places: &Places, program: Option<&[u8]>) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{INSTALL_FAILED}\n\n{e}");
    if let Some(program) = program {
        write_atomic(&places.program, program, 0o755).map_err(fail)?;
    }
    for side in ICON_SIDES {
        if let Some(png) = icon_png(side) {
            write_atomic(&places.icon(side), png, 0o644).map_err(fail)?;
        }
    }
    let entry = desktop_entry(&startup::exec_line(&places.program));
    write_atomic(&places.desktop(), entry.as_bytes(), 0o644).map_err(fail)
}

/// Remove what `install` added (and the autostart entry if it starts the
/// installed program).
pub fn uninstall() -> Result<(), String> {
    let places = Places::of_user().ok_or("WeekNumber could not find your home folder.")?;
    startup::remove_if_starts(&places.program);
    uninstall_from(&places)
}

fn uninstall_from(places: &Places) -> Result<(), String> {
    let mut paths = vec![places.desktop(), places.program.clone()];
    paths.extend(ICON_SIDES.iter().map(|s| places.icon(*s)));
    let failed: Vec<String> = paths
        .iter()
        .filter(|p| fs::remove_file(p).is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound))
        .map(|p| p.display().to_string())
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("Could not remove: {}", failed.join(", ")))
    }
}

fn declined_marker() -> PathBuf {
    crate::os::settings_dir().join(DECLINED)
}

/// Whether to offer installing: the app runs from a place it can't start
/// at login from (Downloads, the desktop, a temporary folder...), as a
/// double-clicked download does. Copies an administrator or an AppImage
/// manager put somewhere safe are left alone.
pub fn worth_installing() -> bool {
    !is_installed_copy() && startup::location_problem(this_exe().as_deref()).is_some()
}

/// Start-up: offer to install, unless the user said "Not Now" before or the
/// installed program is this same build. After installing, the installed
/// program replaces this process.
pub fn offer_at_launch() {
    if std::env::var_os("WEEKNUMBER_NO_INSTALL_PROMPT").is_some()
        || !worth_installing()
        || declined_marker().exists()
    {
        return;
    }
    let same = installed_path()
        .and_then(|p| fs::read(p).ok())
        .zip(own_program())
        .is_some_and(|(a, b)| a == b);
    if same {
        return;
    }
    let text = "Install WeekNumber for your account?\n\n\
                It is copied to ~/.local/bin and added to your applications \
                menu, so it can also start at login. Nothing outside your \
                home folder changes.";
    match crate::os::message_box_choose(text, &["Install", "Not Now"]) {
        Some(0) => offer_accepted(),
        Some(1) => {
            let marker = declined_marker();
            let _ = write_atomic(&marker, b"", 0o600);
        }
        _ => {}
    }
}

/// Install from the menu or the prompt; on success, continue as the
/// installed program.
pub fn offer_accepted() {
    match install() {
        Ok(target) => restart_as(&target),
        Err(e) => crate::os::message_box_error(&e),
    }
}

/// Replace this process with `program` (same process, no child): its
/// descriptors close on exec, so the single-instance lock and the tray
/// icon pass to the new image.
fn restart_as(program: &Path) {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(program)
        .env("WEEKNUMBER_NO_INSTALL_PROMPT", "1")
        .exec();
    crate::os::message_box_error(&format!(
        "WeekNumber was installed but could not be started: {err}"
    ));
}

// ---------------------------------------------------------------------------
// The AppImage side
// ---------------------------------------------------------------------------

/// The end of the ELF image (its section header table), where an
/// AppImage's SquashFS starts -- if `data` carries the AppImage type 2
/// magic ("AI" 2 at offset 8).
pub fn appimage_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 64 || &data[..4] != b"\x7fELF" || data[8..11] != *b"AI\x02" {
        return None;
    }
    // 64-bit little-endian only (x86-64 and AArch64).
    if data[4] != 2 || data[5] != 1 {
        return None;
    }
    let u16_at = |o: usize| usize::from(u16::from_le_bytes([data[o], data[o + 1]]));
    let shoff = u64::from_le_bytes(data[40..48].try_into().ok()?) as usize;
    let end = shoff.checked_add(u16_at(58).checked_mul(u16_at(60))?)?;
    (end <= data.len()).then_some(end)
}

/// The files of the AppDir, as (relative path, contents, mode). AppRun is
/// the program itself, so an extracted AppDir runs on its own.
fn appdir_files(program: &[u8]) -> Vec<(String, Vec<u8>, u32)> {
    let mut files = vec![
        ("AppRun".to_string(), program.to_vec(), 0o755),
        (
            format!("{APP_ID}.desktop"),
            format!("{}X-AppImage-Version=1.0\n", desktop_entry(BIN_NAME)).into_bytes(),
            0o644,
        ),
    ];
    if let Some(png) = icon_png(256) {
        files.push((format!("{APP_ID}.png"), png.to_vec(), 0o644));
        files.push((".DirIcon".to_string(), png.to_vec(), 0o644));
    }
    for side in ICON_SIDES {
        if let Some(png) = icon_png(side) {
            files.push((
                format!("usr/share/icons/hicolor/{side}x{side}/apps/{APP_ID}.png"),
                png.to_vec(),
                0o644,
            ));
        }
    }
    files
}

/// `*` matches any run of characters (the runtime's glob, simplified).
fn glob(pattern: &str, s: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == s,
        Some((head, rest)) => {
            let Some(tail) = s.strip_prefix(head) else {
                return false;
            };
            (0..=tail.len()).any(|i| tail.is_char_boundary(i) && glob(rest, &tail[i..]))
        }
    }
}

/// `--appimage-extract [pattern]`: write the AppDir to ./squashfs-root.
fn extract(pattern: Option<&str>) -> i32 {
    let Some(program) = own_program() else {
        eprintln!("Cannot read the program.");
        return 1;
    };
    let root = Path::new("squashfs-root");
    for (rel, data, mode) in appdir_files(&program) {
        if pattern.is_some_and(|p| !glob(p, &rel)) {
            continue;
        }
        let path = root.join(&rel);
        // Never write through a symlink someone left in the way.
        let mut cur = PathBuf::new();
        for c in Path::new(&rel)
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            cur.push(c);
            if fs::symlink_metadata(root.join(&cur)).is_ok_and(|m| m.file_type().is_symlink()) {
                eprintln!("Refusing to extract through the symlink {}", cur.display());
                return 1;
            }
        }
        if fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            eprintln!("Refusing to extract into a symlinked squashfs-root");
            return 1;
        }
        if let Err(e) = write_atomic(&path, &data, mode) {
            eprintln!("{}: {e}", path.display());
            return 1;
        }
        println!("{}", path.display());
    }
    0
}

/// Handle the command line; Some(exit code) when it was a command, None to
/// start the app.
pub fn command_line() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first()?.as_str();
    let code = match first {
        "--appimage-extract" => extract(args.get(1).map(String::as_str)),
        "--appimage-offset" => {
            let data = this_exe()
                .and_then(|e| fs::read(e).ok())
                .unwrap_or_default();
            match appimage_offset(&data) {
                Some(o) => {
                    println!("{o}");
                    0
                }
                None => {
                    eprintln!("This file is not an AppImage.");
                    1
                }
            }
        }
        "--appimage-version" => {
            println!(
                "WeekNumber {} self-contained AppImage (no FUSE runtime)",
                crate::app::APP_VERSION
            );
            0
        }
        "--appimage-updateinformation" | "--appimage-signature" => 0,
        "--appimage-extract-and-run" => return None,
        "--appimage-mount" => {
            eprintln!("Nothing to mount: WeekNumber is a single self-contained program.");
            1
        }
        "--appimage-help" | "--help" | "-h" => {
            println!(
                "WeekNumber {}: the ISO week number in the system tray.\n\n\
                 Options:\n  \
                 --install                 install for this user (~/.local) and exit\n  \
                 --uninstall               remove that installation and exit\n  \
                 --version                 print the version\n  \
                 --appimage-extract [pat]  write the AppDir to ./squashfs-root\n  \
                 --appimage-offset         print where the AppImage payload starts",
                crate::app::APP_VERSION
            );
            0
        }
        "--version" => {
            println!("WeekNumber {}", crate::app::APP_VERSION);
            0
        }
        "--install" => match install() {
            Ok(p) => {
                println!("Installed {}", p.display());
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        "--uninstall" => match uninstall() {
            Ok(()) => {
                println!("WeekNumber was removed. Its settings stay in ~/.config/weeknumber.");
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf_with_sections(shoff: u64, entsize: u16, num: u16, len: usize) -> Vec<u8> {
        let mut d = vec![0u8; len];
        d[..4].copy_from_slice(b"\x7fELF");
        d[4] = 2;
        d[5] = 1;
        d[40..48].copy_from_slice(&shoff.to_le_bytes());
        d[58..60].copy_from_slice(&entsize.to_le_bytes());
        d[60..62].copy_from_slice(&num.to_le_bytes());
        d
    }

    #[test]
    fn offset_needs_the_magic_and_a_sane_header() {
        let mut d = elf_with_sections(1000, 64, 10, 2000);
        assert_eq!(appimage_offset(&d), None, "no magic");
        d[8..11].copy_from_slice(b"AI\x02");
        assert_eq!(appimage_offset(&d), Some(1640));
        // Section headers past the end of the file.
        let mut short = elf_with_sections(1000, 64, 20, 2000);
        short[8..11].copy_from_slice(b"AI\x02");
        assert_eq!(appimage_offset(&short), None);
        assert_eq!(appimage_offset(b"AI\x02"), None);
        // Installing keeps the ELF part only, without the magic.
        let mut img = d.clone();
        img.extend_from_slice(b"hsqs payload");
        let plain = plain_program(img);
        assert_eq!(plain.len(), 1640);
        assert_eq!(appimage_offset(&plain), None);
        assert_eq!(&plain[8..11], &[0, 0, 0]);
    }

    #[test]
    fn globbing() {
        assert!(glob("*.desktop", &format!("{APP_ID}.desktop")));
        assert!(glob(
            "usr/share/icons/*",
            "usr/share/icons/hicolor/16x16/apps/x.png"
        ));
        assert!(glob("AppRun", "AppRun"));
        assert!(!glob("*.png", "AppRun"));
        assert!(glob("*", ""));
    }

    #[test]
    fn appdir_is_complete() {
        let files = appdir_files(b"prog");
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        assert!(names.contains(&"AppRun"));
        assert!(names.contains(&".DirIcon"));
        assert!(names.contains(&format!("{APP_ID}.desktop").as_str()));
        assert!(names.contains(&format!("{APP_ID}.png").as_str()));
        // Sharp icons: every hicolor size is an exact frame, not a resize.
        for side in ICON_SIDES {
            let png = icon_png(side).expect("frame");
            let img = crate::png::decode(png).expect("decodes");
            assert_eq!((img.w, img.h), (side as usize, side as usize));
        }
        let desktop = &files[1].1;
        let text = std::str::from_utf8(desktop).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains(&format!("Icon={APP_ID}\n")));
        assert!(text.contains("Exec=weeknumber\n"));
    }

    #[test]
    fn install_and_uninstall_in_a_scratch_home() {
        let dir = std::env::temp_dir().join(format!("wn-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let places = Places {
            program: dir.join(".local/bin/weeknumber"),
            data: dir.join(".local/share"),
        };
        install_into(&places, Some(b"#!/bin/sh\n")).expect("installs");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&places.program).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        let entry = fs::read_to_string(places.desktop()).unwrap();
        assert!(entry.contains(&format!("Exec=\"{}\"", places.program.display())));
        for side in ICON_SIDES {
            assert!(places.icon(side).is_file(), "{side}");
        }
        // Re-installing over a planted symlink replaces the link, not its target.
        let victim = dir.join("victim");
        fs::write(&victim, b"keep").unwrap();
        fs::remove_file(places.desktop()).unwrap();
        std::os::unix::fs::symlink(&victim, places.desktop()).unwrap();
        install_into(&places, None).expect("reinstalls");
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        uninstall_from(&places).expect("uninstalls");
        assert!(!places.program.exists());
        assert!(!places.desktop().exists());
        assert!(!places.icon(16).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

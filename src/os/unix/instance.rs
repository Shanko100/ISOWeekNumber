//! One running copy per user and install, with hand-over: the copy that
//! holds an exclusive lock on a file in a private (0700) folder listens on
//! a Unix socket beside it; a newer launch asks it to quit there, waits for
//! the lock, then takes over.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Wait up to 5 s for the running copy to quit, checking every 100 ms.
const HANDOVER_TIMEOUT_MS: u64 = 5000;
const HANDOVER_POLL_MS: u64 = 100;
const QUIT: &[u8] = b"quit\n";
/// Longest socket path every Unix takes (sun_path holds 104 bytes on macOS,
/// 108 on Linux, with the terminating NUL).
const MAX_SOCKET_PATH: usize = 100;

pub struct Instance {
    _lock: Option<File>,
    listener: Option<UnixListener>,
    socket: Option<PathBuf>,
}

/// A folder only this user can enter: created 0700, or an existing one
/// owned by this user with no group or world access.
pub fn private_dir(p: &Path) -> Option<PathBuf> {
    if !p.is_absolute() {
        return None;
    }
    if !p.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(p)
            .ok()?;
    }
    let meta = std::fs::symlink_metadata(p).ok()?;
    let uid = super::sys::uid();
    (meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o077 == 0)
        .then(|| p.to_path_buf())
}

impl Instance {
    /// Claim the instance in `dir` under `tag`; None when a running copy
    /// did not hand over in time (`on_timeout` has told the user).
    pub fn claim_in(
        dir: Option<PathBuf>,
        tag: &str,
        on_timeout: impl FnOnce(),
    ) -> Option<Instance> {
        // Fail open: without a private folder the app still runs.
        let Some(dir) = dir else {
            return Some(Instance {
                _lock: None,
                listener: None,
                socket: None,
            });
        };
        // A socket path too long to bind would leave the running copy deaf
        // to hand-over requests: use a short private folder instead.
        let dir = if dir.join(socket_name(tag)).as_os_str().len() > MAX_SOCKET_PATH {
            match short_dir() {
                Some(d) => d,
                None => {
                    return Some(Instance {
                        _lock: None,
                        listener: None,
                        socket: None,
                    });
                }
            }
        } else {
            dir
        };
        let lock_path = dir.join(format!("weeknumber-{tag}.lock"));
        let socket = dir.join(socket_name(tag));
        let Ok(lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
        else {
            return Some(Instance {
                _lock: None,
                listener: None,
                socket: None,
            });
        };
        if lock.try_lock().is_err() {
            if let Ok(mut s) = UnixStream::connect(&socket) {
                let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
                let _ = s.write_all(QUIT);
            }
            let start = super::core::tick_ms();
            loop {
                std::thread::sleep(Duration::from_millis(HANDOVER_POLL_MS));
                if lock.try_lock().is_ok() {
                    break;
                }
                if super::core::tick_ms() - start >= HANDOVER_TIMEOUT_MS {
                    on_timeout();
                    return None;
                }
            }
        }
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).ok();
        if let Some(l) = &listener {
            let _ = l.set_nonblocking(true);
            let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
        }
        Some(Instance {
            _lock: Some(lock),
            listener,
            socket: Some(socket),
        })
    }

    /// The socket to watch for hand-over requests (for poll).
    pub fn listener(&self) -> Option<&UnixListener> {
        self.listener.as_ref()
    }

    /// Accept waiting connections; true when one asked this copy to quit.
    pub fn quit_requested(&self) -> bool {
        let Some(l) = &self.listener else {
            return false;
        };
        let mut asked = false;
        while let Ok((mut s, _)) = l.accept() {
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            let mut buf = [0u8; 8];
            let mut got = 0;
            while got < QUIT.len() {
                match s.read(&mut buf[got..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got += n,
                }
            }
            asked |= &buf[..got] == QUIT;
        }
        asked
    }
}

fn socket_name(tag: &str) -> String {
    format!("weeknumber-{tag}.sock")
}

/// /tmp/weeknumber-<uid>, private to this user (a folder someone else
/// made there is refused, and the app then runs without hand-over).
fn short_dir() -> Option<PathBuf> {
    private_dir(&Path::new("/tmp").join(format!("weeknumber-{}", super::sys::uid())))
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.listener.take().is_some()
            && let Some(s) = &self.socket
        {
            let _ = std::fs::remove_file(s);
        }
    }
}

/// The per-user, per-install tag: SHA-256 of user, home and executable.
pub fn tag() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let seed = [
        std::env::var("USER").unwrap_or_default(),
        std::env::var("HOME").unwrap_or_default(),
        exe,
    ]
    .join("|");
    crate::sha256::hex_prefix(seed.as_bytes(), 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_over() {
        let base = std::env::temp_dir().join(format!("wn-inst-{}", std::process::id()));
        let dir = private_dir(&base).expect("private dir");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let first = Instance::claim_in(Some(dir.clone()), "t", || {}).unwrap();
        assert!(first.listener().is_some());
        assert!(!first.quit_requested());
        // A second claim asks the first to quit; the first answers by
        // dropping its claim on another thread, as the app would.
        let handle = std::thread::spawn(move || {
            for _ in 0..50 {
                if first.quit_requested() {
                    drop(first);
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        });
        let second = Instance::claim_in(Some(dir.clone()), "t", || panic!("timed out"));
        assert!(handle.join().unwrap());
        assert!(second.is_some());
        drop(second);
        // A folder others can enter is refused.
        let open = base.join("open");
        std::fs::create_dir_all(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&open).is_none());
        assert!(private_dir(Path::new("relative")).is_none());
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(tag().len(), 16);
        // A folder whose socket path would be too long to bind: the claim
        // moves to the short private folder and still hands over.
        let deep = base.join("d".repeat(120));
        let deep = private_dir(&deep).expect("deep dir");
        let a = Instance::claim_in(Some(deep.clone()), "deep", || {}).unwrap();
        let sock = a.socket.clone().expect("socket");
        assert!(
            sock.as_os_str().len() <= MAX_SOCKET_PATH,
            "{}",
            sock.display()
        );
        assert!(a.listener().is_some());
        drop(a);
        let _ = std::fs::remove_dir_all(&base);
    }
}

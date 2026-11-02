//! The few C library calls std does not wrap, shared by macOS and Linux.
//! Like the other backend files, the only place `unsafe` is allowed: each
//! block is one documented call with arguments this module checks.
#![allow(unsafe_code)]

use std::os::fd::RawFd;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 1;
#[cfg(target_os = "linux")]
const POLLOUT: i16 = 4;

unsafe extern "C" {
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
    fn getuid() -> u32;
}

/// The real user id of this process.
pub fn uid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { getuid() }
}

/// Wait until one of `fds` is readable or `timeout_ms` passes (-1: no
/// limit). Returns the readable flags, one per fd.
pub fn wait_readable(fds: &[RawFd], timeout_ms: i32) -> Vec<bool> {
    let mut p: Vec<PollFd> = fds
        .iter()
        .map(|&fd| PollFd {
            fd,
            events: POLLIN,
            revents: 0,
        })
        .collect();
    // SAFETY: `p` is a valid array of `p.len()` pollfd structures that
    // lives for the whole call.
    let n = unsafe { poll(p.as_mut_ptr(), p.len() as std::ffi::c_ulong, timeout_ms) };
    if n <= 0 {
        return vec![false; fds.len()];
    }
    p.iter().map(|f| f.revents != 0).collect()
}

/// Wait until `fd` can take more output, at most `timeout_ms`.
#[cfg(target_os = "linux")]
pub fn wait_writable(fd: RawFd, timeout_ms: i32) {
    let mut p = [PollFd {
        fd,
        events: POLLOUT,
        revents: 0,
    }];
    // SAFETY: one valid pollfd for the duration of the call.
    unsafe {
        poll(p.as_mut_ptr(), 1, timeout_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;

    #[test]
    fn poll_and_uid() {
        let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(wait_readable(&[b.as_raw_fd()], 0), vec![false]);
        a.write_all(b"x").unwrap();
        assert_eq!(wait_readable(&[b.as_raw_fd()], 1000), vec![true]);
        let me = std::fs::metadata(std::env::temp_dir()).is_ok();
        assert!(me);
        let f = std::env::temp_dir().join(format!("wn-uid-{}", std::process::id()));
        std::fs::write(&f, b"").unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&f).unwrap().uid(), uid());
        let _ = std::fs::remove_file(f);
    }
}

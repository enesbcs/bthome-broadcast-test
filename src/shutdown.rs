//! Shutdown signalling.
//!
//! The main loop sleeps in short slices and checks [`requested`], so a signal
//! is handled within 100 ms even when the sleep is restarted after `EINTR`.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Installs the platform signal handlers. Idempotent.
pub fn install() -> io::Result<()> {
    imp::install()
}

/// Whether a shutdown was requested.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// Requests shutdown.
///
/// The console signal handler sets the flag directly; the Windows GUI calls
/// this when the user closes the window or clicks Quit.
pub fn request() {
    REQUESTED.store(true, Ordering::SeqCst);
}

/// Clears the flag. Tests only.
#[cfg(test)]
pub(crate) fn reset() {
    REQUESTED.store(false, Ordering::SeqCst);
}

#[cfg(unix)]
mod imp {
    use super::REQUESTED;
    use std::io;
    use std::sync::atomic::Ordering;

    extern "C" fn handler(_signal: libc::c_int) {
        REQUESTED.store(true, Ordering::SeqCst);
    }

    pub fn install() -> io::Result<()> {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            // No SA_RESTART: blocking calls should return so the loop can exit.
            action.sa_flags = 0;

            if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::REQUESTED;
    use std::ffi::c_void;
    use std::io;
    use std::sync::atomic::Ordering;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    unsafe extern "system" fn handler(_ctrl_type: u32) -> i32 {
        REQUESTED.store(true, Ordering::SeqCst);
        1
    }

    pub fn install() -> io::Result<()> {
        let ok = unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let _: *const c_void = std::ptr::null();
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::io;

    pub fn install() -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_sets_the_flag() {
        reset();
        assert!(!requested());
        request();
        assert!(requested());
        reset();
    }
}

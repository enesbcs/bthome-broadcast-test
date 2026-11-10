//! Single-instance guard.
//!
//! Linux/macOS: an abstract Unix domain socket. Binding the same abstract
//! name twice fails with `EADDRINUSE`; the name is released automatically when
//! the process exits, so no stale lock file is left behind.
//!
//! Windows: a named mutex (`Local\...`), released on process exit.

use std::fmt;

/// Instance lock errors.
#[derive(Debug, PartialEq, Eq)]
pub enum LockError {
    AlreadyRunning,
    Io(String),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::AlreadyRunning => write!(f, "The program is already running."),
            LockError::Io(message) => write!(f, "Single-instance lock error: {message}"),
        }
    }
}

/// Default abstract socket name / mutex name.
pub const INSTANCE_NAME: &str = "bthome-broadcast-test";

/// Holds the single-instance lock until dropped.
pub struct InstanceLock {
    #[cfg(unix)]
    fd: libc::c_int,
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
}

impl InstanceLock {
    /// Acquires the lock; fails with [`LockError::AlreadyRunning`] if another
    /// instance holds it.
    pub fn acquire(name: &str) -> Result<Self, LockError> {
        imp::acquire(name)
    }
}

impl fmt::Debug for InstanceLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceLock").finish_non_exhaustive()
    }
}

#[cfg(unix)]
mod imp {
    use super::{InstanceLock, LockError};

    pub fn acquire(name: &str) -> Result<InstanceLock, LockError> {
        let bytes = name.as_bytes();
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;

        // Abstract namespace: sun_path[0] == 0, the name follows it.
        if bytes.len() + 1 > address.sun_path.len() {
            return Err(LockError::Io("the lock name is too long".to_string()));
        }
        address.sun_path[0] = 0;
        for (index, byte) in bytes.iter().enumerate() {
            address.sun_path[index + 1] = *byte as libc::c_char;
        }

        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(LockError::Io(std::io::Error::last_os_error().to_string()));
        }

        let length =
            (std::mem::size_of::<libc::sa_family_t>() + 1 + bytes.len()) as libc::socklen_t;
        let result = unsafe {
            libc::bind(
                fd,
                &address as *const libc::sockaddr_un as *const libc::sockaddr,
                length,
            )
        };

        if result != 0 {
            let error = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            if error.raw_os_error() == Some(libc::EADDRINUSE) {
                return Err(LockError::AlreadyRunning);
            }
            return Err(LockError::Io(error.to_string()));
        }

        // The kernel keeps the abstract name alive together with the socket;
        // closing the file descriptor releases it.
        Ok(InstanceLock { fd })
    }

    impl Drop for InstanceLock {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{InstanceLock, LockError};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;

    pub fn acquire(name: &str) -> Result<InstanceLock, LockError> {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(LockError::Io(std::io::Error::last_os_error().to_string()));
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return Err(LockError::AlreadyRunning);
        }

        Ok(InstanceLock { handle })
    }

    impl Drop for InstanceLock {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::{InstanceLock, LockError};

    pub fn acquire(_name: &str) -> Result<InstanceLock, LockError> {
        Err(LockError::Io(
            "single-instance locking is not supported on this platform".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_held() {
        let name = format!("{INSTANCE_NAME}-test-{}", std::process::id());
        let first = InstanceLock::acquire(&name).expect("first lock");
        assert_eq!(
            InstanceLock::acquire(&name).unwrap_err(),
            LockError::AlreadyRunning
        );
        drop(first);
        // After dropping, the name is free again.
        let _second = InstanceLock::acquire(&name).expect("lock after drop");
    }
}

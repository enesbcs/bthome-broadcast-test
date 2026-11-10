//! Minimal runtime DLL loader shared by the AMD and Intel providers.
//!
//! The vendor SDKs are not linked at build time. The provider DLLs are loaded
//! from the normal Windows search path only when the rung is probed.

use std::ffi::CString;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// An owned DLL handle.
pub struct Dll {
    handle: HMODULE,
}

impl Dll {
    /// Loads a DLL by name from the Windows search path.
    pub fn load(name: &str) -> Result<Self, String> {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        let handle = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }
            .map_err(|error| format!("cannot load {name}: {error}"))?;
        Ok(Self { handle })
    }

    /// Resolves one exported symbol.
    ///
    /// # Safety
    ///
    /// The caller must use the correct function pointer type for the symbol.
    pub unsafe fn symbol<T: Copy>(&self, name: &str) -> Result<T, String> {
        let cname = CString::new(name).map_err(|_| format!("invalid symbol name: {name}"))?;
        let proc = unsafe { GetProcAddress(self.handle, PCSTR(cname.as_ptr() as *const u8)) }
            .ok_or_else(|| format!("symbol {name} not found"))?;
        if std::mem::size_of::<T>() != std::mem::size_of_val(&proc) {
            return Err(format!("symbol {name} has an unexpected size"));
        }
        Ok(unsafe { std::mem::transmute_copy(&proc) })
    }
}

impl Drop for Dll {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.handle);
        }
    }
}

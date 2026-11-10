//! Console support for the GUI-subsystem Windows executable.
//!
//! The release build is a `WINDOWS_GUI` binary so that a double click starts
//! the graphical window without a console flash. Text modes (`--console`,
//! `--help`, `--version`, `--dry-run`, `--diagnose`) attach to the console of
//! the calling terminal, or create a new console when started from Explorer.

use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};

/// Whether the command line asks for a text-mode console.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let flag = arg.split('=').next().unwrap_or(arg);
        matches!(
            flag,
            "--console" | "--help" | "-h" | "--version" | "-V" | "--dry-run" | "--diagnose"
        )
    })
}

/// Attaches to the parent console, or allocates a new one.
///
/// This must run before the first write to stdout or stderr, because the Rust
/// standard library resolves the Windows standard handles on first use.
pub fn attach() {
    unsafe {
        // A redirected stdout (for example `app --version > file`) must not be
        // replaced by a fresh console, so only allocate when there is neither a
        // parent console nor an inherited standard handle.
        let inherited_stdout = GetStdHandle(STD_OUTPUT_HANDLE).is_ok();
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() && !inherited_stdout {
            let _ = AllocConsole();
        }
    }
}

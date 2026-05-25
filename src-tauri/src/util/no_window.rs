// SPDX-License-Identifier: Apache-2.0
//! Hide the console window of child processes on Windows.
//!
//! Without this, every `Command::new("powershell")` / `Command::new("git")`
//! / etc. flashes a black console window before the bound stdio takes
//! over — the user sees a brief popup on every AI tool call and every
//! auto-checkpoint write. The flag we need is `CREATE_NO_WINDOW`
//! (0x08000000) on `CreationFlags`.
//!
//! The trait is a no-op on non-Windows targets so call sites can use it
//! unconditionally without `#[cfg(windows)]` boilerplate.

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub trait NoWindow {
    /// Apply `CREATE_NO_WINDOW` to the child process on Windows. Returns
    /// the receiver for chaining (`Command::new(...).no_window().arg(...)`).
    fn no_window(self) -> Self;
}

#[cfg(windows)]
impl NoWindow for std::process::Command {
    fn no_window(mut self) -> Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(not(windows))]
impl NoWindow for std::process::Command {
    fn no_window(self) -> Self {
        self
    }
}

#[cfg(windows)]
impl NoWindow for tokio::process::Command {
    fn no_window(mut self) -> Self {
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(not(windows))]
impl NoWindow for tokio::process::Command {
    fn no_window(self) -> Self {
        self
    }
}

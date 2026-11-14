//! Windows-specific process creation helpers.
//!
//! Several places spawn a child process that must not flash a console
//! window. The flag and its `cfg` gate lived in five copies; keeping one
//! means changing the launch policy is a one-line edit.

/// Apply the platform-specific "no console window" flag to `cmd`.
///
/// On Windows this sets `CREATE_NO_WINDOW`; on every other platform it is a
/// no-op so callers do not need their own `#[cfg]` block.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
pub fn hide_console_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_console_window(_cmd: &mut std::process::Command) {}
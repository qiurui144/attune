//! Child-process helpers shared by desktop/server runtime paths.

use std::ffi::OsStr;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a std child process command without flashing a console window on Windows.
#[cfg(windows)]
pub fn command_no_window<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Build a std child process command without flashing a console window on Windows.
#[cfg(not(windows))]
pub fn command_no_window<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    std::process::Command::new(program)
}

/// Build a Tokio child process command without flashing a console window on Windows.
#[cfg(windows)]
pub fn tokio_command_no_window<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Build a Tokio child process command without flashing a console window on Windows.
#[cfg(not(windows))]
pub fn tokio_command_no_window<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    tokio::process::Command::new(program)
}

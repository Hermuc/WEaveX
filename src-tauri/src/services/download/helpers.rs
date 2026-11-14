//! Shared helpers for the download manager: command construction
//! and the terminal-status emitter.
//!
//! Split out of `download.rs` so the manager file reads as orchestration.

use std::process::Stdio;

use tauri::{AppHandle, Emitter};
use tokio::process::Command;

use crate::constants::STEAM_APP_ID;

use super::{DownloadCompleted, Phase, TaskStatus};


pub(super) fn build_depot_cmd(
    plugin_path: &std::path::Path,
    pubfileid: &str,
    username: &str,
    password: &str,
    output_dir: &str,
    dotnet_root: Option<&std::path::PathBuf>,
) -> Command {
    let login_id: u32 = rand::random();
    let mut cmd = Command::new(plugin_path);
    cmd.args([
        "-app",
        STEAM_APP_ID,
        "-pubfile",
        pubfileid,
        "-verify-all",
        "-username",
        username,
        "-password",
        password,
        "-remember-password",
        "-loginid",
        &login_id.to_string(),
        "-max-downloads",
        "32",
        "-dir",
    ])
    .arg(output_dir)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);

    if let Some(root) = dotnet_root {
        cmd.env("DOTNET_ROOT", root);
    }

    #[cfg(windows)]
    {
        cmd.creation_flags(crate::core::platform::CREATE_NO_WINDOW);
    }

    cmd
}


/// Emit a terminal status for one task and its matching `download://completed`
/// event.
///
/// The cancel, success and fail paths each built a `TaskStatus` and then the
/// same `DownloadCompleted` event, differing only in phase/text/success. One
/// helper keeps the two events in lockstep — a caller that forgot the second
/// emit would leave the frontend waiting forever.
pub(super) fn emit_terminal(
    app: &AppHandle,
    pubfileid: &str,
    account: &str,
    status: String,
    phase: Phase,
    progress: Option<f32>,
    success: bool,
) {
    let final_status = TaskStatus {
        pubfileid: pubfileid.to_string(),
        status,
        account: account.to_string(),
        phase,
        progress,
    };
    app.emit("download://status", &final_status).ok();
    app.emit(
        "download://completed",
        DownloadCompleted {
            pubfileid: pubfileid.to_string(),
            success,
        },
    )
    .ok();
}


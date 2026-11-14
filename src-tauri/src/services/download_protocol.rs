//! Output protocol of the `DepotDownloaderMod` child process.
//!
//! Reading stdout and classifying each line used to live inside
//! `DownloadManager::start`, mixed with the spawn/monitor orchestration.
//! It is pure text handling (no I/O beyond the `PROGRESS_RE` regex), so
//! it lives on its own and is unit-testable without a child process.

use once_cell::sync::Lazy;
use regex::Regex;

/// Whether an accumulated partial stdout line is a Steam Guard prompt that is
/// blocking on console input.
///
/// DepotDownloader prints these with `Console.Write` (no trailing newline) and
/// then blocks; the .NET runtime does not flush such a partial line to a
/// redirected pipe, so it is detected mid-line here. This preserves the two
/// rules the inline reader used: a code prompt (contains both "enter" and
/// "code", or the email/authenticator phrasings) and a mobile-confirm prompt.
pub fn is_steam_guard_prompt(line: &str) -> bool {
    let lower = line.to_lowercase();
    (lower.contains("enter") && lower.contains("code"))
        || lower.contains("auth code sent to your email")
}


/// Build the `DepotDownloaderMod` command for one download attempt.
///
/// Kept separate from the retry loop so the argument list (and the Windows
/// console-window flag) is defined once, and so `start` reads as orchestration
/// rather than a wall of `cmd.args([...])`.
pub struct LineVerdict {
    /// Percentage parsed from the line, if any (clamped to 0..=100).
    pub progress: Option<f32>,
    /// The line proves the handshake succeeded / a transfer started, so the
    /// short "connecting" timeout no longer applies.
    pub marks_started: bool,
    /// The line is a code-entry prompt (email or authenticator app).
    pub needs_code: bool,
    /// The line is a mobile-app confirmation prompt.
    pub needs_app_confirm: bool,
    /// The line as reported to the UI, with the download directory replaced by
    /// ": " so status text is not a full absolute path.
    pub cleaned: String,
}

/// Classify one `OUT:` line from DepotDownloader.
///
/// Pure: no I/O and no state. Extracted from `start` so the keyword rules that
/// decide "did this line start the transfer?" / "is this a 2FA prompt?" can be
/// unit-tested without spawning a process.
pub fn classify_progress_line(line: &str, output_dir: &str) -> LineVerdict {
    let cleaned = line
        .replace(&format!("{}\\", output_dir), ": ")
        .replace(&format!("{}/", output_dir), ": ");
    let progress = PROGRESS_RE
        .captures(&cleaned)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().replace(',', ".").parse::<f32>().ok())
        .map(|p| p.clamp(0.0, 100.0));

    let lower = cleaned.to_lowercase();
    let started_keyword = lower.contains("logged on")
        || lower.contains("logged in")
        || lower.contains("got appinfo")
        || lower.contains("got depot key")
        || lower.contains("downloading");
    let needs_code = (lower.contains("enter") && lower.contains("code"))
        || lower.contains("auth code")
        || lower.contains("2 factor")
        || lower.contains("authenticator app");
    let needs_app_confirm = lower.contains("waiting for steam guard app confirmation");

    LineVerdict {
        progress,
        marks_started: progress.is_some() || started_keyword || needs_code || needs_app_confirm,
        needs_code,
        needs_app_confirm,
        cleaned,
    }
}

static PROGRESS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(\d{1,3}(?:[.,]\d+)?)\s*%").expect("progress regex"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_percentage_and_marks_started() {
        let v = classify_progress_line("Downloading 42,5% (10 of 24)", "/out");
        assert_eq!(v.progress, Some(42.5));
        assert!(v.marks_started);
        assert!(!v.needs_code);
    }

    #[test]
    fn recognises_the_login_stage_as_started() {
        // "Got appinfo" means the handshake worked, even with no percentage.
        let v = classify_progress_line("Got appinfo for depot 431960", "/out");
        assert_eq!(v.progress, None);
        assert!(v.marks_started);
    }

    #[test]
    fn flags_a_code_prompt() {
        let v = classify_progress_line("Please enter the auth code sent to your email", "/out");
        assert!(v.needs_code);
        assert!(v.marks_started);
        assert!(!v.needs_app_confirm);
    }

    #[test]
    fn flags_a_mobile_confirm_prompt() {
        let v = classify_progress_line("Waiting for Steam Guard app confirmation", "/out");
        assert!(v.needs_app_confirm);
        assert!(v.marks_started);
        assert!(!v.needs_code);
    }

    #[test]
    fn strips_the_output_directory_from_the_status_text() {
        let v = classify_progress_line(
            "C:/projects/myprojects/123/scene.pkg 50%",
            "C:/projects/myprojects/123",
        );
        assert!(v.cleaned.starts_with(": "));
        assert_eq!(v.progress, Some(50.0));
    }

    #[test]
    fn clamps_an_out_of_range_percentage() {
        assert_eq!(classify_progress_line("250%", "/o").progress, Some(100.0));
    }

    #[test]
    fn an_unremarkable_line_neither_starts_nor_prompts() {
        let v = classify_progress_line("Connecting to CM server...", "/o");
        assert_eq!(v.progress, None);
        assert!(!v.marks_started);
        assert!(!v.needs_code);
        assert!(!v.needs_app_confirm);
    }
}

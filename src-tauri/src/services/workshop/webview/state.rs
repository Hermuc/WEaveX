//! Login-state predicates and the polling loop shared by the profile and
//! login flows.
//!
//! Split out of `webview.rs`; these read the WebView cookie store and the
//! URL fragment only.

use tauri::Manager;

use tokio::time::sleep;

use std::time::Duration;

use super::{SteamWebview, LABEL};

impl SteamWebview {

    /// True if the webview believes it's logged in (i.e. has a
    /// `steamLoginSecure` cookie).
    ///
    /// Ensures the hidden webview window exists before reading cookies.
    /// Without this, calling `is_logged_in` before the window has been
    /// created (e.g. opening Settings shortly after startup, before the
    /// background `init_cookies` thread has instantiated the window) always
    /// returned `false` even though a valid session was persisted on disk by
    /// WebView2 — which is why the status flipped to "Signed in" only after
    /// the user pressed "Open Parser" and incidentally created the window.
    /// Poll `is_logged_in_inner` until it matches `want` or the budget runs out.
    ///
    /// Three near-identical loops used to live in `ensure_profile_loaded`,
    /// `logout_locked` and `is_logged_in`; they differed only in tick count,
    /// interval and the expected polarity. Everything else — the check, the
    /// sleep, the early return — was the same, so it lives here.
    pub(super) async fn wait_for_login(&self, want: bool, ticks: u32, interval_ms: u64) -> bool {
        for _ in 0..ticks {
            if self.is_logged_in_inner() == want {
                return true;
            }
            sleep(Duration::from_millis(interval_ms)).await;
        }
        false
    }

    /// Whether the webview currently holds a valid `steamLoginSecure` cookie.
    pub async fn is_logged_in(&self) -> bool {
        let _g = self.lock.lock().await;
        // Never create the window to answer this question.
        if !self.window_exists() {
            return false;
        }
        // The window already exists (checked above), so the cookie jar is
        // hydrated and the answer is immediate.
        self.is_logged_in_inner()
    }

    /// Read the verdict written by the form probe into the URL fragment.
    ///
    /// `WebviewWindow::eval` returns `Result<()>` — it never yields the JS
    /// value — so a probe cannot "return" anything. Writing the verdict into the
    /// URL fragment and reading `url()` back is the reliable round-trip.
    pub(super) fn read_form_flag(w: &tauri::WebviewWindow) -> bool {
        w.url()
            .ok()
            .and_then(|u| u.fragment().map(|f| f.to_string()))
            .map(|f| f.contains("WE_FORM:1"))
            .unwrap_or(false)
    }

    pub(super) fn is_logged_in_inner(&self) -> bool {
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return false;
        };
        let Ok(cookies) = w.cookies() else {
            return false;
        };
        cookies
            .iter()
            .any(|c| c.name() == "steamLoginSecure" && !c.value().is_empty())
    }
}

//! Hidden Steam WebView: keeps a persistent browser profile (matches the
//! Python app's `QWebEngineProfile` with persistent cookies) so that the
//! user can log in to Steam once, and the authenticated cookies are reused
//! for reqwest-based Workshop scraping on every subsequent run.
//!
//! Ports of:
//!   - `infrastructure/steam/workshop_scripts.py::login_form_fill_script`
//!   - `ui/tabs/workshop_tab.py::_handle_load_finished` (auto-login driver)
//!
//! The webview is created invisible and remains invisible while we drive the
//! login form in JavaScript; only when auto-login fails do we prompt the
//! user to click "Open Steam login" and show the window so they can log in
//! (or answer a Steam Guard challenge) manually.

mod scripts;

use scripts::build_login_script;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::cookie::Jar;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio::time::sleep;

pub const LABEL: &str = "steam-webview";
/// Login page we drive.
///
/// NOTE: `steamcommunity.com/login/home/` used to host a server-rendered
/// `<form name="logon">`. Steam has since replaced it with a React
/// "featuretarget" placeholder that is mounted by a separate SPA bundle
/// (`applications/community/libraries.js`). That bundle fails to load in some
/// networks/WebView2 profiles, leaving the container permanently empty — the
/// page then renders only the header and the account-creation footer, which
/// is exactly the blank shell users were seeing. `store.steampowered.com/login`
/// is still a fully server-rendered form (username / password / sign-in
/// button), so it works regardless of the SPA bundle.
const STEAM_LOGIN_URL: &str =
    "https://store.steampowered.com/login/?redir=workshop%2Fbrowse%2F%3Fappid%3D431960";
const STEAM_HOME_URL: &str = "https://steamcommunity.com/workshop/browse/?appid=431960";

// Timing constants for the Steam login flow. Each is a separate named value
// rather than a literal buried in a loop: the numbers differ because the
// waits are for different events (a page paint, a form render, a cookie
// arriving), and naming them says which event each one waits for.
/// Let WebView2 flush its cookie store after a logout wipe.
const LOGOUT_FLUSH_WAIT: Duration = Duration::from_millis(1500);
/// Poll interval while waiting for the login form to render.
pub(super) const FORM_RENDER_POLL: Duration = Duration::from_millis(500);
/// Settle time after an eval that checks for the form.
pub(super) const FORM_CHECK_SETTLE: Duration = Duration::from_millis(250);
/// Wait after a navigation that failed to land on the form.
pub(super) const NAV_RETRY_WAIT: Duration = Duration::from_millis(800);
/// Poll interval while the credential form paints (30 ticks ~= 18s).
pub(super) const FORM_PAINT_POLL: Duration = Duration::from_millis(600);
/// Time allowed for the login document to paint after navigation.
pub(super) const LOGIN_PAGE_PAINT_WAIT: Duration = Duration::from_millis(3000);
/// Pause between form-submit attempts.
pub(super) const SUBMIT_RETRY_WAIT: Duration = Duration::from_millis(800);
/// Poll interval while waiting for the auth cookie (40 ticks ~= 40s).
pub(super) const AUTH_COOKIE_POLL: Duration = Duration::from_millis(1000);
/// Wait for the community page to settle before re-checking the session.
pub(super) const COMMUNITY_PAGE_SETTLE: Duration = Duration::from_millis(1500);
/// Poll interval while the community page loads (30 ticks ~= 12s).
pub(super) const COMMUNITY_LOAD_POLL: Duration = Duration::from_millis(400);
/// Let the window paint before it is shown to the user.
const WINDOW_SHOW_SETTLE: Duration = Duration::from_millis(600);


pub struct SteamWebview {
    app: AppHandle,
    data_dir: PathBuf,
    jar: Arc<Jar>,
    lock: Mutex<()>,
    /// One-shot guard for `ensure_community_session`. See that method for why
    /// repeating the attempt is actively harmful.
    community_attempted: std::sync::atomic::AtomicBool,
}

/// Write the session cookies for a window, on both Steam domains.
///
/// Shared by the QR and saved-session paths so the HttpOnly / secure /
/// same-site flags cannot drift between them: a mismatch here silently drops
/// the whole login with no error from Steam.
pub fn install_cookies_for_window(
    w: &tauri::WebviewWindow,
    secure: &str,
    session_id: &str,
) -> Result<(), String> {
    for domain in ["steamcommunity.com", "steampowered.com"] {
        for (name, value) in [
            ("steamLoginSecure", secure),
            ("sessionid", session_id),
        ] {
            let cookie = tauri::webview::Cookie::build((name, value))
                .domain(domain)
                .path("/")
                .secure(true)
                .http_only(true)
                .build();
            w.set_cookie(cookie).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

mod cookies;
mod login;
mod state;
mod window;

impl SteamWebview {
    /// Make sure the hidden webview exists and is sitting on the login page.
    ///
    /// This used to call `clear_all_browsing_data()` first, which wiped the
    /// persisted WebView2 profile — including a perfectly good
    /// `steamLoginSecure` cookie — every single time the login dialog opened.
    /// That made "Relogin" log itself out before it even tried, and made a
    /// successful manual login vanish on the next open.
    /// We now only navigate; clearing the profile is an explicit, opt-in
    /// action (see `force` in `auto_login`).
    pub async fn prepare_login(&self) -> Result<(), tauri::Error> {
        let _g = self.lock.lock().await;
        // Do not create the window just because the login dialog opened.
        if !self.window_exists() {
            return Ok(());
        }
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Ok(());
        };

        // Never wipe the profile here.
        //
        // This used to call `logout_locked()` unconditionally, which runs
        // `clear_all_browsing_data()` and therefore destroys the persisted
        // `steamLoginSecure` cookie. Because the login dialog opens
        // automatically whenever a startup session check fails (a network blip
        // is enough), that turned a transient failure into a permanent
        // sign-out: the user appeared to "randomly lose the login".
        //
        // Merely navigating to the login URL is enough for the form to appear;
        // if a session is still active Steam redirects to the signed-in page,
        // and `login_fill` handles that case by signing out deliberately (a
        // real user-initiated action).
        w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))?;
        Ok(())
    }

    /// Navigate away from the current Steam session **without** taking the
    /// webview lock (callers already hold it).
    ///
    /// Why this exists: when a session is already established Steam renders the
    /// logged-in page (or a Family View PIN prompt) at the login URL instead of
    /// a login form. The scripted form fill then finds no `input[type=password]`
    /// and silently does nothing — the UI just spins and finally reports an
    /// error. Logging out first is what makes the form appear at all.
    async fn logout_locked(&self) -> Result<(), String> {
        // Do NOT gate this on `is_logged_in_inner()`.
        //
        // That check only looks for the `steamLoginSecure` cookie. The webview
        // can still be stuck on a signed-in *page* without it: a Family View
        // PIN prompt, a Steam support redirect, a stale parser session for the
        // bundled public account… In exactly those cases the early return left
        // the old state in place, the navigation never produced a login form,
        // and whatever the user typed was silently dropped. A user who pressed
        // "log in" and submitted credentials has explicitly asked for a fresh
        // session, so wiping the browsing data here is both safe and required.
        log::info!("[steam-webview] resetting webview state before login");

        // Keep a copy first: the clear below is irreversible.
        self.backup_cookie_db();

        // `steamLoginSecure` is HttpOnly, so `document.cookie = ...; Max-Age=0`
        // cannot remove it — the JS deletion attempt is a no-op for exactly the
        // cookie that matters. The only reliable way to drop it from WebView2 is
        // to clear the browsing data store, which is what this does. That is
        // safe here because we are *about to* sign in anyway; the earlier bug
        // was calling it unconditionally before checking whether a session even
        // existed (which logged the user out on every dialog open).
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Err("No window".into());
        };
        w.clear_all_browsing_data().map_err(|e| e.to_string())?;

        // Give WebView2 time to actually flush the store. The cookie check alone
        // is not enough to declare success: when the session cookie was already
        // absent (but the page still held a Family View / parser state) the loop
        // would exit on the first tick before the wipe had any effect.
        sleep(LOGOUT_FLUSH_WAIT).await;
        if self.wait_for_login(false, 12, 400).await {
            log::info!("[steam-webview] logout confirmed");
            return Ok(());
        }
        log::warn!("[steam-webview] logout could not clear steamLoginSecure");
        Err("Could not sign out of the current Steam session. Close WEaveR, delete the SteamWebView folder under %LOCALAPPDATA%\\WEave, then reopen and try again."
            .to_string())
    }

    /// Make the webview hold a `steamcommunity.com` session.
    ///
    /// This is the crux of the whole login problem. `steamLoginSecure` — the
    /// cookie every authenticated workshop request depends on — lives on the
    /// `steamcommunity.com` domain. The login form, however, is served from
    /// `store.steampowered.com` (that page is still server-rendered, whereas
    /// the community login page became a React SPA that sometimes mounts
    /// empty). Signing in on the store domain authenticates the account, but
    /// WebView2 only materialises a domain's cookie jar once a page on that
    /// domain has actually been visited. Without the navigation below the
    /// community jar stayed empty, `is_logged_in_inner` never saw
    /// `steamLoginSecure`, and the UI reported "login failed" after every
    /// restart even though the credentials were fine.
    ///
    /// Visiting a community page after a successful sign-in makes Steam issue
    /// the cookie for that domain. Harmless when already signed in.
    async fn ensure_community_session(&self, w: &tauri::WebviewWindow) {
        // Only ever worth one attempt per process.
        //
        // Without this guard every `steam_current_account` poll (the details
        // panel asks constantly) re-ran the whole 12-second navigation+poll
        // cycle, which filled the log with warnings and burned CPU for a
        // session that was never coming back. If Steam did not hand out the
        // cookie on the first visit, no amount of retrying will change that —
        // the user has to sign in again.
        if self
            .community_attempted
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let _ = w.eval(format!("window.location.href = {:?}", STEAM_HOME_URL));
        for _ in 0..30 {
            sleep(COMMUNITY_LOAD_POLL).await;
            if self.is_logged_in_inner() {
                return;
            }
        }
        log::warn!("[steam-webview] community session did not materialise");
    }

    /// Open Steam's real login window for the given account.
    ///
    /// Earlier revisions tried to drive the credential form from a *hidden*
    /// webview with injected JavaScript. That cannot work reliably:
    ///   * WebView2 suspends script execution while the window is hidden;
    ///   * Steam's form is React-controlled, so `el.value = x` is reset unless
    ///     the native setter and `_valueTracker` are used;
    ///   * the page navigates during submit, so any read-back races it.
    ///
    /// So the contract is now: **best-effort prefill, then hand the window to the
    /// user.** If the prefill lands, the user only has to confirm; if it does
    /// not, they simply type into Steam's own form — which is exactly the
    /// behaviour a user expects from a login button. Either way Steam owns the
    /// Steam Guard step, so mobile confirmation works.
    ///
    /// Returns `Ok(true)` when a session cookie is already present (prefill
    /// logged in without any 2FA), `Ok(false)` when the user still has to finish.
    pub async fn login_fill(&self, username: &str, password: &str) -> Result<bool, String> {
        let _g = self.lock.lock().await;
        self.ensure_window().map_err(|e| e.to_string())?;

        // 1. The window MUST be visible: WebView2 does not run scripts (and often
        //    does not even render) while hidden, and the user needs to interact
        //    with Steam Guard anyway.
        if let Some(w) = self.app.get_webview_window(LABEL) {
            let _ = w.set_skip_taskbar(false);
            Self::restore_on_screen(&w);
            let _ = w.show();
            let _ = w.set_focus();
            sleep(WINDOW_SHOW_SETTLE).await;
        }

        // 2. Drop any stale session (Family View lock, bundled parser account…)
        //    so Steam actually serves the credential form.
        if let Err(err) = self.logout_locked().await {
            log::warn!("[steam-webview] logout before login failed: {err}");
        }

        // 3. Get to the login page. A failure here is not fatal for the user —
        //    the window is visible and they can navigate themselves.
        let on_form = self.goto_login_form().await.unwrap_or(false);
        if !on_form {
            log::warn!("[steam-webview] login form not detected; leaving the window to the user");
            return Ok(false);
        }

        // 4. Best-effort prefill. Never propagate a failure: the user can always
        //    type into the form that is right in front of them.
        if let Some(w) = self.app.get_webview_window(LABEL) {
            let script = build_login_script(username, password);
            for _ in 0..3 {
                if w.eval(&script).is_err() {
                    break;
                }
                sleep(SUBMIT_RETRY_WAIT).await;
                if self.is_logged_in_inner() {
                    return Ok(true);
                }
            }
        }

        Ok(self.is_logged_in_inner())
    }
}

// ── Timing constants ──────────────────────────────────────────
/// How long to wait for the main thread to build the hidden window.
const WINDOW_CREATE_TIMEOUT: Duration = Duration::from_secs(15);

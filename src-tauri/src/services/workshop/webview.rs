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

use scripts::{build_login_script, build_page_probe_script};

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
const FORM_RENDER_POLL: Duration = Duration::from_millis(500);
/// Settle time after an eval that checks for the form.
const FORM_CHECK_SETTLE: Duration = Duration::from_millis(250);
/// Wait after a navigation that failed to land on the form.
const NAV_RETRY_WAIT: Duration = Duration::from_millis(800);
/// Poll interval while the credential form paints (30 ticks ~= 18s).
const FORM_PAINT_POLL: Duration = Duration::from_millis(600);
/// Time allowed for the login document to paint after navigation.
const LOGIN_PAGE_PAINT_WAIT: Duration = Duration::from_millis(3000);
/// Pause between form-submit attempts.
const SUBMIT_RETRY_WAIT: Duration = Duration::from_millis(800);
/// Poll interval while waiting for the auth cookie (40 ticks ~= 40s).
const AUTH_COOKIE_POLL: Duration = Duration::from_millis(1000);
/// Wait for the community page to settle before re-checking the session.
const COMMUNITY_PAGE_SETTLE: Duration = Duration::from_millis(1500);
/// Poll interval while the community page loads (30 ticks ~= 12s).
const COMMUNITY_LOAD_POLL: Duration = Duration::from_millis(400);
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
    async fn wait_for_login(&self, want: bool, ticks: u32, interval_ms: u64) -> bool {
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

    /// Navigate to the login page and wait until the credential form is
    /// actually rendered (i.e. a password field exists).
    ///
    /// This has to be a *wait loop*, not a fixed sleep: after a logout Steam
    /// redirects through store/community/help pages, and navigating before that
    /// chain settles leaves the webview on the wrong page (the script then finds
    /// no inputs and the login silently never happens).
    async fn goto_login_form(&self) -> Result<bool, String> {
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Ok(false);
        };
        // WebView2 suspends script execution while the window is hidden, so the
        // form probe below would never run. Keep it visible.
        Self::restore_on_screen(&w);
        let _ = w.show();
        // `eval` never returns a value, so the probe reports its verdict by
        // rewriting the URL fragment and we read it back via `url()`.
        let probe = r#"
            (function() {
                var has = document.querySelector('input[type="password"]') ? 1 : 0;
                try {
                    history.replaceState(null, '', location.pathname + location.search + '#WE_FORM:' + has);
                } catch (e) {}
            })();
        "#;

        // After `clear_all_browsing_data()` WebView2 tears down and rebuilds the
        // page, and a navigation issued while that is in flight is silently
        // dropped — the webview then sits on a Steam support/help page with no
        // inputs and the fill script no-ops. So: first wait for the document to
        // become usable again, and only then navigate. Each navigation is
        // followed by its own wait-for-form loop.
        for _ in 0..40 {
            sleep(FORM_RENDER_POLL).await;
            if w.url().is_ok() && w.eval("1").is_ok() {
                break;
            }
        }

        for _ in 0..6 {
            // Already on a page with the form? Don't navigate again.
            if w.eval(probe).is_ok() {
                sleep(FORM_CHECK_SETTLE).await;
                if Self::read_form_flag(&w) {
                    return Ok(true);
                }
            }
            if w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))
                .is_err()
            {
                sleep(NAV_RETRY_WAIT).await;
                continue;
            }
            // Up to ~18s per attempt for the credential form to render.
            for _ in 0..30 {
                sleep(FORM_PAINT_POLL).await;
                if w.eval(probe).is_err() {
                    continue;
                }
                sleep(FORM_CHECK_SETTLE).await;
                if Self::read_form_flag(&w) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Read the verdict written by the form probe into the URL fragment.
    ///
    /// `WebviewWindow::eval` returns `Result<()>` — it never yields the JS
    /// value — so a probe cannot "return" anything. Writing the verdict into the
    /// URL fragment and reading `url()` back is the reliable round-trip.
    fn read_form_flag(w: &tauri::WebviewWindow) -> bool {
        w.url()
            .ok()
            .and_then(|u| u.fragment().map(|f| f.to_string()))
            .map(|f| f.contains("WE_FORM:1"))
            .unwrap_or(false)
    }

    /// Drive the hidden webview through Steam's login form using the given
    /// credentials. The window stays invisible throughout. Returns true if
    /// we end up with a `steamLoginSecure` cookie within the timeout.
    ///
    /// When `force` is true the existing session check is skipped and the
    /// login form is always submitted — useful for manual re-login.
    pub async fn auto_login(
        &self,
        username: &str,
        password: &str,
        force: bool,
    ) -> Result<bool, String> {
        let _g = self.lock.lock().await;
        // Do not create the window to probe the session: a session implies
        // the window already exists, so a missing window means "not logged in".
        if !self.window_exists() {
            log::warn!("[login-flow] abort: hidden window does not exist");
            return Ok(false);
        }

        // Early return if we're already logged in from a previous run,
        // unless the caller explicitly wants to force a re-login.
        if !force && self.is_logged_in_inner() {
            log::info!("[login-flow] session cookie already present; nothing to do");
            return Ok(true);
        }

        let Some(w) = self.app.get_webview_window(LABEL) else {
            log::warn!("[login-flow] abort: window handle unavailable");
            return Ok(false);
        };

        log::info!(
            "[login-flow] start (force={force}, has_session_cookie={})",
            self.is_logged_in_inner()
        );

        if force {
            // Log out through Steam rather than wiping the whole WebView2
            // profile: a logout clears the session cookies, while
            // clear_all_browsing_data() also nukes unrelated state (and is
            // slow). If the session is already gone this is a no-op.
            let _ = self.logout_locked().await;
        }

        // Navigate to the login page and give the document time to paint.
        w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))
            .map_err(|e| e.to_string())?;
        sleep(LOGIN_PAGE_PAINT_WAIT).await;
        let landed = w.url().map(|u| u.to_string()).unwrap_or_default();
        log::info!(
            "[login-flow] navigated; url_host_has_login={}",
            landed.contains("/login")
        );

        // Fill + submit the login form. Retry a few times: the SPA may render
        // the inputs asynchronously, and the first eval can land before them.
        // When force-login is active we never break early on an existing
        // cookie — the old session may be stale and we always want to
        // submit fresh credentials.
        let script = build_login_script(username, password);
        let mut submitted = 0;
        let mut last_verdict = String::from("(none)");
        for _ in 0..6 {
            w.eval(&script).map_err(|e| e.to_string())?;
            submitted += 1;
            sleep(SUBMIT_RETRY_WAIT).await;
            // The script writes its outcome into the URL fragment because
            // `eval` cannot return a value. Without this, "no password field",
            // "no username field" and "clicked the button" were indistinguishable
            // and the only observable symptom was a silent timeout.
            if let Ok(u) = w.url() {
                if let Some(f) = u.fragment() {
                    if f.starts_with("WE_FILL:") {
                        last_verdict = f.trim_start_matches("WE_FILL:").to_string();
                    }
                }
            }
            log::info!("[login-flow] fill attempt {submitted}: {last_verdict}");
            if !force && self.is_logged_in_inner() {
                break;
            }
        }
        log::info!(
            "[login-flow] form fill attempted {submitted}x; last={last_verdict}; cookie_now={}",
            self.is_logged_in_inner()
        );

        // Poll for the auth cookie to arrive after submit. 40 × 1s gives the
        // user time to approve a Steam Guard prompt on their phone; the old
        // 25 × 800ms window (20s) expired before most people could react.
        let page_probe = build_page_probe_script();
        let mut last_page = String::new();
        for tick in 0..40 {
            sleep(AUTH_COOKIE_POLL).await;
            // Sample the page state every few seconds: whatever Steam rendered
            // after the submit is the only clue to *why* no cookie appeared.
            if tick % 3 == 0 {
                let _ = w.eval(page_probe);
                sleep(FORM_CHECK_SETTLE).await;
                if let Ok(u) = w.url() {
                    if let Some(f) = u.fragment() {
                        if f.starts_with("WE_PAGE:") {
                            let now = f.trim_start_matches("WE_PAGE:").to_string();
                            if now != last_page {
                                log::warn!("[login-flow] page after submit: {now}");
                                last_page = now;
                            }
                        }
                    }
                }
            }
            if self.is_logged_in_inner() {
                // Navigate to a plain Workshop page so any further state
                // (age-gate, community country) is picked up.
                let _ = w.eval(format!("window.location.href = {:?}", STEAM_HOME_URL));
                sleep(COMMUNITY_PAGE_SETTLE).await;
                return Ok(true);
            }
        }

        // The store-domain sign-in can succeed without ever producing a
        // `steamcommunity.com` cookie. Visit a community page so Steam issues
        // one, then re-check: this is the difference between "signed in" and
        // "signed in, but every workshop request is anonymous".
        log::warn!("[login-flow] no auth cookie after polling; trying community page");
        self.ensure_community_session(&w).await;
        let ok = self.is_logged_in_inner();
        log::warn!("[login-flow] giving up: cookie_present={ok}");
        if ok {
            return Ok(true);
        }

        Ok(false)
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

    fn is_logged_in_inner(&self) -> bool {
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

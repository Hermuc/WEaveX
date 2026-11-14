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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::cookie::Jar;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
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

pub struct SteamWebview {
    app: AppHandle,
    data_dir: PathBuf,
    jar: Arc<Jar>,
    lock: Mutex<()>,
    /// One-shot guard for `ensure_community_session`. See that method for why
    /// repeating the attempt is actively harmful.
    community_attempted: std::sync::atomic::AtomicBool,
}

impl SteamWebview {
    pub fn new(app: AppHandle, data_dir: PathBuf, jar: Arc<Jar>) -> Self {
        std::fs::create_dir_all(&data_dir).ok();
        Self {
            app,
            data_dir,
            jar,
            lock: Mutex::new(()),
            community_attempted: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// True when the hidden Steam window already exists.
    ///
    /// Callers use this to avoid *creating* the window just to ask a
    /// question. Creating it is expensive and, worse, makes Windows paint
    /// the window's non-client frame for a moment — visible as a
    /// translucent rounded rectangle flashing over the app on startup.
    fn window_exists(&self) -> bool {
        self.app.get_webview_window(LABEL).is_some()
    }

    /// Create (if missing) and return the hidden Steam webview. Idempotent.
    /// Returns `true` when a new window was created on this call, `false`
    /// when it already existed — callers use this to know whether they
    /// should give WebView2 a moment to restore the persisted cookie jar.
    fn ensure_window(&self) -> Result<bool, tauri::Error> {
        if self.app.get_webview_window(LABEL).is_some() {
            return Ok(false);
        }
        Self::create_hidden_window(&self.app, &self.data_dir)?;
        Ok(true)
    }

    /// Build the hidden Steam window. Safe to call only on the main thread.
    fn create_hidden_window(
        app: &tauri::AppHandle,
        data_dir: &std::path::Path,
    ) -> Result<(), tauri::Error> {
        if app.get_webview_window(LABEL).is_some() {
            return Ok(());
        }
        log::warn!("[steam-webview] CREATING hidden window (this should happen once)");
        let url = WebviewUrl::External(STEAM_LOGIN_URL.parse().expect("valid url"));
        WebviewWindowBuilder::new(app, LABEL, url)
            .title("Steam")
            .data_directory(data_dir.to_path_buf())
            .visible(false)
            // Park it off-screen as well as hiding it.
            //
            // `visible(false)` alone is not enough for a WebView2 host: the
            // control keeps its own composition surface, and DWM composites it
            // even while the window's WS_VISIBLE bit is clear. Measured here:
            // with the window merely hidden, a translucent 1250x900 rectangle
            // (the window's size at 125% DPI) covered the whole main window on
            // every launch; moving the same window to x=-32000 made the
            // artefact disappear entirely — a 34,959-pixel A/B difference.
            // Geometry is restored in `show_login`/`reveal` before the user
            // ever needs to see it.
            .position(-32000.0, -32000.0)
            // No decorations and no shadow: this window is meant to be invisible
            // until the user opens the login dialog. With decorations enabled
            // Windows draws a frame + drop shadow for it, and that frame can
            // become visible during compositing (DWM may paint the non-client
            // area even while the client area is hidden), which looks like a
            // translucent rectangle flashing over the main window.
            .decorations(false)
            .shadow(false)
            .skip_taskbar(true)
            .inner_size(1000.0, 720.0)
            .resizable(true)
            .build()?;
        Ok(())
    }

    /// Make sure the hidden Steam webview exists, so the persisted WebView2
    /// cookie jar is loaded from disk before anyone asks about the session.
    ///
    /// Why this exists: every read path (`sync_cookies`, `is_logged_in`,
    /// `auto_login`) short-circuits to "no session" when the window has not
    /// been created yet. The window used to be created lazily — only when the
    /// user opened the login dialog — so on every launch the startup
    /// `init_cookies` thread called `sync_cookies()` before any window
    /// existed, got zero cookies back, and the UI reported "logged out"
    /// even though WebView2 still held a perfectly valid `steamLoginSecure`
    /// on disk. The user then had to log in again after every restart.
    ///
    /// WebView2 needs a moment after the window is created to rehydrate the
    /// cookie store, so this polls until `steamLoginSecure` shows up (or the
    /// budget runs out). Returns `true` when a session cookie was found.
    pub async fn ensure_profile_loaded(&self) -> bool {
        let _g = self.lock.lock().await;
        // Create the window so WebView2 opens its persisted profile.
        //
        // This MUST happen on the main thread. `init_cookies` runs this on a
        // worker thread, and creating a WebView2 there fails with
        // HRESULT 0x80070057 ("parameter is incorrect") — the window never
        // appears, no cookies load, and the session looks lost again.
        // `run_on_main_thread` + a channel keeps the async flow correct.
        // Only poll when we just built the window: WebView2 rehydrates the
        // cookie jar from disk asynchronously right after creation. If the
        // window already existed, its cookies are already loaded and the
        // current state is authoritative — sleeping here would just add six
        // seconds to every signed-out probe.
        let freshly_created = if !self.window_exists() {
            let app = self.app.clone();
            let app_for_main = self.app.clone();
            let data_dir = self.data_dir.clone();
            let (tx, rx) = std::sync::mpsc::channel::<bool>();
            let created = app.run_on_main_thread(move || {
                let _ = tx.send(Self::create_hidden_window(&app_for_main, &data_dir).is_ok());
            });
            if created.is_err() {
                log::warn!("[steam-webview] could not reach the main thread");
                return false;
            }
            // Wait for the main thread to finish building the window.
            //
            // This runs on a worker thread, so blocking here is safe *provided*
            // the main thread is already pumping its event loop. The caller in
            // `app_state` sleeps briefly before getting here to guarantee that;
            // without it the closure below could never be delivered.
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(true) => true,
                Ok(false) => {
                    log::warn!("[steam-webview] hidden window creation failed");
                    return false;
                }
                Err(e) => {
                    log::warn!("[steam-webview] hidden window was not created in time: {e}");
                    return false;
                }
            }
        } else {
            false
        };
        // IMPORTANT: nothing slow may happen past this point.
        //
        // This method is called from `steam_current_account`, which the frontend
        // polls constantly, and it holds `self.lock` for its whole body. A
        // previous version tried to re-establish the community session here
        // (a 12-second navigate-and-wait loop), which meant every poll — and
        // every `sync_cookies` behind it — queued for twelve seconds. That is
        // what made the app feel frozen and starved the video stream. The
        // community session is established in `auto_login` instead, where a
        // real sign-in is happening and the wait is expected.
        if !freshly_created {
            return self.is_logged_in_inner();
        }
        // Freshly created: give WebView2 up to ~6s (40 * 150ms) to rehydrate.
        for _ in 0..40 {
            if self.is_logged_in_inner() {
                return true;
            }
            sleep(Duration::from_millis(150)).await;
        }
        false
    }

    /// Install a saved Steam session into the webview's cookie store.
    ///
    /// This is the payoff of persisting `{steamid, access_token}`:
    /// `steamLoginSecure` is exactly `{steamid}%7C%7C{access_token}`, so writing
    /// that one cookie authenticates the whole webview — no login page, no
    /// Steam Guard, no password.
    ///
    /// It must go through `set_cookie`, not `document.cookie`: Steam marks
    /// `steamLoginSecure` HttpOnly, and script-visible cookie writes silently
    /// ignore HttpOnly names.
    pub fn install_session_cookies(
        &self,
        steam_id: &str,
        access_token: &str,
    ) -> Result<(), String> {
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Err("webview window not ready".into());
        };
        let secure = crate::accounts::StoredSteamSession::steam_login_secure(steam_id, access_token);
        // A random session id is fine: Steam only uses it as a CSRF token, it
        // is not tied to the account.
        let session_id: String = {
            use rand::RngExt;
            rand::rng()
                .sample_iter(&rand::distr::Alphanumeric)
                .take(32)
                .map(char::from)
                .collect::<String>()
                .to_lowercase()
        };
        for domain in ["steamcommunity.com", "steampowered.com"] {
            for (name, value) in [
                ("steamLoginSecure", secure.as_str()),
                ("sessionid", session_id.as_str()),
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
        log::info!("[steam-webview] installed saved session cookies");
        Ok(())
    }

    /// Move the window back on screen before it is shown.
    ///
    /// The window is created parked at -32000,-32000 so its WebView2
    /// composition surface cannot bleed through while hidden. Every visible
    /// path has to undo that, otherwise the user gets an off-screen window and
    /// thinks the login button does nothing.
    fn restore_on_screen(w: &tauri::WebviewWindow) {
        let _ = w.center();
    }

    /// Show the webview window so the user can interact with Steam's login
    /// form. Cookies are persisted to disk automatically by WebView2.
    pub async fn show_login(&self) -> Result<(), tauri::Error> {
        let _g = self.lock.lock().await;
        self.ensure_window()?;
        if let Some(w) = self.app.get_webview_window(LABEL) {
            let _ = w.set_skip_taskbar(false);
            Self::restore_on_screen(&w);
            w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))?;
            w.show()?;
            w.set_focus()?;
        }
        Ok(())
    }

    /// Show the webview **without navigating**.
    ///
    /// Unlike `show_login()`, which reloads the login URL, this preserves the
    /// page the user is currently on — e.g. the Steam Guard confirmation step
    /// that appeared right after their credentials were submitted. Steam owns
    /// that flow (mobile push, email code, device authorisation); the only
    /// reliable way through it is to let the user interact with the real page.
    pub async fn reveal(&self) -> Result<(), tauri::Error> {
        let _g = self.lock.lock().await;
        if let Some(w) = self.app.get_webview_window(LABEL) {
            let _ = w.set_skip_taskbar(false);
            Self::restore_on_screen(&w);
            w.show()?;
            w.set_focus()?;
        }
        Ok(())
    }
    pub async fn hide(&self) -> Result<(), tauri::Error> {
        if let Some(w) = self.app.get_webview_window(LABEL) {
            w.hide()?;
        }
        Ok(())
    }

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

    /// Back up the WebView cookie database before anything wipes it.
    ///
    /// `clear_all_browsing_data()` is destructive and irreversible: it deletes
    /// the persisted `steamLoginSecure` cookie along with everything else, so a
    /// user who signs out by accident (or gets signed out by a failed startup
    /// check) has to re-authenticate by hand, including Steam Guard. Copying the
    /// cookie DB first means the session can be restored.
    fn backup_cookie_db(&self) -> Option<std::path::PathBuf> {
        let data_dir = crate::app_state::resolve_app_data_dir();
        let src = data_dir
            .join("SteamWebView")
            .join("EBWebView")
            .join("Default")
            .join("Network")
            .join("Cookies");
        if !src.is_file() {
            return None;
        }
        let backup = data_dir.join("SteamWebView.cookies.bak");
        std::fs::copy(&src, &backup).ok()?;
        log::info!(
            "[steam-webview] cookie database backed up to {}",
            backup.display()
        );
        Some(backup)
    }

    /// Put the cookie database back after a destructive clear.
    pub fn restore_cookie_db(&self) -> bool {
        // The resolver always yields a path, so the only failure mode left is
        // "no backup on disk", which the check below already covers.
        let data_dir = crate::app_state::resolve_app_data_dir();
        let backup = data_dir.join("SteamWebView.cookies.bak");
        if !backup.is_file() {
            return false;
        }
        let dst = data_dir
            .join("SteamWebView")
            .join("EBWebView")
            .join("Default")
            .join("Network")
            .join("Cookies");
        match std::fs::copy(&backup, &dst) {
            Ok(_) => {
                log::info!("[steam-webview] cookie database restored from backup");
                true
            }
            Err(e) => {
                log::warn!("[steam-webview] could not restore cookie database: {e}");
                false
            }
        }
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
        sleep(Duration::from_millis(1500)).await;
        for _ in 0..12 {
            if !self.is_logged_in_inner() {
                log::info!("[steam-webview] logout confirmed");
                return Ok(());
            }
            sleep(Duration::from_millis(400)).await;
        }
        log::warn!("[steam-webview] logout could not clear steamLoginSecure");
        Err("Could not sign out of the current Steam session. Close WEaveR, delete the SteamWebView folder under %LOCALAPPDATA%\\WEave, then reopen and try again."
            .to_string())
    }
    /// Pull all `steamcommunity.com` cookies from the webview and load them
    /// into our reqwest jar. WebView2 persists cookies automatically, so no
    /// additional file storage is needed.
    pub async fn sync_cookies(&self) -> Result<usize, String> {
        let _g = self.lock.lock().await;
        // Read cookies only if the window already exists.
        if !self.window_exists() {
            return Ok(0);
        }
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Ok(0);
        };
        let cookies = w.cookies().map_err(|e| e.to_string())?;
        let steam_com = "https://steamcommunity.com"
            .parse::<url::Url>()
            .map_err(|e| e.to_string())?;
        let steam_pow = "https://store.steampowered.com"
            .parse::<url::Url>()
            .map_err(|e| e.to_string())?;
        // Build `Cookie` values directly instead of formatting a Set-Cookie
        // header by hand.
        //
        // The hand-built header (`name=value; Path=...; Domain=...`) omitted the
        // attributes that make a cookie apply to subsequent requests. In
        // particular it produced a *host-only* cookie whenever the domain
        // attribute was dropped during parsing, and `steamLoginSecure` for
        // `steamcommunity.com` then never matched `steamcommunity.com/my/...`.
        // Using the typed API sets domain/path/secure/httponly explicitly and
        // leaves no room for a malformed header to be silently skipped.
        let mut n = 0;
        for c in cookies {
            let domain = c.domain().unwrap_or("steamcommunity.com").to_string();
            if !(domain.ends_with("steamcommunity.com") || domain.ends_with("steampowered.com")) {
                continue;
            }
            let name = c.name().to_string();
            let value = c.value().to_string();

            let path = c.path().unwrap_or("/").to_string();
            // `Domain=` must be present: without it the cookie becomes host-only
            // and stops matching the sub-paths we actually request. Keep the
            // leading dot so subdomains match too.
            let domain_attr = if domain.starts_with('.') {
                domain.clone()
            } else {
                format!(".{domain}")
            };
            let cookie_str =
                format!("{name}={value}; Domain={domain_attr}; Path={path}; Secure; HttpOnly");
            let target = if domain.ends_with("steampowered.com") {
                &steam_pow
            } else {
                &steam_com
            };
            self.jar.add_cookie_str(&cookie_str, target);
            log::info!("[cookie-sync] stored {name} @ {domain_attr} path={path}");
            n += 1;
        }
        Ok(n)
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
    pub async fn is_logged_in(&self) -> bool {
        let _g = self.lock.lock().await;
        // Never create the window to answer this question.
        if !self.window_exists() {
            return false;
        }
        // The window exists (checked above), so nothing was created on this call.
        let created = false;
        // When we just created the window, WebView2 needs a brief moment to
        // restore the persisted cookie jar from disk before they're readable.
        // Poll a few times so a freshly-created window doesn't report a stale
        // "logged out" before the session cookie has been rehydrated.
        if created {
            for _ in 0..20 {
                if self.is_logged_in_inner() {
                    return true;
                }
                sleep(Duration::from_millis(150)).await;
            }
        }
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
            sleep(Duration::from_millis(500)).await;
            if w.url().is_ok() && w.eval("1").is_ok() {
                break;
            }
        }

        for _ in 0..6 {
            // Already on a page with the form? Don't navigate again.
            if w.eval(probe).is_ok() {
                sleep(Duration::from_millis(250)).await;
                if Self::read_form_flag(&w) {
                    return Ok(true);
                }
            }
            if w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))
                .is_err()
            {
                sleep(Duration::from_millis(800)).await;
                continue;
            }
            // Up to ~18s per attempt for the credential form to render.
            for _ in 0..30 {
                sleep(Duration::from_millis(600)).await;
                if w.eval(probe).is_err() {
                    continue;
                }
                sleep(Duration::from_millis(250)).await;
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
        sleep(Duration::from_millis(3000)).await;
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
            sleep(Duration::from_millis(800)).await;
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
            sleep(Duration::from_millis(1000)).await;
            // Sample the page state every few seconds: whatever Steam rendered
            // after the submit is the only clue to *why* no cookie appeared.
            if tick % 3 == 0 {
                let _ = w.eval(page_probe);
                sleep(Duration::from_millis(250)).await;
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
                sleep(Duration::from_millis(1500)).await;
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
            sleep(Duration::from_millis(400)).await;
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
            sleep(Duration::from_millis(600)).await;
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
                sleep(Duration::from_millis(800)).await;
                if self.is_logged_in_inner() {
                    return Ok(true);
                }
            }
        }

        Ok(self.is_logged_in_inner())
    }
}

/// Builds a JS snippet that locates the login/password inputs, fills them
/// in via the native `value` setter (so React's onChange handlers fire),
/// and submits the form. This is a near-direct port of
/// `infrastructure/steam/workshop_scripts.py::login_form_fill_script`.
fn build_login_script(username: &str, password: &str) -> String {
    let user = escape_js_string(username);
    let pass = escape_js_string(password);
    format!(
        r#"
        (function() {{
            try {{
                // Steam's store login form is React-controlled. Assigning
                // `el.value = x` is accepted by the DOM but React immediately
                // resets it from its own state, so the field ends up empty and
                // the submit does nothing — that is exactly the "I typed my
                // password and nothing happened" bug.
                //
                // The reliable way is to go through the native value setter and
                // then dispatch the events React listens for. React tracks the
                // previous value in `_valueTracker`; if that still holds the old
                // text it treats the change as a no-op, so the tracker has to be
                // reset as well.
                var nativeSetter = Object.getOwnPropertyDescriptor(
                    HTMLInputElement.prototype, 'value'
                ).set;
                function setReactValue(el, v) {{
                    var tracker = el._valueTracker;
                    if (tracker) tracker.setValue('');
                    nativeSetter.call(el, v);
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    // Some Steam forms listen for key events to enable the
                    // submit button.
                    el.dispatchEvent(new KeyboardEvent('keyup', {{ bubbles: true, key: 'a' }}));
                }}

                var isSearch = function(el) {{
                    var n = (el.getAttribute('name') || '').toLowerCase();
                    var ph = (el.getAttribute('placeholder') || '').toLowerCase();
                    if (n === 'term' || n === 'snr' || n === 'search') return true;
                    if (ph.indexOf('搜索') >= 0 || ph.indexOf('search') >= 0) return true;
                    return false;
                }};
                var isCodeBox = function(el) {{
                    var id = (el.getAttribute('id') || '').toLowerCase();
                    var ph = (el.getAttribute('placeholder') || '').toLowerCase();
                    return id.indexOf('authcode') >= 0 || id.indexOf('twofactor') >= 0
                        || id.indexOf('friendlyname') >= 0
                        || ph.indexOf('验证码') >= 0;
                }};
                var visible = function(el) {{ return el.offsetParent !== null && !el.disabled; }};

                var inputs = Array.prototype.slice.call(document.querySelectorAll('input'));
                // The password field anchors the credential form.
                var pass = null;
                for (var i = 0; i < inputs.length; i++) {{
                    if ((inputs[i].getAttribute('type') || '') === 'password' && visible(inputs[i])) {{
                        pass = inputs[i]; break;
                    }}
                }}
                if (!pass) {{
                    try {{ history.replaceState(null, '', location.pathname + location.search + '#WE_FILL:no-password'); }} catch (e) {{}}
                    return {{ ready: false, why: 'no-password' }};
                }}

                // Username = the visible text input right before the password
                // field in the same form, skipping search / auth-code boxes.
                var scope = pass.form
                    ? Array.prototype.slice.call(pass.form.querySelectorAll('input'))
                    : inputs;
                var pIdx = scope.indexOf(pass);
                var login = null;
                for (var j = pIdx - 1; j >= 0; j--) {{
                    var c = scope[j];
                    var t = (c.getAttribute('type') || '').toLowerCase();
                    if (t !== 'text' && t !== 'email' && t !== 'tel') continue;
                    if (!visible(c) || isSearch(c) || isCodeBox(c)) continue;
                    login = c; break;
                }}
                if (!login) {{
                    try {{ history.replaceState(null, '', location.pathname + location.search + '#WE_FILL:no-username'); }} catch (e) {{}}
                    return {{ ready: false, why: 'no-username' }};
                }}

                setReactValue(login, "{user}");
                setReactValue(pass, "{pass}");

                // Submit: prefer the button whose label is the login action.
                var labelHit = function(el) {{
                    var t = ((el.innerText || el.value || '') + '').trim().toLowerCase();
                    if (!t) return false;
                    return t === '登录' || t === '登入' || t === 'sign in' || t === 'login'
                        || t.indexOf('sign in') >= 0 || t.indexOf('登录') >= 0;
                }};
                var btn = null;
                var containers = pass.form ? [pass.form, document] : [document];
                for (var ci = 0; ci < containers.length && !btn; ci++) {{
                    var cands = Array.prototype.slice.call(
                        containers[ci].querySelectorAll('button, input[type="submit"]')
                    );
                    for (var bi = 0; bi < cands.length; bi++) {{
                        if (cands[bi].disabled || !visible(cands[bi])) continue;
                        if (labelHit(cands[bi])) {{ btn = cands[bi]; break; }}
                    }}
                }}
                if (btn) {{ btn.disabled = false; btn.click(); }}
                try {{
                    history.replaceState(null, '', location.pathname + location.search
                        + '#WE_FILL:ready,clicked=' + (btn ? 1 : 0));
                }} catch (e) {{}}
                return {{ ready: true, clicked: !!btn }};
            }} catch (e) {{
                try {{
                    history.replaceState(null, '', location.pathname + location.search
                        + '#WE_FILL:error');
                }} catch (e2) {{}}
                return {{ error: String(e) }};
            }}
        }})();
        "#
    )
}

/// Classify whatever page the login flow ended up on.
///
/// This exists because the only observable symptom of a failed auto-login was
/// "no cookie", which is indistinguishable between a wrong password, a Steam
/// Guard prompt, a CAPTCHA and a rate-limit block. Each needs a completely
/// different response from the user (and from us), so the page has to be
/// inspected rather than guessed at. Only coarse keyword flags are reported —
/// never page text, never form values.
fn build_page_probe_script() -> &'static str {
    r#"
    (function() {
        try {
            var t = ((document.body && document.body.innerText) || '').toLowerCase();
            var title = (document.title || '').toLowerCase();
            var all = t + ' ' + title;
            var flags = [];
            if (all.indexOf('steam guard') >= 0 || all.indexOf('two-factor') >= 0
                || all.indexOf('authenticator') >= 0 || all.indexOf('验证器') >= 0
                || all.indexOf('安全令牌') >= 0) flags.push('GUARD');
            if (all.indexOf('captcha') >= 0 || all.indexOf('验证码') >= 0
                || all.indexOf('人机') >= 0) flags.push('CAPTCHA');
            if (all.indexOf('incorrect') >= 0 || all.indexOf('wrong password') >= 0
                || all.indexOf('密码错误') >= 0 || all.indexOf('密碼錯誤') >= 0
                || all.indexOf('try again') >= 0) flags.push('BADPASS');
            if (all.indexOf('too many') >= 0 || all.indexOf('rate limit') >= 0
                || all.indexOf('频繁') >= 0) flags.push('RATELIMIT');
            if (all.indexOf('sign in') >= 0 || all.indexOf('登录') >= 0
                || all.indexOf('log in') >= 0) flags.push('LOGINPAGE');
            if (location.pathname.indexOf('/login') < 0) flags.push('LEFTLOGIN');
            try {
                history.replaceState(null, '', location.pathname + location.search
                    + '#WE_PAGE:' + (flags.length ? flags.join('+') : 'NONE'));
            } catch (e) {}
        } catch (e) {}
    })();
    "#
}

fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

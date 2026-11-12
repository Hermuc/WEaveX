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

use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::HeaderValue;
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
}

impl SteamWebview {
    pub fn new(app: AppHandle, data_dir: PathBuf, jar: Arc<Jar>) -> Self {
        std::fs::create_dir_all(&data_dir).ok();
        Self {
            app,
            data_dir,
            jar,
            lock: Mutex::new(()),
        }
    }

    pub fn jar(&self) -> Arc<Jar> {
        self.jar.clone()
    }

    /// Create (if missing) and return the hidden Steam webview. Idempotent.
    /// Returns `true` when a new window was created on this call, `false`
    /// when it already existed — callers use this to know whether they
    /// should give WebView2 a moment to restore the persisted cookie jar.
    fn ensure_window(&self) -> Result<bool, tauri::Error> {
        if self.app.get_webview_window(LABEL).is_some() {
            return Ok(false);
        }
        let url = WebviewUrl::External(STEAM_LOGIN_URL.parse().expect("valid url"));
        WebviewWindowBuilder::new(&self.app, LABEL, url)
            .title("Steam")
            .data_directory(self.data_dir.clone())
            .visible(false)
            .decorations(true)
            .skip_taskbar(true)
            .inner_size(1000.0, 720.0)
            .resizable(true)
            .build()?;
        Ok(true)
    }

    /// Show the webview window so the user can interact with Steam's login
    /// form. Cookies are persisted to disk automatically by WebView2.
    pub async fn show_login(&self) -> Result<(), tauri::Error> {
        let _g = self.lock.lock().await;
        self.ensure_window()?;
        if let Some(w) = self.app.get_webview_window(LABEL) {
            let _ = w.set_skip_taskbar(false);
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
        self.ensure_window()?;
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
        let data_dir = dirs::data_local_dir()?.join("WEave");
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
        let Some(data_dir) = dirs::data_local_dir().map(|d| d.join("WEave")) else {
            return false;
        };
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
        Err("Could not sign out of the current Steam session. Close WEave, delete the SteamWebView folder under %LOCALAPPDATA%\\WEave, then reopen and try again."
            .to_string())
    }
    /// Pull all `steamcommunity.com` cookies from the webview and load them
    /// into our reqwest jar. WebView2 persists cookies automatically, so no
    /// additional file storage is needed.
    pub async fn sync_cookies(&self) -> Result<usize, String> {
        let _g = self.lock.lock().await;
        self.ensure_window().map_err(|e| e.to_string())?;
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
        let mut n = 0;
        for c in cookies {
            let domain = c.domain().unwrap_or("steamcommunity.com").to_string();
            if !(domain.ends_with("steamcommunity.com") || domain.ends_with("steampowered.com")) {
                continue;
            }
            let path = c.path().unwrap_or("/").to_string();
            let name = c.name().to_string();
            let value = c.value().to_string();
            let header = format!("{}={}; Path={}; Domain={}", name, value, path, domain);
            if let Ok(hv) = header.parse::<HeaderValue>() {
                let values = [hv];
                let target = if domain.ends_with("steampowered.com") {
                    &steam_pow
                } else {
                    &steam_com
                };
                self.jar.set_cookies(&mut values.iter(), target);
                n += 1;
            }
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
        let created = match self.ensure_window() {
            Ok(created) => created,
            Err(_) => return false,
        };
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
        self.ensure_window().map_err(|e| e.to_string())?;

        // Early return if we're already logged in from a previous run,
        // unless the caller explicitly wants to force a re-login.
        if !force && self.is_logged_in_inner() {
            return Ok(true);
        }

        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Ok(false);
        };

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

        // Fill + submit the login form. Retry a few times: the SPA may render
        // the inputs asynchronously, and the first eval can land before them.
        // When force-login is active we never break early on an existing
        // cookie — the old session may be stale and we always want to
        // submit fresh credentials.
        let script = build_login_script(username, password);
        for _ in 0..6 {
            w.eval(&script).map_err(|e| e.to_string())?;
            sleep(Duration::from_millis(800)).await;
            if !force && self.is_logged_in_inner() {
                break;
            }
        }

        // Poll for the auth cookie to arrive after submit. 40 × 1s gives the
        // user time to approve a Steam Guard prompt on their phone; the old
        // 25 × 800ms window (20s) expired before most people could react.
        for _ in 0..40 {
            sleep(Duration::from_millis(1000)).await;
            if self.is_logged_in_inner() {
                // Navigate to a plain Workshop page so any further state
                // (age-gate, community country) is picked up.
                let _ = w.eval(format!("window.location.href = {:?}", STEAM_HOME_URL));
                sleep(Duration::from_millis(1500)).await;
                return Ok(true);
            }
        }

        Ok(false)
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
                if (!pass) return {{ ready: false, why: 'no-password' }};

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
                if (!login) return {{ ready: false, why: 'no-username' }};

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
                return {{ ready: true, clicked: !!btn }};
            }} catch (e) {{
                return {{ error: String(e) }};
            }}
        }})();
        "#
    )
}

fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

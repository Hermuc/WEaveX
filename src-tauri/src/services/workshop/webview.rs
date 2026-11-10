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
    /// That made "Relogin to system account" log itself out before it even
    /// tried, and made a successful manual login vanish on the next open.
    /// We now only navigate; clearing the profile is an explicit, opt-in
    /// action (see `force` in `auto_login`).
    pub async fn prepare_login(&self) -> Result<(), tauri::Error> {
        let _g = self.lock.lock().await;
        self.ensure_window()?;
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Ok(());
        };
        w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))?;
        Ok(())
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
            // A forced re-login is the ONE place where dropping the persisted
            // profile is intended (the caller explicitly asked to start over).
            let _ = w.clear_all_browsing_data();
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

    pub async fn login_fill(&self, username: &str, password: &str) -> Result<(), String> {
        let _g = self.lock.lock().await;
        self.ensure_window().map_err(|e| e.to_string())?;

        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Err("No window".into());
        };

        // Do NOT clear the profile here: it also drops any in-flight session
        // cookie the user may still need. Just (re)load the form.
        w.eval(format!("window.location.href = {:?}", STEAM_LOGIN_URL))
            .map_err(|e| e.to_string())?;
        sleep(Duration::from_millis(2500)).await;

        let script = build_login_script(username, password);
        for _ in 0..4 {
            let _ = w.eval(&script);
            sleep(Duration::from_millis(600)).await;
        }
        Ok(())
    }

    pub async fn login_fill_2fa(&self, code: &str) -> Result<(), String> {
        let _g = self.lock.lock().await;
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Err("No window".into());
        };
        let script = build_2fa_script(code);
        w.eval(&script).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn login_switch_to_code(&self) -> Result<(), String> {
        let _g = self.lock.lock().await;
        let Some(w) = self.app.get_webview_window(LABEL) else {
            return Err("No window".into());
        };
        let script = r#"
        (function() {
            var switchBtn = Array.from(document.querySelectorAll('div, button, a')).find(el => {
                var t = el.innerText || '';
                return (t.includes('кода') || t.includes('code') || t.includes('Code')) && !el.querySelector('div');
            });
            if (switchBtn) {
                switchBtn.click();
            }
        })();
        "#;
        w.eval(script).map_err(|e| e.to_string())?;
        Ok(())
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
                // IMPORTANT: `querySelector('input[type="text"]')` is NOT safe
                // here. The login page also carries a store search box
                // (`input[name="term"]`, placeholder "搜索商店"/"search") and
                // hidden auth-code boxes — the old code happily typed the
                // username into the search field, so the credential form stayed
                // empty and the submit button never enabled. Pick the field by
                // what identifies a *credential* form instead, and explicitly
                // skip the known non-login inputs.
                var isHidden = function(el) {{
                    return !el.offsetParent && el.style.display === 'none' || el.disabled;
                }};
                var looksLikeSearch = function(el) {{
                    var n = (el.getAttribute('name') || '').toLowerCase();
                    var ph = (el.getAttribute('placeholder') || '').toLowerCase();
                    var ty = (el.getAttribute('type') || '').toLowerCase();
                    if (ty === 'hidden' || ty === 'submit' || ty === 'button') return true;
                    if (n === 'term' || n === 'snr' || n === 'search') return true;
                    if (ph.indexOf('搜索') >= 0 || ph.indexOf('search') >= 0) return true;
                    return false;
                }};
                var isAuthCodeBox = function(el) {{
                    var id = (el.getAttribute('id') || '').toLowerCase();
                    var cls = (el.getAttribute('class') || '').toLowerCase();
                    return id.indexOf('authcode') >= 0
                        || id.indexOf('twofactor') >= 0
                        || id.indexOf('friendlyname') >= 0
                        || cls.indexOf('authcode') >= 0;
                }};

                var inputs = Array.prototype.slice.call(document.querySelectorAll('input'));
                var pass = null, login = null;
                // 1) The password field anchors the credential form.
                for (var i = 0; i < inputs.length; i++) {{
                    var el = inputs[i];
                    if ((el.getAttribute('type') || '') === 'password' && !looksLikeSearch(el)) {{
                        pass = el; break;
                    }}
                }}
                if (pass) {{
                    // 2) Username = nearest preceding visible text input in the
                    //    same form (or, failing that, the first credential-ish
                    //    text input that is not a search/auth-code box).
                    var form = pass.form || pass.closest('form') || document.body;
                    var scope = Array.prototype.slice.call(form.querySelectorAll('input'));
                    var passIdx = scope.indexOf(pass);
                    for (var j = passIdx - 1; j >= 0; j--) {{
                        var c = scope[j];
                        var t = (c.getAttribute('type') || '').toLowerCase();
                        if (t !== 'text' && t !== 'email' && t !== 'tel') continue;
                        if (looksLikeSearch(c) || isAuthCodeBox(c)) continue;
                        login = c; break;
                    }}
                    if (!login) {{
                        for (var k = 0; k < inputs.length; k++) {{
                            var d = inputs[k];
                            var td = (d.getAttribute('type') || '').toLowerCase();
                            if (td !== 'text' && td !== 'email' && td !== 'tel') continue;
                            if (looksLikeSearch(d) || isAuthCodeBox(d)) continue;
                            login = d; break;
                        }}
                    }}
                }}
                if (!login || !pass) return {{ready:false}};
                function fill(el, v) {{
                    el.focus();
                    var s = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
                    s.call(el, v);
                    el.dispatchEvent(new Event('input', {{bubbles:true}}));
                    el.dispatchEvent(new Event('change', {{bubbles:true}}));
                }}
                fill(login, "{user}");
                fill(pass, "{pass}");
                // Submit button: never trust `button[type="submit"]` — the page
                // header is full of submit buttons ("浏览"/"推荐"/"类别"…) that are
                // part of navigation forms. Match on the visible label instead,
                // and only inside the credential form when one is available.
                var labelHit = function(el) {{
                    var t = ((el.innerText || el.value || '') + '').trim().toLowerCase();
                    if (!t) return false;
                    return t === '登录' || t === '登入' || t === 'sign in' || t === 'login'
                        || t.indexOf('sign in') >= 0 || t.indexOf('登录') >= 0;
                }};
                var btn = null;
                var containers = [];
                if (pass.form) containers.push(pass.form);
                else if (pass.closest('form')) containers.push(pass.closest('form'));
                containers.push(document);
                for (var ci = 0; ci < containers.length && !btn; ci++) {{
                    var cands = Array.prototype.slice.call(
                        containers[ci].querySelectorAll('button, input[type="submit"], a')
                    );
                    for (var bi = 0; bi < cands.length; bi++) {{
                        var cand = cands[bi];
                        if (cand.disabled) continue;
                        if ((cand.getAttribute('type') || '').toLowerCase() === 'hidden') continue;
                        if (labelHit(cand)) {{ btn = cand; break; }}
                    }}
                }}
                if (btn) {{
                    btn.disabled = false;
                    btn.click();
                    return {{ready:true, clicked:true}};
                }}
                // Fallback: submit the form directly (Enter on the password box
                // triggers the same handler in the server-rendered page).
                if (pass.form) {{
                    pass.focus();
                    var ev = new KeyboardEvent('keydown', {{bubbles:true, key:'Enter', keyCode:13, which:13}});
                    pass.dispatchEvent(ev);
                    return {{ready:true, clicked:false, submitted:true}};
                }}
                return {{ready:true, clicked:false}};
            }} catch (e) {{
                return {{error: String(e)}};
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

fn build_2fa_script(code: &str) -> String {
    let code = escape_js_string(code);
    format!(
        r#"
        (function(code) {{
            try {{
                var all = Array.prototype.slice.call(document.querySelectorAll('input'));
                var usable = all.filter(function(i) {{
                    var ty = (i.getAttribute('type') || '').toLowerCase();
                    if (ty !== 'text' && ty !== 'tel' && ty !== 'number') return false;
                    if (i.disabled) return false;
                    var id = (i.getAttribute('id') || '').toLowerCase();
                    var nm = (i.getAttribute('name') || '').toLowerCase();
                    var ph = (i.getAttribute('placeholder') || '').toLowerCase();
                    // Skip the store search box and unrelated inputs — the old
                    // code typed the Steam Guard code into the search field.
                    if (nm === 'term' || nm === 'snr' || nm === 'search') return false;
                    if (ph.indexOf('搜索') >= 0 || ph.indexOf('search') >= 0) return false;
                    // Prefer the boxes Steam actually uses for 2FA.
                    return true;
                }});
                var seg = usable.filter(function(i) {{
                    var cls = (i.getAttribute('class') || '');
                    return cls.indexOf('Segmented') >= 0 || cls.indexOf('twofactor') >= 0
                        || (i.getAttribute('id') || '').toLowerCase().indexOf('twofactor') >= 0;
                }});
                if (seg.length === 0) seg = usable;
                if (seg.length > 0) {{
                    if (seg.length >= 5) {{
                        for (var i = 0; i < code.length && i < seg.length; i++) {{
                            seg[i].focus();
                            var s = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
                            if(s) s.call(seg[i], code[i]);
                            else seg[i].value = code[i];
                            seg[i].dispatchEvent(new Event('input', {{bubbles:true}}));
                            seg[i].dispatchEvent(new Event('change', {{bubbles:true}}));
                            seg[i].dispatchEvent(new KeyboardEvent('keyup', {{bubbles:true, key: code[i]}}));
                        }}
                    }} else {{
                        var i0 = seg[0];
                        i0.focus();
                        var s = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
                        if(s) s.call(i0, code);
                        else i0.value = code;
                        i0.dispatchEvent(new Event('input', {{bubbles:true}}));
                        i0.dispatchEvent(new Event('change', {{bubbles:true}}));
                        i0.dispatchEvent(new KeyboardEvent('keydown', {{bubbles:true, key: 'Enter', keyCode: 13}}));
                    }}
                }}
            }} catch (e) {{
                console.error(e);
            }}
        }})("{code}");
        "#
    )
}

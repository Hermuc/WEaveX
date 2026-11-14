//! Driving the login form: navigate, fill, and wait for the session.
//!
//! Split out of `webview.rs`; these methods steer the webview through
//! Steam's login page and the community-session step.

use tauri::Manager;

use tokio::time::sleep;

use super::{SteamWebview, LABEL, STEAM_HOME_URL, STEAM_LOGIN_URL};

use super::scripts::{build_login_script, build_page_probe_script};
use super::{
    AUTH_COOKIE_POLL, COMMUNITY_PAGE_SETTLE, FORM_CHECK_SETTLE,
    FORM_PAINT_POLL, FORM_RENDER_POLL, LOGIN_PAGE_PAINT_WAIT, NAV_RETRY_WAIT, SUBMIT_RETRY_WAIT,
};

impl SteamWebview {
    /// Navigate to the login page and wait until the credential form is
    /// actually rendered (i.e. a password field exists).
    ///
    /// This has to be a *wait loop*, not a fixed sleep: after a logout Steam
    /// redirects through store/community/help pages, and navigating before that
    /// chain settles leaves the webview on the wrong page (the script then finds
    /// no inputs and the login silently never happens).
    pub(super) async fn goto_login_form(&self) -> Result<bool, String> {
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
}

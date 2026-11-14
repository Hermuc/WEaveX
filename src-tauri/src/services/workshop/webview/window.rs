//! Window lifecycle for the hidden Steam webview: creation, visibility
//! and the off-screen parking that stops its compositor from bleeding
//! through when hidden.
//!
//! Split out of `webview.rs`; these methods touch only `self.app` and
//! the window handle.

use std::path::PathBuf;
use std::sync::Arc;

use reqwest::cookie::Jar;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex;

use super::{install_cookies_for_window, SteamWebview, LABEL, STEAM_LOGIN_URL, WINDOW_CREATE_TIMEOUT};


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
    pub(super) fn window_exists(&self) -> bool {
        self.app.get_webview_window(LABEL).is_some()
    }

    /// Create (if missing) and return the hidden Steam webview. Idempotent.
    /// Returns `true` when a new window was created on this call, `false`
    /// when it already existed — callers use this to know whether they
    /// should give WebView2 a moment to restore the persisted cookie jar.
    pub(super) fn ensure_window(&self) -> Result<bool, tauri::Error> {
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
            match rx.recv_timeout(WINDOW_CREATE_TIMEOUT) {
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
        self.wait_for_login(true, 40, 150).await
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
        install_cookies_for_window(&w, &secure, &session_id)?;
        log::info!("[steam-webview] installed saved session cookies");
        Ok(())
    }

    /// Move the window back on screen before it is shown.
    ///
    /// The window is created parked at -32000,-32000 so its WebView2
    /// composition surface cannot bleed through while hidden. Every visible
    /// path has to undo that, otherwise the user gets an off-screen window and
    /// thinks the login button does nothing.
    pub(super) fn restore_on_screen(w: &tauri::WebviewWindow) {
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
}

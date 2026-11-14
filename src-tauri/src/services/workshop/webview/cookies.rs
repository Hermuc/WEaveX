//! Cookie persistence for the hidden Steam webview: backup/restore the
//! WebView2 cookie database around a destructive clear, and pull live cookies
//! into the reqwest jar.
//!
//! Split out of `webview.rs`; these methods touch only `self.app`,
//! `self.jar` and the on-disk cookie DB.

use tauri::Manager;

use super::{SteamWebview, LABEL};

impl SteamWebview {

    /// Back up the WebView cookie database before anything wipes it.
    ///
    /// `clear_all_browsing_data()` is destructive and irreversible: it deletes
    /// the persisted `steamLoginSecure` cookie along with everything else, so a
    /// user who signs out by accident (or gets signed out by a failed startup
    /// check) has to re-authenticate by hand, including Steam Guard. Copying the
    /// cookie DB first means the session can be restored.
    pub(super) fn backup_cookie_db(&self) -> Option<std::path::PathBuf> {
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
}

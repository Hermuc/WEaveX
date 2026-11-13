use tauri::Manager;

use crate::commands::AppStateHandle;
use crate::workshop::SteamAccount;

use tauri::Emitter;

/// Just opens the parser webview window without starting any login-polling loop.
/// Safe to call when already logged in — will not fire "steam-login-success".
#[tauri::command]
pub async fn steam_parser_show(state: AppStateHandle<'_>) -> Result<(), String> {
    state
        .steam_webview
        .show_login()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn steam_login_hide(state: AppStateHandle<'_>) -> Result<(), String> {
    state.steam_webview.hide().await.map_err(|e| e.to_string())
}

/// Show the Steam login window **on its current page** so the user can finish
/// Steam Guard / device authorisation themselves. Does NOT reload the URL.
#[tauri::command]
pub async fn steam_login_reveal(state: AppStateHandle<'_>) -> Result<(), String> {
    state
        .steam_webview
        .reveal()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn steam_sync_cookies(state: AppStateHandle<'_>) -> Result<usize, String> {
    let n = state.steam_webview.sync_cookies().await?;
    // Clear page cache so that subsequent requests go through with the new
    // cookies and don't return stale anonymous pages.
    state.workshop.clear_caches();
    Ok(n)
}

#[tauri::command]
pub async fn steam_is_logged_in(state: AppStateHandle<'_>) -> Result<bool, String> {
    // Same startup race as `steam_current_account`: `is_logged_in` reports
    // `false` when the window does not exist yet, which is exactly the state
    // during the first moments after launch. Ensure the profile is loaded
    // first so the answer reflects the persisted session, not the timing.
    state.steam_webview.ensure_profile_loaded().await;
    Ok(state.steam_webview.is_logged_in().await)
}

/// Ask Steam itself who the scraper is currently acting as. Returns `None`
/// if no valid Steam session is attached to the reqwest client — which is
/// also the hook the frontend uses to warn the user that the Workshop
/// parser is running anonymously. This is the check the user requested:
/// the account is resolved by hitting steamcommunity.com, not by reading
/// whatever is selected in the Settings dialog.
#[tauri::command]
pub async fn steam_current_account(
    state: AppStateHandle<'_>,
) -> Result<Option<SteamAccount>, String> {
    // Make sure the webview exists BEFORE syncing.
    //
    // `sync_cookies` returns `Ok(0)` and does nothing when the hidden window
    // has not been created yet — and at startup it is created on a background
    // thread by `init_cookies`. The frontend's bootstrap probe fires at the
    // same moment, so whichever won the race decided whether the user looked
    // signed in. Waiting here (idempotent, and free once the window exists)
    // makes the result deterministic instead of a coin flip.
    state.steam_webview.ensure_profile_loaded().await;
    // Make sure whatever cookies are in the webview profile are
    // reflected in the reqwest jar before we query Steam.
    let _ = state.steam_webview.sync_cookies().await;
    Ok(state.workshop.current_account().await)
}

/// Attempt to sign the hidden webview into Steam.
///
/// When `account_index` is supplied that account is used; otherwise the first
/// account the user added is used (there is no bundled fallback). Returns true
/// on success. Safe to call repeatedly — no-ops if already logged in. When
/// `force` is true the current session is terminated first and a fresh login
/// is always attempted.
#[tauri::command]
pub async fn steam_auto_login(
    state: AppStateHandle<'_>,
    account_index: Option<usize>,
    force: Option<bool>,
) -> Result<bool, String> {
    if !force.unwrap_or(false) {
        // When not forcing, check for an existing session — but verify it's
        // actually valid by hitting Steam's profile endpoint. A stale
        // `steamLoginSecure` cookie can survive for a long time after the
        // session has expired server-side, so we must not trust it blindly.
        if state.steam_webview.is_logged_in().await {
            let _ = state.steam_webview.sync_cookies().await;
            if state.workshop.current_account().await.is_some() {
                state.workshop.clear_caches();
                return Ok(true);
            }
            // Session cookie exists but is stale — fall through to re-login.
        }
    }
    // Account selection:
    //   Some(i) -> the account at that index in the user's account list
    //   None    -> the first account the user added (there is no bundled pool,
    //              so this is simply that account, or empty if none exists).
    let creds = match account_index {
        Some(i) => state.accounts.credentials(i),
        None => state.accounts.preferred_parser_credentials(),
    };
    if creds.username.is_empty() || creds.password.is_empty() {
        // Surfacing this as Ok(false) made the UI print "Unknown error".
        return Err("No Steam credentials available. Add your account in Settings -> Accounts                     (Add custom account) and try again."
            .to_string());
    }
    let ok = state
        .steam_webview
        .auto_login(&creds.username, &creds.password, force.unwrap_or(false))
        .await?;
    if ok {
        let _ = state.steam_webview.sync_cookies().await;
        state.workshop.clear_caches();
    }
    Ok(ok)
}

/// Sign in to Steam **with the credentials the user typed**.
///
/// This is the manual path and must never be re-routed through a stored
/// account: an earlier flow dropped the typed credentials and called
/// `steam_auto_login(accountIndex: None)`, so a manual login could never
/// succeed as the user.
#[tauri::command]
pub async fn steam_login_fill(
    state: AppStateHandle<'_>,
    username: &str,
    password: &str,
) -> Result<bool, String> {
    state
        .steam_webview
        .login_fill(username.trim(), password)
        .await
}

#[tauri::command]
pub async fn steam_login_prepare(state: AppStateHandle<'_>) -> Result<(), String> {
    state
        .steam_webview
        .prepare_login()
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct QrBeginResponse {
    pub client_id: String,
    pub challenge_url: String,
    pub request_id: String,
    /// Ready-to-use `data:image/png;base64,...` rendering of `challenge_url`.
    /// Generated locally so the dialog never depends on an external QR service
    /// (which the CSP blocks and which is often unreachable from CN).
    pub qr_data_url: String,
}

#[tauri::command]
pub async fn steam_qr_begin() -> Result<QrBeginResponse, String> {
    let client = reqwest::Client::new();
    let res = client
        .post("https://api.steampowered.com/IAuthenticationService/BeginAuthSessionViaQR/v1/")
        .form(&[("device_friendly_name", "WEave"), ("platform_type", "2")])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

    let response = json.get("response").ok_or("No response field")?;
    let client_id = response
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or("No client_id")?
        .to_string();
    let challenge_url = response
        .get("challenge_url")
        .and_then(|v| v.as_str())
        .ok_or("No challenge_url")?
        .to_string();
    let request_id = response
        .get("request_id")
        .and_then(|v| v.as_str())
        .ok_or("No request_id")?
        .to_string();

    // Render the challenge locally. A failure here must not break the login
    // flow — the dialog can still fall back to the raw challenge URL.
    let qr_data_url = crate::qr::render_qr_data_url(&challenge_url).unwrap_or_default();

    Ok(QrBeginResponse {
        client_id,
        challenge_url,
        request_id,
        qr_data_url,
    })
}

#[tauri::command]
pub async fn steam_qr_poll(
    client_id: String,
    request_id: String,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let res = client
        .post("https://api.steampowered.com/IAuthenticationService/PollAuthSessionStatus/v1/")
        .form(&[("client_id", &client_id), ("request_id", &request_id)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(json)
}

#[tauri::command]
pub async fn steam_qr_login_finalize(
    state: AppStateHandle<'_>,
    access_token: String,
    _refresh_token: String,
) -> Result<(), String> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() < 2 {
        return Err("Invalid access token".into());
    }

    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| e.to_string())?;
    let jwt: serde_json::Value = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;

    let steamid = jwt["sub"].as_str().ok_or("No sub in JWT")?;
    let steam_login_secure = format!("{}%7C%7C{}", steamid, access_token);

    use rand::RngExt;
    let sessionid: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();

    let Some(w) = state
        .app_handle
        .get_webview_window(crate::workshop::webview::LABEL)
    else {
        return Err("No webview window".into());
    };

    let script = format!(
        r#"
        document.cookie = "steamLoginSecure={}; path=/; domain=steamcommunity.com; secure; samesite=none; max-age=31536000";
        document.cookie = "sessionid={}; path=/; domain=steamcommunity.com; secure; samesite=none; max-age=31536000";
        window.location.href = "https://steamcommunity.com/";
        "#,
        steam_login_secure, sessionid
    );
    w.eval(&script).map_err(|e| e.to_string())?;

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    let _ = state.steam_webview.sync_cookies().await;
    state.workshop.clear_caches();

    let _ = state.app_handle.emit("steam-login-success", ());

    Ok(())
}

#[tauri::command]
pub async fn steam_login_poll_error(state: AppStateHandle<'_>) -> Result<Option<String>, String> {
    let Some(w) = state
        .app_handle
        .get_webview_window(crate::workshop::webview::LABEL)
    else {
        return Ok(None);
    };

    let script = r#"
        (function() {
            try {
                var errDiv = Array.from(document.querySelectorAll('form div')).find(d => {
                    var style = window.getComputedStyle(d);
                    if (style.opacity === '0' || style.display === 'none' || style.visibility === 'hidden' || d.offsetHeight === 0) return false;
                    
                    var t = d.innerText || '';
                    if (!t || t.length < 10 || t.length > 200) return false;
                    
                    if (d.querySelector('input') || d.querySelector('button') || d.querySelector('svg') || d.querySelector('a')) return false;

                    var tLower = t.toLowerCase();
                    var isError = false;
                    var errorWords = [
                        'incorrect', 'check', 'try again', 'too many', 'invalid', 'error', 'captcha', 'sorry',
                        '\u043f\u0440\u043e\u0432\u0435\u0440\u044c\u0442\u0435', // проверьте
                        '\u043d\u0435\u0432\u0435\u0440', // невер
                        '\u0441\u043d\u043e\u0432\u0430', // снова
                        '\u0441\u043b\u0438\u0448\u043a\u043e\u043c', // слишком
                        '\u043e\u0448\u0438\u0431', // ошиб
                        '\u043a\u0430\u043f\u0447', // капч
                        '\u0441\u043e\u0432\u043f\u0430\u0434', // совпад
                        '\u0438\u0437\u0432\u0438\u043d\u0438\u0442\u0435' // извините
                    ];
                    
                    for (var i = 0; i < errorWords.length; i++) {
                        if (tLower.includes(errorWords[i])) {
                            isError = true;
                            break;
                        }
                    }
                    
                    return isError;
                });
                if (errDiv) {
                    var errStr = errDiv.innerText.trim();
                    history.replaceState(null, '', window.location.pathname + window.location.search + '#WE_ERR:' + encodeURIComponent(errStr));
                } else {
                    history.replaceState(null, '', window.location.pathname + window.location.search);
                }
            } catch(e) {}
        })();
    "#;
    w.eval(script).ok();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if let Ok(url) = w.url() {
        if let Some(fragment) = url.fragment() {
            if fragment.starts_with("WE_ERR:") {
                let encoded = fragment.replace("WE_ERR:", "");
                if let Ok(decoded) = urlencoding::decode(&encoded) {
                    return Ok(Some(decoded.to_string()));
                }
            }
        }
    }

    Ok(None)
}

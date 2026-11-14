use tauri::Manager;

use crate::commands::AppStateHandle;
use crate::workshop::SteamAccount;

use tauri::Emitter;

use crate::services::steam_auth::jwt_subject;
use steamroom_client::login::CredentialsLoginFlow;
use steamroom_client::login::LoginBuilder;

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
    // A forced login destroys the current session before retrying. Only the
    // interactive "sign in again" path passes `force`, so refuse to do it when
    // a perfectly good session cookie is still present: wiping it would log the
    // user out for no reason, and that is precisely how a signed-in account used
    // to disappear on restart.
    let force = force.unwrap_or(false);
    let wants_forced_relogin = force && !creds.username.is_empty();
    if wants_forced_relogin && state.steam_webview.is_logged_in().await {
        let _ = state.steam_webview.sync_cookies().await;
        if state.workshop.current_account().await.is_some() {
            state.workshop.clear_caches();
            log::info!("[steam-session] refusing forced re-login: session is valid");
            return Ok(true);
        }
    }

    let ok = state
        .steam_webview
        .auto_login(&creds.username, &creds.password, force)
        .await?;
    if ok {
        let _ = state.steam_webview.sync_cookies().await;
        state.workshop.clear_caches();
    }
    Ok(ok)
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
    // Same JWT decoding as the OAuth path; one implementation, not two.
    let steamid = jwt_subject(&access_token)
        .ok_or_else(|| "Invalid access token: no SteamID claim".to_string())?;
    let steam_login_secure =
        crate::accounts::StoredSteamSession::steam_login_secure(&steamid, &access_token);

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

    // Write the cookies through WebView2’s own cookie store, not via
    // `document.cookie`.
    //
    // `steamLoginSecure` is HttpOnly — Steam sets it that way precisely so
    // scripts cannot read or forge it. A `document.cookie` assignment is
    // silently ignored for HttpOnly names, so the old code ran without error
    // and produced no session at all. `WebviewWindow::set_cookie` goes through
    // the platform cookie manager and can set HttpOnly values.
    //
    // Both domains matter: the community site serves the pages we scrape, while
    // the store domain is what the login flow itself uses.
    crate::workshop::webview::install_cookies_for_window(
        &w,
        &steam_login_secure,
        &sessionid,
    )?;

    w.eval("window.location.href = 'https://steamcommunity.com/'")
        .map_err(|e| e.to_string())?;

    // Let the new community page finish loading before reading cookies back;
    // the navigation above is asynchronous from the webview's point of view.
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    let _ = state.steam_webview.sync_cookies().await;
    state.workshop.clear_caches();

    // Persist the session so the next launch can skip the whole QR dance.
    let account = state.accounts.preferred_parser_credentials().username;
    if !account.is_empty() {
        let session = crate::accounts::StoredSteamSession {
            steam_id: steamid.to_string(),
            access_token: access_token.clone(),
            refresh_token: _refresh_token.clone(),
            saved_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        match state.accounts.set_session(&account, session) {
            Ok(()) => log::info!("[steam-session] saved QR session for {account}"),
            Err(e) => log::warn!("[steam-session] could not save QR session: {e}"),
        }
    }

    let _ = state.app_handle.emit("steam-login-success", ());

    Ok(())
}

/// Probe the current Steam session, distinguishing failure from signed-out.
///
/// Returns `{ state: "signed_in" | "signed_out" | "unknown", account?: {...} }`.
/// `unknown` means the HTTP call itself failed — the caller must not treat that
/// as "the user is signed out".
#[tauri::command]
pub async fn steam_probe_account(state: AppStateHandle<'_>) -> Result<serde_json::Value, String> {
    state.steam_webview.ensure_profile_loaded().await;
    let _ = state.steam_webview.sync_cookies().await;

    let result = state.workshop.probe_account().await;
    log::info!(
        "[probe] steam_probe_account -> {}",
        match &result {
            crate::workshop::AccountProbe::SignedIn(a) => format!("signed_in({})", a.account_name),
            crate::workshop::AccountProbe::SignedOut => "signed_out".to_string(),
            crate::workshop::AccountProbe::Unknown => "unknown".to_string(),
        }
    );
    match result {
        crate::workshop::AccountProbe::SignedIn(a) => Ok(serde_json::json!({
            "state": "signed_in",
            "account": {
                "persona_name": a.persona_name,
                "account_name": a.account_name,
                "steamid": a.steamid,
                "profile_url": a.profile_url,
                "avatar_url": a.avatar_url,
            },
        })),
        crate::workshop::AccountProbe::SignedOut => {
            Ok(serde_json::json!({ "state": "signed_out" }))
        }
        crate::workshop::AccountProbe::Unknown => Ok(serde_json::json!({ "state": "unknown" })),
    }
}

/// Try to restore a Steam session from the encrypted account store.
///
/// This is the fast path at startup: if a session was saved after an earlier
/// sign-in, its `{steamid, access_token}` pair is enough to rebuild the
/// `steamLoginSecure` cookie and skip the login page (and Steam Guard).
///
/// Returns `true` only when Steam actually accepts the reconstructed session —
/// we verify by asking Steam who we are, not by trusting the cookie.
#[tauri::command]
pub async fn steam_restore_session(
    state: AppStateHandle<'_>,
    username: Option<String>,
) -> Result<bool, String> {
    // Which account? Default to the first one the user added.
    let account = match username {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => state.accounts.preferred_parser_credentials().username,
    };
    if account.is_empty() {
        return Ok(false);
    }

    let Some(session) = state.accounts.session_for(&account) else {
        log::info!("[steam-session] no saved session for {account}");
        return Ok(false);
    };
    if session.steam_id.is_empty() || session.access_token.is_empty() {
        log::info!("[steam-session] saved session for {account} is incomplete");
        return Ok(false);
    }

    // The window has to exist before cookies can be written into it.
    state.steam_webview.ensure_profile_loaded().await;

    state
        .steam_webview
        .install_session_cookies(&session.steam_id, &session.access_token)
        .map_err(|e| format!("install cookies: {e}"))?;

    // Give the webview a moment, then confirm with Steam itself.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let _ = state.steam_webview.sync_cookies().await;
    state.workshop.clear_caches();

    match state.workshop.current_account().await {
        Some(acc) => {
            log::info!("[steam-session] restored session for {}", acc.account_name);
            Ok(true)
        }
        None => {
            log::warn!("[steam-session] saved session for {account} was rejected by Steam");
            Ok(false)
        }
    }
}

/// Persist the OAuth tokens that came out of a successful sign-in.
///

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
                        'incorrect', 'check', 'try again', 'too many', 'invalid', 'error', 'captcha', 'sorry'
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

    // The injected probe writes its result into location.hash; give the browser
    // a tick to run it before we read the URL back.
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

// ───────────────────────── OAuth sign-in ─────────────────────────
//
// The WebView2 form-filling path cannot get past Steam Guard: it fills the
// password and presses the button, Steam answers with a GUARD page, and nothing
// in a headless webview can approve a phone prompt. Steam's own OAuth API is
// designed for this — it reports *which* guard is required, and accepts a code
// typed by the user. `steamroom` wraps the whole flow.
//
// The flow is split across three commands because each step may need something
// from the user (a code, or a tap on their phone), and a Tauri command cannot
// stay suspended waiting for that.

/// A pending OAuth sign-in that still needs a Steam Guard code or a mobile tap.
///
/// Kept in a process-wide slot because the challenge owns an open CM connection
/// and cannot be serialised back to the frontend.
static PENDING_GUARD: parking_lot::Mutex<Option<steamroom_client::login::GuardChallenge>> =
    parking_lot::Mutex::new(None);

#[derive(serde::Serialize)]
pub struct OAuthBeginResult {
    /// `approved` | `guard_code` | `mobile_confirm`
    pub status: String,
    /// For `guard_code`: the kinds Steam accepts ("device_code"/"email_code").
    pub allowed_kinds: Vec<String>,
    /// Set when the sign-in completed without any extra step.
    pub steam_id: Option<String>,
}

fn guard_kind_name(k: steamroom::auth::GuardType) -> &'static str {
    match k {
        steamroom::auth::GuardType::DeviceCode => "device_code",
        steamroom::auth::GuardType::EmailCode => "email_code",
        steamroom::auth::GuardType::DeviceConfirmation => "device_confirmation",
        _ => "other",
    }
}

/// Turn a completed auth flow into a stored session + live webview cookies.
async fn complete_oauth_login(
    state: &AppStateHandle<'_>,
    username: String,
    approved: steamroom_client::login::ApprovedAuth,
) -> Result<String, String> {
    let tokens = approved.tokens().clone();

    // The access token is a JWT; its `sub` claim is the SteamID64.
    let steam_id = jwt_subject(&tokens.access_token)
        .ok_or_else(|| "access token is missing a SteamID".to_string())?;

    // Persist first: even if the browser part fails, the session is safe.
    let session = crate::accounts::StoredSteamSession {
        steam_id: steam_id.clone(),
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    state
        .accounts
        .set_session(&username, session)
        .map_err(|e| format!("save session: {e}"))?;

    // Now make the webview authenticated so the Workshop UI works.
    state.steam_webview.ensure_profile_loaded().await;
    state
        .steam_webview
        .install_session_cookies(&steam_id, &tokens.access_token)
        .map_err(|e| format!("install cookies: {e}"))?;
    // Give the webview a moment to apply the new cookies before syncing them
    // into the reqwest jar and querying Steam.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let _ = state.steam_webview.sync_cookies().await;
    state.workshop.clear_caches();
    let _ = state.app_handle.emit("steam-login-success", ());

    // `finish()` sends the CM logon; do it last and ignore failure (the web
    // session above is what the UI actually needs).
    let _ = approved.finish().await;

    Ok(steam_id)
}

/// Start an OAuth sign-in with a username and password.
///
/// Returns `approved` when no Steam Guard is needed (a saved session exists, or
/// Steam trusts this device); `guard_code` when the user must type a code;
/// `mobile_confirm` when they must approve on their phone.
#[tauri::command]
pub async fn steam_oauth_begin(
    state: AppStateHandle<'_>,
    username: String,
    password: String,
) -> Result<OAuthBeginResult, String> {
    let user = username.trim().to_string();
    if user.is_empty() || password.is_empty() {
        return Err("Username and password are required".into());
    }

    // Remember the credentials so a later retry (or a password change) works.
    let _ = state.accounts.add_user_account(&user, &password);

    let flow = LoginBuilder::new()
        .device_name("WEaveR")
        .with_credentials(&user, &password)
        .begin()
        .await
        .map_err(|e| match e {
            steamroom_client::login::LoginError::InvalidPassword => "INVALID_PASSWORD".to_string(),
            other => format!("login failed: {other}"),
        })?;

    match flow {
        CredentialsLoginFlow::Approved(auth) => {
            let steam_id = complete_oauth_login(&state, user, auth).await?;
            Ok(OAuthBeginResult {
                status: "approved".into(),
                allowed_kinds: Vec::new(),
                steam_id: Some(steam_id),
            })
        }
        CredentialsLoginFlow::NeedsGuardCode(challenge) => {
            let kinds = challenge
                .allowed_kinds()
                .iter()
                .map(|k| guard_kind_name(*k).to_string())
                .collect();
            *PENDING_GUARD.lock() = Some(challenge);
            Ok(OAuthBeginResult {
                status: "guard_code".into(),
                allowed_kinds: kinds,
                steam_id: None,
            })
        }
        CredentialsLoginFlow::NeedsMobileConfirm(challenge) => {
            // Wait for the tap inside this call; the UI shows a spinner meanwhile.
            let approved = challenge
                .wait_for_confirmation()
                .await
                .map_err(|e| format!("mobile confirmation failed: {e}"))?;
            let steam_id = complete_oauth_login(&state, user, approved).await?;
            Ok(OAuthBeginResult {
                status: "approved".into(),
                allowed_kinds: Vec::new(),
                steam_id: Some(steam_id),
            })
        }
        _ => Err("unsupported Steam confirmation flow".into()),
    }
}

/// Submit the Steam Guard code the user typed.
#[tauri::command]
pub async fn steam_oauth_submit_guard(
    state: AppStateHandle<'_>,
    code: String,
    kind: Option<String>,
) -> Result<String, String> {
    let challenge = PENDING_GUARD
        .lock()
        .take()
        .ok_or_else(|| "no pending Steam Guard challenge".to_string())?;

    let guard_kind = match kind.as_deref() {
        Some("device_code") => steamroom::auth::GuardType::DeviceCode,
        Some("email_code") => steamroom::auth::GuardType::EmailCode,
        _ => *challenge
            .allowed_kinds()
            .first()
            .ok_or_else(|| "no guard kind available".to_string())?,
    };

    let approved = match challenge.submit_code(code.trim(), guard_kind).await {
        Ok(a) => a,
        Err((challenge_back, e)) => {
            // Keep the challenge alive so the user can retype the code.
            *PENDING_GUARD.lock() = Some(challenge_back);
            return Err(match e {
                steamroom_client::login::LoginError::InvalidGuardCode => {
                    "INVALID_GUARD_CODE".to_string()
                }
                other => format!("guard submit failed: {other}"),
            });
        }
    };

    let user = state.accounts.preferred_parser_credentials().username;
    complete_oauth_login(&state, user, approved).await
}

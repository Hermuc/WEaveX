//! OAuth (non-QR) Steam sign-in: begin, submit-guard, and the shared
//! flow that turns an approved challenge into a stored session.
//!
//! Split out of `commands/steam.rs` so the QR and OAuth paths read as
//! separate concerns; re-exported from the parent module.

use tauri::Emitter;

use crate::commands::AppStateHandle;
use crate::services::steam_auth::jwt_subject;
use steamroom_client::login::{CredentialsLoginFlow, LoginBuilder};

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

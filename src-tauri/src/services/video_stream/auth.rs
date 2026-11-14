//! Steam login for the streaming pipeline, plus the token cache it leans on.
//!
//! Split out of the main module because it is a self-contained concern: nothing
//! here knows about manifests, chunk layouts or byte ranges — it only answers
//! "give me a logged-in client".

use steamroom::client::{LoggedIn, SteamClient};
use steamroom_client::login::CredentialsLoginFlow;
use steamroom_client::login::LoginBuilder;

/// Cached login so repeated clicks do not each pay a full CM handshake.
///
/// A `SteamClient` is cheap to reuse but expensive to create (connect, encrypt,
/// logon). The streaming UI can be clicked several times in a row, and each
/// miss used to cost 10-40s. The cached client is validated lazily: if a call
/// on it fails the caller falls back to a fresh login.
static SESSION: std::sync::Mutex<Option<CachedSession>> = std::sync::Mutex::new(None);

struct CachedSession {
    username: String,
    client: SteamClient<LoggedIn>,
}

/// Get a logged-in client, reusing the cached one when the account matches.
pub async fn shared_login(
    username: &str,
    password: &str,
) -> anyhow::Result<std::sync::Arc<SteamClient<LoggedIn>>> {
    // Reuse only for the same account; a different account must re-auth.
    {
        let guard = SESSION.lock().unwrap();
        if let Some(s) = guard.as_ref() {
            if s.username == username {
                return Ok(std::sync::Arc::new(s.client.clone()));
            }
        }
    }
    let client = login(username, password).await?;
    {
        let mut guard = SESSION.lock().unwrap();
        *guard = Some(CachedSession {
            username: username.to_string(),
            client: client.clone(),
        });
    }
    Ok(std::sync::Arc::new(client))
}

/// Drop the cached session (call when the account changes or a call fails).
pub fn forget_session() {
    *SESSION.lock().unwrap() = None;
}

/// Establish a logged-in Steam session for the streaming pipeline.
///
/// Tried in order, most convenient first:
///   1. Local Steam client token (DPAPI) — no password, no Steam Guard.
///   2. A refresh token WEave cached from an earlier sign-in.
///   3. Username + password — only for accounts without Steam Guard.
pub async fn login(username: &str, password: &str) -> anyhow::Result<SteamClient<LoggedIn>> {
    if let Some(tok) = steamroom_client::steam_creds::extract_token(username) {
        match LoginBuilder::new()
            .device_name("WEaveR")
            .with_refresh_token(&tok.account_name, &tok.refresh_token)
            .login()
            .await
        {
            Ok(client) => {
                log::info!("[video-stream] logged in via local Steam token");
                return Ok(client);
            }
            Err(e) => log::warn!("[video-stream] local Steam token failed: {e}"),
        }
    }
    if let Some(tok) = load_cached_token(username) {
        match LoginBuilder::new()
            .device_name("WEaveR")
            .with_refresh_token(username, &tok)
            .login()
            .await
        {
            Ok(client) => {
                log::info!("[video-stream] logged in via cached refresh token");
                return Ok(client);
            }
            Err(e) => log::warn!("[video-stream] cached refresh token failed: {e}"),
        }
    }
    let flow = LoginBuilder::new()
        .device_name("WEaveR")
        .with_credentials(username, password)
        .begin()
        .await
        .map_err(|e| anyhow::anyhow!("steam login: {e}"))?;
    match flow {
        CredentialsLoginFlow::Approved(a) => {
            let tokens = a.tokens();
            if let Some(rt) = tokens.refresh_token.clone().into() {
                let _ = save_cached_token(username, &rt);
            }
            a.finish()
                .await
                .map_err(|e| anyhow::anyhow!("steam logon: {e}"))
        }
        CredentialsLoginFlow::NeedsGuardCode(_) | CredentialsLoginFlow::NeedsMobileConfirm(_) => {
            Err(anyhow::anyhow!(
                "STEAM_GUARD_REQUIRED: sign in to the Steam desktop app so a reusable token exists"
            ))
        }
        _ => Err(anyhow::anyhow!(
            "steam login: unsupported confirmation flow"
        )),
    }
}

fn token_cache_path(username: &str) -> Option<std::path::PathBuf> {
    let base = crate::app_state::resolve_app_data_dir();
    std::fs::create_dir_all(&base).ok()?;
    let safe: String = username
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(base.join(format!("steam_token_{safe}.txt")))
}

fn load_cached_token(username: &str) -> Option<String> {
    let p = token_cache_path(username)?;
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn save_cached_token(username: &str, token: &str) -> std::io::Result<()> {
    if let Some(p) = token_cache_path(username) {
        std::fs::write(p, token)?;
    }
    Ok(())
}

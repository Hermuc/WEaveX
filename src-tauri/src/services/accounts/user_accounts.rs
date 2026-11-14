//! Encrypted storage for user-added Steam accounts.
//!
//! Faithful Rust port of the Python `EncryptedStorage` + `UserAccountsService`:
//! keys are derived from a machine-bound seed via PBKDF2-HMAC-SHA256
//! (100 000 iterations, identical to the original) and the resulting 32-byte
//! key is fed to AES-256-GCM (the Python version used Fernet, which is
//! AES-128-CBC + HMAC; we upgrade to AES-GCM for authenticated encryption).
//!
//! On-disk layout: `<app_data>/user_accounts.enc`
//!   first 12 bytes: nonce
//!   remaining: AES-256-GCM ciphertext + 16-byte tag of a UTF-8 JSON array
//!   `[{"username": "...", "password": "..."}, ...]`.

use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use parking_lot::Mutex;
use pbkdf2::pbkdf2_hmac;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const SALT: &[u8] = b"WEave_Salt_2026_v1";
const ITERATIONS: u32 = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredUserAccount {
    pub username: String,
    pub password: String,
    /// Steam OAuth session, once one has been established for this account.
    ///
    /// `#[serde(default)]` keeps files written by older builds readable — they
    /// simply have no session yet and fall back to a password sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<StoredSteamSession>,
}

/// A Steam OAuth session, as persisted alongside the account.
///
/// Holding the tokens is what makes "sign in once, stay signed in" work:
/// `access_token` can be turned straight into the `steamLoginSecure` cookie
/// (`{steamid}%7C%7C{access_token}`), and `refresh_token` can mint a fresh
/// access token when the old one expires — both without touching Steam Guard.
///
/// Everything here lives inside the AES-256-GCM blob in `user_accounts.enc`, so
/// it is protected by the same machine-bound key as the password.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredSteamSession {
    pub steam_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds, used to reason about token freshness.
    #[serde(default)]
    pub saved_at: u64,
}

impl StoredSteamSession {
    /// Build the cookies that reconstruct a logged-in browser session.
    ///
    /// `steamLoginSecure` is `{steamid}%7C%7C{access_token}` — `%7C%7C` being the
    /// URL-encoded form of `||`, which is exactly how Steam's own front end
    /// spells it. The same value is valid on both the community and store
    /// domains, so both get a copy.
    pub fn login_cookies(&self) -> Vec<(&'static str, String)> {
        let secure = Self::steam_login_secure(&self.steam_id, &self.access_token);
        vec![
            ("steamcommunity.com", secure.clone()),
            ("steampowered.com", secure),
        ]
    }

    /// The one place that spells the `steamLoginSecure` value.
    ///
    /// `%7C%7C` is the URL-encoded form of `||`, which is exactly what Steam's
    /// own front end writes. Callers that only hold the two strings (the
    /// webview's `install_session_cookies`, for instance) use this instead of
    /// re-deriving the format, so there is a single place to get it wrong.
    pub fn steam_login_secure(steam_id: &str, access_token: &str) -> String {
        format!("{steam_id}%7C%7C{access_token}")
    }
}

pub struct UserAccountsStore {
    file_path: PathBuf,
    cipher: Aes256Gcm,
    accounts: Mutex<Vec<StoredUserAccount>>,
}

impl UserAccountsStore {
    pub fn open(app_data_dir: &std::path::Path) -> Self {
        let file_path = app_data_dir.join("user_accounts.enc");
        let cipher = build_cipher();
        let accounts = Mutex::new(load_accounts(&file_path, &cipher));
        Self {
            file_path,
            cipher,
            accounts,
        }
    }

    pub fn list(&self) -> Vec<StoredUserAccount> {
        self.accounts.lock().clone()
    }

    /// Store an account, updating the password when the username already exists.
    ///
    /// This used to reject duplicates outright. That made re-signing-in useless:
    /// the login flow calls this *after* a successful Steam sign-in, so a stale
    /// password on disk could never be refreshed — every later auto-login failed
    /// with the old credentials, and the UI only ever showed "login failed" with
    /// no hint that the stored password was the problem.
    pub fn add(&self, username: &str, password: &str) -> Result<(), String> {
        if username.is_empty() || password.is_empty() {
            return Err("Username and password are required".into());
        }
        let mut guard = self.accounts.lock();
        if let Some(existing) = guard.iter_mut().find(|a| a.username == username) {
            existing.password = password.into();
        } else {
            guard.push(StoredUserAccount {
                username: username.into(),
                password: password.into(),
                session: None,
            });
        }
        save_accounts(&self.file_path, &self.cipher, &guard)
    }

    /// Persist an OAuth session for one account.
    ///
    /// Called right after a successful sign-in so the next launch can skip the
    /// login page entirely.
    pub fn set_session(&self, username: &str, session: StoredSteamSession) -> Result<(), String> {
        let mut guard = self.accounts.lock();
        let Some(entry) = guard.iter_mut().find(|a| a.username == username) else {
            return Err("Account not found".into());
        };
        entry.session = Some(session);
        save_accounts(&self.file_path, &self.cipher, &guard)
    }

    /// The stored session for one account, if any.
    pub fn session_for(&self, username: &str) -> Option<StoredSteamSession> {
        self.accounts
            .lock()
            .iter()
            .find(|a| a.username == username)
            .and_then(|a| a.session.clone())
    }

    pub fn remove(&self, username: &str) -> Result<(), String> {
        let mut guard = self.accounts.lock();
        let before = guard.len();
        guard.retain(|a| a.username != username);
        if guard.len() == before {
            return Err("Account not found".into());
        }
        save_accounts(&self.file_path, &self.cipher, &guard)
    }

    pub fn clear(&self) -> Result<(), String> {
        let mut guard = self.accounts.lock();
        guard.clear();
        save_accounts(&self.file_path, &self.cipher, &guard)
    }
}

fn machine_seed() -> Vec<u8> {
    let node = hostname_string();
    let machine = std::env::consts::ARCH.to_string();
    format!("{node}{machine}").into_bytes()
}

fn hostname_string() -> String {
    // Read the same identifiers that the Python `platform.node()` returns.
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    if let Ok(bytes) = std::fs::read("/etc/hostname") {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "localhost".into()
}

fn build_cipher() -> Aes256Gcm {
    let seed = machine_seed();
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(&seed, SALT, ITERATIONS, &mut key);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key))
}

fn load_accounts(path: &std::path::Path, cipher: &Aes256Gcm) -> Vec<StoredUserAccount> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if bytes.len() < 12 {
        return Vec::new();
    }
    let (nonce_bytes, ct) = bytes.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = match cipher.decrypt(nonce, ct) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    serde_json::from_slice(&plaintext).unwrap_or_default()
}

fn save_accounts(
    path: &std::path::Path,
    cipher: &Aes256Gcm,
    accounts: &[StoredUserAccount],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec(accounts).map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, json.as_ref())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    std::fs::write(path, out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_login_secure_joins_with_the_encoded_pipe() {
        assert_eq!(
            StoredSteamSession::steam_login_secure("76561198000000000", "abc123"),
            "76561198000000000%7C%7Cabc123"
        );
    }

    #[test]
    fn steam_login_secure_keeps_the_token_verbatim() {
        // The token is opaque base64-ish text; it must not be altered.
        let tok = "eyJhbGciOiJIUzI1NiJ9.payload.signature";
        let out = StoredSteamSession::steam_login_secure("1", tok);
        assert!(out.ends_with(tok));
        assert!(out.contains("%7C%7C"));
    }
}

//! Account management. Built-in public accounts are embedded as an encrypted
//! bundle; user-added accounts are stored encrypted on disk via
//! `user_accounts::UserAccountsStore`.

use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose, Engine as _};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod user_accounts;

use user_accounts::UserAccountsStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCredentials {
    pub username: String,
    pub password: String,
    pub is_custom: bool,
}

pub struct AccountManager {
    /// Default + CLI-injected accounts (never persisted to disk).
    builtin: RwLock<Vec<AccountCredentials>>,
    /// Encrypted user accounts persisted to `<app_data>/user_accounts.enc`.
    user_store: Arc<UserAccountsStore>,
}

const BUILTIN_ACCOUNT_BUNDLE_KEY: &[u8] = b"WEave built-in Steam accounts bundle v1";
const BUILTIN_ACCOUNT_BUNDLE: &str = concat!(
    "h0m6wUVpNDnJPxcGwGQcdocH6IIllAM2x1yyS7sHXfqwhaBtXusl+kNUwBne2g8PNUK0xen+chgB49muEjjp",
    "MvUMM5g03SarxUoZf1dhECB0KwsfEZEjRWsBGSD2aO/8ZsUaDOdf6ITjwscsB/bp5yQLRk7PagyfJdBhIiNF2lRoiVJ9Y7DZlJSc7/qAV1eq8z3d2C/VE4djAg9lyQr4Ggde+fYsztziOL7NEQ8JKzoOISS9oIg="
);

#[derive(Debug, Deserialize, Default)]
struct BuiltinAccountsBundle {
    download: Vec<(String, String)>,
    parser: (String, String),
}

impl AccountManager {
    pub fn from_runtime(app_data_dir: &std::path::Path) -> Self {
        let builtin = build_default();
        let user_store = Arc::new(UserAccountsStore::open(app_data_dir));
        Self {
            builtin: RwLock::new(builtin),
            user_store,
        }
    }

    fn merged(&self) -> Vec<AccountCredentials> {
        let mut all = self.builtin.read().clone();
        for stored in self.user_store.list() {
            all.push(AccountCredentials {
                username: stored.username,
                password: stored.password,
                is_custom: true,
            });
        }
        all
    }

    pub fn builtin_count(&self) -> usize {
        self.builtin.read().len()
    }

    pub fn list_usernames(&self) -> Vec<String> {
        self.merged().into_iter().map(|a| a.username).collect()
    }

    /// Return per-account metadata without leaking passwords to the frontend.
    pub fn list_detailed(&self) -> Vec<AccountCredentials> {
        self.merged()
            .into_iter()
            .map(|a| AccountCredentials {
                username: a.username,
                password: String::new(),
                is_custom: a.is_custom,
            })
            .collect()
    }

    pub fn credentials(&self, index: usize) -> AccountCredentials {
        let all = self.merged();
        if all.is_empty() {
            return AccountCredentials {
                username: String::new(),
                password: String::new(),
                is_custom: false,
            };
        }
        all.get(index).cloned().unwrap_or_else(|| all[0].clone())
    }

    pub fn username_at(&self, index: usize) -> String {
        self.credentials(index).username
    }

    pub fn add_user_account(&self, username: &str, password: &str) -> Result<(), String> {
        self.user_store.add(username, password)
    }

    pub fn remove_user_account(&self, username: &str) -> Result<(), String> {
        self.user_store.remove(username)
    }

    pub fn clear_user_accounts(&self) -> Result<(), String> {
        self.user_store.clear()
    }

    /// Custom accounts (only) — used by the SettingsDialog.
    pub fn list_custom_usernames(&self) -> Vec<String> {
        self.user_store
            .list()
            .into_iter()
            .map(|a| a.username)
            .collect()
    }

    /// Credentials to use for the *parser* (Workshop scraping) session.
    ///
    /// Selection order:
    ///   1. the first user-added account (``is_custom == true``) — this is
    ///      what lets a user browse the Workshop as *themselves* instead of
    ///      sharing the bundled public account;
    ///   2. the built-in dedicated parser account, so a fresh install still
    ///      works with zero configuration.
    ///
    /// Before this existed, "Relogin to system account" always forced the
    /// built-in account and silently ignored any account the user had added —
    /// which is exactly why adding a custom account never changed who the
    /// parser was signed in as.
    pub fn preferred_parser_credentials(&self) -> AccountCredentials {
        if let Some(custom) = self.user_store.list().into_iter().next() {
            return AccountCredentials {
                username: custom.username,
                password: custom.password,
                is_custom: true,
            };
        }
        self.parser_credentials()
    }

    /// Credentials for the dedicated parser login account
    /// (`weworkshopmanager2`). Used by `steam_auto_login` when no specific
    /// download account was requested, so the hidden webview always logs
    /// in under this identity regardless of the user's Download Account
    /// selection.
    pub fn parser_credentials(&self) -> AccountCredentials {
        let bundle = load_builtin_bundle().unwrap_or_default();
        AccountCredentials {
            username: bundle.parser.0,
            password: bundle.parser.1,
            is_custom: false,
        }
    }
}

fn build_default() -> Vec<AccountCredentials> {
    load_builtin_bundle()
        .map(|bundle| {
            bundle
                .download
                .into_iter()
                .map(|(username, password)| AccountCredentials {
                    username,
                    password,
                    is_custom: false,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn load_builtin_bundle() -> Result<BuiltinAccountsBundle, String> {
    let raw = general_purpose::STANDARD
        .decode(BUILTIN_ACCOUNT_BUNDLE)
        .map_err(|e| e.to_string())?;
    if raw.len() < 12 {
        return Err("Built-in account bundle is malformed".into());
    }
    let (nonce, ciphertext) = raw.split_at(12);
    let key = Sha256::digest(BUILTIN_ACCOUNT_BUNDLE_KEY);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&plaintext).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Regression: the download path must pick the custom account when the
    /// user has added one and selected it, not silently fall back to an
    /// expired bundled account.
    #[test]
    fn custom_account_is_reachable_by_its_listed_index() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());

        let builtin = mgr.builtin_count();
        mgr.add_user_account("my_steam_user", "my_password")
            .unwrap();

        // accounts_list enumerates merged(), so the custom account sits right
        // after the bundled ones.
        let listed = mgr.list_detailed();
        assert_eq!(listed.len(), builtin + 1);
        let my_index = listed
            .iter()
            .position(|a| a.username == "my_steam_user")
            .expect("custom account must appear in the list");

        // credentials(index) must resolve to that exact account — this is what
        // download_start feeds into starting_index.
        let creds = mgr.credentials(my_index);
        assert_eq!(creds.username, "my_steam_user");
        assert!(
            creds.is_custom,
            "must be flagged custom so it is not rotated"
        );

        // And the parser preference must also prefer it.
        let parser = mgr.preferred_parser_credentials();
        assert_eq!(parser.username, "my_steam_user");
    }

    /// A fresh install has no custom accounts, so the bundled parser account
    /// is used — the app still works out of the box.
    #[test]
    fn preferred_parser_falls_back_to_builtin() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        let creds = mgr.preferred_parser_credentials();
        let builtin = mgr.parser_credentials();
        assert_eq!(creds.username, builtin.username);
        assert!(!creds.is_custom);
    }

    /// Once the user adds their own account it must win over the bundled one;
    /// this is the whole point of the A-fix (the parser logs in as the user).
    #[test]
    fn preferred_parser_prefers_custom_account() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("my_own_account", "secret").unwrap();
        let creds = mgr.preferred_parser_credentials();
        assert_eq!(creds.username, "my_own_account");
        assert_eq!(creds.password, "secret");
        assert!(creds.is_custom);
    }

    /// Removing the custom account restores the built-in fallback.
    #[test]
    fn preferred_parser_reverts_after_removal() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("temp_account", "pw").unwrap();
        assert_eq!(mgr.preferred_parser_credentials().username, "temp_account");
        mgr.remove_user_account("temp_account").unwrap();
        let builtin = mgr.parser_credentials();
        assert_eq!(
            mgr.preferred_parser_credentials().username,
            builtin.username
        );
    }
}

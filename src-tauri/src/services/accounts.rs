//! Account management.
//!
//! Only the accounts a user adds themselves exist: the old encrypted bundle of
//! shared public credentials was removed deliberately. Those accounts belonged
//! to the upstream author, were frequently rate-limited or had their passwords
//! changed (the source of most `InvalidPassword` download failures), and
//! sharing one Steam account across many installs is exactly what Steam's
//! anti-abuse checks look for.
//!
//! A user therefore signs in once, and that single account serves both jobs:
//! the Workshop parser session (via the webview) and downloads (via
//! DepotDownloader). User accounts are stored encrypted on disk via
//! `user_accounts::UserAccountsStore`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod user_accounts;

pub use user_accounts::StoredSteamSession;
use user_accounts::UserAccountsStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCredentials {
    pub username: String,
    pub password: String,
    pub is_custom: bool,
}

pub struct AccountManager {
    /// Encrypted user accounts persisted to `<app_data>/user_accounts.enc`.
    /// This is the only account source: there is no bundled account pool any
    /// more.
    user_store: Arc<UserAccountsStore>,
}

impl AccountManager {
    pub fn from_runtime(app_data_dir: &std::path::Path) -> Self {
        let user_store = Arc::new(UserAccountsStore::open(app_data_dir));
        Self { user_store }
    }

    /// Every account the user has added, in insertion order.
    fn merged(&self) -> Vec<AccountCredentials> {
        self.user_store
            .list()
            .into_iter()
            .map(|stored| AccountCredentials {
                username: stored.username,
                password: stored.password,
                is_custom: true,
            })
            .collect()
    }

    /// How many accounts exist. Zero means "nothing to download with yet".
    ///
    /// Named `builtin_count` historically (it counted the bundled pool); the
    /// download retry loop uses it as "how many accounts can I rotate through",
    /// which is still exactly right — there is just no built-in pool now.
    pub fn builtin_count(&self) -> usize {
        self.user_store.list().len()
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

    pub fn add_user_account(&self, username: &str, password: &str) -> Result<(), String> {
        self.user_store.add(username, password)
    }

    /// Persist a Steam OAuth session for one account.
    pub fn set_session(&self, username: &str, session: StoredSteamSession) -> Result<(), String> {
        self.user_store.set_session(username, session)
    }

    /// The stored OAuth session for one account, if any.
    pub fn session_for(&self, username: &str) -> Option<StoredSteamSession> {
        self.user_store.session_for(username)
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

    /// Credentials for the *parser* (Workshop scraping) session, i.e. the
    /// identity the hidden Steam webview signs in as.
    ///
    /// The first account the user added is used, so browsing the Workshop is
    /// always done as *them* — and that same account is what downloads use,
    /// which is the point of removing the shared pool. Returns empty
    /// credentials when nothing has been added yet; callers surface that as an
    /// "add your account in Settings" message.
    pub fn preferred_parser_credentials(&self) -> AccountCredentials {
        self.user_store
            .list()
            .into_iter()
            .next()
            .map(|custom| AccountCredentials {
                username: custom.username,
                password: custom.password,
                is_custom: true,
            })
            .unwrap_or_else(|| AccountCredentials {
                username: String::new(),
                password: String::new(),
                is_custom: true,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Acceptance test for dropping the bundled pool: a single user account
    /// must satisfy every consumer of credentials — the parser session, the
    /// download path, and the retry-loop budget.
    /// Re-signing in must refresh the stored password, not be rejected.
    ///
    /// `accounts_set_custom` runs after a successful Steam sign-in. When it
    /// refused duplicates, a changed password could never be persisted: the
    /// stale credential on disk made every subsequent auto-login fail, and the
    /// user saw only "login failed" with no way to recover short of deleting
    /// the account file.
    /// The full round-trip the app performs on every restart: save, reopen the
    /// store from disk, and confirm the credentials come back usable.

    /// `steamLoginSecure` must be `{steamid}%7C%7C{access_token}`.
    ///
    /// This exact spelling is what Steam's own front end uses; anything else is
    /// silently rejected and the user sees an endless "not signed in" loop.
    #[test]
    fn session_cookies_use_the_steam_login_secure_format() {
        let session = StoredSteamSession {
            steam_id: "76561198000000001".into(),
            access_token: "eyJhbGciOiJSUzI1NiJ9.test".into(),
            refresh_token: "refresh-abc".into(),
            saved_at: 0,
        };
        let cookies = session.login_cookies();
        assert_eq!(cookies.len(), 2, "community and store domains");
        assert_eq!(cookies[0].0, "steamcommunity.com");
        assert_eq!(cookies[1].0, "steampowered.com");
        assert_eq!(
            cookies[0].1,
            "76561198000000001%7C%7CeyJhbGciOiJSUzI1NiJ9.test"
        );
    }

    /// A session must survive the encrypted round-trip, including its tokens.
    #[test]
    fn session_survives_a_restart() {
        let dir = TempDir::new().unwrap();
        {
            let mgr = AccountManager::from_runtime(dir.path());
            mgr.add_user_account("re_ann1", "pw").unwrap();
            mgr.set_session(
                "re_ann1",
                StoredSteamSession {
                    steam_id: "76561198000000001".into(),
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    saved_at: 12345,
                },
            )
            .unwrap();
        }
        let reopened = AccountManager::from_runtime(dir.path());
        let s = reopened
            .session_for("re_ann1")
            .expect("session must persist");
        assert_eq!(s.steam_id, "76561198000000001");
        assert_eq!(s.access_token, "access");
        assert_eq!(s.refresh_token, "refresh");
    }

    #[test]
    fn saved_account_survives_a_restart() {
        let dir = TempDir::new().unwrap();
        {
            let mgr = AccountManager::from_runtime(dir.path());
            mgr.add_user_account("re_ann1", "secret").unwrap();
        }
        // Fresh instance = what AppState::initialize does on the next launch.
        let reopened = AccountManager::from_runtime(dir.path());
        let creds = reopened.preferred_parser_credentials();
        assert_eq!(creds.username, "re_ann1", "username must survive restart");
        assert_eq!(
            creds.password, "secret",
            "password must decrypt after restart"
        );
    }

    #[test]
    fn re_adding_an_account_updates_the_password() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("re_ann1", "old-password").unwrap();
        mgr.add_user_account("re_ann1", "new-password").unwrap();

        let creds = mgr.preferred_parser_credentials();
        assert_eq!(creds.username, "re_ann1");
        assert_eq!(creds.password, "new-password", "password must be refreshed");

        // And there must still be exactly one entry, not a duplicate.
        assert_eq!(mgr.list_custom_usernames().len(), 1);
    }

    #[test]
    fn single_account_covers_every_consumer() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("solo", "pw").unwrap();

        // 1) Parser session (steam_auto_login with accountIndex: null).
        let p = mgr.preferred_parser_credentials();
        assert_eq!(p.username, "solo");
        assert!(!p.password.is_empty(), "parser needs a password");

        // 2) Download path (download_start -> credentials(starting_index)).
        let d = mgr.credentials(0);
        assert_eq!(d.username, "solo");
        assert!(!d.password.is_empty(), "download needs a password");

        // 3) Retry-loop budget: one account means exactly one attempt.
        assert_eq!(mgr.builtin_count(), 1);

        // 4) Out-of-range index clamps to the only account, never to a phantom.
        assert_eq!(mgr.credentials(99).username, "solo");
    }

    /// A freshly installed app has no accounts at all — no hidden pool.
    #[test]
    fn fresh_install_has_no_accounts() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        assert_eq!(mgr.builtin_count(), 0);
        assert!(mgr.list_detailed().is_empty());
        let creds = mgr.preferred_parser_credentials();
        assert!(creds.username.is_empty());
        assert!(creds.password.is_empty());
    }

    /// The one account a user adds is reachable by the index the download path
    /// uses, and is also the parser identity.
    #[test]
    fn single_user_account_serves_both_download_and_parser() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("my_steam_user", "my_password")
            .unwrap();

        let listed = mgr.list_detailed();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "my_steam_user");
        assert!(listed[0].is_custom);

        // Index 0 is what download_start feeds into starting_index.
        let creds = mgr.credentials(0);
        assert_eq!(creds.username, "my_steam_user");
        assert!(
            creds.is_custom,
            "must be flagged custom so it is not rotated"
        );

        // And the parser session signs in with the very same account.
        let parser = mgr.preferred_parser_credentials();
        assert_eq!(parser.username, "my_steam_user");
        assert_eq!(parser.password, "my_password");
    }

    /// Removing the only account leaves the app account-less rather than
    /// resurrecting some hidden default.
    #[test]
    fn removing_the_only_account_leaves_none() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("temp_account", "pw").unwrap();
        assert_eq!(mgr.preferred_parser_credentials().username, "temp_account");

        mgr.remove_user_account("temp_account").unwrap();
        assert_eq!(mgr.builtin_count(), 0);
        assert!(mgr.preferred_parser_credentials().username.is_empty());
        assert!(mgr.credentials(0).username.is_empty());
    }

    /// The download retry loop asks for the count; with one account it must
    /// report 1 so the loop makes exactly one attempt instead of cycling.
    #[test]
    fn account_count_drives_single_attempt_rotation() {
        let dir = TempDir::new().unwrap();
        let mgr = AccountManager::from_runtime(dir.path());
        mgr.add_user_account("only_one", "pw").unwrap();
        assert_eq!(mgr.builtin_count(), 1);
    }
}

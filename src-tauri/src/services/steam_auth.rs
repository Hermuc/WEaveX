//! Steam authentication helpers.
//!
//! These are the pieces of the sign-in flow that have no Tauri state in them,
//! so they can live in the service layer and be unit-tested directly. Anything
//! that needs `AppState` stays in `commands/steam.rs`, where the command
//! adapter supplies the dependencies.

/// The `sub` claim of a Steam access-token JWT — the SteamID64.
///
/// Steam's access tokens are JWTs and the account id is the `sub` claim; there
/// is no separate "who am I" call in the OAuth flow. A token that does not
/// decode, or that carries no `sub`, yields `None` so callers can report a
/// clear error instead of guessing.
pub fn jwt_subject(token: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let payload = token.split('.').nth(1)?;
    let raw = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    json.get("sub")?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a JWT-shaped string with the given JSON payload.
    fn token_with(payload: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let encoded = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("header.{encoded}.signature")
    }

    #[test]
    fn reads_the_sub_claim() {
        let token = token_with(r#"{"sub":"76561198000000001","aud":"steam"}"#);
        assert_eq!(jwt_subject(&token).as_deref(), Some("76561198000000001"));
    }

    #[test]
    fn returns_none_without_a_sub_claim() {
        assert_eq!(jwt_subject(&token_with(r#"{"aud":"steam"}"#)), None);
    }

    #[test]
    fn returns_none_for_a_malformed_token() {
        assert_eq!(jwt_subject("not-a-jwt"), None);
        assert_eq!(jwt_subject(""), None);
        assert_eq!(jwt_subject("a.!!!not-base64!!!.c"), None);
    }
}

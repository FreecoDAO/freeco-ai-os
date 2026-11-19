//! Stateless session token authentication for the dashboard.
//!
//! Tokens are HMAC-SHA256 signed and carry the username, the role the account
//! had when it signed in, and an expiry. Every place that signs or verifies a
//! token takes its secret from [`session_secret`], so a cookie issued by
//! `/api/auth/login` verifies the same way in the auth middleware, the
//! WebSocket upgrade and every handler that asks who is calling.

use freeco_types::auth::UserRole;
use freeco_types::config::{KernelConfig, UserConfig};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// The one secret that signs and verifies dashboard sessions.
///
/// In order: an explicit `auth.session_secret`; the API key, when one is set;
/// the admin password hash; and, when only `[[users]]` accounts have
/// passwords, a digest of their password hashes (so the secret is never an
/// empty, forgeable key while someone can sign in). Empty only when nobody can
/// sign in at all, and [`verify_session_claims`] accepts nothing signed with
/// an empty secret.
///
/// Before this, the server and login signed with the API key whenever one was
/// set while `caller_tenant` verified against the password hash, so with an
/// API key configured no caller's tenant was ever resolved.
pub fn session_secret(config: &KernelConfig) -> String {
    let explicit = config.auth.session_secret.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let api_key = config.api_key.trim();
    if !api_key.is_empty() {
        return api_key.to_string();
    }
    if !config.auth.password_hash.trim().is_empty() {
        return config.auth.password_hash.clone();
    }
    let mut hashes: Vec<&str> = config
        .users
        .iter()
        .filter(|u| u.enabled)
        .filter_map(|u| u.password_hash.as_deref())
        .filter(|h| !h.trim().is_empty())
        .collect();
    if hashes.is_empty() {
        return String::new();
    }
    hashes.sort_unstable();
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(b"freeco-session-secret\0");
    for h in hashes {
        hasher.update(h.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// What a verified session token says about its holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClaims {
    /// The account that signed in.
    pub username: String,
    /// The role the account had at sign-in. `None` for a token issued before
    /// sessions carried a role; see [`effective_role`] for how that is read.
    pub role: Option<UserRole>,
    /// Unix time after which the token is no longer accepted.
    pub expiry_unix: i64,
}

fn sign(payload: &str, secret: &str) -> Option<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}

/// Create a session token: `base64(username:role:expiry_unix:hmac_hex)`.
pub fn create_session_token(
    username: &str,
    role: UserRole,
    secret: &str,
    ttl_hours: u64,
) -> String {
    use base64::Engine;
    let expiry = chrono::Utc::now().timestamp() + (ttl_hours as i64 * 3600);
    let payload = format!("{username}:{}:{expiry}", role.as_str());
    let signature = sign(&payload, secret).expect("HMAC accepts any key length");
    base64::engine::general_purpose::STANDARD.encode(format!("{payload}:{signature}"))
}

/// Extract the `freeco_session` cookie value from a `Cookie` header string.
///
/// Returns `None` if the header is absent or the cookie is not present.
/// Used by both the HTTP auth middleware and the WebSocket upgrade handler so
/// that browser sessions established via `sessionLogin()` are honored on both
/// surfaces (issue #1085).
pub fn extract_session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                c.trim()
                    .strip_prefix("freeco_session=")
                    .map(|v| v.to_string())
            })
        })
}

/// Verify a session token. Returns its claims if it is genuine and unexpired.
///
/// Accepts the current `username:role:expiry:sig` form and the older
/// `username:expiry:sig` form (its `role` is `None`). A token signed with an
/// empty secret is never accepted: an empty HMAC key is one anyone can use.
pub fn verify_session_claims(token: &str, secret: &str) -> Option<SessionClaims> {
    use base64::Engine;
    if secret.is_empty() {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token)
        .ok()?;
    let decoded_str = String::from_utf8(decoded).ok()?;
    let (payload, provided_sig) = decoded_str.rsplit_once(':')?;
    let parts: Vec<&str> = payload.split(':').collect();
    let (username, role, expiry_str) = match parts.as_slice() {
        [username, role, expiry] => (*username, Some(UserRole::parse(role)?), *expiry),
        [username, expiry] => (*username, None, *expiry),
        _ => return None,
    };

    let expiry_unix: i64 = expiry_str.parse().ok()?;
    if chrono::Utc::now().timestamp() > expiry_unix {
        return None;
    }

    let expected_sig = sign(payload, secret)?;
    use subtle::ConstantTimeEq;
    if provided_sig.len() != expected_sig.len() {
        return None;
    }
    if provided_sig
        .as_bytes()
        .ct_eq(expected_sig.as_bytes())
        .into()
    {
        Some(SessionClaims {
            username: username.to_string(),
            role,
            expiry_unix,
        })
    } else {
        None
    }
}

/// Verify a session token. Returns the username if valid and not expired.
pub fn verify_session_token(token: &str, secret: &str) -> Option<String> {
    verify_session_claims(token, secret).map(|c| c.username)
}

/// The role an account has in the configuration right now.
///
/// The admin account (`auth.username`) is always the owner. Any other name is
/// looked up in `[[users]]`; an enabled match has its configured role, and an
/// unknown or disabled name is a `Viewer`, so a session can never inherit
/// power by default.
pub fn role_for(username: &str, admin_username: &str, users: &[UserConfig]) -> UserRole {
    if username == admin_username {
        return UserRole::Owner;
    }
    users
        .iter()
        .find(|u| u.enabled && u.name == username)
        .map(|u| UserRole::from_str_role(&u.role))
        .unwrap_or(UserRole::Viewer)
}

/// The role a verified session acts with: the lower of the role in its token
/// and the role the account has now.
///
/// Taking the lower one means demoting or disabling an account takes effect
/// on its existing sessions, and a token can never grant more than the config
/// does. A token from before sessions carried a role gets the account's
/// current role, which keeps the single owner's existing session working
/// (the admin account resolves to `Owner`) while any other old session gets
/// no more than its account is allowed today.
pub fn effective_role(
    claims: &SessionClaims,
    admin_username: &str,
    users: &[UserConfig],
) -> UserRole {
    let configured = role_for(&claims.username, admin_username, users);
    match claims.role {
        Some(signed) => signed.min(configured),
        None => configured,
    }
}

/// Hash a password with Argon2id for config storage.
///
/// Returns a PHC-format string (e.g. `$argon2id$v=19$m=19456,t=2,p=1$...`).
pub fn hash_password(password: &str) -> String {
    use argon2::{
        password_hash::{rand_core::OsRng, SaltString},
        Argon2, PasswordHasher,
    };
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2 hashing should not fail with valid inputs")
        .to_string()
}

/// Verify a password against a stored Argon2id hash (PHC string format).
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify_password() {
        let hash = hash_password("secret123");
        assert!(
            hash.starts_with("$argon2id$"),
            "should produce Argon2id PHC string"
        );
        assert!(verify_password("secret123", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn test_hash_produces_unique_salts() {
        let h1 = hash_password("same");
        let h2 = hash_password("same");
        assert_ne!(h1, h2, "each hash should use a unique salt");
        assert!(verify_password("same", &h1));
        assert!(verify_password("same", &h2));
    }

    #[test]
    fn test_rejects_non_argon2_hash() {
        // A plain SHA256 hex string should no longer be accepted.
        use sha2::Digest;
        let sha256_hash = hex::encode(sha2::Sha256::digest(b"password"));
        assert!(!verify_password("password", &sha256_hash));
    }

    /// A token in the format used before sessions carried a role.
    fn legacy_token(username: &str, secret: &str) -> String {
        use base64::Engine;
        let expiry = chrono::Utc::now().timestamp() + 3600;
        let payload = format!("{username}:{expiry}");
        let sig = sign(&payload, secret).unwrap();
        base64::engine::general_purpose::STANDARD.encode(format!("{payload}:{sig}"))
    }

    fn user(name: &str, role: &str, enabled: bool) -> UserConfig {
        UserConfig {
            name: name.to_string(),
            role: role.to_string(),
            channel_bindings: Default::default(),
            api_key_hash: None,
            password_hash: Some(format!("$argon2id$hash-of-{name}")),
            enabled,
        }
    }

    #[test]
    fn test_create_and_verify_token() {
        let token = create_session_token("admin", UserRole::Owner, "my-secret", 1);
        let user = verify_session_token(&token, "my-secret");
        assert_eq!(user, Some("admin".to_string()));
    }

    #[test]
    fn a_token_carries_its_role() {
        let token = create_session_token("kiddo", UserRole::Kid, "my-secret", 1);
        let claims = verify_session_claims(&token, "my-secret").expect("valid claims");
        assert_eq!(claims.username, "kiddo");
        assert_eq!(claims.role, Some(UserRole::Kid));
    }

    #[test]
    fn a_role_cannot_be_edited_without_the_secret() {
        use base64::Engine;
        let token = create_session_token("bob", UserRole::Viewer, "my-secret", 1);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&token)
            .unwrap();
        let forged = String::from_utf8(raw)
            .unwrap()
            .replacen(":viewer:", ":owner:", 1);
        let forged = base64::engine::general_purpose::STANDARD.encode(forged);
        assert_eq!(verify_session_claims(&forged, "my-secret"), None);
    }

    #[test]
    fn an_unknown_role_name_is_rejected() {
        use base64::Engine;
        let expiry = chrono::Utc::now().timestamp() + 3600;
        let payload = format!("bob:root:{expiry}");
        let sig = sign(&payload, "my-secret").unwrap();
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{payload}:{sig}"));
        assert_eq!(verify_session_claims(&token, "my-secret"), None);
    }

    #[test]
    fn an_expired_token_is_rejected() {
        use base64::Engine;
        let expiry = chrono::Utc::now().timestamp() - 1;
        let payload = format!("bob:owner:{expiry}");
        let sig = sign(&payload, "my-secret").unwrap();
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{payload}:{sig}"));
        assert_eq!(verify_session_claims(&token, "my-secret"), None);
    }

    #[test]
    fn a_legacy_token_still_verifies_without_a_role() {
        let token = legacy_token("admin", "my-secret");
        let claims = verify_session_claims(&token, "my-secret").unwrap();
        assert_eq!(claims.username, "admin");
        assert_eq!(claims.role, None);
    }

    #[test]
    fn an_empty_secret_verifies_nothing() {
        let token = create_session_token("admin", UserRole::Owner, "", 1);
        assert_eq!(verify_session_claims(&token, ""), None);
    }

    #[test]
    fn test_token_wrong_secret() {
        let token = create_session_token("admin", UserRole::Owner, "my-secret", 1);
        let user = verify_session_token(&token, "wrong-secret");
        assert_eq!(user, None);
    }

    #[test]
    fn test_token_invalid_base64() {
        let user = verify_session_token("not-valid-base64!!!", "secret");
        assert_eq!(user, None);
    }

    fn config(api_key: &str, password_hash: &str) -> KernelConfig {
        KernelConfig {
            api_key: api_key.to_string(),
            auth: freeco_types::config::AuthConfig {
                enabled: true,
                password_hash: password_hash.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn the_session_secret_is_the_api_key_when_one_is_set() {
        let c = config("  the-api-key ", "$argon2id$admin");
        assert_eq!(session_secret(&c), "the-api-key");
        let token = create_session_token("admin", UserRole::Owner, &session_secret(&c), 1);
        assert!(verify_session_claims(&token, &session_secret(&c)).is_some());
        // Not the password hash: that mismatch is what lost the caller's tenant.
        assert_eq!(verify_session_claims(&token, "$argon2id$admin"), None);
    }

    #[test]
    fn the_session_secret_is_the_password_hash_without_an_api_key() {
        let c = config("", "$argon2id$admin");
        assert_eq!(session_secret(&c), "$argon2id$admin");
        let token = create_session_token("admin", UserRole::Owner, &session_secret(&c), 1);
        assert!(verify_session_claims(&token, &session_secret(&c)).is_some());
    }

    #[test]
    fn an_explicit_session_secret_wins_and_rotating_it_ends_sessions() {
        let mut c = config("the-api-key", "$argon2id$admin");
        c.auth.session_secret = "rotated-1".into();
        assert_eq!(session_secret(&c), "rotated-1");
        let token = create_session_token("admin", UserRole::Owner, &session_secret(&c), 1);
        c.auth.session_secret = "rotated-2".into();
        assert_eq!(verify_session_claims(&token, &session_secret(&c)), None);
    }

    #[test]
    fn accounts_with_passwords_never_get_an_empty_secret() {
        let mut c = config("", "");
        assert_eq!(session_secret(&c), "");
        c.users = vec![user("bob", "user", true)];
        let s = session_secret(&c);
        assert_eq!(s.len(), 64);
        // A disabled account's password does not count.
        c.users = vec![user("bob", "user", false)];
        assert_eq!(session_secret(&c), "");
    }

    #[test]
    fn role_for_resolves_admin_users_and_strangers() {
        let users = vec![
            user("alice", "admin", true),
            user("kim", "kid", true),
            user("gone", "owner", false),
        ];
        assert_eq!(role_for("admin", "admin", &users), UserRole::Owner);
        assert_eq!(role_for("alice", "admin", &users), UserRole::Admin);
        assert_eq!(role_for("kim", "admin", &users), UserRole::Kid);
        assert_eq!(role_for("gone", "admin", &users), UserRole::Viewer);
        assert_eq!(role_for("nobody", "admin", &users), UserRole::Viewer);
    }

    #[test]
    fn the_effective_role_is_the_lower_of_token_and_config() {
        let users = vec![user("alice", "user", true)];
        let claims = |role| SessionClaims {
            username: "alice".into(),
            role,
            expiry_unix: 0,
        };
        // Signed as admin, demoted to user since: user.
        assert_eq!(
            effective_role(&claims(Some(UserRole::Admin)), "admin", &users),
            UserRole::User
        );
        // Signed as viewer: stays viewer even though config says user.
        assert_eq!(
            effective_role(&claims(Some(UserRole::Viewer)), "admin", &users),
            UserRole::Viewer
        );
        // Legacy token: the account's current role.
        assert_eq!(
            effective_role(&claims(None), "admin", &users),
            UserRole::User
        );
        let owner = SessionClaims {
            username: "admin".into(),
            role: None,
            expiry_unix: 0,
        };
        assert_eq!(effective_role(&owner, "admin", &users), UserRole::Owner);
    }

    #[test]
    fn test_rejects_garbage_input() {
        assert!(!verify_password("x", "short"));
        assert!(!verify_password("x", ""));
    }

    #[test]
    fn test_verify_malformed_argon2_hash() {
        // Starts with $argon2 but is not a valid PHC string.
        assert!(!verify_password("x", "$argon2id$garbage"));
    }

    #[test]
    fn test_extract_session_cookie_present() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "cookie",
            "foo=bar; freeco_session=abc.def.ghi; baz=qux"
                .parse()
                .unwrap(),
        );
        assert_eq!(extract_session_cookie(&h).as_deref(), Some("abc.def.ghi"));
    }

    #[test]
    fn test_extract_session_cookie_absent() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("cookie", "foo=bar; baz=qux".parse().unwrap());
        assert_eq!(extract_session_cookie(&h), None);
    }

    #[test]
    fn test_extract_session_cookie_no_header() {
        let h = axum::http::HeaderMap::new();
        assert_eq!(extract_session_cookie(&h), None);
    }

    #[test]
    fn test_extract_session_cookie_only_value() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("cookie", "freeco_session=lonely".parse().unwrap());
        assert_eq!(extract_session_cookie(&h).as_deref(), Some("lonely"));
    }
}

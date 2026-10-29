//! Keeps secrets the user types into a chat out of the conversation.
//!
//! A key pasted into a chat message becomes part of the conversation: it is
//! sent to the model provider, and resent with the history on every later
//! call. The data gateway masks what it recognises on the way out, but the
//! only way to keep a key out of the conversation altogether is to catch it
//! before the message is sent. So the chat asks here first
//! (`POST /api/gateway/scan`), and when a message holds a secret it offers to
//! store it in the vault (`POST /api/vault/secrets`) and send the message with
//! the key replaced by its name.

use crate::routes::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use freeco_datagateway::DataKind;
use std::sync::Arc;

/// Kinds worth stopping a message for. Emails and addresses are ordinary
/// working data; card numbers are already refused outright by the gateway.
fn is_secret(kind: DataKind) -> bool {
    matches!(kind, DataKind::ApiKey | DataKind::PrivateKey)
}

/// Name a secret after what the message says it is for, so the vault entry is
/// the one the tools that need it will read.
fn suggest_name(message: &str, value: &str) -> String {
    let m = message.to_ascii_lowercase();
    let by_prefix = [
        // Before `sk-ant-`: a Claude Code subscription token is not an API key,
        // and the `claude-code` provider reads it under this name.
        ("sk-ant-oat", "CLAUDE_CODE_OAUTH_TOKEN"),
        ("sk-ant-", "ANTHROPIC_API_KEY"),
        ("sk-or-", "OPENROUTER_API_KEY"),
        ("gsk_", "GROQ_API_KEY"),
        ("ghp_", "GITHUB_TOKEN"),
        ("github_pat_", "GITHUB_TOKEN"),
        ("hf_", "HF_TOKEN"),
        ("csk-", "CEREBRAS_API_KEY"),
        ("pplx-", "PERPLEXITY_API_KEY"),
    ];
    if let Some((_, name)) = by_prefix.iter().find(|(p, _)| value.starts_with(p)) {
        return (*name).to_string();
    }
    if m.contains("crm") || m.contains("twenty") {
        return crate::crm::KEY_ENV.to_string();
    }
    if m.contains("github") {
        return "GITHUB_TOKEN".to_string();
    }
    "SECRET_1".to_string()
}

/// Byte offset → UTF-16 offset, which is what a browser string indexes by.
/// Without this, a message with Cyrillic before the key would have the wrong
/// characters cut out of it.
fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

/// POST /api/gateway/scan — find secrets in a message before it is sent.
pub async fn scan(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let text = body["text"].as_str().unwrap_or("");
    let mut used: Vec<String> = Vec::new();
    let found: Vec<serde_json::Value> = freeco_datagateway::detect::scan(text)
        .into_iter()
        .filter(|f| is_secret(f.kind))
        .map(|f| {
            // Two secrets in one message must not be offered the same name:
            // saving the second would silently overwrite the first.
            let name = unique_name(suggest_name(text, &f.value), &used);
            used.push(name.clone());
            serde_json::json!({
                "kind": f.kind.as_str(),
                "start": utf16_offset(text, f.start),
                "end": utf16_offset(text, f.end),
                // Saving under a name that already holds a key replaces it;
                // the dialog says so instead of doing it silently.
                "exists": std::env::var(&name).is_ok_and(|v| !v.is_empty()),
                "suggested_name": name,
            })
        })
        .collect();
    Json(serde_json::json!({ "secrets": found }))
}

/// `name`, or `name_2`, `name_3`... if an earlier secret in the same
/// message already took it.
fn unique_name(name: String, used: &[String]) -> String {
    if !used.contains(&name) {
        return name;
    }
    (2..)
        .map(|n| format!("{name}_{n}"))
        .find(|c| !used.contains(c))
        .unwrap_or(name)
}

/// Variables the operating system or the runtime reads. Stored as a "secret",
/// one would be written into this process's environment and change how the
/// daemon itself runs -- `PATH` decides which programs it starts.
const RESERVED_NAMES: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "SYSTEMROOT",
    "WINDIR",
    "TEMP",
    "TMP",
    "COMSPEC",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "SHELL",
    "USER",
    "USERNAME",
    "LANG",
    "TZ",
    "PWD",
];
const RESERVED_PREFIXES: &[&str] = &["LD_", "DYLD_", "RUST_", "CARGO_"];

/// A vault entry name: an environment variable name, which is how tools and
/// `secret:` references look it up -- and not one the system itself relies on.
pub(crate) fn valid_secret_name(name: &str) -> bool {
    if RESERVED_NAMES.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    let mut chars = name.chars();
    name.len() >= 2
        && name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Store a secret where every part of FreEco.ai reads secrets from: the vault,
/// `secrets.env` (which survives a vault problem), and this process's
/// environment, so it works now rather than after a restart.
pub(crate) fn store_secret(state: &AppState, name: &str, value: &str) -> Result<(), String> {
    // Before the vault, so a value secrets.env would refuse is not half-stored.
    freeco_types::secrets::validate_secret_env_entry(name, value)?;
    state.kernel.store_credential(name, value);
    let secrets_path = state.kernel.config.home_dir.join("secrets.env");
    freeco_types::secrets::write_secret_env(&secrets_path, name, value)
        .map_err(|e| format!("could not write secrets.env: {e}"))?;
    std::env::set_var(name, value);
    // The name may be a provider's key variable; let the model list notice.
    state
        .kernel
        .model_catalog
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .detect_auth();
    Ok(())
}

/// POST /api/vault/secrets — `{ "name": "CRM_API_KEY", "value": "…" }`.
///
/// Never echoes the value back.
pub async fn save_secret(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = body["name"].as_str().unwrap_or("").trim().to_string();
    let value = body["value"].as_str().unwrap_or("").trim().to_string();
    if !valid_secret_name(&name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "The name must be capital letters, digits and underscores, starting with a letter (for example CRM_API_KEY), and not a system variable such as PATH."
            })),
        );
    }
    if value.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Nothing to save."})),
        );
    }
    // A line break or NUL is the caller's mistake, not a storage failure.
    if let Err(e) = freeco_types::secrets::validate_secret_env_entry(&name, &value) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        );
    }
    match store_secret(&state, &name, &value) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"ok": true, "name": name})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_what_the_message_says() {
        assert_eq!(
            suggest_name("here is the CRM key eyJx", "eyJx"),
            "CRM_API_KEY"
        );
        assert_eq!(suggest_name("key", "sk-ant-abc"), "ANTHROPIC_API_KEY");
        assert_eq!(
            suggest_name("token", "sk-ant-oat01-abc"),
            "CLAUDE_CODE_OAUTH_TOKEN"
        );
        assert_eq!(suggest_name("use this", "zzz"), "SECRET_1");
    }

    #[test]
    fn offsets_are_utf16_for_the_browser() {
        let t = "ключ: x";
        // "ключ" is 4 characters but 8 bytes.
        assert_eq!(utf16_offset(t, t.find('x').unwrap()), 6);
    }

    #[test]
    fn secret_names_are_env_var_names() {
        assert!(valid_secret_name("CRM_API_KEY"));
        assert!(!valid_secret_name("crm_key"));
        assert!(!valid_secret_name("1KEY"));
        assert!(!valid_secret_name("A-B"));
        assert!(!valid_secret_name("X"));
        // System variables are not secrets: storing one changes the daemon.
        assert!(!valid_secret_name("PATH"));
        assert!(!valid_secret_name("LD_PRELOAD"));
        assert!(!valid_secret_name("RUST_LOG"));
    }

    #[test]
    fn two_secrets_in_one_message_get_different_names() {
        let used = vec!["CRM_API_KEY".to_string()];
        assert_eq!(unique_name("CRM_API_KEY".into(), &used), "CRM_API_KEY_2");
        let used = vec!["CRM_API_KEY".to_string(), "CRM_API_KEY_2".to_string()];
        assert_eq!(unique_name("CRM_API_KEY".into(), &used), "CRM_API_KEY_3");
        assert_eq!(unique_name("GROQ_API_KEY".into(), &used), "GROQ_API_KEY");
    }
}

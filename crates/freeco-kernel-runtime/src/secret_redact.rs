//! Keep secrets out of what agents say and write.
//!
//! The data gateway masks secrets on the way to the provider and restores
//! them in the reply, so the agent can work with real data. That restore is
//! also how a secret ends up in a stored reply, a channel message, a tool
//! result the dashboard shows, or a file the agent writes. This module is the
//! last step before any of those:
//!
//! - [`redact`] replaces every known secret value (vault entries, provider
//!   keys and tokens from the environment, MCP header values: whatever the
//!   kernel reports through `KernelHandle::known_secrets`) and every
//!   well-known key pattern (`sk-…`, `ghp_…`, `gsk_…`, `AKIA…`, `xoxb-…`, PEM
//!   private keys) with `[REDACTED:<name or kind>]`.
//! - [`find_secret`] reports the first one, so a file write can be refused
//!   instead of silently writing a file that no longer works.
//!
//! Pattern detection reuses `freeco_datagateway::detect::scan_credentials`,
//! the strict variant of the gateway's own scanner.

use std::borrow::Cow;

/// Known values shorter than this are ignored: short values ("admin",
/// "8080") occur in ordinary text and redacting them would mangle it.
pub const MIN_KNOWN_SECRET_LEN: usize = 12;

/// Is `value` worth treating as a secret if a secret-named variable holds it?
///
/// Rejects values too short to be unique, plain numbers (`MAX_TOKENS`),
/// values with whitespace (descriptions), filesystem paths and URLs without
/// credentials (`*_TOKEN_URL`): redacting those everywhere would break
/// ordinary output.
pub fn is_plausible_secret_value(value: &str) -> bool {
    let v = value.trim();
    if v.chars().count() < MIN_KNOWN_SECRET_LEN {
        return false;
    }
    if v.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if v.chars().any(char::is_whitespace) {
        return false;
    }
    // Filesystem paths (`TOKENIZER_PATH=/models/...`).
    if v.starts_with('/') || v.starts_with('~') || v.starts_with("./") {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://")) && !v.contains('@') {
        return false;
    }
    true
}

/// Does an environment variable name look like it holds a secret?
///
/// Extends `freeco_skills::config_injection::is_secret_name` (the `_KEY`,
/// `_TOKEN`, `_SECRET`, `PASSWORD` suffix rule skills already use) with the
/// spellings provider and channel settings use in practice.
pub fn is_secret_env_name(name: &str) -> bool {
    if freeco_skills::config_injection::is_secret_name(name) {
        return true;
    }
    let upper = name.to_ascii_uppercase();
    ["API_KEY", "APIKEY", "SECRET", "TOKEN", "PASSWORD", "PASSWD"]
        .iter()
        .any(|w| upper.contains(w))
        || upper.ends_with("_PAT")
        || upper.ends_with("_CREDENTIALS")
}

/// Pull the secret part out of an HTTP header line such as
/// `Authorization: Bearer abc…` or `X-Api-Key: abc…`.
pub fn header_secret_value(header: &str) -> Option<String> {
    let (name, value) = header.split_once(':')?;
    let name = name.trim().to_ascii_lowercase();
    let credential_header = ["auth", "key", "token", "secret", "cookie", "password"]
        .iter()
        .any(|w| name.contains(w));
    if !credential_header {
        return None;
    }
    let mut v = value.trim();
    for scheme in ["Bearer ", "bearer ", "Basic ", "basic ", "Token ", "token "] {
        if let Some(rest) = v.strip_prefix(scheme) {
            v = rest.trim();
            break;
        }
    }
    if v.contains("${") || !is_plausible_secret_value(v) {
        return None;
    }
    Some(v.to_string())
}

fn usable(known: &[(String, String)]) -> Vec<(&str, &str)> {
    let mut list: Vec<(&str, &str)> = known
        .iter()
        .filter(|(_, v)| v.chars().count() >= MIN_KNOWN_SECRET_LEN)
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    // Longest first, so a value that contains another is replaced whole.
    list.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    list
}

/// Replace known secret values and key patterns in `text`.
///
/// `known` is `(name, value)`; the marker names the variable
/// (`[REDACTED:GROQ_API_KEY]`) so the reader knows which secret it was
/// without seeing any of it. Pattern matches are named by kind
/// (`[REDACTED:github_token]`).
pub fn redact<'a>(text: &'a str, known: &[(String, String)]) -> Cow<'a, str> {
    let mut out: Cow<'a, str> = Cow::Borrowed(text);
    for (name, value) in usable(known) {
        if out.contains(value) {
            let marker = format!("[REDACTED:{name}]");
            out = Cow::Owned(out.replace(value, &marker));
        }
    }
    let findings = freeco_datagateway::detect::scan_credentials(&out);
    if findings.is_empty() {
        return out;
    }
    let mut s = out.into_owned();
    // Back to front, so earlier byte offsets stay valid.
    for f in findings.iter().rev() {
        let marker = format!(
            "[REDACTED:{}]",
            freeco_datagateway::detect::credential_label(&f.value)
        );
        s.replace_range(f.start..f.end, &marker);
    }
    Cow::Owned(s)
}

/// The name or kind of the first secret in `text`, if there is one.
pub fn find_secret(text: &str, known: &[(String, String)]) -> Option<String> {
    for (name, value) in usable(known) {
        if text.contains(value) {
            return Some(name.to_string());
        }
    }
    freeco_datagateway::detect::scan_credentials(text)
        .first()
        .map(|f| freeco_datagateway::detect::credential_label(&f.value).to_string())
}

/// Error for a file write that would put a secret on disk.
pub fn written_secret_error(tool: &str, what: &str) -> String {
    format!(
        "{tool} refused: the content contains a secret ({what}). Never write keys, tokens \
         or passwords into files: read them at runtime from an environment variable or the \
         vault (for example os.environ[\"NAME\"] / process.env.NAME / std::env::var(\"NAME\")), \
         or let a connector or hand inject them. Write the code with the variable name instead \
         of the value."
    )
}

/// The text a patch adds: its `+` lines. Removed and context lines are not
/// checked, so a patch that deletes a leaked key is allowed.
pub fn patch_added_text(patch: &str) -> String {
    patch
        .lines()
        .filter_map(|l| l.strip_prefix('+'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<(String, String)> {
        vec![
            ("CRM_API_KEY".into(), "crm-7f3k9QzLmP2wXa8B".into()),
            ("SHORT".into(), "abc123".into()),
            (
                "OMNIROUTE_MCP_KEY".into(),
                "omni_live_4b8c2d9e1f7a3c5b".into(),
            ),
        ]
    }

    #[test]
    fn known_values_are_replaced_by_name() {
        let t = "use crm-7f3k9QzLmP2wXa8B for the CRM";
        assert_eq!(
            redact(t, &known()),
            "use [REDACTED:CRM_API_KEY] for the CRM"
        );
    }

    #[test]
    fn short_known_values_are_left_alone() {
        let t = "abc123 is a product code";
        assert_eq!(redact(t, &known()), t);
        assert!(matches!(redact(t, &known()), Cow::Borrowed(_)));
    }

    #[test]
    fn patterns_are_replaced_by_kind() {
        let gh = format!("{}{}", "ghp_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        let t = format!("token is {gh}.");
        assert_eq!(redact(&t, &[]), "token is [REDACTED:github_token].");

        let t = "key=gsk_7Hd82kLmN3pQ9rStUv4wXyZ1 and AKIAIOSFODNN7EXAMPLE";
        assert_eq!(
            redact(t, &[]),
            "key=[REDACTED:groq_key] and [REDACTED:aws_access_key]"
        );

        let pem = "a\n-----BEGIN PRIVATE KEY-----\nMIIEvQ\n-----END PRIVATE KEY-----\nb";
        assert_eq!(redact(pem, &[]), "a\n[REDACTED:private_key]\nb");
    }

    #[test]
    fn known_and_pattern_together() {
        let t = "crm-7f3k9QzLmP2wXa8B / sk-ant-api03-Zx9Yw8Vu7Ts6Rq5Po4Nm3";
        assert_eq!(
            redact(t, &known()),
            "[REDACTED:CRM_API_KEY] / [REDACTED:anthropic_key]"
        );
    }

    #[test]
    fn ordinary_text_is_untouched() {
        for t in [
            "Deployed commit 3f2a9c1b7e4d8a6f0c2b9e1d7a3f5c8b0a1b2c3d to prod",
            "risk-assessment-for-the-new-quarter-2026",
            "Email bob@example.com, server 10.0.0.1",
            "Set GROQ_API_KEY in the environment",
        ] {
            assert!(matches!(redact(t, &known()), Cow::Borrowed(_)), "{t}");
        }
    }

    #[test]
    fn find_secret_reports_name_or_kind() {
        assert_eq!(
            find_secret("x = 'omni_live_4b8c2d9e1f7a3c5b'", &known()).as_deref(),
            Some("OMNIROUTE_MCP_KEY")
        );
        assert_eq!(
            find_secret("API_KEY = 'sk-proj-Ab3dEf6hIj9kLm2nOp5qRs8tUv'", &[]).as_deref(),
            Some("openai_key")
        );
        assert_eq!(
            find_secret("api_key = os.environ['OPENAI_API_KEY']", &known()),
            None
        );
    }

    #[test]
    fn write_error_points_to_env_and_vault() {
        let e = written_secret_error("file_write", "CRM_API_KEY");
        assert!(e.contains("file_write refused"));
        assert!(e.contains("CRM_API_KEY"));
        assert!(e.contains("environment variable or the vault"));
    }

    #[test]
    fn patch_check_covers_added_lines_only() {
        let patch = "*** Begin Patch\n*** Update File: a.py\n@@ x @@\n-KEY = 'crm-7f3k9QzLmP2wXa8B'\n+KEY = os.environ['CRM_API_KEY']\n*** End Patch";
        assert_eq!(find_secret(&patch_added_text(patch), &known()), None);
        let patch =
            "*** Begin Patch\n*** Add File: b.py\n+KEY = 'crm-7f3k9QzLmP2wXa8B'\n*** End Patch";
        assert_eq!(
            find_secret(&patch_added_text(patch), &known()).as_deref(),
            Some("CRM_API_KEY")
        );
    }

    #[test]
    fn plausible_secret_values() {
        assert!(is_plausible_secret_value("crm-7f3k9QzLmP2wXa8B"));
        assert!(!is_plausible_secret_value("short"));
        assert!(!is_plausible_secret_value("123456789012345"));
        assert!(!is_plausible_secret_value("http://localhost:11434/v1"));
        assert!(is_plausible_secret_value("https://user:pw1234@host/x"));
        assert!(!is_plausible_secret_value("a value with spaces in it"));
        assert!(!is_plausible_secret_value("/models/tokenizer.json"));
    }

    #[test]
    fn secret_env_names() {
        assert!(is_secret_env_name("GROQ_API_KEY"));
        assert!(is_secret_env_name("TELEGRAM_BOT_TOKEN"));
        assert!(is_secret_env_name("GITHUB_PAT"));
        assert!(is_secret_env_name("DB_PASSWORD"));
        assert!(is_secret_env_name("OPENAI_APIKEY"));
        assert!(!is_secret_env_name("PATH"));
        assert!(!is_secret_env_name("RUST_LOG"));
        assert!(!is_secret_env_name("OLLAMA_BASE_URL"));
    }

    #[test]
    fn header_values() {
        assert_eq!(
            header_secret_value("Authorization: Bearer omni_live_4b8c2d9e1f7a3c5b").as_deref(),
            Some("omni_live_4b8c2d9e1f7a3c5b")
        );
        assert_eq!(
            header_secret_value("X-Api-Key: crm-7f3k9QzLmP2wXa8B").as_deref(),
            Some("crm-7f3k9QzLmP2wXa8B")
        );
        assert_eq!(
            header_secret_value("Authorization: Bearer ${OMNI_KEY}"),
            None
        );
        assert_eq!(header_secret_value("Accept: application/json"), None);
    }
}

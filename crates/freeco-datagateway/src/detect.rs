//! Finding sensitive values in text.
//!
//! Deliberately written without a regex engine. The formats that matter here —
//! card numbers, IBANs, provider keys — are defined by structure and checksums,
//! not by patterns: a regex matches sixteen digits, but only Luhn tells you
//! whether they are a card number or an order reference. Scanning explicitly
//! also keeps the false-positive rate low enough to mask by default, which is
//! the only setting that protects anyone.

use serde::{Deserialize, Serialize};

/// A class of sensitive data, so policy can differ per kind.
///
/// A credit card and an email address both matter, but not equally: masking
/// every email would break ordinary work like "draft a reply to this address",
/// whereas a card number has no business reaching a model at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    /// Provider API keys and tokens.
    ApiKey,
    /// Payment card numbers (Luhn-valid).
    CreditCard,
    /// International bank account numbers.
    Iban,
    /// Email addresses.
    Email,
    /// IPv4 addresses.
    IpAddress,
    /// Private key material (PEM blocks).
    PrivateKey,
}

impl DataKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DataKind::ApiKey => "api_key",
            DataKind::CreditCard => "credit_card",
            DataKind::Iban => "iban",
            DataKind::Email => "email",
            DataKind::IpAddress => "ip_address",
            DataKind::PrivateKey => "private_key",
        }
    }
}

/// One sensitive value located in a piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: DataKind,
    /// Byte range in the scanned text.
    pub start: usize,
    pub end: usize,
    /// The matched text itself.
    pub value: String,
}

/// Luhn checksum, the thing that separates a card number from any other digits.
fn luhn_valid(digits: &[u8]) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = u32::from(*d);
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum.is_multiple_of(10)
}

/// Known provider key prefixes, and how long the key runs after them.
///
/// Prefix-anchored rather than "any long random string", because the latter
/// also matches commit hashes, UUIDs and base64 payloads — and a gateway that
/// masks those makes the product unusable while looking like it is working.
const KEY_PREFIXES: &[&str] = &[
    "sk-or-v1-", // OpenRouter
    "sk-ant-",   // Anthropic
    "sk-proj-",  // OpenAI project keys
    "sk-",       // OpenAI and compatible
    "ghp_",      // GitHub personal
    "gho_",      // GitHub OAuth
    "github_pat_",
    "gsk_",   // Groq
    "AKIA",   // AWS access key id
    "AIza",   // Google
    "xoxb-",  // Slack bot
    "xoxp-",  // Slack user
    "hf_",    // Hugging Face
    "r8_",    // Replicate
    "csk-",   // Cerebras
    "pplx-",  // Perplexity
    "glpat-", // GitLab
];

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_b64url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Characters a secret handed over after "key:" or "token=" can contain.
fn is_secret_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-.+/=~".contains(c)
}

/// A JSON Web Token: `eyJ<header>.<payload>.<signature>`, all base64url.
///
/// Self-hosted services hand these out as API keys (Twenty CRM does), and none
/// of them carries a vendor prefix, so the prefix list alone never saw one.
/// The three-part shape starting with the base64 of `{"` is specific enough to
/// mask without a keyword next to it.
fn scan_jwt(text: &str, start: usize) -> Option<Finding> {
    if !word_boundary_before(text, start) {
        return None;
    }
    let rest = &text[start..];
    if !rest.starts_with("eyJ") {
        return None;
    }
    let mut end = 0usize;
    let mut parts = 0usize;
    loop {
        let seg = rest[end..]
            .char_indices()
            .find(|(_, c)| !is_b64url_char(*c))
            .map(|(p, _)| p)
            .unwrap_or(rest.len() - end);
        if seg < 10 {
            return None;
        }
        end += seg;
        parts += 1;
        if parts == 3 {
            break;
        }
        if !rest[end..].starts_with('.') {
            return None;
        }
        end += 1;
    }
    Some(Finding {
        kind: DataKind::ApiKey,
        start,
        end: start + end,
        value: rest[..end].to_string(),
    })
}

/// Words that announce a secret: "api key: …", "token=…", "password is …".
/// The shorter minimum applies to passwords, which are shorter than keys.
const SECRET_KEYWORDS: &[(&str, usize)] = &[
    ("api_key", 20),
    ("apikey", 20),
    ("api key", 20),
    ("access key", 20),
    ("secret", 20),
    ("token", 20),
    ("bearer", 20),
    ("key", 20),
    ("password", 8),
    ("passwd", 8),
];

/// A value handed over right after a word that announces it, e.g.
/// `CRM key: 7fk3...`. Returns the value, not the keyword, and the byte offset
/// where scanning resumes.
///
/// Anchored on the keyword so that a long random string on its own -- a commit
/// hash, a UUID in a log line -- is left alone: the prefix list's reasoning
/// still holds, and this only widens it to the case where the user has said
/// out loud that what follows is a secret.
fn scan_announced_secret(text: &str, start: usize) -> Option<(Finding, usize)> {
    // `_` counts as a boundary here so `CRM_API_KEY=…` announces a secret the
    // way `key: …` does; a letter before does not, or "monkey" would.
    if text[..start]
        .chars()
        .next_back()
        .is_some_and(|p| p.is_ascii_alphanumeric())
    {
        return None;
    }
    let rest = &text[start..];
    let lower: String = rest
        .chars()
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    let (kw, min_len) = SECRET_KEYWORDS
        .iter()
        .find(|(kw, _)| lower.starts_with(kw))?;
    let mut pos = kw.len();
    // A keyword glued to more letters is another word: "keyboard", "tokens".
    if rest[pos..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    // Separator: any mix of `:`, `=`, quotes and spaces, optionally " is ".
    let sep_start = pos;
    while let Some(c) = rest[pos..].chars().next() {
        if c == ':' || c == '=' || c == '"' || c == '\'' || c == ' ' || c == '\t' || c == '`' {
            pos += c.len_utf8();
        } else {
            break;
        }
    }
    if rest[pos..].to_ascii_lowercase().starts_with("is ") {
        pos += 3;
        while rest[pos..].starts_with(' ') {
            pos += 1;
        }
    }
    if pos == sep_start || pos - sep_start > 8 {
        return None;
    }
    let value_len = rest[pos..]
        .char_indices()
        .find(|(_, c)| !is_secret_char(*c))
        .map(|(p, _)| p)
        .unwrap_or(rest.len() - pos);
    let value = rest[pos..pos + value_len].trim_end_matches('.');
    let has_digit = value.chars().any(|c| c.is_ascii_digit());
    let has_alpha = value.chars().any(|c| c.is_ascii_alphabetic());
    let has_other = value.chars().any(|c| !c.is_ascii_alphabetic());
    let plausible = if *min_len < 20 {
        // A password: anything but a plain word.
        has_other
    } else {
        has_digit && has_alpha
    };
    if value.len() < *min_len || !plausible {
        return None;
    }
    let vstart = start + pos;
    Some((
        Finding {
            kind: DataKind::ApiKey,
            start: vstart,
            end: vstart + value.len(),
            value: value.to_string(),
        },
        vstart + value.len(),
    ))
}

/// Locate every sensitive value in `text`.
///
/// Findings never overlap: the longest match at a given position wins, so a
/// card number inside a longer token is not reported twice.
pub fn scan(text: &str) -> Vec<Finding> {
    let bytes = text.as_bytes();
    let mut out: Vec<Finding> = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];

        // PEM private key blocks — matched whole, because half a key is still
        // a leak and the header alone tells you the rest follows.
        if let Some(end_marker) = pem_private_key_len(rest) {
            out.push(Finding {
                kind: DataKind::PrivateKey,
                start: i,
                end: i + end_marker,
                value: rest[..end_marker].to_string(),
            });
            i += end_marker;
            continue;
        }

        // Provider API keys.
        if let Some(prefix) = KEY_PREFIXES.iter().find(|p| rest.starts_with(**p)) {
            let mut end = prefix.len();
            while end < rest.len() && rest[end..].chars().next().is_some_and(is_key_char) {
                end += rest[end..].chars().next().unwrap().len_utf8();
            }
            // Require real entropy after the prefix, or "sk-" in prose matches.
            if end - prefix.len() >= 16 {
                out.push(Finding {
                    kind: DataKind::ApiKey,
                    start: i,
                    end: i + end,
                    value: rest[..end].to_string(),
                });
                i += end;
                continue;
            }
        }

        let c = rest.chars().next().unwrap();

        // JSON Web Tokens, which self-hosted services issue as API keys.
        if c == 'e' {
            if let Some(f) = scan_jwt(text, i) {
                i = f.end;
                out.push(f);
                continue;
            }
        }

        // A value the user announced as a secret: "CRM key: …".
        if c.is_ascii_alphabetic() {
            if let Some((f, resume)) = scan_announced_secret(text, i) {
                // The value may itself be a JWT or prefixed key; one finding
                // covers it either way.
                out.push(f);
                i = resume;
                continue;
            }
        }

        // Email addresses.
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '%' || c == '+' || c == '-' {
            if let Some(f) = scan_email(text, i) {
                let adv = f.end - f.start;
                out.push(f);
                i += adv;
                continue;
            }
        }

        // IBAN: two letters, two check digits, then up to 30 alphanumerics.
        if c.is_ascii_uppercase() {
            if let Some(f) = scan_iban(text, i) {
                let adv = f.end - f.start;
                out.push(f);
                i += adv;
                continue;
            }
        }

        // Digit runs: card numbers (Luhn) or IPv4.
        if c.is_ascii_digit() {
            if let Some(f) = scan_ipv4(text, i) {
                let adv = f.end - f.start;
                out.push(f);
                i += adv;
                continue;
            }
            if let Some(f) = scan_card(text, i) {
                let adv = f.end - f.start;
                out.push(f);
                i += adv;
                continue;
            }
        }

        i += c.len_utf8();
    }
    out
}

/// Length of a PEM private key block at the start of `rest`, if one is there.
fn pem_private_key_len(rest: &str) -> Option<usize> {
    if !rest.starts_with("-----BEGIN") {
        return None;
    }
    let hdr_end = rest.find("-----END")?;
    if !rest[..hdr_end].contains("PRIVATE KEY") {
        return None;
    }
    Some(
        rest[hdr_end..]
            .find("-----\n")
            .or_else(|| rest[hdr_end..].find("-----"))
            .map(|p| hdr_end + p + 5)
            .unwrap_or(rest.len()),
    )
}

/// What a credential found by [`scan_credentials`] is, for a
/// `[REDACTED:<label>]` marker. Never contains any part of the value.
pub fn credential_label(value: &str) -> &'static str {
    const LABELS: &[(&str, &str)] = &[
        ("sk-or-", "openrouter_key"),
        ("sk-ant-", "anthropic_key"),
        ("sk-", "openai_key"),
        ("ghp_", "github_token"),
        ("gho_", "github_token"),
        ("github_pat_", "github_token"),
        ("gsk_", "groq_key"),
        ("AKIA", "aws_access_key"),
        ("AIza", "google_key"),
        ("xoxb-", "slack_token"),
        ("xoxp-", "slack_token"),
        ("hf_", "huggingface_token"),
        ("r8_", "replicate_token"),
        ("csk-", "cerebras_key"),
        ("pplx-", "perplexity_key"),
        ("glpat-", "gitlab_token"),
    ];
    if value.starts_with("-----BEGIN") {
        return "private_key";
    }
    LABELS
        .iter()
        .find(|(p, _)| value.starts_with(p))
        .map(|(_, l)| *l)
        .unwrap_or("api_key")
}

/// Find provider keys and private keys only, with stricter rules than
/// [`scan`].
///
/// [`scan`] feeds a reversible mask, where a false positive costs nothing
/// because the value comes back in the reply. This feeds irreversible
/// redaction and refusals (a file an agent writes, a reply that is stored), so
/// it must not fire on ordinary text: a key prefix has to start a word (`risk-`
/// and `task-` end in `sk-`), the body has to be long and mix letters and
/// digits (slugs such as `sk-learn-tutorial-notes-draft` do not), and no
/// emails, IPs, JWTs or "key: …" heuristics are reported.
pub fn scan_credentials(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if let Some(len) = pem_private_key_len(rest) {
            out.push(Finding {
                kind: DataKind::PrivateKey,
                start: i,
                end: i + len,
                value: rest[..len].to_string(),
            });
            i += len;
            continue;
        }
        let boundary = text[..i]
            .chars()
            .next_back()
            .is_none_or(|p| !(p.is_ascii_alphanumeric() || p == '_' || p == '-'));
        if boundary {
            if let Some(prefix) = KEY_PREFIXES.iter().find(|p| rest.starts_with(**p)) {
                let body_len = rest[prefix.len()..]
                    .char_indices()
                    .find(|(_, c)| !is_key_char(*c))
                    .map(|(p, _)| p)
                    .unwrap_or(rest.len() - prefix.len());
                let body = &rest[prefix.len()..prefix.len() + body_len];
                let min_len = if prefix.starts_with("sk-") { 20 } else { 16 };
                let has_digit = body.chars().any(|c| c.is_ascii_digit());
                let has_alpha = body.chars().any(|c| c.is_ascii_alphabetic());
                if body.len() >= min_len && has_digit && has_alpha {
                    let end = prefix.len() + body_len;
                    out.push(Finding {
                        kind: DataKind::ApiKey,
                        start: i,
                        end: i + end,
                        value: rest[..end].to_string(),
                    });
                    i += end;
                    continue;
                }
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

fn word_boundary_before(text: &str, at: usize) -> bool {
    if at == 0 {
        return true;
    }
    match text[..at].chars().next_back() {
        Some(p) => !p.is_ascii_alphanumeric() && p != '_' && p != '.' && p != '-' && p != '+',
        None => true,
    }
}

fn scan_email(text: &str, start: usize) -> Option<Finding> {
    if !word_boundary_before(text, start) {
        return None;
    }
    let rest = &text[start..];
    let at = rest.find('@')?;
    if at == 0 || at > 64 {
        return None;
    }
    let local = &rest[..at];
    if !local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
    {
        return None;
    }
    let after = &rest[at + 1..];
    let mut dlen = 0usize;
    for ch in after.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            dlen += ch.len_utf8();
        } else {
            break;
        }
    }
    let domain = &after[..dlen];
    // Must have a dot and a plausible TLD, or "user@localhost" style noise matches.
    let tld = domain.rsplit('.').next().unwrap_or("");
    if !domain.contains('.') || tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let domain = domain.trim_end_matches('.');
    let end = start + at + 1 + domain.len();
    Some(Finding {
        kind: DataKind::Email,
        start,
        end,
        value: text[start..end].to_string(),
    })
}

fn scan_iban(text: &str, start: usize) -> Option<Finding> {
    if !word_boundary_before(text, start) {
        return None;
    }
    let rest = &text[start..];
    let mut chars = rest.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    let d0 = chars.next()?;
    let d1 = chars.next()?;
    if !(c0.is_ascii_uppercase()
        && c1.is_ascii_uppercase()
        && d0.is_ascii_digit()
        && d1.is_ascii_digit())
    {
        return None;
    }
    let mut end = 4usize;
    while end < rest.len() {
        let ch = rest[end..].chars().next().unwrap();
        if ch.is_ascii_alphanumeric() && end < 34 {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    if end < 15 {
        return None;
    }
    Some(Finding {
        kind: DataKind::Iban,
        start,
        end: start + end,
        value: rest[..end].to_string(),
    })
}

fn scan_ipv4(text: &str, start: usize) -> Option<Finding> {
    if !word_boundary_before(text, start) {
        return None;
    }
    let rest = &text[start..];
    let mut end = 0usize;
    let mut octets = 0usize;
    let mut cur: u32 = 0;
    let mut cur_len = 0usize;
    for ch in rest.chars() {
        if ch.is_ascii_digit() {
            cur = cur * 10 + ch.to_digit(10).unwrap();
            cur_len += 1;
            if cur > 255 || cur_len > 3 {
                return None;
            }
            end += 1;
        } else if ch == '.' && cur_len > 0 && octets < 3 {
            octets += 1;
            cur = 0;
            cur_len = 0;
            end += 1;
        } else {
            break;
        }
    }
    if octets != 3 || cur_len == 0 {
        return None;
    }
    // A trailing dot belongs to the sentence, not the address.
    let value = rest[..end].trim_end_matches('.');
    Some(Finding {
        kind: DataKind::IpAddress,
        start,
        end: start + value.len(),
        value: value.to_string(),
    })
}

fn scan_card(text: &str, start: usize) -> Option<Finding> {
    if !word_boundary_before(text, start) {
        return None;
    }
    let rest = &text[start..];
    let mut digits: Vec<u8> = Vec::new();
    let mut end = 0usize;
    for ch in rest.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch.to_digit(10).unwrap() as u8);
            end += 1;
        } else if (ch == ' ' || ch == '-') && !digits.is_empty() && digits.len() < 19 {
            // Cards are commonly written in groups; stop if the separator does
            // not lead back into digits.
            let next = rest[end + ch.len_utf8()..].chars().next();
            if next.is_some_and(|n| n.is_ascii_digit()) {
                end += ch.len_utf8();
            } else {
                break;
            }
        } else {
            break;
        }
        if digits.len() > 19 {
            return None;
        }
    }
    if !luhn_valid(&digits) {
        return None;
    }
    // Digits running straight into letters are part of an identifier -- a hex
    // message id, a hash -- not a card someone wrote down. One in ten digit
    // runs passes the checksum by chance, so ids were being flagged.
    if rest[end..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return None;
    }
    Some(Finding {
        kind: DataKind::CreditCard,
        start,
        end: start + end,
        value: rest[..end].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<DataKind> {
        scan(text).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn finds_provider_keys() {
        let t = "use sk-or-v1-abcdef0123456789abcdef and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert_eq!(kinds(t), vec![DataKind::ApiKey, DataKind::ApiKey]);
    }

    /// "sk-" appears in ordinary prose. Masking it would be worse than useless:
    /// the user would see mangled text and stop trusting the feature.
    #[test]
    fn ignores_short_lookalikes() {
        assert!(scan("the sk-1 label and gh_x").is_empty());
    }

    #[test]
    fn card_numbers_need_a_valid_checksum() {
        // Luhn-valid test number.
        assert_eq!(
            kinds("pay with 4242 4242 4242 4242"),
            vec![DataKind::CreditCard]
        );
        // Same shape, checksum deliberately wrong -> an order number, not a card.
        assert!(scan("order 4242 4242 4242 4243").is_empty());
    }

    /// A Luhn-valid digit run inside an identifier is not a card.
    #[test]
    fn digits_inside_an_identifier_are_not_a_card() {
        assert!(scan("id 4242424242424242abc").is_empty());
        assert!(scan("ref 4242424242424242_x").is_empty());
        // Still found on its own, or before punctuation.
        assert_eq!(kinds("pay 4242424242424242."), vec![DataKind::CreditCard]);
    }

    #[test]
    fn finds_emails_but_not_every_at_sign() {
        assert_eq!(
            kinds("write to a.b+c@example.com now"),
            vec![DataKind::Email]
        );
        assert!(scan("mention @someone in chat").is_empty());
    }

    #[test]
    fn finds_ipv4_and_rejects_impossible_octets() {
        assert_eq!(kinds("host 192.168.1.10 up"), vec![DataKind::IpAddress]);
        assert!(scan("version 999.1.1.1 here").is_empty());
    }

    #[test]
    fn finds_iban() {
        assert_eq!(
            kinds("send to DE89370400440532013000 today"),
            vec![DataKind::Iban]
        );
    }

    #[test]
    fn finds_private_key_blocks() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\n-----END RSA PRIVATE KEY-----";
        assert_eq!(kinds(pem), vec![DataKind::PrivateKey]);
    }

    /// Twenty CRM hands out JWTs as API keys, with no vendor prefix. This is
    /// the key that went to a model provider in plain text.
    #[test]
    fn finds_jwt_api_keys() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ3b3Jrc3BhY2UifQ.c2lnbmF0dXJlLXZhbHVlLTEyMw";
        let f = scan(&format!("here is my key {jwt} thanks"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, DataKind::ApiKey);
        assert_eq!(f[0].value, jwt);
        // Bare, with no keyword next to it.
        assert_eq!(scan(jwt).len(), 1);
    }

    #[test]
    fn finds_values_announced_as_secrets() {
        let f = scan("CRM key: 7fk3Qx9LmP2vR8tW4yZ6aB1c done");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].value, "7fk3Qx9LmP2vR8tW4yZ6aB1c");
        let f = scan("CRM_API_KEY=7fk3Qx9LmP2vR8tW4yZ6aB1c");
        assert_eq!(f[0].value, "7fk3Qx9LmP2vR8tW4yZ6aB1c");
        let f = scan("the password is Hunter2-x9.");
        assert_eq!(f[0].value, "Hunter2-x9");
    }

    /// The keywords are ordinary words. Prose around them must survive.
    #[test]
    fn keywords_in_prose_are_left_alone() {
        for t in [
            "the key is to plan first",
            "a token of appreciation for the whole team",
            "password reset link",
            "my keyboard broke",
            "secret: keep calm",
            "monkey: 7fk3Qx9LmP2vR8tW4yZ6aB1c",
        ] {
            assert!(scan(t).is_empty(), "masked prose: {t}");
        }
    }

    #[test]
    fn clean_text_produces_nothing() {
        assert!(scan("Please summarise the attached quarterly report.").is_empty());
    }

    #[test]
    fn findings_do_not_overlap() {
        let t = "card 4242424242424242 mail a@b.co ip 10.0.0.1";
        let f = scan(t);
        for w in f.windows(2) {
            assert!(w[0].end <= w[1].start, "overlapping findings: {:?}", f);
        }
    }

    // ── scan_credentials ────────────────────────────────────────────────

    #[test]
    fn credentials_finds_prefixed_keys_and_labels_them() {
        let text =
            "openai sk-proj-Ab3dEf6hIj9kLm2nOp5qRs8tUv and groq gsk_7Hd82kLmN3pQ9rStUv4wXyZ1";
        let f = scan_credentials(text);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(credential_label(&f[0].value), "openai_key");
        assert_eq!(credential_label(&f[1].value), "groq_key");
        assert_eq!(&text[f[1].start..f[1].end], f[1].value);
    }

    #[test]
    fn credentials_finds_github_aws_slack_and_pem() {
        let gh = format!("token={}{}", "ghp_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
        let f = scan_credentials(&gh);
        assert_eq!(f.len(), 1);
        assert_eq!(credential_label(&f[0].value), "github_token");

        let f = scan_credentials("aws AKIAIOSFODNN7EXAMPLE end");
        assert_eq!(credential_label(&f[0].value), "aws_access_key");

        let f = scan_credentials("xoxb-1234567890-abcdefghijkl");
        assert_eq!(credential_label(&f[0].value), "slack_token");

        let pem = "x\n-----BEGIN RSA PRIVATE KEY-----\nMIIEabc\n-----END RSA PRIVATE KEY-----\ny";
        let f = scan_credentials(pem);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, DataKind::PrivateKey);
        assert_eq!(credential_label(&f[0].value), "private_key");
    }

    #[test]
    fn credentials_ignores_ordinary_text() {
        // Words ending in "sk-", slugs without digits, short tokens, emails,
        // IPs, JWT-looking strings and "key: ..." prose are not credentials.
        for text in [
            "risk-assessment-for-the-new-quarter-2026",
            "task-abcdefghij1234567890",
            "sk-learn-tutorial-notes-draft-version",
            "sk-short1",
            "mail bob@example.com from 10.0.0.1",
            "the key: 3f2a9c1b7e4d8a6f0c2b9e1d7a3f5c8b",
            "commit 3f2a9c1b7e4d8a6f0c2b9e1d7a3f5c8b0a1b2c3d",
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        ] {
            assert!(
                scan_credentials(text).is_empty(),
                "false positive in {text:?}"
            );
        }
    }
}

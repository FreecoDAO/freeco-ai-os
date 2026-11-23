//! `accounting_api` — agents' read-only access to the books on this machine.
//!
//! The accounting hand runs Akaunting on the loopback interface (port 8080).
//! `web_fetch` refuses loopback addresses on purpose, so without this tool an
//! accounting agent had no way to read the books it is meant to report on:
//! the hand's prompt promised "accounting tools" that did not exist.
//!
//! The same narrow exception as `crm_api`: one fixed address, only Akaunting's
//! API paths, credentials read from the vault by the tool itself (they never
//! pass through the model), and redirects refused so the address cannot be
//! changed by a reply.
//!
//! **Read-only.** Only `GET` is sent, and the GET routes Akaunting uses as
//! actions (enable, disable, mark received/sent/cancelled, duplicate, email)
//! are refused. Writing into the books -- and any country's tax or
//! bookkeeping rules -- is the owner's decision, not this tool's. Nothing here
//! moves money, contacts a bank or touches a real account: Akaunting is a
//! ledger, and this tool only reads it.

use std::time::Duration;

/// Vault entry holding the Akaunting user's e-mail (HTTP Basic user name).
pub const EMAIL_ENV: &str = "ACCOUNTING_API_EMAIL";
/// Vault entry holding that user's password.
pub const PASSWORD_ENV: &str = "ACCOUNTING_API_PASSWORD";
/// Optional vault/env entry with the Akaunting company id. Default `1`, the
/// company the first-run wizard creates.
pub const COMPANY_ENV: &str = "ACCOUNTING_COMPANY_ID";

/// Akaunting's address, as published by the accounting hand (`8080:80`).
pub const BASE_URL: &str = "http://localhost:8080";

/// Largest response handed back to the model.
const MAX_RESPONSE_CHARS: usize = 40_000;

/// Akaunting API resources this tool may read. The first path segment after
/// `/api/` must be one of these.
const READABLE: &[&str] = &[
    "ping",
    "accounts",
    "categories",
    "companies",
    "contacts",
    "currencies",
    "dashboards",
    "documents",
    "items",
    "reports",
    "taxes",
    "transactions",
    "transfers",
];

/// Path segments Akaunting serves on GET but which change state or reach
/// people outside this machine. Refused even though the method is GET.
const ACTION_SEGMENTS: &[&str] = &[
    "enable",
    "disable",
    "received",
    "sent",
    "cancelled",
    "cancel",
    "duplicate",
    "email",
    "emails",
    "send",
    "import",
    "restore",
];

/// Accept only Akaunting's API, read-only.
pub fn validate_path(path: &str) -> Result<(), String> {
    if path.contains("..")
        || path.contains("://")
        || path.contains('\\')
        || path.contains('@')
        || path.chars().any(|c| c.is_control() || c == ' ')
    {
        return Err(format!("'{path}' is not a plain API path."));
    }
    let Some(rest) = path.strip_prefix("/api/") else {
        return Err(format!(
            "'{path}' is not an accounting API path. Use /api/<resource>, for \
             example /api/transactions, /api/documents?search=type:invoice, \
             /api/accounts, /api/contacts, /api/categories."
        ));
    };
    let route = rest.split(['?', '#']).next().unwrap_or("");
    let mut segments = route.split('/').filter(|s| !s.is_empty());
    let resource = segments.next().unwrap_or("");
    if !READABLE.contains(&resource) {
        return Err(format!(
            "'{resource}' is not an accounting resource this tool reads. Readable: {}.",
            READABLE.join(", ")
        ));
    }
    let is_action = |s: &&str| ACTION_SEGMENTS.contains(&s.to_ascii_lowercase().as_str());
    if let Some(action) = segments.find(is_action) {
        return Err(format!(
            "'{action}' changes the books or contacts someone; this tool only reads. \
             Ask the owner to do it in Akaunting itself."
        ));
    }
    Ok(())
}

/// The stored credentials, or an answer that tells the agent -- and through
/// it the owner -- exactly what to do.
fn credentials() -> Result<(String, String), String> {
    let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    match (get(EMAIL_ENV), get(PASSWORD_ENV)) {
        (Some(e), Some(p)) => Ok((e.trim().to_string(), p)),
        _ => Err(format!(
            "Accounting is not connected: no Akaunting login is stored ({EMAIL_ENV} / \
             {PASSWORD_ENV}). Ask the owner to finish Akaunting's first-run wizard at \
             {BASE_URL}, then open Services -> Akaunting -> Connect and enter the login \
             there. Do not ask for the password in chat, and do not try other ways to \
             reach the books."
        )),
    }
}

fn company_id() -> String {
    std::env::var(COMPANY_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| "1".to_string())
}

fn client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        // The fixed address is the whole point; a redirect would undo it.
        // Akaunting also answers a bad login with a redirect to its login page.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("could not build HTTP client: {e}"))
}

/// Read from the books. `input`: `path` (required, under `/api/`), optional
/// `method`, which must be `GET` if given.
pub async fn accounting_api(input: &serde_json::Value) -> Result<String, String> {
    let method = input["method"]
        .as_str()
        .unwrap_or("GET")
        .to_ascii_uppercase();
    if method != "GET" {
        return Err(format!(
            "'{method}' is not allowed: accounting_api only reads the books. Recording, \
             editing or deleting entries is the owner's decision; ask them to do it in \
             Akaunting at {BASE_URL}."
        ));
    }
    let path = input["path"].as_str().unwrap_or("").trim();
    validate_path(path)?;
    let (email, password) = credentials()?;

    let resp = client(30)?
        .get(format!("{BASE_URL}{path}"))
        .basic_auth(email, Some(password))
        .header("Accept", "application/json")
        .header("X-Company", company_id())
        .send()
        .await
        .map_err(|e| {
            format!(
                "Akaunting did not answer at {BASE_URL} ({e}). It may be stopped: check \
                 the accounting hand (hand_status accounting)."
            )
        })?;
    let status = resp.status();
    let mut text = resp
        .text()
        .await
        .map_err(|e| format!("could not read Akaunting's reply: {e}"))?;
    if text.len() > MAX_RESPONSE_CHARS {
        let mut cut = MAX_RESPONSE_CHARS;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n…[truncated: ask for fewer records with ?limit=]");
    }
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || status.is_redirection()
    {
        return Err(format!(
            "Akaunting refused the stored login (HTTP {status}). The password may have \
             changed, or the user lacks API access (the read-api permission): the owner \
             can re-enter it under Services -> Akaunting -> Connect."
        ));
    }
    if !status.is_success() {
        return Err(format!("Akaunting returned HTTP {status}: {text}"));
    }
    Ok(text)
}

/// Check a login against Akaunting before it is stored, so "Connect" says
/// whether it works instead of saving one that does not.
pub async fn verify_credentials(email: &str, password: &str) -> Result<(), String> {
    let resp = client(15)?
        .get(format!("{BASE_URL}/api/ping"))
        .basic_auth(email.trim(), Some(password))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Akaunting did not answer at {BASE_URL}: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || status.is_redirection()
    {
        Err(
            "Akaunting refused this login. Use the e-mail and password of an Akaunting \
             user with API access (the admin from the first-run wizard has it)."
                .to_string(),
        )
    } else {
        Err(format!("Akaunting answered HTTP {status}."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_readable_accounting_paths_are_accepted() {
        for ok in [
            "/api/ping",
            "/api/transactions",
            "/api/transactions?search=type:income&limit=20",
            "/api/documents?search=type:invoice",
            "/api/documents/12",
            "/api/accounts",
            "/api/contacts?search=type:customer",
            "/api/reports",
        ] {
            assert!(validate_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "api/transactions",
            "/rest/tasks",
            "/api/users",
            "/api/settings",
            "/api/apps",
            "/api/../install",
            "/api/transactions@evil.com",
            "/api/x://evil",
            "/api/transactions\\..",
            "/api/a b",
        ] {
            assert!(validate_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn action_routes_served_on_get_are_refused() {
        for bad in [
            "/api/documents/3/received",
            "/api/documents/3/sent",
            "/api/documents/3/cancelled",
            "/api/contacts/4/enable",
            "/api/accounts/1/disable",
            "/api/documents/3/email",
            "/api/documents/3/Duplicate",
        ] {
            let err = validate_path(bad).unwrap_err();
            assert!(err.contains("only reads"), "{bad}: {err}");
        }
    }

    #[tokio::test]
    async fn writes_are_refused_before_any_request() {
        for method in ["POST", "PUT", "PATCH", "DELETE", "post"] {
            let err = accounting_api(&serde_json::json!({
                "method": method,
                "path": "/api/transactions",
                "body": {"amount": 1}
            }))
            .await
            .unwrap_err();
            assert!(err.contains("only reads the books"), "{method}: {err}");
        }
    }

    #[tokio::test]
    async fn missing_credentials_say_how_to_connect() {
        // Only meaningful where no login is stored; the message is what matters.
        if std::env::var(EMAIL_ENV).is_err() || std::env::var(PASSWORD_ENV).is_err() {
            let err = accounting_api(&serde_json::json!({"path": "/api/transactions"}))
                .await
                .unwrap_err();
            assert!(err.contains("Connect"), "{err}");
        }
    }

    #[test]
    fn company_id_defaults_to_the_wizard_company() {
        if std::env::var(COMPANY_ENV).is_err() {
            assert_eq!(company_id(), "1");
        }
    }
}

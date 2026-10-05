//! `crm_api` — agents' access to the CRM running on this machine.
//!
//! Twenty CRM listens on the loopback interface. `web_fetch` refuses loopback
//! addresses on purpose (a model that can reach 127.0.0.1 can reach this
//! product's own API and every local database), and that refusal is right. It
//! also meant no agent could reach the CRM at all, so a crm agent given its
//! key had no way to use it and asked for host shell access instead.
//!
//! This tool is the narrow exception: one fixed address, only the CRM's API
//! paths, and the key read from the vault by the tool itself. The key never
//! passes through the model, and the model cannot point the tool anywhere else.

use std::time::Duration;

/// Where the vault keeps the CRM key. Saved from the Services page or from a
/// chat message the secret guard caught.
pub const KEY_ENV: &str = "CRM_API_KEY";

/// Twenty's address. Host networking puts it on the loopback interface; 3200,
/// not Twenty's default 3000, because dograh's UI already listens there.
pub const BASE_URL: &str = "http://localhost:3200";

/// Largest response handed back to the model. A full export of a CRM would
/// otherwise fill the context window in one call.
const MAX_RESPONSE_CHARS: usize = 40_000;

/// Accept only the CRM's own API: REST under `/rest/`, and `/graphql`.
///
/// Refuses anything that could turn a path into another destination or climb
/// out of the API: a scheme, an authority, `..`, backslashes, control
/// characters.
pub fn validate_path(path: &str) -> Result<(), String> {
    let api = path.starts_with("/rest/")
        || path == "/graphql"
        || path.starts_with("/graphql?")
        || path == "/metadata";
    if !api {
        return Err(format!(
            "'{path}' is not a CRM API path. Use /rest/<object> (for example \
             /rest/tasks, /rest/people, /rest/companies, /rest/notes, \
             /rest/opportunities) or /graphql."
        ));
    }
    if path.contains("..")
        || path.contains("://")
        || path.contains('\\')
        || path.contains('@')
        || path.chars().any(|c| c.is_control() || c == ' ')
    {
        return Err(format!("'{path}' is not a plain API path."));
    }
    Ok(())
}

/// The key, or an answer that tells the agent -- and through it the owner --
/// exactly what to do.
fn api_key() -> Result<String, String> {
    match std::env::var(KEY_ENV) {
        Ok(k) if !k.trim().is_empty() => Ok(k.trim().to_string()),
        _ => Err(format!(
            "The CRM is not connected: no API key is stored ({KEY_ENV}). Ask the \
             owner to open Services -> Twenty CRM -> Connect and paste a key \
             created in the CRM under Settings -> APIs & Webhooks. Do not ask \
             for the key in chat, and do not try other ways to reach the CRM."
        )),
    }
}

/// Call the CRM. `input`: `method` (GET/POST/PATCH/DELETE, default GET),
/// `path`, optional JSON `body`.
pub async fn crm_api(input: &serde_json::Value) -> Result<String, String> {
    let method = input["method"]
        .as_str()
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let method = match method.as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PATCH" => reqwest::Method::PATCH,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        other => return Err(format!("Unsupported method '{other}'.")),
    };
    let path = input["path"].as_str().unwrap_or("").trim();
    validate_path(path)?;
    let key = api_key()?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // The fixed address is the whole point; a redirect would undo it.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("could not build HTTP client: {e}"))?;
    let mut req = client
        .request(method, format!("{BASE_URL}{path}"))
        .bearer_auth(key)
        .header("Accept", "application/json");
    match &input["body"] {
        serde_json::Value::Null => {}
        // Models sometimes send the body as a JSON string.
        serde_json::Value::String(s) => {
            let v: serde_json::Value =
                serde_json::from_str(s).map_err(|e| format!("body is not valid JSON: {e}"))?;
            req = req.json(&v);
        }
        v => req = req.json(v),
    }

    let resp = req.send().await.map_err(|e| {
        format!(
            "The CRM did not answer at {BASE_URL} ({e}). It may be stopped: \
             check Services -> Twenty CRM."
        )
    })?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("could not read the CRM's reply: {e}"))?;
    let mut text = text;
    if text.len() > MAX_RESPONSE_CHARS {
        let mut cut = MAX_RESPONSE_CHARS;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n…[truncated: ask for fewer records with ?limit=]");
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!(
            "The CRM refused the stored key (HTTP {status}). It may have been \
             revoked or belong to another workspace: the owner can replace it \
             under Services -> Twenty CRM -> Connect. Reply: {text}"
        ));
    }
    if !status.is_success() {
        return Err(format!("CRM returned HTTP {status}: {text}"));
    }
    Ok(text)
}

/// Check a key against the CRM before it is stored, so "Connect" says whether
/// it works instead of saving a key that does not.
pub async fn verify_key(key: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{BASE_URL}/rest/tasks?limit=1"))
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(|e| format!("The CRM did not answer at {BASE_URL}: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        Err(
            "The CRM refused this key. Create a new one in the CRM under \
             Settings -> APIs & Webhooks and paste it here."
                .to_string(),
        )
    } else {
        Err(format!("The CRM answered HTTP {status}."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_crm_api_paths_are_accepted() {
        for ok in [
            "/rest/tasks",
            "/rest/tasks?limit=5&orderBy=dueAt",
            "/rest/people/0f6e-11",
            "/graphql",
        ] {
            assert!(validate_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "/api/agents",
            "rest/tasks",
            "/rest/../api/agents",
            "/rest/x@evil.com",
            "/rest//evil.com://",
            "/rest/tasks\\..",
            "/rest/a b",
            "",
        ] {
            assert!(validate_path(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_missing_key_says_how_to_connect() {
        // Only meaningful where no key is set; the message is what matters.
        if std::env::var(KEY_ENV).is_err() {
            let err = crm_api(&serde_json::json!({"path": "/rest/tasks"}))
                .await
                .unwrap_err();
            assert!(err.contains("Connect"), "{err}");
        }
    }
}

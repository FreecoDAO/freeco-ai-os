//! Connecting the CRM: the key goes to the vault, never into a conversation.
//!
//! `POST /api/services/crm/connect` with `{ "key": "…" }` checks the key
//! against the CRM, stores it, and from then on the `crm_api` tool can reach
//! the CRM for every agent that has the tool.

use crate::routes::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;

pub use freeco_kernel_runtime::crm_tool::KEY_ENV;

/// Is a CRM key stored? What the Services page shows as "connected".
pub fn is_connected() -> bool {
    std::env::var(KEY_ENV).is_ok_and(|k| !k.trim().is_empty())
}

/// POST /api/services/{id}/connect — store a service's API key.
pub async fn connect_service(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if id != "crm" {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("'{id}' has no key to connect.")})),
        );
    }
    let key = body["key"].as_str().unwrap_or("").trim().to_string();
    if key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Paste the key first."})),
        );
    }
    // Check before storing: a saved key that does not work would show the CRM
    // as connected while every agent call fails.
    if let Err(e) = freeco_kernel_runtime::crm_tool::verify_key(&key).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        );
    }
    if let Err(e) = crate::secret_guard::store_secret(&state, KEY_ENV, &key) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "message": "CRM connected. Agents with the crm_api tool can now read and create tasks, people, companies and notes."
        })),
    )
}

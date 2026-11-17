//! What the dashboard's sidebar and Live view read.
//!
//! - `GET /api/activity` — what every agent is doing this second.
//! - `GET /api/org/chats` — the newest conversations of every agent, for the
//!   chat list in the sidebar.
//! - `GET /api/tasks` — the shared task board. It was reachable only through
//!   agents' own tools and MCP, so a task made with "New task" could be
//!   created from the dashboard but never seen there again.

use crate::routes::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use freeco_kernel_runtime::activity::{ActivityState, AgentActivity};
use std::collections::HashMap;
use std::sync::Arc;

/// One agent's row in `/api/activity`.
fn activity_row(
    id: &str,
    name: &str,
    seen: Option<&AgentActivity>,
    waiting_approvals: usize,
) -> serde_json::Value {
    let (state, tool, since_ms, turns, steps) = match seen {
        Some(a) => (
            a.state,
            a.tool.clone(),
            a.since_ms,
            a.turns,
            serde_json::to_value(&a.steps).unwrap_or_default(),
        ),
        None => (ActivityState::Idle, None, 0, 0, serde_json::json!([])),
    };
    // A tool waiting on the owner's yes is the one state the owner can end,
    // so it outranks "running a tool" in what the Live view shows.
    let state_name = if waiting_approvals > 0 {
        serde_json::json!("waiting_for_you")
    } else {
        serde_json::to_value(state).unwrap_or_default()
    };
    serde_json::json!({
        "agent_id": id,
        "name": name,
        "state": state_name,
        "tool": tool,
        "since_ms": since_ms,
        "turns": turns,
        "waiting_approvals": waiting_approvals,
        "steps": steps,
    })
}

/// GET /api/activity — every agent's live state.
pub async fn activity(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let seen: HashMap<String, AgentActivity> = state
        .kernel
        .activity
        .snapshot(now_ms)
        .into_iter()
        .map(|a| (a.agent_id.clone(), a))
        .collect();
    let mut waiting: HashMap<String, usize> = HashMap::new();
    for req in state.kernel.approval_manager.list_pending() {
        *waiting.entry(req.agent_id).or_default() += 1;
    }
    let mut agents: Vec<serde_json::Value> = state
        .kernel
        .registry
        .list()
        .iter()
        .map(|e| {
            let id = e.id.to_string();
            activity_row(
                &id,
                &e.name,
                seen.get(&id),
                waiting.get(&id).copied().unwrap_or(0),
            )
        })
        .collect();
    agents.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    Json(serde_json::json!({ "now_ms": now_ms, "agents": agents }))
}

#[derive(serde::Deserialize, Default)]
pub struct ChatsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/org/chats?limit=60 — newest conversations of every agent.
pub async fn recent_chats(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ChatsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(60).clamp(1, 500);
    match state.kernel.memory.org().recent_sessions(limit) {
        Ok(mut rows) => {
            for row in &mut rows {
                let busy = row["id"]
                    .as_str()
                    .and_then(|sid| uuid::Uuid::parse_str(sid).ok())
                    .is_some_and(|u| {
                        state
                            .kernel
                            .is_conversation_busy(freeco_types::agent::SessionId(u))
                    });
                let agent_name = row["agent_id"]
                    .as_str()
                    .and_then(|a| a.parse::<freeco_types::agent::AgentId>().ok())
                    .and_then(|a| state.kernel.registry.get(a))
                    .map(|e| e.name);
                row["busy"] = serde_json::Value::Bool(busy);
                row["agent_name"] = serde_json::json!(agent_name);
            }
            (StatusCode::OK, Json(serde_json::json!({ "chats": rows })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

#[derive(serde::Deserialize, Default)]
pub struct TasksQuery {
    #[serde(default)]
    pub status: Option<String>,
}

/// GET /api/tasks?status=pending|in_progress|completed — the task board.
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TasksQuery>,
) -> impl IntoResponse {
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status {
        if !["pending", "in_progress", "completed"].contains(&s) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "status must be pending, in_progress or completed"
                })),
            );
        }
    }
    match state.kernel.memory.task_list(status).await {
        Ok(mut tasks) => {
            // Name the agents: an id is not something the owner can read.
            for t in &mut tasks {
                let name = t["assigned_to"]
                    .as_str()
                    .and_then(|a| a.parse::<freeco_types::agent::AgentId>().ok())
                    .and_then(|a| state.kernel.registry.get(a))
                    .map(|e| e.name);
                t["assigned_name"] = serde_json::json!(name);
            }
            (StatusCode::OK, Json(serde_json::json!({ "tasks": tasks })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeco_kernel_runtime::activity::Step;

    fn seen(state: ActivityState, tool: Option<&str>) -> AgentActivity {
        AgentActivity {
            agent_id: "a1".into(),
            agent_name: "Shopping".into(),
            state,
            tool: tool.map(String::from),
            since_ms: 42,
            turns: 1,
            steps: vec![Step {
                kind: "tool",
                name: tool.map(String::from),
                started_ms: 42,
                ended_ms: None,
                ok: None,
            }],
        }
    }

    #[test]
    fn an_agent_never_seen_is_idle() {
        let row = activity_row("a1", "Shopping", None, 0);
        assert_eq!(row["state"], "idle");
        assert_eq!(row["steps"], serde_json::json!([]));
    }

    #[test]
    fn a_running_tool_is_reported_with_its_name_and_start() {
        let a = seen(ActivityState::Tool, Some("web_fetch"));
        let row = activity_row("a1", "Shopping", Some(&a), 0);
        assert_eq!(row["state"], "tool");
        assert_eq!(row["tool"], "web_fetch");
        assert_eq!(row["since_ms"], 42);
        assert_eq!(row["steps"][0]["name"], "web_fetch");
    }

    /// The owner's yes is what the agent is really waiting for.
    #[test]
    fn a_pending_approval_outranks_the_running_tool() {
        let a = seen(ActivityState::Tool, Some("shell_exec"));
        let row = activity_row("a1", "Shopping", Some(&a), 1);
        assert_eq!(row["state"], "waiting_for_you");
        assert_eq!(row["waiting_approvals"], 1);
        assert_eq!(row["tool"], "shell_exec");
    }

    #[test]
    fn waiting_on_a_helper_keeps_its_own_state() {
        let a = seen(ActivityState::WaitingOnHelper, Some("agent_delegate"));
        let row = activity_row("a1", "Concierge", Some(&a), 0);
        assert_eq!(row["state"], "waiting_on_helper");
    }
}

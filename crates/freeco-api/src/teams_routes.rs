//! The Teams floor: every team, who is on it, what each member is doing right
//! now, the team's task board, and the latest hand-offs between agents.
//!
//! One rule shapes every field here: the page may only show what the daemon
//! can prove. "Working" means a run holds one of that agent's conversations
//! at this moment, not that it was busy a while ago; a hand-off is listed
//! because the kernel recorded it starting, and it says "done" only once the
//! other agent's reply came back.

use crate::routes::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How many hand-offs the floor shows.
const FLOOR_HANDOFFS: usize = 30;

fn fail(code: StatusCode, msg: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg.to_string() })))
}

/// Per agent: how many of its conversations a run is working in right now.
fn working_counts(state: &AppState) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for sid in state.kernel.busy_conversations() {
        if let Ok(Some(session)) = state.kernel.memory.get_session(sid) {
            *out.entry(session.agent_id.to_string()).or_default() += 1;
        }
    }
    out
}

/// GET /api/teams/floor — the live view the Teams page draws.
pub async fn floor(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let teams = match org.list_teams(false) {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let tasks = match state.kernel.memory.task_board(None, None).await {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let working = working_counts(&state);
    let agents: HashMap<String, (String, String)> = state
        .kernel
        .registry
        .list()
        .into_iter()
        .map(|e| (e.id.to_string(), (e.name.clone(), format!("{:?}", e.state))))
        .collect();

    let member = |agent_id: &str, role: Option<&str>| {
        let (name, agent_state) = agents
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| ("(removed agent)".to_string(), "Missing".to_string()));
        let busy = working.get(agent_id).copied().unwrap_or(0);
        serde_json::json!({
            "agent_id": agent_id,
            "name": name,
            "role": role,
            "state": agent_state,
            "working": busy > 0,
            "working_in": busy,
        })
    };

    let mut on_a_team: HashSet<String> = HashSet::new();
    let mut out_teams = Vec::with_capacity(teams.len());
    for team in &teams {
        let team_id = team["id"].as_str().unwrap_or_default();
        let members: Vec<serde_json::Value> = team["members"]
            .as_array()
            .map(|ms| {
                ms.iter()
                    .filter_map(|m| {
                        let aid = m["agent_id"].as_str()?;
                        on_a_team.insert(aid.to_string());
                        Some(member(aid, m["role"].as_str()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let board: Vec<serde_json::Value> = tasks
            .iter()
            .filter(|t| t["team_id"].as_str() == Some(team_id))
            .cloned()
            .collect();
        let count = |pred: &dyn Fn(&serde_json::Value) -> bool| board.iter().filter(|t| pred(t)).count();
        out_teams.push(serde_json::json!({
            "id": team_id,
            "name": team["name"],
            "project_id": team["project_id"],
            "project": team["project"],
            "members": members,
            "tasks": {
                "ready": count(&|t| t["status"] == "pending" && t["blocked"] != true),
                "blocked": count(&|t| t["blocked"] == true),
                "in_progress": count(&|t| t["status"] == "in_progress"),
                "completed": count(&|t| t["status"] == "completed"),
            },
            "board": board,
        }));
    }

    let mut unassigned: Vec<serde_json::Value> = agents
        .keys()
        .filter(|id| !on_a_team.contains(*id))
        .map(|id| member(id, None))
        .collect();
    unassigned.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default())
    });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "teams": out_teams,
            "unassigned": unassigned,
            "handoffs": state.kernel.recent_handoffs(FLOOR_HANDOFFS),
            "generated_at": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

#[derive(Deserialize)]
pub struct AddMember {
    pub agent_id: String,
    /// `lead` for the agent that splits the work and hands it out; anything
    /// else, or nothing, for a member.
    pub role: Option<String>,
}

/// POST /api/org/teams/{id}/members — put an agent on a team.
pub async fn add_member(
    State(state): State<Arc<AppState>>,
    Path(team): Path<String>,
    Json(body): Json<AddMember>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let (team_id, team_name) = match org.find_team(&team) {
        Ok(Some(t)) => t,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such team"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let agent = match body.agent_id.trim().parse::<freeco_types::agent::AgentId>() {
        Ok(id) => state.kernel.registry.get(id),
        Err(_) => state.kernel.registry.find_by_name(body.agent_id.trim()),
    };
    let Some(agent) = agent else {
        return fail(StatusCode::NOT_FOUND, "No such agent");
    };
    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    match org.add_team_member(&team_id, &agent.id.to_string(), role) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "team_id": team_id,
                "team": team_name,
                "agent_id": agent.id.to_string(),
                "name": agent.name,
                "role": role,
            })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/org/teams/{id}/members/{agent_id} — take an agent off a team.
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    Path((team, agent_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let team_id = match org.find_team(&team) {
        Ok(Some((id, _))) => id,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such team"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    match org.remove_team_member(&team_id, agent_id.trim()) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "removed": true }))),
        Ok(false) => fail(StatusCode::NOT_FOUND, "That agent is not on this team"),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
pub struct PostTask {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub assigned_to: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// GET /api/org/teams/{id}/tasks — one team's board.
pub async fn team_tasks(
    State(state): State<Arc<AppState>>,
    Path(team): Path<String>,
) -> impl IntoResponse {
    let team_id = match state.kernel.memory.org().find_team(&team) {
        Ok(Some((id, _))) => id,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such team"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    match state.kernel.memory.task_board(None, Some(&team_id)).await {
        Ok(tasks) => (StatusCode::OK, Json(serde_json::json!({ "tasks": tasks }))),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/org/teams/{id}/tasks — the user puts a task on a team's board.
///
/// The same path an agent's `task_post` takes, so the checks are the same:
/// the team must exist, an assignee must be on it, dependencies must exist.
pub async fn post_team_task(
    State(state): State<Arc<AppState>>,
    Path(team): Path<String>,
    Json(body): Json<PostTask>,
) -> impl IntoResponse {
    use freeco_kernel_runtime::kernel_handle::KernelHandle;
    let title = body.title.trim();
    if title.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "A task needs a title.");
    }
    let assigned = body
        .assigned_to
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    match state
        .kernel
        .team_task_post(
            title,
            body.description.trim(),
            assigned,
            Some("user"),
            Some(&team),
            &body.blocked_by,
        )
        .await
    {
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))),
        Err(e) => fail(StatusCode::BAD_REQUEST, e),
    }
}

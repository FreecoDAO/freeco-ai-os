//! HTTP surface for companies, projects and teams.
//!
//! The data layer alone is invisible: conversations can belong to a project
//! and the user still has no way to see it, which is indistinguishable from
//! the feature not existing. These endpoints are what make the structure real.

use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct ArchivedFlag {
    /// Include archived rows. Off by default so finished work does not crowd
    /// the list, but reachable rather than hidden forever.
    #[serde(default)]
    pub archived: bool,
}

#[derive(Deserialize)]
pub struct CreateCompany {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub company_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateTeam {
    pub name: String,
    pub project_id: Option<String>,
}

#[derive(Deserialize)]
pub struct AssignSession {
    pub project_id: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Deserialize)]
pub struct StateFlag {
    pub value: bool,
}

fn fail(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
}

/// GET /api/org/overview — one line per project with its conversation count.
///
/// This is what lets the assistant act across the whole organisation without
/// loading every message into a prompt.
pub async fn overview(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.kernel.memory.org().org_overview() {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({ "projects": rows })),
        ),
        Err(e) => fail(e),
    }
}

pub async fn list_companies(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ArchivedFlag>,
) -> impl IntoResponse {
    match state.kernel.memory.org().list_companies(q.archived) {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({ "companies": rows })),
        ),
        Err(e) => fail(e),
    }
}

pub async fn create_company(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateCompany>,
) -> impl IntoResponse {
    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "A company needs a name." })),
        );
    }
    match state
        .kernel
        .memory
        .org()
        .create_company(body.name.trim(), body.description.as_deref())
    {
        Ok(c) => {
            state
                .kernel
                .record_org_change("owner", format!("created company \"{}\"", body.name.trim()));
            (StatusCode::OK, Json(serde_json::json!(c)))
        }
        Err(e) => fail(e),
    }
}

#[derive(Deserialize)]
pub struct SetMember {
    pub agent_id: String,
    /// `supervisor`, `member`, or a free description of the role.
    pub role: Option<String>,
}

/// PUT /api/org/teams/{id}/members — add an agent to a team, or change its
/// role there (`supervisor` makes it the team's supervisor; a team has one).
pub async fn set_team_member(
    State(state): State<Arc<AppState>>,
    Path(team_id): Path<String>,
    Json(body): Json<SetMember>,
) -> impl IntoResponse {
    let Some(agent) = body
        .agent_id
        .parse()
        .ok()
        .and_then(|id| state.kernel.registry.get(id))
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No such agent." })),
        );
    };
    let org = state.kernel.memory.org();
    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    // One supervisor per team: appointing one makes the previous a member.
    if role == Some("supervisor") {
        if let Ok(teams) = org.list_teams(true) {
            let current: Vec<String> = teams
                .iter()
                .filter(|t| t["id"].as_str() == Some(team_id.as_str()))
                .flat_map(|t| t["members"].as_array().cloned().unwrap_or_default())
                .filter(|m| m["role"].as_str() == Some("supervisor"))
                .filter_map(|m| m["agent_id"].as_str().map(str::to_string))
                .filter(|id| id != &body.agent_id)
                .collect();
            for id in current {
                let _ = org.add_team_member(&team_id, &id, Some("member"));
            }
        }
    }
    match org.add_team_member(&team_id, &body.agent_id, role) {
        Ok(()) => {
            state.kernel.record_org_change(
                "owner",
                format!(
                    "set {} in team {} as {}",
                    agent.name,
                    team_id,
                    role.unwrap_or("member")
                ),
            );
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }
        Err(e) => fail(e),
    }
}

/// DELETE /api/org/teams/{id}/members/{agent_id} — take an agent out of a team.
pub async fn remove_team_member(
    State(state): State<Arc<AppState>>,
    Path((team_id, agent_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match state
        .kernel
        .memory
        .org()
        .remove_team_member(&team_id, &agent_id)
    {
        Ok(removed) => {
            if removed {
                let name = agent_id
                    .parse()
                    .ok()
                    .and_then(|id| state.kernel.registry.get(id))
                    .map(|e| e.name.clone())
                    .unwrap_or(agent_id.clone());
                state
                    .kernel
                    .record_org_change("owner", format!("removed {name} from team {team_id}"));
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "removed": removed })),
            )
        }
        Err(e) => fail(e),
    }
}

/// GET /api/org/changes — the organisation change log, newest first:
/// structure created, members set, hires made or held for the owner.
pub async fn changes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "changes": state.kernel.org_changes(100) }))
}

pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ArchivedFlag>,
) -> impl IntoResponse {
    match state.kernel.memory.org().list_projects(q.archived) {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({ "projects": rows })),
        ),
        Err(e) => fail(e),
    }
}

/// GET /api/org/teams — every team, with its project and its members.
///
/// The route accepted POST only, so a team could be created and never listed:
/// the dashboard had no way to show one and an agent had no way to find the id
/// its own team memory is keyed by.
pub async fn list_teams(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ArchivedFlag>,
) -> impl IntoResponse {
    match state.kernel.memory.org().list_teams(q.archived) {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "teams": rows }))),
        Err(e) => fail(e),
    }
}

/// POST /api/org/projects — create, or return the existing project of that
/// name.
///
/// Routing rather than blind creation: two projects called "Kubuntu USB" and
/// "kubuntu usb" would split one team's history in half, which is the memory
/// loss this structure exists to prevent.
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProject>,
) -> impl IntoResponse {
    let name = body.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "A project needs a name." })),
        );
    }
    let org = state.kernel.memory.org();
    match org.find_project_by_name(name) {
        Ok(Some(existing)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "project": existing, "reused": true })),
        ),
        Ok(None) => match org.create_project(
            name,
            body.company_id.as_deref(),
            body.description.as_deref(),
        ) {
            Ok(p) => {
                state
                    .kernel
                    .record_org_change("owner", format!("created project \"{name}\""));
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "project": p, "reused": false })),
                )
            }
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    }
}

pub async fn create_team(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTeam>,
) -> impl IntoResponse {
    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "A team needs a name." })),
        );
    }
    match state
        .kernel
        .memory
        .org()
        .create_team(body.name.trim(), body.project_id.as_deref())
    {
        Ok(t) => {
            state
                .kernel
                .record_org_change("owner", format!("created team \"{}\"", body.name.trim()));
            (StatusCode::OK, Json(serde_json::json!(t)))
        }
        Err(e) => fail(e),
    }
}

/// GET /api/org/projects/:id/sessions — a project's own history.
pub async fn project_sessions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.kernel.memory.org().sessions_for_project(&id) {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({ "sessions": rows })),
        ),
        Err(e) => fail(e),
    }
}

/// PUT /api/sessions/:id/assign — move a conversation into a project/team.
pub async fn assign_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<AssignSession>,
) -> impl IntoResponse {
    match state.kernel.memory.org().assign_session(
        &id,
        body.project_id.as_deref(),
        body.team_id.as_deref(),
    ) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))),
        Err(e) => fail(e),
    }
}

/// PUT /api/sessions/:id/archive — "done, keep it".
pub async fn archive_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<StateFlag>,
) -> impl IntoResponse {
    match state
        .kernel
        .memory
        .org()
        .set_session_archived(&id, body.value)
    {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "archived": body.value })),
        ),
        Err(e) => fail(e),
    }
}

/// PUT /api/sessions/:id/trash — "wrong, hide it".
///
/// Hides, never deletes. After losing history to a compaction bug, trash here
/// means recoverable; only an explicit purge removes anything.
pub async fn trash_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<StateFlag>,
) -> impl IntoResponse {
    match state
        .kernel
        .memory
        .org()
        .set_session_trashed(&id, body.value)
    {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "trashed": body.value })),
        ),
        Err(e) => fail(e),
    }
}

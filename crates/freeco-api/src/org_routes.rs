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

#[derive(Deserialize)]
pub struct UpdateProject {
    pub name: Option<String>,
    /// The project's brief, read by every agent working in it.
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct StartConversation {
    pub agent_id: String,
    pub label: Option<String>,
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
        Ok(c) => (StatusCode::OK, Json(serde_json::json!(c))),
        Err(e) => fail(e),
    }
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
            Ok(p) => (
                StatusCode::OK,
                Json(serde_json::json!({ "project": p, "reused": false })),
            ),
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
        Ok(t) => (StatusCode::OK, Json(serde_json::json!(t))),
        Err(e) => fail(e),
    }
}

/// GET /api/org/projects/:id/sessions — a project's own history.
///
/// Each conversation carries its agent's name and whether a run is working in
/// it right now, so the page shows a project's parallel work as it happens.
pub async fn project_sessions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.kernel.memory.org().sessions_for_project(&id) {
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
            let working = rows.iter().filter(|r| r["busy"] == true).count();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "sessions": rows, "working": working })),
            )
        }
        Err(e) => fail(e),
    }
}

/// GET /api/org/projects/:id — one project, with its brief.
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.kernel.memory.org().get_project(&id) {
        Ok(Some(p)) => (StatusCode::OK, Json(serde_json::json!({ "project": p }))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No such project" })),
        ),
        Err(e) => fail(e),
    }
}

/// PUT /api/org/projects/:id — rename a project or rewrite its brief.
pub async fn update_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProject>,
) -> impl IntoResponse {
    let name = body.name.as_deref().map(str::trim);
    if name.is_some_and(str::is_empty) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "A project needs a name." })),
        );
    }
    match state
        .kernel
        .memory
        .org()
        .update_project(&id, name, body.description.as_deref())
    {
        Ok(Some(p)) => (StatusCode::OK, Json(serde_json::json!({ "project": p }))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No such project" })),
        ),
        Err(e) => fail(e),
    }
}

/// POST /api/org/projects/:id/conversations — start a conversation in a project.
///
/// A new conversation of the chosen agent, filed under the project. It does
/// not become the agent's current conversation, so starting one never pulls
/// the agent out of a chat already in progress: any number run side by side.
pub async fn start_project_conversation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<StartConversation>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let project = match org.get_project(&id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "No such project" })),
            )
        }
        Err(e) => return fail(e),
    };
    let Some(entry) = body
        .agent_id
        .parse::<freeco_types::agent::AgentId>()
        .ok()
        .and_then(|a| state.kernel.registry.get(a))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No such agent" })),
        );
    };
    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .unwrap_or_else(|| format!("{} — {}", project.name, entry.name));
    let session = match state
        .kernel
        .memory
        .create_session_with_label(entry.id, Some(&label))
    {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let session_id = session.id.0.to_string();
    if let Err(e) = org.assign_session(&session_id, Some(&project.id), None) {
        return fail(e);
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "session_id": session_id,
            "agent_id": entry.id.to_string(),
            "agent_name": entry.name,
            "label": label,
            "project_id": project.id,
        })),
    )
}

#[derive(Debug, Deserialize)]
pub struct AddTeamMember {
    pub agent_id: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// POST /api/org/teams/:id/members — put an agent on a team.
///
/// Members are what a team run hands work to, and the dashboard had no way
/// to add one: a team could be created and never staffed.
pub async fn add_team_member(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<AddTeamMember>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    match org.get_team(&id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "No such team" })),
            )
        }
        Err(e) => return fail(e),
    }
    let Some(entry) = body
        .agent_id
        .parse::<freeco_types::agent::AgentId>()
        .ok()
        .and_then(|a| state.kernel.registry.get(a))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No such agent" })),
        );
    };
    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if let Err(e) = org.add_team_member(&id, &entry.id.to_string(), role) {
        return fail(e);
    }
    match org.get_team(&id) {
        Ok(team) => (StatusCode::OK, Json(serde_json::json!({ "team": team }))),
        Err(e) => fail(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct RunTeam {
    pub goal: String,
}

/// POST /api/org/teams/:id/run — give a team a goal.
///
/// Its lead gets a new conversation filed under the team, with the goal as
/// its first message, and hands parts to the other members, who work at the
/// same time. Returns at once with the lead's conversation; the work shows
/// on the project page as it happens.
pub async fn run_team(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RunTeam>,
) -> impl IntoResponse {
    match state.kernel.memory.org().get_team(&id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "No such team" })),
            )
        }
        Err(e) => return fail(e),
    }
    match state.kernel.start_team_run(&id, &body.goal) {
        Ok(run) => (StatusCode::OK, Json(run)),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
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

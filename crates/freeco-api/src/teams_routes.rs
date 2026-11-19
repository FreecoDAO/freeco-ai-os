//! The Teams floor: every team, who is on it, what each member is doing right
//! now, the team's task board, and the latest hand-offs between agents.
//!
//! One rule shapes every field here: the page may only show what the daemon
//! can prove. "Working" means a run holds one of that agent's conversations
//! at this moment, not that it was busy a while ago; a hand-off is listed
//! because the kernel recorded it starting, and it says "done" only once the
//! other agent's reply came back.
//!
//! Also here: companies (read, rename, archive, delete), a team's members
//! (agents and people, with team roles) and the team board's cards. Every
//! handler checks the caller with [`crate::team_access`]: an owner or admin
//! reaches everything, anyone else only the teams they are on.

use crate::middleware::Caller;
use crate::routes::AppState;
use crate::team_access::{self, TeamAccess};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use freeco_memory::board::{assignee_person, person_assignee, TaskChange};
use freeco_memory::org::{OrgStore, TeamRole};
use freeco_types::auth::UserRole;
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
///
/// A caller who is not an owner or admin sees only the teams they are on,
/// only the hand-offs between agents of those teams, and no "not on a team"
/// lobby. Each team says what the caller may do on it (`access`).
pub async fn floor(State(state): State<Arc<AppState>>, caller: Caller) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let visible: HashSet<String> = match team_access::teams_visible_to(org, &caller) {
        Ok(v) => v.into_iter().collect(),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let teams: Vec<serde_json::Value> = match org.list_teams(false) {
        Ok(t) => t
            .into_iter()
            .filter(|t| t["id"].as_str().is_some_and(|id| visible.contains(id)))
            .collect(),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let companies_of = match org.team_companies() {
        Ok(c) => c,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let all_teams = team_access::manages_all_teams(&caller);
    // The company switcher: the companies the caller may see.
    let companies: Vec<freeco_memory::org::Company> = match (
        org.list_companies(false),
        team_access::companies_visible_to(org, &caller),
    ) {
        (Ok(list), Ok(visible)) => list
            .into_iter()
            .filter(|c| visible.as_ref().is_none_or(|v| v.contains(&c.id)))
            .collect(),
        (Err(e), _) | (_, Err(e)) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
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
        let count =
            |pred: &dyn Fn(&serde_json::Value) -> bool| board.iter().filter(|t| pred(t)).count();
        let people = match org.team_people(team_id) {
            Ok(p) => people_json(&p),
            Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        let access = match team_access::caller_team_access(org, &caller, team_id) {
            Ok(a) => a.map(TeamAccess::as_str),
            Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        out_teams.push(serde_json::json!({
            "id": team_id,
            "name": team["name"],
            "description": team["description"],
            "project_id": team["project_id"],
            "project": team["project"],
            "company_id": companies_of.get(team_id),
            "members": members,
            "people": people,
            "access": access,
            "tasks": {
                "ready": count(&|t| t["status"] == "pending" && t["blocked"] != true),
                "blocked": count(&|t| t["blocked"] == true),
                "in_progress": count(&|t| t["status"] == "in_progress"),
                "review": count(&|t| t["status"] == "review"),
                "completed": count(&|t| t["status"] == "completed"),
            },
            "board": board,
        }));
    }

    let mut unassigned: Vec<serde_json::Value> = agents
        .keys()
        .filter(|id| all_teams && !on_a_team.contains(*id))
        .map(|id| member(id, None))
        .collect();
    unassigned.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default())
    });

    // Someone who sees only their own teams sees only the hand-offs between
    // agents on them.
    let handoffs: Vec<_> = state
        .kernel
        .recent_handoffs(FLOOR_HANDOFFS)
        .into_iter()
        .filter(|h| {
            all_teams
                || (on_a_team.contains(&h.to_agent)
                    && h.from_agent.as_ref().is_none_or(|f| on_a_team.contains(f)))
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "teams": out_teams,
            "companies": companies,
            "manage_all": all_teams,
            "unassigned": unassigned,
            "handoffs": handoffs,
            "generated_at": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

/// People on a team as the API shows them, next to the agents.
fn people_json(people: &[freeco_memory::org::TeamPerson]) -> Vec<serde_json::Value> {
    people
        .iter()
        .map(|p| {
            serde_json::json!({
                "kind": "user",
                "id": person_assignee(&p.username),
                "username": p.username,
                "role": p.role,
                "joined_at": p.created_at,
            })
        })
        .collect()
}

/// A live team by id or name, as its id; the 404 otherwise.
fn resolve_team(org: &OrgStore, team: &str) -> Result<String, team_access::Denied> {
    match org.find_team(team) {
        Ok(Some((id, _))) => Ok(id),
        Ok(None) => Err(fail(StatusCode::NOT_FOUND, "No such team")),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// Resolve the team and check the caller may do `min` on it.
fn team_for(
    org: &OrgStore,
    caller: &Caller,
    team: &str,
    min: TeamAccess,
) -> Result<(String, TeamAccess), team_access::Denied> {
    let id = resolve_team(org, team)?;
    let access = team_access::require_team_access(org, caller, &id, min)?;
    Ok((id, access))
}

/// The accounts that exist: `auth.username` (the owner) and every enabled
/// `[[users]]` entry. A person can only be put on a team if they are one.
fn known_accounts(state: &AppState) -> Vec<String> {
    let config = &state.kernel.config;
    let mut names: Vec<String> = Vec::new();
    if config.auth.enabled && !config.auth.username.trim().is_empty() {
        names.push(config.auth.username.trim().to_string());
    }
    for u in config.users.iter().filter(|u| u.enabled) {
        let n = u.name.trim();
        if !n.is_empty() && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    }
    names
}

/// Who `created_by` names for a card a person put on the board.
fn poster(caller: &Caller) -> String {
    match caller.username.as_deref() {
        Some(u) => person_assignee(u),
        None => "user".to_string(),
    }
}

/// GET /api/org/teams/{id}/members — the agents and people on a team.
///
/// Agents keep their free-text role (`lead` leads a team run); people have a
/// team role of `lead`, `member` or `viewer`. `access` is what the caller may
/// do here; a caller who manages the team also gets the accounts that could
/// be added (`accounts`).
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(team): Path<String>,
) -> Response {
    let org = state.kernel.memory.org();
    let (team_id, access) = match team_for(org, &caller, &team, TeamAccess::Read) {
        Ok(t) => t,
        Err(denied) => return denied.into_response(),
    };
    let team = match org.get_team(&team_id) {
        Ok(Some(t)) => t,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such team").into_response(),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let mut members: Vec<serde_json::Value> = team["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let aid = m["agent_id"].as_str()?;
            let name = aid
                .parse::<freeco_types::agent::AgentId>()
                .ok()
                .and_then(|a| state.kernel.registry.get(a))
                .map(|e| e.name)
                .unwrap_or_else(|| "(removed agent)".to_string());
            Some(serde_json::json!({
                "kind": "agent",
                "id": aid,
                "agent_id": aid,
                "name": name,
                "role": m["role"],
            }))
        })
        .collect();
    match org.team_people(&team_id) {
        Ok(p) => members.extend(people_json(&p)),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
    let mut body = serde_json::json!({
        "team_id": team_id,
        "members": members,
        "access": access,
        "roles": TeamRole::ALL,
    });
    if access == TeamAccess::Manage {
        body["accounts"] = serde_json::json!(known_accounts(&state));
    }
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AddMember {
    /// An agent to put on the team.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Or a person: a `[[users]]` account name.
    #[serde(default)]
    pub user: Option<String>,
    /// For an agent, free text (`lead` leads team runs). For a person,
    /// `lead`, `member` (the default) or `viewer`.
    #[serde(default)]
    pub role: Option<String>,
}

/// POST /api/org/teams/{id}/members — put an agent or a person on a team.
///
/// The team's lead or an admin may. A person must be an existing account;
/// adding one already on the team changes their role. An agent goes through
/// the same path as before (`org_routes::add_team_member`).
pub async fn add_member(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(team): Path<String>,
    Json(body): Json<AddMember>,
) -> Response {
    let org = state.kernel.memory.org();
    let team_id = match team_for(org, &caller, &team, TeamAccess::Manage) {
        Ok((id, _)) => id,
        Err(denied) => return denied.into_response(),
    };
    let user = body.user.as_deref().map(str::trim).filter(|u| !u.is_empty());
    let agent = body
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    match (agent, user) {
        (Some(_), Some(_)) | (None, None) => fail(
            StatusCode::BAD_REQUEST,
            "Name either an agent (agent_id) or a person (user).",
        )
        .into_response(),
        (Some(agent), None) => crate::org_routes::add_team_member(
            State(state.clone()),
            Path(team_id),
            Json(crate::org_routes::AddTeamMember {
                agent_id: agent.to_string(),
                role: body.role,
            }),
        )
        .await
        .into_response(),
        (None, Some(user)) => {
            let role = match body.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                None => TeamRole::Member,
                Some(r) => match TeamRole::parse(r) {
                    Some(role) => role,
                    None => {
                        return fail(
                            StatusCode::BAD_REQUEST,
                            "A person's team role is lead, member or viewer.",
                        )
                        .into_response()
                    }
                },
            };
            if !known_accounts(&state).iter().any(|n| n == user) {
                return fail(
                    StatusCode::NOT_FOUND,
                    format!("No account called \"{user}\": add it under Users first."),
                )
                .into_response();
            }
            if let Err(e) = org.add_team_person(&team_id, user, role) {
                return fail(StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
            }
            list_members(State(state.clone()), caller, Path(team_id)).await
        }
    }
}

/// DELETE /api/org/teams/{id}/members/{member} — take an agent or a person
/// off a team. `{member}` is an agent id, or `user:<name>` for a person.
///
/// Adding one is [`add_member`]; this is its other half, so a member put on
/// the wrong team can be taken off again. The team's lead or an admin may.
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path((team, member)): Path<(String, String)>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let team_id = match team_for(org, &caller, &team, TeamAccess::Manage) {
        Ok((id, _)) => id,
        Err(denied) => return denied,
    };
    let member = member.trim();
    let removed = match assignee_person(member) {
        Some(user) => org.remove_team_person(&team_id, user),
        None => org.remove_team_member(&team_id, member),
    };
    match removed {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "removed": true }))),
        Ok(false) => fail(StatusCode::NOT_FOUND, "That member is not on this team"),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Check an assignee for a card on `team_id`: an agent on the team (by id or
/// name), a person on the team (`user:<name>`), or empty for anyone. Returns
/// the value to store.
fn team_assignee(
    state: &AppState,
    team_id: &str,
    assigned: &str,
) -> Result<String, team_access::Denied> {
    let assigned = assigned.trim();
    if assigned.is_empty() {
        return Ok(String::new());
    }
    let org = state.kernel.memory.org();
    if let Some(user) = assignee_person(assigned) {
        return match org.team_role_of(team_id, user) {
            Ok(Some(_)) => Ok(person_assignee(user)),
            Ok(None) => Err(fail(
                StatusCode::BAD_REQUEST,
                format!("{user} is not on this team: add them to the team first"),
            )),
            Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e)),
        };
    }
    let agent_id = match assigned.parse::<freeco_types::agent::AgentId>() {
        Ok(id) => Some(id.to_string()),
        Err(_) => state
            .kernel
            .registry
            .find_by_name(assigned)
            .map(|e| e.id.to_string()),
    };
    let on_team = match (&agent_id, org.get_team(team_id)) {
        (Some(a), Ok(Some(team))) => team["members"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["agent_id"].as_str() == Some(a.as_str())),
        (_, Err(e)) => return Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e)),
        _ => false,
    };
    match agent_id {
        Some(a) if on_team => Ok(a),
        _ => Err(fail(
            StatusCode::BAD_REQUEST,
            "That agent is not on this team: add it to the team first",
        )),
    }
}

#[derive(Deserialize)]
pub struct PostTask {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// An agent (id or name) or a person (`user:<name>`) on the team.
    pub assigned_to: Option<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// GET /api/org/teams/{id}/tasks — one team's board.
///
/// Every card carries its `column` (todo, doing, review, done). Anyone who
/// may see the team may read it.
pub async fn team_tasks(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(team): Path<String>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let (team_id, access) = match team_for(org, &caller, &team, TeamAccess::Read) {
        Ok(t) => t,
        Err(denied) => return denied,
    };
    match state.kernel.memory.task_board(None, Some(&team_id)).await {
        Ok(tasks) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "tasks": tasks,
                "columns": freeco_memory::board::BOARD_COLUMNS
                    .iter()
                    .map(|(c, _)| *c)
                    .collect::<Vec<_>>(),
                "access": access,
            })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/org/teams/{id}/tasks — a person puts a card on a team's board.
///
/// A member or lead of the team (or an admin) may. For an agent assignee it
/// is the same path an agent's `task_post` takes, so the checks are the
/// same: the team must exist, an assignee must be on it, dependencies must
/// exist. A person on the team can be the assignee too (`user:<name>`).
pub async fn post_team_task(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(team): Path<String>,
    Json(body): Json<PostTask>,
) -> impl IntoResponse {
    use freeco_kernel_runtime::kernel_handle::KernelHandle;
    let org = state.kernel.memory.org();
    let team_id = match team_for(org, &caller, &team, TeamAccess::Work) {
        Ok((id, _)) => id,
        Err(denied) => return denied,
    };
    let title = body.title.trim();
    if title.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "A task needs a title.");
    }
    let assigned = body
        .assigned_to
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let for_person = assigned.and_then(assignee_person).is_some();
    let created_by = poster(&caller);
    let posted = match for_person {
        // A person: checked here, then straight onto the board.
        true => {
            let assignee = match team_assignee(&state, &team_id, assigned.unwrap_or_default()) {
                Ok(a) => a,
                Err(denied) => return denied,
            };
            state
                .kernel
                .memory
                .task_post_on_board(
                    title,
                    body.description.trim(),
                    Some(&assignee),
                    Some(&created_by),
                    Some(&team_id),
                    &body.blocked_by,
                )
                .await
                .map_err(|e| e.to_string())
        }
        false => {
            state
                .kernel
                .team_task_post(
                    title,
                    body.description.trim(),
                    assigned,
                    Some(&created_by),
                    Some(&team_id),
                    &body.blocked_by,
                )
                .await
        }
    };
    match posted {
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))),
        Err(e) => fail(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    /// An agent (id or name), a person (`user:<name>`), or empty for anyone.
    pub assigned_to: Option<String>,
    /// The column to move the card to: todo, doing, review or done.
    #[serde(alias = "column")]
    pub status: Option<String>,
}

/// The card `task_id` if it is on `team_id`'s board; 404 otherwise, so a
/// card on another team's board cannot be reached through this one.
async fn card_on_team(
    state: &AppState,
    team_id: &str,
    task_id: &str,
) -> Result<serde_json::Value, team_access::Denied> {
    match state.kernel.memory.task_get(task_id).await {
        Ok(Some(task)) if task["team_id"].as_str() == Some(team_id) => Ok(task),
        Ok(_) => Err(fail(StatusCode::NOT_FOUND, "No such task on this team's board")),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// PUT /api/org/teams/{id}/tasks/{task_id} — move a card to another column,
/// assign it, or edit it. A member or lead of the team (or an admin) may.
pub async fn update_team_task(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path((team, task_id)): Path<(String, String)>,
    Json(body): Json<UpdateTask>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let team_id = match team_for(org, &caller, &team, TeamAccess::Work) {
        Ok((id, _)) => id,
        Err(denied) => return denied,
    };
    if let Err(denied) = card_on_team(&state, &team_id, &task_id).await {
        return denied;
    }
    let assigned_to = match body.assigned_to.as_deref() {
        None => None,
        Some(a) => match team_assignee(&state, &team_id, a) {
            Ok(v) => Some(v),
            Err(denied) => return denied,
        },
    };
    let change = TaskChange {
        title: body.title,
        description: body.description,
        assigned_to,
        status: body.status,
    };
    match state.kernel.memory.task_update(&task_id, change).await {
        Ok(Some(task)) => (StatusCode::OK, Json(serde_json::json!({ "task": task }))),
        Ok(None) => fail(StatusCode::NOT_FOUND, "No such task"),
        Err(freeco_types::error::FreecoError::InvalidInput(msg)) => {
            fail(StatusCode::BAD_REQUEST, msg)
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/org/teams/{id}/tasks/{task_id} — take a card off the board
/// for good. The team's lead or an admin may.
pub async fn delete_team_task(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path((team, task_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    let team_id = match team_for(org, &caller, &team, TeamAccess::Manage) {
        Ok((id, _)) => id,
        Err(denied) => return denied,
    };
    if let Err(denied) = card_on_team(&state, &team_id, &task_id).await {
        return denied;
    }
    match state.kernel.memory.task_delete(&task_id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": true }))),
        Ok(false) => fail(StatusCode::NOT_FOUND, "No such task"),
        Err(freeco_types::error::FreecoError::InvalidInput(msg)) => {
            fail(StatusCode::CONFLICT, msg)
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateTeam {
    pub name: Option<String>,
    pub description: Option<String>,
    pub archived: Option<bool>,
    /// The company the team belongs to; an empty string takes it out of one.
    pub company_id: Option<String>,
}

/// PUT /api/org/teams/{id} — rename a team, describe it, archive it, or
/// move it into a company. Owner and admin only.
pub async fn update_team(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(team): Path<String>,
    Json(body): Json<UpdateTeam>,
) -> impl IntoResponse {
    if !caller.has(UserRole::Admin) {
        return fail(StatusCode::FORBIDDEN, "Only an admin can change a team.");
    }
    let org = state.kernel.memory.org();
    // By id, archived or not, so an archived team can be restored.
    match org.get_team(team.trim()) {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such team"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    let team_id = team.trim();
    let name = body.name.as_deref().map(str::trim);
    if name.is_some_and(str::is_empty) {
        return fail(StatusCode::BAD_REQUEST, "A team needs a name.");
    }
    if let Some(c) = body.company_id.as_deref().map(str::trim) {
        let company = (!c.is_empty()).then_some(c);
        if let Err(e) = org.set_team_company(team_id, company) {
            return fail(StatusCode::BAD_REQUEST, e);
        }
    }
    if let Err(e) = org.update_team(team_id, name, body.description.as_deref(), body.archived) {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let company = org
        .team_companies()
        .ok()
        .and_then(|m| m.get(team_id).cloned());
    match org.get_team(team_id) {
        Ok(Some(mut t)) => {
            t["company_id"] = serde_json::json!(company);
            (StatusCode::OK, Json(serde_json::json!({ "team": t })))
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "No such team"),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// GET /api/org/companies/{id} — one company, with the teams in it the
/// caller may see.
pub async fn get_company(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let org = state.kernel.memory.org();
    match team_access::companies_visible_to(org, &caller) {
        Ok(Some(visible)) if !visible.contains(&id) => {
            return fail(StatusCode::NOT_FOUND, "No such company")
        }
        Ok(_) => {}
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    let company = match org.get_company(&id) {
        Ok(Some(c)) => c,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "No such company"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let visible: HashSet<String> = match team_access::teams_visible_to(org, &caller) {
        Ok(v) => v.into_iter().collect(),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let teams: Vec<String> = match org.team_companies() {
        Ok(map) => {
            let mut t: Vec<String> = map
                .into_iter()
                .filter(|(team, company)| *company == id && visible.contains(team))
                .map(|(team, _)| team)
                .collect();
            t.sort();
            t
        }
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "company": company, "team_ids": teams })),
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateCompany {
    pub name: Option<String>,
    pub description: Option<String>,
    pub archived: Option<bool>,
}

/// PUT /api/org/companies/{id} — rename, describe, archive or restore a
/// company. Owner and admin only.
pub async fn update_company(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<UpdateCompany>,
) -> impl IntoResponse {
    if !caller.has(UserRole::Admin) {
        return fail(StatusCode::FORBIDDEN, "Only an admin can change a company.");
    }
    let name = body.name.as_deref().map(str::trim);
    if name.is_some_and(str::is_empty) {
        return fail(StatusCode::BAD_REQUEST, "A company needs a name.");
    }
    match state.kernel.memory.org().update_company(
        &id,
        name,
        body.description.as_deref(),
        body.archived,
    ) {
        Ok(Some(c)) => (StatusCode::OK, Json(serde_json::json!({ "company": c }))),
        Ok(None) => fail(StatusCode::NOT_FOUND, "No such company"),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/org/companies/{id} — delete a company. Its projects and teams
/// stay, in no company. Owner and admin only.
pub async fn delete_company(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !caller.has(UserRole::Admin) {
        return fail(StatusCode::FORBIDDEN, "Only an admin can delete a company.");
    }
    match state.kernel.memory.org().delete_company(&id) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": true }))),
        Ok(false) => fail(StatusCode::NOT_FOUND, "No such company"),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

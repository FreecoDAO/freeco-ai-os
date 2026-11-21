//! What the dashboard's sidebar and Live view read.
//!
//! - `GET /api/activity` — what every agent is doing this second.
//! - `GET /api/org/chats` — the newest conversations of every agent, for the
//!   chat list in the sidebar.
//! - `GET /api/tasks` — the shared task board. It was reachable only through
//!   agents' own tools and MCP, so a task made with "New task" could be
//!   created from the dashboard but never seen there again.

use crate::middleware::Caller;
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
pub async fn activity(State(state): State<Arc<AppState>>, caller: Caller) -> impl IntoResponse {
    // Agents the caller may not see are dropped from the feed (owner/admin
    // see everyone), matching the Chamber's rule.
    let access = crate::access::Access::resolve(&state.kernel, &caller);
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
    // Hidden agents are dropped, not blanked (Chamber's rule).
    let visible = access.is_admin();
    let agents: Vec<serde_json::Value> = agents
        .into_iter()
        .filter(|row| {
            visible
                || access.can_see_agent(
                    row["agent_id"].as_str().unwrap_or_default(),
                    row["agent_name"].as_str().unwrap_or_default(),
                )
        })
        .collect();
    Json(serde_json::json!({ "now_ms": now_ms, "agents": agents }))
}

/// How long a failed tool keeps an idle agent's desk showing the error.
const STARNET_ERROR_WINDOW_MS: i64 = 60_000;

/// Name of the room for agents on no team.
const STARNET_NO_TEAM: &str = "No team";

/// One agent the Starnet view draws: id, name, and whether the registry
/// marks it crashed.
pub(crate) struct StarnetAgent {
    pub id: String,
    pub name: String,
    pub crashed: bool,
}

/// The newest failed step, when the agent has gone idle right after it.
fn recent_failure(seen: Option<&AgentActivity>, now_ms: i64) -> Option<String> {
    let a = seen?;
    if a.state != ActivityState::Idle {
        return None;
    }
    let last = a.steps.last()?;
    let ended = last.ended_ms?;
    if last.ok == Some(false) && now_ms - ended <= STARNET_ERROR_WINDOW_MS {
        Some(last.name.clone().unwrap_or_else(|| "a tool".to_string()))
    } else {
        None
    }
}

/// Build the Starnet station: rooms are teams, desks are agents.
///
/// Every agent sits at exactly one desk: in the first team (by name) that
/// lists it, or in the "No team" room. Rooms and desks are sorted by name so
/// the picture does not reshuffle between polls. `tasks` are task-board rows;
/// the newest `in_progress` task assigned to an agent is its speech bubble.
pub(crate) fn starnet_snapshot(
    agents: &[StarnetAgent],
    seen: &HashMap<String, AgentActivity>,
    waiting: &HashMap<String, usize>,
    teams: &[serde_json::Value],
    tasks: &[serde_json::Value],
    now_ms: i64,
) -> serde_json::Value {
    let known: HashMap<&str, &StarnetAgent> = agents.iter().map(|a| (a.id.as_str(), a)).collect();

    // Rooms, by name; members that are no longer registered are left out.
    let mut sorted_teams: Vec<&serde_json::Value> = teams.iter().collect();
    sorted_teams.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    let mut room_of: HashMap<String, String> = HashMap::new();
    let mut rooms: Vec<serde_json::Value> = Vec::new();
    for team in sorted_teams {
        let Some(team_id) = team["id"].as_str() else {
            continue;
        };
        let mut members: Vec<&StarnetAgent> = Vec::new();
        let listed = team["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["agent_id"].as_str());
        for aid in listed {
            if let Some(&a) = known.get(aid) {
                if !room_of.contains_key(&a.id) && !members.iter().any(|m| m.id == a.id) {
                    members.push(a);
                }
            }
        }
        members.sort_by(|a, b| a.name.cmp(&b.name));
        for m in &members {
            room_of.insert(m.id.clone(), team_id.to_string());
        }
        rooms.push(serde_json::json!({
            "id": team_id,
            "name": team["name"],
            "project": team["project"],
            "agents": members.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        }));
    }
    let mut loose: Vec<&StarnetAgent> = agents
        .iter()
        .filter(|a| !room_of.contains_key(&a.id))
        .collect();
    if !loose.is_empty() {
        loose.sort_by(|a, b| a.name.cmp(&b.name));
        rooms.push(serde_json::json!({
            "id": serde_json::Value::Null,
            "name": STARNET_NO_TEAM,
            "project": serde_json::Value::Null,
            "agents": loose.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        }));
    }

    // Speech bubbles: tasks arrive newest first, so the first match wins.
    let mut task_of: HashMap<&str, &str> = HashMap::new();
    for t in tasks {
        if t["status"].as_str() != Some("in_progress") {
            continue;
        }
        if let (Some(aid), Some(title)) = (t["assigned_to"].as_str(), t["title"].as_str()) {
            if !title.is_empty() {
                task_of.entry(aid).or_insert(title);
            }
        }
    }

    let mut out_agents: Vec<serde_json::Value> = agents
        .iter()
        .map(|a| {
            let activity = seen.get(&a.id);
            let mut row = activity_row(
                &a.id,
                &a.name,
                activity,
                waiting.get(&a.id).copied().unwrap_or(0),
            );
            // The view draws states, not step histories.
            if let Some(obj) = row.as_object_mut() {
                obj.remove("steps");
            }
            if row["state"] != "waiting_for_you" {
                if a.crashed {
                    row["state"] = serde_json::json!("error");
                    row["error"] = serde_json::json!("the agent crashed");
                } else if let Some(tool) = recent_failure(activity, now_ms) {
                    row["state"] = serde_json::json!("error");
                    row["error"] = serde_json::json!(format!("{tool} failed"));
                    row["tool"] = serde_json::json!(tool);
                }
            }
            row["task"] = serde_json::json!(task_of.get(a.id.as_str()));
            row["room"] = serde_json::json!(room_of.get(&a.id));
            row
        })
        .collect();
    out_agents.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    serde_json::json!({ "now_ms": now_ms, "rooms": rooms, "agents": out_agents })
}

/// GET /api/starnet/state — the Starnet station in one read-only poll:
/// rooms (teams), desks (agents), each agent's live state and current task.
///
/// Teams and tasks fail soft: the desks still show, and `degraded` says
/// which part is missing, so a broken task table does not blank the view.
pub async fn starnet_state(
    State(state): State<Arc<AppState>>,
    caller: Caller,
) -> impl IntoResponse {
    // The pixel-art stage shows every agent: restrict it to those who may
    // see them all, exactly like the activity page (kid/user with grants
    // see the scoped activity feed instead).
    let access = crate::access::Access::resolve(&state.kernel, &caller);
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
    let agents: Vec<StarnetAgent> = state
        .kernel
        .registry
        .list()
        .into_iter()
        .map(|e| StarnetAgent {
            id: e.id.to_string(),
            crashed: e.state == freeco_types::agent::AgentState::Crashed,
            name: e.name,
        })
        .collect();
    let mut degraded: Vec<&str> = Vec::new();
    let teams = state
        .kernel
        .memory
        .org()
        .list_teams(false)
        .unwrap_or_else(|e| {
            tracing::warn!("starnet: teams unavailable: {e}");
            degraded.push("teams");
            Vec::new()
        });
    let tasks = match state.kernel.memory.task_list(Some("in_progress")).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("starnet: tasks unavailable: {e}");
            degraded.push("tasks");
            Vec::new()
        }
    };
    // Owner/admin see every desk; anyone else only their own agents'
    // desks. Teams whose every member is hidden are dropped too.
    let visible = access.is_admin() || {
        let any = agents.is_empty();
        any
    };
    let total = agents.len();
    let filtered_agents: Vec<StarnetAgent> = agents
        .into_iter()
        .filter(|a| visible || access.can_see_agent(&a.id, &a.name))
        .collect();
    let hidden = total - filtered_agents.len();
    let mut body = starnet_snapshot(&filtered_agents, &seen, &waiting, &teams, &tasks, now_ms);
    body["hidden_agents"] = serde_json::json!(hidden);
    body["degraded"] = serde_json::json!(degraded);
    Json(body)
}

#[derive(serde::Deserialize, Default)]
pub struct ChatsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/org/chats?limit=60 — newest conversations of every agent.
pub async fn recent_chats(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<ChatsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(60).clamp(1, 500);
    // Owner/admin see every conversation; anyone else only the agents their
    // grants and teams allow (same rule as the Chamber: hidden = gone, to
    // avoid leaking existence).
    let access = crate::access::Access::resolve(&state.kernel, &caller);
    let mut hidden = 0usize;
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
            // Filter after enrichment: keep only rows whose agent the caller
            // may see (owner/admin see all).
            rows.retain(|row| {
                let id = row["agent_id"].as_str().unwrap_or_default();
                let name = row["agent_name"].as_str().unwrap_or_default();
                let allowed = access.can_see_agent(id, name);
                if !allowed {
                    hidden += 1;
                }
                allowed
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({ "chats": rows, "hidden": hidden })),
            )
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
    caller: Caller,
    Query(q): Query<TasksQuery>,
) -> impl IntoResponse {
    // Same access rule as recent_chats: tasks of agents the caller cannot
    // see are dropped, not shown.
    let access = crate::access::Access::resolve(&state.kernel, &caller);
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
            // Tasks of hidden agents are dropped, not shown.
            tasks.retain(|t| {
                let id = t["assigned_to"].as_str().unwrap_or_default();
                let name = t["assigned_name"].as_str().unwrap_or("");
                access.can_see_agent(id, name)
            });
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

    // ---- Starnet ---------------------------------------------------------

    fn agent(id: &str, name: &str) -> StarnetAgent {
        StarnetAgent {
            id: id.into(),
            name: name.into(),
            crashed: false,
        }
    }

    fn team(id: &str, name: &str, members: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "project": null,
            "members": members.iter().map(|m| serde_json::json!({ "agent_id": m, "role": null })).collect::<Vec<_>>(),
        })
    }

    fn idle_after(tool: &str, ok: bool, ended_ms: i64) -> AgentActivity {
        AgentActivity {
            agent_id: "a1".into(),
            agent_name: "Shopping".into(),
            state: ActivityState::Idle,
            tool: None,
            since_ms: ended_ms,
            turns: 0,
            steps: vec![Step {
                kind: "tool",
                name: Some(tool.into()),
                started_ms: ended_ms - 10,
                ended_ms: Some(ended_ms),
                ok: Some(ok),
            }],
        }
    }

    fn snap(
        agents: &[StarnetAgent],
        seen: &[AgentActivity],
        waiting: &[(&str, usize)],
        teams: &[serde_json::Value],
        tasks: &[serde_json::Value],
        now_ms: i64,
    ) -> serde_json::Value {
        let seen: HashMap<String, AgentActivity> = seen
            .iter()
            .map(|a| (a.agent_id.clone(), a.clone()))
            .collect();
        let waiting: HashMap<String, usize> =
            waiting.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        starnet_snapshot(agents, &seen, &waiting, teams, tasks, now_ms)
    }

    fn row<'a>(snap: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        snap["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent_id"] == id)
            .unwrap()
    }

    #[test]
    fn starnet_rooms_are_teams_sorted_by_name_with_no_team_last() {
        let agents = [
            agent("a1", "Shopping"),
            agent("a2", "Coder"),
            agent("a3", "Loner"),
        ];
        let teams = [team("t2", "Zeta", &["a1"]), team("t1", "Alpha", &["a2"])];
        let s = snap(&agents, &[], &[], &teams, &[], 0);
        let rooms = s["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 3);
        assert_eq!(rooms[0]["name"], "Alpha");
        assert_eq!(rooms[0]["agents"], serde_json::json!(["a2"]));
        assert_eq!(rooms[1]["name"], "Zeta");
        assert_eq!(rooms[2]["name"], "No team");
        assert!(rooms[2]["id"].is_null());
        assert_eq!(rooms[2]["agents"], serde_json::json!(["a3"]));
        assert_eq!(row(&s, "a1")["room"], "t2");
        assert!(row(&s, "a3")["room"].is_null());
        // Steps stay on /api/activity; the station only needs states.
        assert!(row(&s, "a1").get("steps").is_none());
    }

    /// One agent, one desk: on two teams it sits in the first by name only.
    #[test]
    fn starnet_an_agent_on_two_teams_gets_one_desk() {
        let agents = [agent("a1", "Shopping")];
        let teams = [
            team("t2", "Beta", &["a1"]),
            team("t1", "Alpha", &["a1", "a1"]),
        ];
        let s = snap(&agents, &[], &[], &teams, &[], 0);
        let rooms = s["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 2, "no 'No team' room when everyone is seated");
        assert_eq!(rooms[0]["agents"], serde_json::json!(["a1"]));
        assert_eq!(rooms[1]["agents"], serde_json::json!([]));
        assert_eq!(row(&s, "a1")["room"], "t1");
    }

    #[test]
    fn starnet_skips_members_that_are_no_longer_registered() {
        let agents = [agent("a1", "Shopping")];
        let teams = [team("t1", "Alpha", &["gone", "a1"])];
        let s = snap(&agents, &[], &[], &teams, &[], 0);
        assert_eq!(s["rooms"][0]["agents"], serde_json::json!(["a1"]));
        assert_eq!(s["agents"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn starnet_bubble_is_the_newest_in_progress_task() {
        let agents = [agent("a1", "Shopping"), agent("a2", "Coder")];
        // The task board lists newest first.
        let tasks = [
            serde_json::json!({ "title": "Compare prices", "status": "in_progress", "assigned_to": "a1" }),
            serde_json::json!({ "title": "Older task", "status": "in_progress", "assigned_to": "a1" }),
            serde_json::json!({ "title": "Done already", "status": "completed", "assigned_to": "a2" }),
        ];
        let s = snap(&agents, &[], &[], &[], &tasks, 0);
        assert_eq!(row(&s, "a1")["task"], "Compare prices");
        assert!(row(&s, "a2")["task"].is_null());
    }

    #[test]
    fn starnet_a_crashed_agent_shows_an_error() {
        let mut a = agent("a1", "Shopping");
        a.crashed = true;
        let s = snap(&[a], &[], &[], &[], &[], 0);
        assert_eq!(row(&s, "a1")["state"], "error");
        assert_eq!(row(&s, "a1")["error"], "the agent crashed");
    }

    #[test]
    fn starnet_a_tool_that_just_failed_shows_an_error_then_fades() {
        let agents = [agent("a1", "Shopping")];
        let failed = idle_after("web_fetch", false, 1_000);
        let s = snap(
            &agents,
            std::slice::from_ref(&failed),
            &[],
            &[],
            &[],
            1_000 + 5_000,
        );
        assert_eq!(row(&s, "a1")["state"], "error");
        assert_eq!(row(&s, "a1")["tool"], "web_fetch");
        assert_eq!(row(&s, "a1")["error"], "web_fetch failed");

        let later = 1_000 + STARNET_ERROR_WINDOW_MS + 1;
        let s = snap(&agents, &[failed], &[], &[], &[], later);
        assert_eq!(row(&s, "a1")["state"], "idle");

        let ok = idle_after("web_fetch", true, 1_000);
        let s = snap(&agents, &[ok], &[], &[], &[], 2_000);
        assert_eq!(row(&s, "a1")["state"], "idle");
    }

    /// The owner's yes is still the one thing the owner can act on.
    #[test]
    fn starnet_waiting_for_you_outranks_an_error() {
        let mut a = agent("a1", "Shopping");
        a.crashed = true;
        let s = snap(&[a], &[], &[("a1", 1)], &[], &[], 0);
        assert_eq!(row(&s, "a1")["state"], "waiting_for_you");
    }

    #[test]
    fn waiting_on_a_helper_keeps_its_own_state() {
        let a = seen(ActivityState::WaitingOnHelper, Some("agent_delegate"));
        let row = activity_row("a1", "Concierge", Some(&a), 0);
        assert_eq!(row["state"], "waiting_on_helper");
    }
}

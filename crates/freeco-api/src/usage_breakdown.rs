//! Where the money and tokens went: by agent, model, team, project and company.
//!
//! The Budget page showed nothing useful. Its agent list came from an
//! endpoint that only included agents with a spending limit set, and the
//! per-agent token counts came from an in-memory counter that resets on every
//! restart -- while every call sat in `usage_events` with its agent, model,
//! tokens and cost. This reads that table.
//!
//! Teams, projects and companies have no usage of their own: an agent belongs
//! to teams (`team_members`), a team to a project, a project to a company. A
//! team's figure is the sum of its members'; a project's and a company's sum
//! each agent once, however many of their teams it is in.
//!
//! Some calls were recorded at $0 because the model's price was missing from
//! the catalog. Where the catalog now knows a price, those rows are estimated
//! at today's rate and marked `estimated`; the stored history is not changed.

use crate::routes::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub struct BreakdownQuery {
    /// today | 7d | 30d | all (default 30d)
    pub period: Option<String>,
}

/// agent, model, calls, input tokens, output tokens, stored cost, tool calls
type UsageRow = (String, String, u64, u64, u64, f64, u64);

#[derive(Default, Clone, serde::Serialize)]
struct Totals {
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    tool_calls: u64,
    cost_usd: f64,
    /// Part of `cost_usd` estimated from the current price for calls that were
    /// recorded at $0 before the price was known.
    estimated_usd: f64,
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.calls += o.calls;
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.tool_calls += o.tool_calls;
        self.cost_usd += o.cost_usd;
        self.estimated_usd += o.estimated_usd;
    }
}

/// The earliest timestamp in a period, as stored (RFC 3339, comparable as text).
fn since(period: &str) -> Option<String> {
    let now = chrono::Utc::now();
    let start = match period {
        "today" => now.date_naive().and_hms_opt(0, 0, 0)?.and_utc(),
        "7d" => now - chrono::Duration::days(7),
        "30d" => now - chrono::Duration::days(30),
        _ => return None,
    };
    Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// GET /api/usage/breakdown?period=today|7d|30d|all
pub async fn breakdown(
    State(state): State<Arc<AppState>>,
    Query(q): Query<BreakdownQuery>,
) -> impl IntoResponse {
    let period = q.period.unwrap_or_else(|| "30d".to_string());
    if !["today", "7d", "30d", "all"].contains(&period.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "period must be today, 7d, 30d or all"})),
        );
    }
    let from = since(&period);

    // Prices for estimating calls recorded at $0, read once.
    let prices: HashMap<String, (f64, f64)> = {
        let catalog = state
            .kernel
            .model_catalog
            .read()
            .unwrap_or_else(|e| e.into_inner());
        catalog
            .list_models()
            .iter()
            .map(|m| {
                (
                    m.id.to_lowercase(),
                    (m.input_cost_per_m, m.output_cost_per_m),
                )
            })
            .collect()
    };
    let price_of = |model: &str| -> Option<(f64, f64)> {
        let m = model.to_lowercase();
        prices
            .get(&m)
            .or_else(|| m.split_once('/').and_then(|(_, rest)| prices.get(rest)))
            .copied()
            .filter(|(i, o)| *i > 0.0 || *o > 0.0)
    };

    let conn = state.kernel.memory.connection();
    let rows = {
        let db = match conn.lock() {
            Ok(db) => db,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            }
        };
        let read = || -> rusqlite::Result<Vec<UsageRow>> {
            let sql = "SELECT agent_id, model, COUNT(*), SUM(input_tokens), SUM(output_tokens), \
                       SUM(cost_usd), SUM(tool_calls) FROM usage_events \
                       WHERE (?1 IS NULL OR timestamp >= ?1) GROUP BY agent_id, model";
            let mut stmt = db.prepare(sql)?;
            let it = stmt.query_map(rusqlite::params![from], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0) as u64,
                    r.get::<_, Option<i64>>(4)?.unwrap_or(0) as u64,
                    r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    r.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64,
                ))
            })?;
            it.collect()
        };
        match read() {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("usage query failed: {e}")})),
                )
            }
        }
    };

    // Per (agent, model), with the $0 rows estimated where a price is known.
    let mut by_agent: HashMap<String, Totals> = HashMap::new();
    let mut by_model: HashMap<String, Totals> = HashMap::new();
    let mut total = Totals::default();
    for (agent, model, calls, input, output, cost, tools) in rows {
        let mut t = Totals {
            calls,
            input_tokens: input,
            output_tokens: output,
            tool_calls: tools,
            cost_usd: cost,
            estimated_usd: 0.0,
        };
        if cost == 0.0 {
            if let Some((pi, po)) = price_of(&model) {
                let est = input as f64 / 1e6 * pi + output as f64 / 1e6 * po;
                t.cost_usd = est;
                t.estimated_usd = est;
            }
        }
        by_agent.entry(agent).or_default().add(&t);
        by_model.entry(model).or_default().add(&t);
        total.add(&t);
    }

    // Organisation: team -> agents, team -> project, project -> company.
    let org = read_org(&conn);
    let name_of = |id: &str| -> String {
        id.parse()
            .ok()
            .and_then(|aid| state.kernel.registry.get(aid))
            .map(|e| e.name)
            .unwrap_or_else(|| format!("removed agent {}", id.get(..8).unwrap_or(id)))
    };

    let mut agents: Vec<serde_json::Value> = by_agent
        .iter()
        .map(|(id, t)| {
            let teams: Vec<&str> = org
                .members
                .iter()
                .filter(|(_, a)| a == id)
                .filter_map(|(team, _)| org.teams.get(team).map(|t| t.0.as_str()))
                .collect();
            serde_json::json!({ "agent_id": id, "name": name_of(id), "teams": teams, "totals": t })
        })
        .collect();
    agents.sort_by(|a, b| cost_of(b).total_cmp(&cost_of(a)));

    let mut models: Vec<serde_json::Value> = by_model
        .iter()
        .map(|(m, t)| serde_json::json!({ "model": m, "totals": t }))
        .collect();
    models.sort_by(|a, b| cost_of(b).total_cmp(&cost_of(a)));

    // Teams: their members' totals.
    let mut team_agents: HashMap<String, HashSet<String>> = HashMap::new();
    for (team, agent) in &org.members {
        team_agents
            .entry(team.clone())
            .or_default()
            .insert(agent.clone());
    }
    let sum = |set: &HashSet<String>| -> Totals {
        let mut t = Totals::default();
        for a in set {
            if let Some(x) = by_agent.get(a) {
                t.add(x);
            }
        }
        t
    };
    let mut teams: Vec<serde_json::Value> = org
        .teams
        .iter()
        .map(|(id, (name, project))| {
            let members = team_agents.get(id).cloned().unwrap_or_default();
            serde_json::json!({
                "team_id": id, "name": name,
                "project": project.as_ref().and_then(|p| org.projects.get(p)).map(|p| p.0.clone()),
                "agents": members.len(), "totals": sum(&members),
            })
        })
        .collect();
    teams.sort_by(|a, b| cost_of(b).total_cmp(&cost_of(a)));

    // Projects and companies: each agent counted once.
    let mut project_agents: HashMap<String, HashSet<String>> = HashMap::new();
    for (team, (_, project)) in &org.teams {
        if let (Some(p), Some(members)) = (project, team_agents.get(team)) {
            project_agents
                .entry(p.clone())
                .or_default()
                .extend(members.iter().cloned());
        }
    }
    let mut projects: Vec<serde_json::Value> = org
        .projects
        .iter()
        .map(|(id, (name, company))| {
            let members = project_agents.get(id).cloned().unwrap_or_default();
            serde_json::json!({
                "project_id": id, "name": name,
                "company": company.as_ref().and_then(|c| org.companies.get(c)).cloned(),
                "agents": members.len(), "totals": sum(&members),
            })
        })
        .collect();
    projects.sort_by(|a, b| cost_of(b).total_cmp(&cost_of(a)));

    let mut company_agents: HashMap<String, HashSet<String>> = HashMap::new();
    for (project, (_, company)) in &org.projects {
        if let (Some(c), Some(members)) = (company, project_agents.get(project)) {
            company_agents
                .entry(c.clone())
                .or_default()
                .extend(members.iter().cloned());
        }
    }
    let mut companies: Vec<serde_json::Value> = org
        .companies
        .iter()
        .map(|(id, name)| {
            let members = company_agents.get(id).cloned().unwrap_or_default();
            serde_json::json!({ "company_id": id, "name": name, "agents": members.len(), "totals": sum(&members) })
        })
        .collect();
    companies.sort_by(|a, b| cost_of(b).total_cmp(&cost_of(a)));

    // Spend no team accounts for, so the parts visibly add up to the whole.
    let in_a_team: HashSet<&String> = org.members.iter().map(|(_, a)| a).collect();
    let mut unassigned = Totals::default();
    for (a, t) in &by_agent {
        if !in_a_team.contains(a) {
            unassigned.add(t);
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "period": period,
            "since": from,
            "totals": total,
            "by_agent": agents,
            "by_model": models,
            "by_team": teams,
            "by_project": projects,
            "by_company": companies,
            "not_in_any_team": unassigned,
        })),
    )
}

fn cost_of(v: &serde_json::Value) -> f64 {
    v["totals"]["cost_usd"].as_f64().unwrap_or(0.0)
}

#[derive(Default)]
struct Org {
    /// (team_id, agent_id)
    members: Vec<(String, String)>,
    /// team_id -> (name, project_id)
    teams: HashMap<String, (String, Option<String>)>,
    /// project_id -> (name, company_id)
    projects: HashMap<String, (String, Option<String>)>,
    /// company_id -> name
    companies: HashMap<String, String>,
}

/// The organisation tables, archived rows left out. Missing tables (an
/// install that never created an organisation) read as empty.
fn read_org(conn: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>) -> Org {
    let mut org = Org::default();
    let Ok(db) = conn.lock() else { return org };
    if let Ok(mut s) = db.prepare("SELECT team_id, agent_id FROM team_members") {
        if let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            org.members = rows.flatten().collect();
        }
    }
    if let Ok(mut s) =
        db.prepare("SELECT id, name, project_id FROM teams WHERE NOT COALESCE(archived, 0)")
    {
        if let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        }) {
            org.teams = rows.flatten().map(|(i, n, p)| (i, (n, p))).collect();
        }
    }
    if let Ok(mut s) =
        db.prepare("SELECT id, name, company_id FROM projects WHERE NOT COALESCE(archived, 0)")
    {
        if let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        }) {
            org.projects = rows.flatten().map(|(i, n, c)| (i, (n, c))).collect();
        }
    }
    if let Ok(mut s) = db.prepare("SELECT id, name FROM companies WHERE NOT COALESCE(archived, 0)")
    {
        if let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            org.companies = rows.flatten().collect();
        }
    }
    // Members of archived teams do not count towards anything.
    org.members.retain(|(team, _)| org.teams.contains_key(team));
    org
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_start_where_they_say() {
        assert!(since("all").is_none());
        let today = since("today").unwrap();
        assert!(today.ends_with("T00:00:00Z"), "{today}");
        let week = since("7d").unwrap();
        assert!(week < chrono::Utc::now().to_rfc3339());
    }
}

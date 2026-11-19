//! Companies, team members and team boards over HTTP, with real sessions.
//!
//! Boots a kernel with dashboard login on and a handful of `[[users]]`, puts
//! the team routes behind the real auth middleware, and walks the permission
//! matrix: owner/admin manage everything, a team lead manages their team, a
//! member works on its board, a viewer reads, an outsider sees nothing.
//!
//! Run: cargo test -p freeco-api --test teams_api_test

use axum::body::Body;
use axum::http::{Method, Request};
use axum::Router;
use freeco_api::middleware::{self, AuthState};
use freeco_api::routes::AppState;
use freeco_api::{org_routes, teams_routes};
use freeco_kernel::FreecoKernel;
use freeco_types::auth::UserRole;
use freeco_types::config::{AuthConfig, DefaultModelConfig, KernelConfig, UserConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tower::ServiceExt;

struct Harness {
    app: Router,
    auth: AuthState,
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.state.kernel.shutdown();
    }
}

fn account(name: &str, role: &str) -> UserConfig {
    UserConfig {
        name: name.to_string(),
        role: role.to_string(),
        enabled: true,
        ..Default::default()
    }
}

fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let config = KernelConfig {
        home_dir: tmp.path().to_path_buf(),
        data_dir: tmp.path().join("data"),
        default_model: DefaultModelConfig {
            provider: "ollama".to_string(),
            model: "test-model".to_string(),
            api_key_env: "OLLAMA_API_KEY".to_string(),
            base_url: None,
            subprocess_timeout_secs: None,
        },
        auth: AuthConfig {
            enabled: true,
            username: "admin".to_string(),
            // A fixture value, not a real hash or secret.
            password_hash: "$argon2id$test-fixture".to_string(),
            session_secret: "teams-api-test-signing-fixture".to_string(),
            ..Default::default()
        },
        users: vec![
            account("alice", "admin"),
            account("lena", "user"),
            account("mo", "user"),
            account("otto", "user"),
            account("val", "viewer"),
        ],
        ..KernelConfig::default()
    };
    let auth = AuthState::from_config(&config, false);
    let kernel = Arc::new(FreecoKernel::boot_with_config(config).expect("kernel boots"));
    kernel.set_self_handle();
    let state = Arc::new(AppState {
        kernel,
        started_at: Instant::now(),
        peer_registry: None,
        bridge_manager: tokio::sync::Mutex::new(None),
        channels_config: tokio::sync::RwLock::new(Default::default()),
        shutdown_notify: Arc::new(tokio::sync::Notify::new()),
        clawhub_cache: dashmap::DashMap::new(),
        provider_probe_cache: freeco_kernel_runtime::provider_health::ProbeCache::new(),
        budget_config: Arc::new(tokio::sync::RwLock::new(Default::default())),
        local_ai: Arc::new(tokio::sync::RwLock::new(Default::default())),
        services: Arc::new(tokio::sync::RwLock::new(Default::default())),
        frozen: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        frozen_agents: Arc::new(std::sync::Mutex::new(Default::default())),
        security: Arc::new(freeco_api::security::SecurityService::default()),
    });

    use axum::routing::{get, put};
    let app = Router::new()
        .route(
            "/api/org/companies",
            get(org_routes::list_companies).post(org_routes::create_company),
        )
        .route(
            "/api/org/companies/{id}",
            get(teams_routes::get_company)
                .put(teams_routes::update_company)
                .delete(teams_routes::delete_company),
        )
        .route(
            "/api/org/teams",
            get(org_routes::list_teams).post(org_routes::create_team),
        )
        .route("/api/org/teams/{id}", put(teams_routes::update_team))
        .route(
            "/api/org/teams/{id}/members",
            get(teams_routes::list_members).post(teams_routes::add_member),
        )
        .route(
            "/api/org/teams/{id}/members/{agent_id}",
            axum::routing::delete(teams_routes::remove_member),
        )
        .route(
            "/api/org/teams/{id}/tasks",
            get(teams_routes::team_tasks).post(teams_routes::post_team_task),
        )
        .route(
            "/api/org/teams/{id}/tasks/{task_id}",
            put(teams_routes::update_team_task).delete(teams_routes::delete_team_task),
        )
        .route("/api/teams/floor", get(teams_routes::floor))
        .layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            middleware::auth,
        ))
        .with_state(state.clone());
    Harness {
        app,
        auth,
        state,
        _tmp: tmp,
    }
}

impl Harness {
    /// Call as `user` (with the role their session claims) and return the
    /// status and the JSON body.
    async fn call(
        &self,
        user: &str,
        role: UserRole,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let token = freeco_api::session_auth::create_session_token(
            user,
            role,
            &self.auth.session_secret,
            1,
        );
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", format!("freeco_session={token}"));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .app
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }
}

const OWNER: (&str, UserRole) = ("admin", UserRole::Owner);
const ADMIN: (&str, UserRole) = ("alice", UserRole::Admin);
const LEAD: (&str, UserRole) = ("lena", UserRole::User);
const MEMBER: (&str, UserRole) = ("mo", UserRole::User);
const OUTSIDER: (&str, UserRole) = ("otto", UserRole::User);
const VIEWER: (&str, UserRole) = ("val", UserRole::Viewer);

macro_rules! call {
    ($h:expr, $who:expr, $m:ident, $uri:expr) => {
        $h.call($who.0, $who.1, Method::$m, &$uri, None).await
    };
    ($h:expr, $who:expr, $m:ident, $uri:expr, $body:expr) => {
        $h.call($who.0, $who.1, Method::$m, &$uri, Some($body)).await
    };
}

#[tokio::test]
async fn companies_teams_members_and_boards_follow_the_permission_matrix() {
    let h = harness();

    // ── Companies: admins create; others cannot ─────────────────────────
    let (s, acme) = call!(h, ADMIN, POST, "/api/org/companies", json!({ "name": "Acme" }));
    assert_eq!(s, 200, "{acme}");
    let acme_id = acme["id"].as_str().unwrap().to_string();
    let (s, globex) = call!(h, OWNER, POST, "/api/org/companies", json!({ "name": "Globex" }));
    assert_eq!(s, 200);
    let globex_id = globex["id"].as_str().unwrap().to_string();
    let (s, _) = call!(h, LEAD, POST, "/api/org/companies", json!({ "name": "Mine" }));
    assert_eq!(s, 403);

    // ── Teams in companies ──────────────────────────────────────────────
    let (s, web) = call!(
        h,
        ADMIN,
        POST,
        "/api/org/teams",
        json!({ "name": "Web", "company_id": acme_id })
    );
    assert_eq!(s, 200, "{web}");
    assert_eq!(web["company_id"], acme_id.as_str());
    let web_id = web["id"].as_str().unwrap().to_string();
    let (s, ops) = call!(
        h,
        ADMIN,
        POST,
        "/api/org/teams",
        json!({ "name": "Ops", "company_id": globex_id })
    );
    assert_eq!(s, 200);
    let ops_id = ops["id"].as_str().unwrap().to_string();
    let (s, _) = call!(
        h,
        ADMIN,
        POST,
        "/api/org/teams",
        json!({ "name": "Nowhere", "company_id": "no-such-company" })
    );
    assert_eq!(s, 400);

    let members = format!("/api/org/teams/{web_id}/members");

    // ── Members: an admin seats the lead, the lead staffs the team ──────
    let (s, body) = call!(h, ADMIN, POST, members, json!({ "user": "lena", "role": "lead" }));
    assert_eq!(s, 200, "{body}");
    let (s, _) = call!(h, LEAD, POST, members, json!({ "user": "mo" }));
    assert_eq!(s, 200, "member is the default role");
    let (s, _) = call!(h, LEAD, POST, members, json!({ "user": "val", "role": "viewer" }));
    assert_eq!(s, 200);
    let (s, _) = call!(h, LEAD, POST, members, json!({ "user": "ghost" }));
    assert_eq!(s, 404, "only existing accounts");
    let (s, _) = call!(h, LEAD, POST, members, json!({ "user": "otto", "role": "boss" }));
    assert_eq!(s, 400, "a person's role is lead, member or viewer");
    let (s, _) = call!(h, LEAD, POST, members, json!({}));
    assert_eq!(s, 400);
    // A member does not staff the team, and a lead of one team not another.
    let (s, _) = call!(h, MEMBER, POST, members, json!({ "user": "otto" }));
    assert_eq!(s, 403);
    let (s, _) = call!(
        h,
        LEAD,
        POST,
        format!("/api/org/teams/{ops_id}/members"),
        json!({ "user": "otto" })
    );
    assert_eq!(s, 403);

    let (s, list) = call!(h, MEMBER, GET, members);
    assert_eq!(s, 200);
    assert_eq!(list["access"], "work");
    assert!(list.get("accounts").is_none(), "only managers see accounts");
    let people: Vec<&str> = list["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "user")
        .map(|m| m["username"].as_str().unwrap())
        .collect();
    assert_eq!(people, vec!["lena", "mo", "val"]);
    let (s, list) = call!(h, LEAD, GET, members);
    assert_eq!(s, 200);
    assert_eq!(list["access"], "manage");
    assert!(list["accounts"].as_array().unwrap().iter().any(|a| a == "otto"));
    let (s, _) = call!(h, OUTSIDER, GET, members);
    assert_eq!(s, 403);

    // ── What each one sees ──────────────────────────────────────────────
    let (_, teams) = call!(h, ADMIN, GET, "/api/org/teams");
    assert_eq!(teams["teams"].as_array().unwrap().len(), 2);
    let (_, teams) = call!(h, MEMBER, GET, "/api/org/teams");
    let teams = teams["teams"].as_array().unwrap().clone();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0]["id"], web_id.as_str());
    assert_eq!(teams[0]["company_id"], acme_id.as_str());
    let (_, teams) = call!(h, OUTSIDER, GET, "/api/org/teams");
    assert!(teams["teams"].as_array().unwrap().is_empty());

    let (_, companies) = call!(h, MEMBER, GET, "/api/org/companies");
    let names: Vec<&str> = companies["companies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Acme"]);
    let (_, companies) = call!(h, ADMIN, GET, "/api/org/companies");
    assert_eq!(companies["companies"].as_array().unwrap().len(), 2);
    let (s, _) = call!(h, MEMBER, GET, format!("/api/org/companies/{globex_id}"));
    assert_eq!(s, 404, "another company is not even there");
    let (s, one) = call!(h, MEMBER, GET, format!("/api/org/companies/{acme_id}"));
    assert_eq!(s, 200);
    assert_eq!(one["team_ids"], json!([web_id]));

    let (_, floor) = call!(h, OUTSIDER, GET, "/api/teams/floor");
    assert!(floor["teams"].as_array().unwrap().is_empty());
    assert!(floor["unassigned"].as_array().unwrap().is_empty());
    let (_, floor) = call!(h, VIEWER, GET, "/api/teams/floor");
    assert_eq!(floor["teams"].as_array().unwrap().len(), 1);
    assert_eq!(floor["teams"][0]["access"], "read");
    assert_eq!(floor["teams"][0]["company_id"], acme_id.as_str());
    assert_eq!(floor["companies"].as_array().unwrap().len(), 1);
    assert_eq!(floor["manage_all"], false);
    let (_, floor) = call!(h, ADMIN, GET, "/api/teams/floor");
    assert_eq!(floor["companies"].as_array().unwrap().len(), 2);
    assert_eq!(floor["manage_all"], true);

    // ── The board ───────────────────────────────────────────────────────
    let tasks = format!("/api/org/teams/{web_id}/tasks");
    let (s, posted) = call!(
        h,
        MEMBER,
        POST,
        tasks,
        json!({ "title": "Ship the page", "assigned_to": "user:lena" })
    );
    assert_eq!(s, 200, "{posted}");
    let task_id = posted["id"].as_str().unwrap().to_string();
    let (s, _) = call!(
        h,
        MEMBER,
        POST,
        tasks,
        json!({ "title": "Nope", "assigned_to": "user:otto" })
    );
    assert_eq!(s, 400, "the assignee must be on the team");
    let (s, _) = call!(h, VIEWER, POST, tasks, json!({ "title": "Mine" }));
    assert_eq!(s, 403, "a viewer changes nothing");
    let (s, _) = call!(h, OUTSIDER, POST, tasks, json!({ "title": "Mine" }));
    assert_eq!(s, 403);

    let (s, board) = call!(h, VIEWER, GET, tasks);
    assert_eq!(s, 200);
    assert_eq!(board["columns"], json!(["todo", "doing", "review", "done"]));
    let card = &board["tasks"][0];
    assert_eq!(card["column"], "todo");
    assert_eq!(card["created_by"], "user:mo");
    assert_eq!(card["assigned_to"], "user:lena");
    let (s, _) = call!(h, OUTSIDER, GET, tasks);
    assert_eq!(s, 403);

    let card_uri = format!("{tasks}/{task_id}");
    for (column, status) in [
        ("doing", "in_progress"),
        ("review", "review"),
        ("done", "completed"),
        ("todo", "pending"),
    ] {
        let (s, moved) = call!(h, MEMBER, PUT, card_uri, json!({ "status": column }));
        assert_eq!(s, 200, "{moved}");
        assert_eq!(moved["task"]["status"], status);
    }
    let (s, _) = call!(h, MEMBER, PUT, card_uri, json!({ "column": "nowhere" }));
    assert_eq!(s, 400);
    let (s, _) = call!(h, MEMBER, PUT, card_uri, json!({ "assigned_to": "user:otto" }));
    assert_eq!(s, 400);
    // An agent on the team can be assigned; anyone is the empty string.
    let agent = freeco_types::agent::AgentId::new().to_string();
    h.state
        .kernel
        .memory
        .org()
        .add_team_member(&web_id, &agent, None)
        .unwrap();
    let (s, moved) = call!(h, MEMBER, PUT, card_uri, json!({ "assigned_to": agent }));
    assert_eq!(s, 200);
    assert_eq!(moved["task"]["assigned_to"], agent.as_str());
    let (s, moved) = call!(h, MEMBER, PUT, card_uri, json!({ "assigned_to": "" }));
    assert_eq!(s, 200);
    assert_eq!(moved["task"]["assigned_to"], "");
    let (s, _) = call!(h, VIEWER, PUT, card_uri, json!({ "status": "done" }));
    assert_eq!(s, 403);
    let (s, _) = call!(h, OUTSIDER, PUT, card_uri, json!({ "status": "done" }));
    assert_eq!(s, 403);

    // A card on another team's board is not reachable through this one.
    let (s, ops_card) = call!(
        h,
        ADMIN,
        POST,
        format!("/api/org/teams/{ops_id}/tasks"),
        json!({ "title": "Rotate keys" })
    );
    assert_eq!(s, 200);
    let ops_task = ops_card["id"].as_str().unwrap();
    let (s, _) = call!(h, LEAD, PUT, format!("{tasks}/{ops_task}"), json!({ "status": "done" }));
    assert_eq!(s, 404);

    // Deleting is the lead's (or an admin's).
    let (s, _) = call!(h, MEMBER, DELETE, card_uri);
    assert_eq!(s, 403);
    let (s, _) = call!(h, LEAD, DELETE, card_uri);
    assert_eq!(s, 200);
    let (s, _) = call!(h, LEAD, DELETE, card_uri);
    assert_eq!(s, 404);

    // ── Team and company changes are admin-only ─────────────────────────
    let team_uri = format!("/api/org/teams/{web_id}");
    let (s, _) = call!(h, LEAD, PUT, team_uri, json!({ "name": "Website" }));
    assert_eq!(s, 403);
    let (s, t) = call!(
        h,
        ADMIN,
        PUT,
        team_uri,
        json!({ "name": "Website", "company_id": globex_id })
    );
    assert_eq!(s, 200, "{t}");
    assert_eq!(t["team"]["name"], "Website");
    assert_eq!(t["team"]["company_id"], globex_id.as_str());

    let company_uri = format!("/api/org/companies/{acme_id}");
    let (s, _) = call!(h, LEAD, PUT, company_uri, json!({ "name": "Acme AG" }));
    assert_eq!(s, 403);
    let (s, c) = call!(h, ADMIN, PUT, company_uri, json!({ "name": "Acme AG" }));
    assert_eq!(s, 200);
    assert_eq!(c["company"]["name"], "Acme AG");
    let (s, _) = call!(h, LEAD, DELETE, company_uri);
    assert_eq!(s, 403);
    let (s, _) = call!(h, ADMIN, DELETE, company_uri);
    assert_eq!(s, 200);
    let (s, _) = call!(h, ADMIN, GET, company_uri);
    assert_eq!(s, 404);

    // ── Leaving ─────────────────────────────────────────────────────────
    let (s, _) = call!(h, MEMBER, DELETE, format!("{members}/user:lena"));
    assert_eq!(s, 403, "a member does not remove people");
    let (s, _) = call!(h, LEAD, DELETE, format!("{members}/user:mo"));
    assert_eq!(s, 200);
    let (s, _) = call!(h, LEAD, DELETE, format!("{members}/user:mo"));
    assert_eq!(s, 404);
    let (s, _) = call!(h, MEMBER, GET, members);
    assert_eq!(s, 403, "off the team, out of sight");
    let (s, _) = call!(h, LEAD, DELETE, format!("{members}/{agent}"));
    assert_eq!(s, 200, "agents leave the same way");
}

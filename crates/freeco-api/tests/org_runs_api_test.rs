//! The organisation editing and run-history routes, through the daemon's own
//! router (`server::build_router`), so a route missing from `server.rs` fails
//! here rather than only in a live daemon.
//!
//! Run: cargo test -p freeco-api --test org_runs_api_test

use freeco_kernel::FreecoKernel;
use freeco_types::config::{DefaultModelConfig, KernelConfig};
use std::net::SocketAddr;
use std::sync::Arc;

struct Daemon {
    base: String,
    kernel: Arc<FreecoKernel>,
    _tmp: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kernel.shutdown();
    }
}

async fn start() -> Daemon {
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
        ..KernelConfig::default()
    };
    let kernel = Arc::new(FreecoKernel::boot_with_config(config).unwrap());
    kernel.set_self_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (app, _state) = freeco_api::server::build_router(kernel.clone(), addr).await;
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .ok();
    });
    Daemon {
        base: format!("http://{addr}"),
        kernel,
        _tmp: tmp,
    }
}

fn spawn(kernel: &FreecoKernel, name: &str) -> String {
    let manifest: freeco_types::agent::AgentManifest = toml::from_str(&format!(
        r#"
name = "{name}"
module = "builtin:chat"

[model]
provider = "ollama"
model = "test-model"
system_prompt = "Test agent."
"#
    ))
    .unwrap();
    kernel.spawn_agent(manifest).unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appointing_a_lead_demotes_the_previous_one_and_is_logged() {
    let d = start().await;
    let http = reqwest::Client::new();
    let a = spawn(&d.kernel, "alpha");
    let b = spawn(&d.kernel, "beta");

    let team: serde_json::Value = http
        .post(format!("{}/api/org/teams", d.base))
        .json(&serde_json::json!({ "name": "Launch" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();
    let members = format!("{}/api/org/teams/{team_id}/members", d.base);

    let r = http
        .put(&members)
        .json(&serde_json::json!({ "agent_id": a, "role": "lead" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // `supervisor` is another word for the lead: one per team.
    let r: serde_json::Value = http
        .put(&members)
        .json(&serde_json::json!({ "agent_id": b, "role": "supervisor" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let roles: std::collections::HashMap<String, String> = r["team"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["agent_id"].as_str().unwrap().to_string(),
                m["role"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(roles[&b], "lead");
    assert_eq!(roles[&a], "member", "the previous lead steps down");

    let unknown = http
        .put(&members)
        .json(&serde_json::json!({ "agent_id": "not-an-agent", "role": "lead" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);

    // Taking a member off goes through main's DELETE route and is logged too.
    let r = http.delete(format!("{members}/{a}")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    let log: serde_json::Value = http
        .get(format!("{}/api/org/changes", d.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let changes: Vec<String> = log["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["change"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(changes[0], "took alpha off team \"Launch\"", "{changes:?}");
    assert_eq!(changes[1], "set beta in team \"Launch\" as lead");
    assert!(changes.contains(&"created team \"Launch\"".to_string()));
    assert_eq!(log["changes"][0]["actor"], "owner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_history_lists_recorded_runs() {
    let d = start().await;
    let http = reqwest::Client::new();
    let a = spawn(&d.kernel, "night-shift");
    let id: freeco_types::agent::AgentId = a.parse().unwrap();
    let failed = Err(freeco_kernel::error::KernelError::Freeco(
        freeco_types::error::FreecoError::Internal("provider down".to_string()),
    ));
    d.kernel
        .record_run(id, "trigger", "on new mail", chrono::Utc::now(), &failed);

    let body: serde_json::Value = http
        .get(format!("{}/api/runs/history", d.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["agent_name"], "night-shift");
    assert_eq!(runs[0]["source"], "trigger");
    assert_eq!(runs[0]["status"], "failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owners_hire_is_reviewed_and_logged_not_held() {
    let d = start().await;
    let http = reqwest::Client::new();
    let manifest = r#"
name = "ops-shell"
module = "builtin:chat"

[model]
provider = "ollama"
model = "test-model"
system_prompt = "Runs maintenance."

[capabilities]
shell = ["*"]
tools = ["shell_exec"]
"#;
    // A manifest asking for the host shell is held by the Security Agent
    // first (409); the owner approves that exact content, then the hire goes
    // through, and the hire review still records what it found.
    let held = http
        .post(format!("{}/api/agents", d.base))
        .json(&serde_json::json!({ "manifest_toml": manifest }))
        .send()
        .await
        .unwrap();
    assert_eq!(held.status(), 409);
    let held: serde_json::Value = held.json().await.unwrap();
    let hash = held["audit"]["content_hash"].as_str().unwrap().to_string();
    let ok = http
        .post(format!("{}/api/security/approve/{hash}", d.base))
        .json(&serde_json::json!({ "approved_by": "owner" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let r = http
        .post(format!("{}/api/agents", d.base))
        .json(&serde_json::json!({ "manifest_toml": manifest }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let body: serde_json::Value = r.json().await.unwrap();
    let review = body["security_review"].as_str().unwrap();
    assert!(review.starts_with("needs the owner's approval"), "{review}");

    let log = d.kernel.org_changes(5);
    assert_eq!(log[0]["actor"], "owner");
    assert!(log[0]["change"]
        .as_str()
        .unwrap()
        .starts_with("hired \"ops-shell\""));
}

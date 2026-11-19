//! The Chamber's routes show each caller only what `access.rs` allows.
//!
//! Boots a real kernel with `[[users]]` grants, writes conversations into its
//! session store directly, and calls the Chamber handlers with a `Caller`
//! put on the request, as the auth middleware would. Channel replies go to an
//! in-process recording adapter: no test sends a real message.
//!
//! Run: cargo test -p freeco-api --test chamber_test

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use freeco_api::middleware::{AuthMethod, Caller};
use freeco_api::routes::AppState;
use freeco_channels::types::{
    ChannelAdapter, ChannelContent, ChannelMessage, ChannelType, ChannelUser,
};
use freeco_kernel::FreecoKernel;
use freeco_types::agent::{AgentId, AgentManifest};
use freeco_types::auth::UserRole;
use freeco_types::config::{DefaultModelConfig, KernelConfig, UserConfig};
use freeco_types::message::Message;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tower::ServiceExt;

/// Records what it was asked to send; never contacts anything.
struct RecordingAdapter {
    sent: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait]
impl ChannelAdapter for RecordingAdapter {
    fn name(&self) -> &str {
        "telegram"
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::Telegram
    }

    async fn start(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = ChannelMessage> + Send>>, Box<dyn std::error::Error>>
    {
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn send(
        &self,
        user: &ChannelUser,
        content: ChannelContent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let ChannelContent::Text(text) = content {
            self.sent
                .lock()
                .unwrap()
                .push((user.platform_id.clone(), text));
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}

fn account(name: &str, role: &str, agents: &[&str], channels: &[&str]) -> UserConfig {
    UserConfig {
        name: name.to_string(),
        role: role.to_string(),
        agents: agents.iter().map(|s| s.to_string()).collect(),
        channels: channels.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

struct Fixture {
    state: Arc<AppState>,
    sent: Arc<Mutex<Vec<(String, String)>>>,
    /// An agent chat with `desk`.
    desk_chat: String,
    /// A guest's WhatsApp conversation with `desk`.
    desk_whatsapp: String,
    /// A guest's Telegram conversation with `desk`.
    desk_telegram: String,
    /// The owner's own WhatsApp conversation with `desk`.
    owner_whatsapp: String,
    /// An agent chat with `vault`, granted to nobody.
    vault_chat: String,
    _tmp: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.kernel.shutdown();
    }
}

fn conversation(kernel: &FreecoKernel, agent: AgentId, label: Option<&str>) -> String {
    let mut session = kernel
        .memory
        .create_session_with_label(agent, label)
        .unwrap();
    session.messages = vec![
        Message::user(format!("hello from {}", label.unwrap_or("the dashboard"))),
        Message::assistant("Hi, how can I help?"),
    ];
    kernel.memory.save_session(&session).unwrap();
    session.id.0.to_string()
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let mut owner = account("olga", "owner", &[], &[]);
    owner.channel_bindings = HashMap::from([("whatsapp".to_string(), "+41790000001".to_string())]);
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
        users: vec![
            owner,
            account("alice", "admin", &[], &[]),
            account("uma", "user", &["desk"], &["telegram"]),
            account("kim", "kid", &["desk"], &["*"]),
            account("val", "viewer", &["desk"], &["*"]),
            account("bob", "admin", &[], &[]),
        ],
        ..KernelConfig::default()
    };
    let kernel = Arc::new(FreecoKernel::boot_with_config(config).expect("kernel boots"));
    kernel.set_self_handle();

    let desk = kernel
        .spawn_agent(AgentManifest {
            name: "desk".to_string(),
            ..Default::default()
        })
        .unwrap();
    let vault = kernel
        .spawn_agent(AgentManifest {
            name: "vault".to_string(),
            ..Default::default()
        })
        .unwrap();
    let desk_chat = conversation(&kernel, desk, Some("Planning the trip"));
    let desk_whatsapp = conversation(&kernel, desk, Some("WhatsApp +41790000002"));
    let desk_telegram = conversation(&kernel, desk, Some("telegram:4242"));
    let owner_whatsapp = conversation(&kernel, desk, Some("whatsapp:+41790000001"));
    let vault_chat = conversation(&kernel, vault, Some("Secrets"));

    let sent = Arc::new(Mutex::new(Vec::new()));
    kernel.channel_adapters.insert(
        "telegram".to_string(),
        Arc::new(RecordingAdapter { sent: sent.clone() }),
    );

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
        local_ai: std::sync::Arc::new(tokio::sync::RwLock::new(Default::default())),
        services: std::sync::Arc::new(tokio::sync::RwLock::new(Default::default())),
        frozen: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        frozen_agents: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
        security: std::sync::Arc::new(freeco_api::security::SecurityService::default()),
    });
    Fixture {
        state,
        sent,
        desk_chat,
        desk_whatsapp,
        desk_telegram,
        owner_whatsapp,
        vault_chat,
        _tmp: tmp,
    }
}

fn session(name: &str, role: UserRole) -> Caller {
    Caller {
        username: Some(name.to_string()),
        role,
        method: AuthMethod::Session,
    }
}

fn app(state: &Arc<AppState>, caller: Caller) -> Router {
    use axum::routing::{get, post};
    Router::new()
        .route(
            "/api/chamber/conversations",
            get(freeco_api::chamber::list_conversations),
        )
        .route(
            "/api/chamber/conversations/{id}/messages",
            get(freeco_api::chamber::conversation_messages),
        )
        .route(
            "/api/chamber/conversations/{id}/reply",
            post(freeco_api::chamber::reply),
        )
        .layer(axum::Extension(caller))
        .with_state(state.clone())
}

async fn call(
    state: &Arc<AppState>,
    caller: Caller,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app(state, caller)
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn listed_ids(state: &Arc<AppState>, caller: Caller, query: &str) -> Vec<String> {
    let (status, body) = call(
        state,
        caller,
        Method::GET,
        &format!("/api/chamber/conversations{query}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn each_role_lists_only_what_it_may_see() {
    let f = fixture();
    let s = &f.state;
    let all = [
        &f.desk_chat,
        &f.desk_whatsapp,
        &f.desk_telegram,
        &f.owner_whatsapp,
        &f.vault_chat,
    ];

    // Owner and admin: everything.
    for caller in [
        Caller {
            username: None,
            role: UserRole::Owner,
            method: AuthMethod::Local,
        },
        session("alice", UserRole::Admin),
    ] {
        let ids = listed_ids(s, caller, "").await;
        for id in all {
            assert!(ids.contains(id), "admin sees {id}");
        }
    }

    // User: desk's chat and Telegram (granted), not WhatsApp (not granted),
    // not the owner's own conversation, not vault.
    let uma = listed_ids(s, session("uma", UserRole::User), "").await;
    assert!(uma.contains(&f.desk_chat));
    assert!(uma.contains(&f.desk_telegram));
    assert!(!uma.contains(&f.desk_whatsapp));
    assert!(!uma.contains(&f.owner_whatsapp));
    assert!(!uma.contains(&f.vault_chat));

    // Kid: granted every channel, still sees none of them.
    let kim = listed_ids(s, session("kim", UserRole::Kid), "").await;
    assert!(kim.contains(&f.desk_chat));
    for hidden in [
        &f.desk_whatsapp,
        &f.desk_telegram,
        &f.owner_whatsapp,
        &f.vault_chat,
    ] {
        assert!(!kim.contains(hidden), "kid sees {hidden}");
    }

    // Viewer with every channel: guests' channel conversations, read-only,
    // but never the owner's own.
    let val = listed_ids(s, session("val", UserRole::Viewer), "").await;
    assert!(val.contains(&f.desk_chat));
    assert!(val.contains(&f.desk_whatsapp));
    assert!(val.contains(&f.desk_telegram));
    assert!(!val.contains(&f.owner_whatsapp));
    assert!(!val.contains(&f.vault_chat));

    // A signed-in name with no [[users]] entry: nothing.
    let nobody = listed_ids(s, session("mallory", UserRole::User), "").await;
    assert!(nobody.is_empty());
}

#[tokio::test]
async fn filters_and_the_filter_choices_follow_access() {
    let f = fixture();
    let s = &f.state;
    let (_, body) = call(
        s,
        session("uma", UserRole::User),
        Method::GET,
        "/api/chamber/conversations",
        None,
    )
    .await;
    let channels: Vec<&str> = body["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(channels, vec!["agent", "telegram"], "no whatsapp offered");
    let agents: Vec<&str> = body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(agents, vec!["desk"]);

    let tg = listed_ids(s, session("uma", UserRole::User), "?channel=telegram").await;
    assert_eq!(tg, vec![f.desk_telegram.clone()]);
    let wa = listed_ids(s, session("alice", UserRole::Admin), "?channel=whatsapp").await;
    assert!(wa.contains(&f.desk_whatsapp) && wa.contains(&f.owner_whatsapp));
    assert!(!wa.contains(&f.desk_telegram));
    // Spawning an agent opens its first conversation too, so not only these.
    let vault = listed_ids(s, session("alice", UserRole::Admin), "?agent=vault").await;
    assert!(vault.contains(&f.vault_chat));
    assert!(!vault.contains(&f.desk_chat));
}

#[tokio::test]
async fn hidden_conversations_are_404_and_reading_marks_them_read() {
    let f = fixture();
    let s = &f.state;
    let uma = || session("uma", UserRole::User);

    for hidden in [&f.desk_whatsapp, &f.owner_whatsapp, &f.vault_chat] {
        let uri = format!("/api/chamber/conversations/{hidden}/messages");
        let (status, _) = call(s, uma(), Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
        let uri = format!("/api/chamber/conversations/{hidden}/reply");
        let body = serde_json::json!({ "text": "hi" });
        let (status, _) = call(s, uma(), Method::POST, &uri, Some(body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "reply {hidden}");
    }
    // Same answer as for something that does not exist at all.
    let missing = uuid::Uuid::new_v4();
    let uri = format!("/api/chamber/conversations/{missing}/messages");
    assert_eq!(
        call(s, uma(), Method::GET, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );

    let unread = |body: &serde_json::Value, id: &str| {
        body["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .map(|c| c["unread"].as_u64().unwrap())
    };
    let (_, before) = call(s, uma(), Method::GET, "/api/chamber/conversations", None).await;
    assert_eq!(unread(&before, f.desk_telegram.as_str()), Some(2));

    let uri = format!("/api/chamber/conversations/{}/messages", f.desk_telegram);
    let (status, body) = call(s, uma(), Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["text"], "Hi, how can I help?");
    assert_eq!(body["conversation"]["channel"], "telegram");
    assert_eq!(body["conversation"]["peer"], "4242");

    let (_, after) = call(s, uma(), Method::GET, "/api/chamber/conversations", None).await;
    assert_eq!(unread(&after, f.desk_telegram.as_str()), Some(0));
    let (_, unread_only) = call(
        s,
        uma(),
        Method::GET,
        "/api/chamber/conversations?unread=1",
        None,
    )
    .await;
    assert_eq!(unread(&unread_only, f.desk_telegram.as_str()), None);
}

#[tokio::test]
async fn another_tenants_admin_sees_nothing_here() {
    let f = fixture();
    let s = &f.state;
    let tenancy = s.kernel.memory.tenancy();
    let other = tenancy.create_tenant("Elsewhere", "cloud").unwrap();
    tenancy
        .create_user(
            &other.id,
            "bob",
            None,
            freeco_memory::tenancy::Role::Admin,
            None,
            None,
            None,
        )
        .unwrap();
    let bob = || session("bob", UserRole::Admin);
    assert!(listed_ids(s, bob(), "").await.is_empty());
    let uri = format!("/api/chamber/conversations/{}/messages", f.desk_chat);
    assert_eq!(
        call(s, bob(), Method::GET, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_channel_reply_goes_through_the_adapter_and_is_recorded() {
    let f = fixture();
    let s = &f.state;
    let uri = format!("/api/chamber/conversations/{}/reply", f.desk_telegram);
    let body = serde_json::json!({ "text": "A person here: on it." });
    let (status, json) = call(
        s,
        session("uma", UserRole::User),
        Method::POST,
        &uri,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        f.sent.lock().unwrap().clone(),
        vec![("4242".to_string(), "A person here: on it.".to_string())]
    );
    let sid = freeco_types::agent::SessionId(uuid::Uuid::parse_str(&f.desk_telegram).unwrap());
    let session = s.kernel.memory.get_session(sid).unwrap().unwrap();
    assert_eq!(
        session.messages.last().unwrap().content.text_content(),
        "A person here: on it."
    );
}

#[tokio::test]
async fn a_viewer_reads_but_cannot_reply() {
    let f = fixture();
    let s = &f.state;
    let val = || session("val", UserRole::Viewer);
    for id in [&f.desk_chat, &f.desk_telegram] {
        let uri = format!("/api/chamber/conversations/{id}/messages");
        assert_eq!(
            call(s, val(), Method::GET, &uri, None).await.0,
            StatusCode::OK
        );
        let uri = format!("/api/chamber/conversations/{id}/reply");
        let body = serde_json::json!({ "text": "hi" });
        let (status, _) = call(s, val(), Method::POST, &uri, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{id}");
    }
    let (_, list) = call(s, val(), Method::GET, "/api/chamber/conversations", None).await;
    assert!(list["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["can_reply"] == false));
    assert!(f.sent.lock().unwrap().is_empty(), "nothing was sent");
}

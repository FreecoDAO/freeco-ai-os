//! The Chamber: one inbox for every conversation -- agent chats and each
//! person's conversation on WhatsApp, Telegram, email and every other
//! channel -- showing a caller only what [`crate::access`] lets them see.
//!
//! - `GET  /api/chamber/conversations` lists them (filters: `channel`,
//!   `agent`, `unread=1`);
//! - `GET  /api/chamber/conversations/{id}/messages` opens one and marks it
//!   read for this caller;
//! - `POST /api/chamber/conversations/{id}/reply` writes in one: in an agent
//!   chat the text goes to the agent, in a channel conversation it is sent to
//!   the person through the channel's adapter (the same send path agents and
//!   cron deliveries use) and recorded in the conversation;
//! - `GET  /api/chamber/stream` is a server-sent event per conversation that
//!   changes, filtered for the caller the same way.
//!
//! A conversation the caller may not see answers 404 everywhere, exactly like
//! one that does not exist.

use crate::access::{classify, Access, ConversationKind, ConversationRef};
use crate::middleware::{AuthMethod, Caller};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use freeco_kernel::FreecoKernel;
use freeco_kernel_runtime::kernel_handle::KernelHandle;
use freeco_memory::session::ConversationSummary;
use freeco_types::agent::{AgentId, SessionId};
use freeco_types::auth::UserRole;
use freeco_types::message::{Message, Role};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Longest reply accepted, as for a chat message.
const MAX_REPLY_BYTES: usize = 64 * 1024;

/// How often the live stream looks for changed conversations.
const STREAM_POLL: std::time::Duration = std::time::Duration::from_secs(2);

type ApiError = (StatusCode, Json<serde_json::Value>);

fn fail(code: StatusCode, msg: impl std::fmt::Display) -> ApiError {
    (code, Json(serde_json::json!({ "error": msg.to_string() })))
}

fn not_found() -> ApiError {
    fail(StatusCode::NOT_FOUND, "No such conversation")
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

// ── Read marks ──────────────────────────────────────────────────────────────

/// Who reads, for read marks: the account, or `owner` for the local user and
/// an API-key holder.
fn reader(caller: &Caller) -> String {
    match (caller.method, caller.username.as_deref()) {
        (AuthMethod::Session, Some(name)) => format!("user:{name}"),
        _ => "owner".to_string(),
    }
}

type ReadMarks = HashMap<String, HashMap<String, usize>>;

/// How many readable messages each reader has seen of each conversation,
/// kept in `<data_dir>/chamber_read.json` so unread counts survive a restart.
fn marks_path(kernel: &FreecoKernel) -> std::path::PathBuf {
    kernel.config.data_dir.join("chamber_read.json")
}

/// Serialises read-modify-write of the marks file.
static MARKS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn load_marks(kernel: &FreecoKernel) -> ReadMarks {
    std::fs::read(marks_path(kernel))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn mark_read(kernel: &FreecoKernel, reader: &str, session_id: &str, seen: usize) {
    let _guard = MARKS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut marks = load_marks(kernel);
    marks
        .entry(reader.to_string())
        .or_default()
        .insert(session_id.to_string(), seen);
    let path = marks_path(kernel);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(&marks) {
        if let Err(e) = std::fs::write(&path, json) {
            tracing::warn!(error = %e, "Could not save Chamber read marks");
        }
    }
}

// ── Listing ─────────────────────────────────────────────────────────────────

/// One conversation as the Chamber lists it, with what access needs.
struct Listed {
    summary: ConversationSummary,
    kind: ConversationKind,
    agent_name: String,
    peer_is_operator: bool,
}

impl Listed {
    fn as_ref(&self) -> ConversationRef<'_> {
        ConversationRef {
            agent_id: &self.summary.agent_id,
            agent_name: &self.agent_name,
            tenant_id: &self.summary.tenant_id,
            kind: &self.kind,
            peer_is_operator: self.peer_is_operator,
        }
    }
}

/// Whether a channel peer is bound to an owner or admin account: the guest
/// rule of `freeco_kernel::guest`, which says a non-guest's conversation
/// carries the owner's private context.
fn peer_is_operator(kernel: &FreecoKernel, kind: &ConversationKind) -> bool {
    kind.peer_key()
        .is_some_and(|key| freeco_kernel::guest::guest_for(&kernel.auth, Some(&key)).is_none())
}

fn listed(
    kernel: &FreecoKernel,
    summary: ConversationSummary,
    names: &HashMap<String, String>,
) -> Listed {
    let kind = classify(summary.label.as_deref());
    let agent_name = names.get(&summary.agent_id).cloned().unwrap_or_default();
    let peer_is_operator = peer_is_operator(kernel, &kind);
    Listed {
        summary,
        kind,
        agent_name,
        peer_is_operator,
    }
}

fn agent_names(kernel: &FreecoKernel) -> HashMap<String, String> {
    kernel
        .registry
        .list()
        .into_iter()
        .map(|e| (e.id.to_string(), e.name))
        .collect()
}

/// Every conversation `access` may see, newest first.
fn visible(kernel: &FreecoKernel, access: &Access) -> Vec<Listed> {
    let names = agent_names(kernel);
    let summaries = match kernel.memory.list_conversation_summaries() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "Chamber could not list conversations");
            return Vec::new();
        }
    };
    summaries
        .into_iter()
        .map(|s| listed(kernel, s, &names))
        .filter(|l| access.can_see_conversation(&l.as_ref()))
        .collect()
}

/// A person's name for a channel peer: the `[[users]]` account bound to it,
/// otherwise the id itself.
fn peer_name(kernel: &FreecoKernel, kind: &ConversationKind) -> Option<String> {
    let ConversationKind::Channel { channel, peer } = kind else {
        return None;
    };
    let bound = kernel
        .config
        .users
        .iter()
        .find(|u| u.enabled && u.channel_bindings.get(channel).is_some_and(|id| id == peer));
    Some(
        bound
            .map(|u| u.name.clone())
            .unwrap_or_else(|| peer.clone()),
    )
}

fn item_json(
    kernel: &FreecoKernel,
    access: &Access,
    l: &Listed,
    seen: Option<usize>,
) -> serde_json::Value {
    let s = &l.summary;
    let (channel, peer) = match &l.kind {
        ConversationKind::Agent => ("agent".to_string(), None),
        ConversationKind::Channel { channel, peer } => (channel.clone(), Some(peer.clone())),
    };
    let title = match (&l.kind, &s.label) {
        (ConversationKind::Channel { .. }, _) => peer_name(kernel, &l.kind),
        (ConversationKind::Agent, Some(label)) if !label.trim().is_empty() => Some(label.clone()),
        _ => None,
    }
    .unwrap_or_else(|| format!("Chat with {}", l.agent_name));
    serde_json::json!({
        "id": s.session_id,
        "channel": channel,
        "peer": peer,
        "title": title,
        "agent_id": s.agent_id,
        "agent_name": l.agent_name,
        "last_message": s.last_text.as_ref().map(|text| serde_json::json!({
            "role": s.last_role.map(role_name),
            "text": text,
        })),
        "message_count": s.text_count,
        "unread": s.text_count.saturating_sub(seen.unwrap_or(0)),
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "can_reply": can_reply(access, l),
    })
}

/// Whether the caller may reply here: write access, and in a channel
/// conversation (which sends to a person outside) at least `user`.
fn can_reply(access: &Access, l: &Listed) -> bool {
    access.can_write_conversation(&l.as_ref())
        && (matches!(l.kind, ConversationKind::Agent) || access.caller.role >= UserRole::User)
}

/// Query filters for the list.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// `agent` for agent chats, or a channel name.
    pub channel: Option<String>,
    /// An agent id or name.
    pub agent: Option<String>,
    /// `1`/`true`: only conversations with unread messages.
    pub unread: Option<String>,
}

fn is_true(v: Option<&str>) -> bool {
    matches!(v.map(str::trim), Some("1" | "true" | "yes"))
}

/// GET /api/chamber/conversations — every conversation the caller may see.
pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let kernel = &state.kernel;
    let access = Access::resolve(kernel, &caller);
    let marks = load_marks(kernel);
    let mine = marks.get(&reader(&caller));
    let all = visible(kernel, &access);

    // Filter choices come from what the caller can see, never from what
    // exists: listing a channel they cannot open would leak it.
    let mut channels: Vec<String> = all
        .iter()
        .map(|l| l.kind.channel().unwrap_or("agent").to_string())
        .collect();
    channels.sort();
    channels.dedup();
    let agents: Vec<serde_json::Value> = access
        .visible_agents(kernel.registry.list())
        .into_iter()
        .map(|a| serde_json::json!({ "id": a.id.to_string(), "name": a.name }))
        .collect();

    let channel = q
        .channel
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let agent = q.agent.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let unread_only = is_true(q.unread.as_deref());
    let conversations: Vec<serde_json::Value> = all
        .iter()
        .filter(|l| match channel {
            None => true,
            Some(c) => l.kind.channel().unwrap_or("agent").eq_ignore_ascii_case(c),
        })
        .filter(|l| match agent {
            None => true,
            Some(a) => l.summary.agent_id == a || l.agent_name.eq_ignore_ascii_case(a),
        })
        .map(|l| {
            let seen = mine.and_then(|m| m.get(&l.summary.session_id)).copied();
            item_json(kernel, &access, l, seen)
        })
        .filter(|item| !unread_only || item["unread"].as_u64().unwrap_or(0) > 0)
        .collect();

    Json(serde_json::json!({
        "conversations": conversations,
        "channels": channels,
        "agents": agents,
    }))
}

/// The conversation `id` if the caller may see it.
fn find_visible(kernel: &FreecoKernel, access: &Access, id: &str) -> Option<Listed> {
    let uuid = uuid::Uuid::parse_str(id.trim()).ok()?;
    let id = uuid.to_string();
    let summary = kernel
        .memory
        .list_conversation_summaries()
        .ok()?
        .into_iter()
        .find(|s| s.session_id == id)?;
    let l = listed(kernel, summary, &agent_names(kernel));
    if access.can_see_conversation(&l.as_ref()) {
        Some(l)
    } else {
        None
    }
}

/// Whether `access` may write in conversation `session_id`, for a chat that
/// names the conversation it sends to (the WebSocket's `session_id`).
///
/// A conversation that no longer exists is fine: the kernel then uses the
/// agent's current one, which chatting with the agent already covers. One in
/// the trash is for admins only.
pub(crate) fn can_write_in(kernel: &FreecoKernel, access: &Access, session_id: &str) -> bool {
    let Ok(uuid) = uuid::Uuid::parse_str(session_id.trim()) else {
        return false;
    };
    let id = uuid.to_string();
    let listed_one = kernel
        .memory
        .list_conversation_summaries()
        .ok()
        .and_then(|all| all.into_iter().find(|s| s.session_id == id));
    match listed_one {
        Some(summary) => {
            let l = listed(kernel, summary, &agent_names(kernel));
            access.can_write_conversation(&l.as_ref())
        }
        None => match kernel.memory.get_session(SessionId(uuid)) {
            Ok(None) => true,
            _ => access.caller.role >= UserRole::Admin,
        },
    }
}

/// GET /api/chamber/conversations/{id}/messages — one conversation's
/// readable messages (no system prompt, no tool traffic). Marks it read.
pub async fn conversation_messages(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kernel = &state.kernel;
    let access = Access::resolve(kernel, &caller);
    let l = find_visible(kernel, &access, &id).ok_or_else(not_found)?;
    let session_id =
        SessionId(uuid::Uuid::parse_str(&l.summary.session_id).map_err(|_| not_found())?);
    let session = kernel
        .memory
        .get_session(session_id)
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(not_found)?;
    let messages: Vec<serde_json::Value> = session
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .filter_map(|m| {
            let text = m.content.text_content();
            let text = text.trim();
            (!text.is_empty()).then(|| {
                serde_json::json!({
                    "id": m.msg_id,
                    "role": role_name(m.role),
                    "text": text,
                })
            })
        })
        .collect();
    mark_read(
        kernel,
        &reader(&caller),
        &l.summary.session_id,
        messages.len(),
    );
    Ok(Json(serde_json::json!({
        "conversation": item_json(kernel, &access, &l, Some(messages.len())),
        "messages": messages,
    })))
}

/// Body of a reply.
#[derive(Debug, Deserialize)]
pub struct ReplyRequest {
    /// What to say.
    pub text: String,
}

/// POST /api/chamber/conversations/{id}/reply — write in a conversation.
///
/// An agent chat: the text goes to the agent in that conversation and its
/// answer comes back. A channel conversation: the text is sent to the person
/// through the channel's adapter and recorded in the conversation as the
/// agent's side, so the agent knows it was said.
pub async fn reply(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(id): Path<String>,
    Json(req): Json<ReplyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let kernel = &state.kernel;
    let access = Access::resolve(kernel, &caller);
    let l = find_visible(kernel, &access, &id).ok_or_else(not_found)?;
    // Visible, so saying "not allowed" leaks nothing.
    if !can_reply(&access, &l) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            format!(
                "You can read this conversation but not reply ({})",
                caller.role
            ),
        ));
    }
    let text = req.text.trim();
    if text.is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Nothing to send"));
    }
    if text.len() > MAX_REPLY_BYTES {
        return Err(fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Reply too long (max 64KB)",
        ));
    }
    if state.frozen.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "System is frozen. Unfreeze from the dashboard to resume.",
        ));
    }
    let session_id =
        SessionId(uuid::Uuid::parse_str(&l.summary.session_id).map_err(|_| not_found())?);
    let agent_id: AgentId = l.summary.agent_id.parse().map_err(|_| not_found())?;

    match &l.kind {
        ConversationKind::Agent => {
            let handle: Arc<dyn KernelHandle> = kernel.clone();
            let result = kernel
                .send_message_in_session(agent_id, session_id, text, Some(handle), None)
                .await
                .map_err(|e| fail(StatusCode::BAD_GATEWAY, e))?;
            let response = if result.silent {
                String::new()
            } else {
                result.response
            };
            Ok(Json(
                serde_json::json!({ "sent": true, "response": response }),
            ))
        }
        ConversationKind::Channel { channel, peer } => {
            KernelHandle::send_channel_message(&**kernel, channel, peer, text, None)
                .await
                .map_err(|e| fail(StatusCode::BAD_GATEWAY, e))?;
            // Recorded only once it has gone out. Not atomic with an agent
            // run writing the same conversation at this moment.
            if let Ok(Some(mut session)) = kernel.memory.get_session(session_id) {
                session.messages.push(Message::assistant(text));
                if let Err(e) = kernel.memory.save_session(&session) {
                    tracing::warn!(error = %e, "Sent a Chamber reply but could not record it");
                }
            }
            tracing::info!(
                channel = %channel,
                by = %reader(&caller),
                "Chamber reply sent on a channel"
            );
            Ok(Json(
                serde_json::json!({ "sent": true, "channel": channel }),
            ))
        }
    }
}

/// GET /api/chamber/stream — a server-sent event for every conversation the
/// caller may see that changes, as `{"type":"conversation", "conversation":
/// {...}}`. The caller's access is resolved when the stream opens.
pub async fn stream(
    State(state): State<Arc<AppState>>,
    caller: Caller,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(64);
    let access = Access::resolve(&state.kernel, &caller);
    let reader = reader(&caller);
    tokio::spawn(async move {
        let kernel = &state.kernel;
        // What the caller has already: no burst of events on open.
        let mut known: HashMap<String, String> = visible(kernel, &access)
            .into_iter()
            .map(|l| (l.summary.session_id, l.summary.updated_at))
            .collect();
        loop {
            tokio::time::sleep(STREAM_POLL).await;
            if tx.is_closed() {
                return;
            }
            let marks = load_marks(kernel);
            let mine = marks.get(&reader);
            for l in visible(kernel, &access) {
                let id = &l.summary.session_id;
                if known.get(id) == Some(&l.summary.updated_at) {
                    continue;
                }
                known.insert(id.clone(), l.summary.updated_at.clone());
                let seen = mine.and_then(|m| m.get(id)).copied();
                let data = serde_json::json!({
                    "type": "conversation",
                    "conversation": item_json(kernel, &access, &l, seen),
                });
                if tx
                    .send(Ok(Event::default().data(data.to_string())))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

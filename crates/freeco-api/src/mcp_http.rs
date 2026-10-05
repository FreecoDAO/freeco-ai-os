//! MCP over Streamable HTTP: lets an external coding client (Claude Code,
//! Codex) work as a member of the owner's agent team.
//!
//! `freeco-ai mcp` (the stdio server in the CLI) exposes each agent as a tool
//! to talk to. This endpoint goes further: the client acts *as* one of the
//! team's agents -- named by the `X-Freeco-As-Agent` header -- reads and claims
//! work on the shared task board, posts into an agent's conversation where the
//! owner sees it, asks other agents, and runs commands in that agent's own
//! sandbox container. Never on the host: there is no tool here that executes
//! anything outside the sandbox.
//!
//! Transport: `POST /mcp` with one JSON-RPC 2.0 message (or a batch), answered
//! with `application/json`. Notifications get `202 Accepted` and no body.
//! There is no server-to-client stream, so `GET /mcp` is `405`, as the spec
//! allows, and there are no sessions to `DELETE`.
//!
//! Auth: `Authorization: Bearer <FREECO_MCP_TOKEN>`. Without that variable set
//! every request is refused -- an endpoint that can run commands must not
//! default to open just because the request came from loopback. The route
//! stays behind the global CSRF guard like every other route.
//!
//! Identity: the token may act as any existing agent, named by
//! `X-Freeco-As-Agent`; a name that matches no agent is refused. The agent's
//! own manifest must grant the FreEco tool behind each acting tool -- the
//! client gets that agent's capabilities, never more.

use crate::routes::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use freeco_types::agent::{AgentEntry, AgentId};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Environment variable holding the bearer token clients must present.
pub const TOKEN_ENV: &str = "FREECO_MCP_TOKEN";
/// Header naming the agent the caller acts as.
pub const AS_AGENT_HEADER: &str = "x-freeco-as-agent";
/// Header naming the client, used to label what it posts into a chat.
pub const CLIENT_HEADER: &str = "x-freeco-client";
/// Agent assumed when the caller does not say.
pub const DEFAULT_AGENT: &str = "Developer";
/// Label used when the client does not name itself.
pub const DEFAULT_CLIENT: &str = "Claude Code";

/// Protocol revisions this server speaks; the first is the preferred one.
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Longest wait for another agent's reply.
const ASK_TIMEOUT: Duration = Duration::from_secs(600);
/// Same cap the REST message endpoint applies.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const READ_DEFAULT: usize = 30;
const READ_MAX: usize = 200;

/// Every tool this endpoint offers, in the order `tools/list` returns them.
pub const TOOL_NAMES: &[&str] = &[
    "freeco_list_agents",
    "freeco_list_tasks",
    "freeco_post_task",
    "freeco_claim_task",
    "freeco_complete_task",
    "freeco_read_chat",
    "freeco_post_to_chat",
    "freeco_ask_agent",
    "freeco_sandbox_exec",
    "freeco_crm",
];

// ── JSON-RPC plumbing ──────────────────────────────────────────────────────

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool outcome as MCP wants it: failures are results with `isError`, not
/// protocol errors, so the model sees them and can react.
fn tool_result(outcome: Result<String, String>) -> Value {
    match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    }
}

/// What one incoming message asks for.
#[derive(Debug)]
enum Routed {
    /// Answer immediately with this JSON-RPC response.
    Reply(Value),
    /// A notification or a client response: nothing to send back.
    NoReply,
    /// Run a tool, then answer with its result.
    CallTool {
        id: Value,
        name: String,
        args: Value,
    },
}

/// Decide what to do with one message. Pure, so the protocol can be tested
/// without a kernel.
fn route(msg: &Value) -> Routed {
    let Some(obj) = msg.as_object() else {
        return Routed::Reply(rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response from the client (to a request this server never sends),
        // or garbage. Neither gets an answer if it has no id.
        return match id {
            Some(id) if obj.get("result").is_none() && obj.get("error").is_none() => {
                Routed::Reply(rpc_error(id, -32600, "Invalid Request"))
            }
            _ => Routed::NoReply,
        };
    };
    // No id: a notification. `notifications/initialized` and every other
    // notification are acknowledged with 202 and otherwise ignored.
    let Some(id) = id else {
        return Routed::NoReply;
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = if SUPPORTED_VERSIONS.contains(&asked) {
                asked
            } else {
                SUPPORTED_VERSIONS[0]
            };
            Routed::Reply(rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "freeco", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "You are working as a member of the owner's FreEco agent team, \
                        acting as the agent named in the X-Freeco-As-Agent header. Read the task \
                        board first, claim a task before working on it, do the work with \
                        freeco_sandbox_exec (never with commands on the host), post progress to \
                        the agent's chat, then complete the task. Never ask for keys in chat.",
                }),
            ))
        }
        "ping" => Routed::Reply(rpc_result(id, json!({}))),
        "tools/list" => Routed::Reply(rpc_result(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_string();
            if !TOOL_NAMES.contains(&name.as_str()) {
                return Routed::Reply(rpc_error(id, -32602, &format!("Unknown tool: {name}")));
            }
            let args = match params.get("arguments") {
                Some(Value::Object(_)) => params["arguments"].clone(),
                _ => json!({}),
            };
            Routed::CallTool { id, name, args }
        }
        other => Routed::Reply(rpc_error(id, -32601, &format!("Method not found: {other}"))),
    }
}

/// Turn the collected answers into the HTTP reply: nothing to say is `202`.
fn finish(replies: Vec<Value>, batch: bool) -> Response {
    if replies.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }
    if batch {
        Json(Value::Array(replies)).into_response()
    } else {
        Json(replies.into_iter().next().unwrap_or(Value::Null)).into_response()
    }
}

// ── Auth ───────────────────────────────────────────────────────────────────

/// Check the bearer token against the configured one, in constant time.
///
/// `expected` is the value of `FREECO_MCP_TOKEN`; passed in so tests do not
/// touch the process environment.
fn check_bearer(expected: Option<&str>, authorization: Option<&str>) -> Result<(), String> {
    let expected = expected.map(str::trim).unwrap_or("");
    if expected.is_empty() {
        return Err(format!(
            "The FreEco MCP endpoint is switched off: no token is configured. Set {TOKEN_ENV} \
             in ~/.freeco-ai/secrets.env and restart the daemon (or save it now with POST \
             /api/vault/secrets), then give the same value to the client as its bearer token."
        ));
    }
    let Some(value) = authorization else {
        return Err("Missing Authorization: Bearer <token> header.".to_string());
    };
    let token = match value.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => rest.trim(),
        _ => return Err("The Authorization header must use the Bearer scheme.".to_string()),
    };
    use subtle::ConstantTimeEq;
    let matches =
        token.len() == expected.len() && bool::from(token.as_bytes().ct_eq(expected.as_bytes()));
    if matches {
        Ok(())
    } else {
        Err("Invalid MCP token.".to_string())
    }
}

fn unauthorized(message: String) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"freeco\"")],
        Json(json!({ "error": message })),
    )
        .into_response()
}

// ── HTTP handlers ──────────────────────────────────────────────────────────

/// Who is calling, from the request headers.
struct Caller {
    /// The agent name or id from `X-Freeco-As-Agent`.
    as_agent: String,
    /// Short label for the client, used on what it posts into a chat.
    client: String,
}

impl Caller {
    fn from_headers(headers: &HeaderMap) -> Self {
        let as_agent = headers
            .get(AS_AGENT_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_AGENT)
            .to_string();
        let client = headers
            .get(CLIENT_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(sanitize_client_label)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_CLIENT.to_string());
        Self { as_agent, client }
    }
}

/// A client label goes into the owner's chat, so keep it short and plain.
fn sanitize_client_label(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || " -_.".contains(*c))
        .take(32)
        .collect::<String>()
        .trim()
        .to_string()
}

/// POST /mcp — one JSON-RPC message or a batch.
pub async fn handle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = std::env::var(TOKEN_ENV).ok();
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Err(message) = check_bearer(token.as_deref(), authorization) {
        return unauthorized(message);
    }

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, -32700, &format!("Parse error: {e}"))),
            )
                .into_response()
        }
    };
    let caller = Caller::from_headers(&headers);

    let (messages, batch) = match parsed {
        Value::Array(items) if items.is_empty() => {
            return Json(rpc_error(
                Value::Null,
                -32600,
                "Invalid Request: empty batch",
            ))
            .into_response()
        }
        Value::Array(items) => (items, true),
        other => (vec![other], false),
    };

    let mut replies = Vec::new();
    for msg in &messages {
        match route(msg) {
            Routed::Reply(v) => replies.push(v),
            Routed::NoReply => {}
            Routed::CallTool { id, name, args } => {
                tracing::info!(tool = %name, as_agent = %caller.as_agent, client = %caller.client, "MCP tool call");
                let outcome = call_tool(&state, &caller, &name, &args).await;
                replies.push(rpc_result(id, tool_result(outcome)));
            }
        }
    }
    finish(replies, batch)
}

/// GET and DELETE /mcp — no server-sent stream and no sessions.
pub async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        Json(json!({ "error": "This MCP endpoint answers POST only; it offers no event stream and no sessions." })),
    )
        .into_response()
}

// ── Tool definitions ───────────────────────────────────────────────────────

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "freeco_list_agents",
            "description": "List the agents on the owner's FreEco team: id, name, state, and the model provider and model each one runs on.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "freeco_list_tasks",
            "description": "Read the team's shared task board, newest first. Check it before starting work.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "Only tasks in this state. Omit for all."
                    }
                }
            }
        }),
        json!({
            "name": "freeco_post_task",
            "description": "Post a task to the shared board, optionally assigned to one agent. It is recorded as created by the agent you act as.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short task title." },
                    "description": { "type": "string", "description": "What needs doing, with enough context to do it." },
                    "assignee": { "type": "string", "description": "Name or id of the agent to assign it to. Omit to leave it open for anyone." }
                },
                "required": ["title", "description"]
            }
        }),
        json!({
            "name": "freeco_claim_task",
            "description": "Claim a pending task as the agent you act as, marking it in progress. Give task_id to claim that task; omit it to claim the next task assigned to you or to no one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Id of the task to claim, from freeco_list_tasks." }
                }
            }
        }),
        json!({
            "name": "freeco_complete_task",
            "description": "Mark a task completed and record its result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Id of the task." },
                    "result": { "type": "string", "description": "What was done and where the output is." }
                },
                "required": ["task_id", "result"]
            }
        }),
        json!({
            "name": "freeco_read_chat",
            "description": "Read the recent messages of an agent's conversation, the one the owner sees in the dashboard, oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Agent name or id." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 30, "description": "How many recent messages." }
                },
                "required": ["agent"]
            }
        }),
        json!({
            "name": "freeco_post_to_chat",
            "description": "Add a message to an agent's conversation so the owner sees it in the dashboard. Use it for progress and results. The message is labelled with the client and the agent you act as, e.g. '[Claude Code as Developer] '.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Agent name or id whose conversation to post in." },
                    "text": { "type": "string", "description": "The message." }
                },
                "required": ["agent", "text"]
            }
        }),
        json!({
            "name": "freeco_ask_agent",
            "description": "Send a message to a FreEco agent and wait for its reply (up to 10 minutes). The exchange is kept in that agent's conversation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Agent name or id." },
                    "message": { "type": "string", "description": "What to ask or tell it." }
                },
                "required": ["agent", "message"]
            }
        }),
        json!({
            "name": "freeco_sandbox_exec",
            "description": "Run a shell command in the sandbox container of the agent you act as, with its workspace mounted at /workspace. This is the only way to execute anything on this machine; nothing runs on the host. 'workspace' (default) is writable and networked for builds and tests; 'exec' is a throwaway container without network.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run." },
                    "tier": { "type": "string", "enum": ["exec", "workspace"], "default": "workspace", "description": "Sandbox tier." }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "freeco_crm",
            "description": "Call the CRM on this machine (Twenty, REST). The key is read from the vault; never ask for it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": { "type": "string", "enum": ["GET", "POST", "PATCH", "PUT", "DELETE"], "default": "GET" },
                    "path": { "type": "string", "description": "API path, e.g. /rest/companies?limit=20" },
                    "body": { "type": "object", "description": "JSON body for POST, PATCH or PUT." }
                },
                "required": ["path"]
            }
        }),
    ]
}

// ── Tool implementations ───────────────────────────────────────────────────

/// Pick an agent out of `(id, name)` pairs: by id, then exact name, then the
/// name in any case. Pure, so resolution can be tested without a kernel.
fn find_agent(agents: &[(String, String)], name_or_id: &str) -> Result<usize, String> {
    let wanted = name_or_id.trim();
    if wanted.is_empty() {
        return Err("No agent named.".to_string());
    }
    if let Some(i) = agents.iter().position(|(id, _)| id == wanted) {
        return Ok(i);
    }
    if let Some(i) = agents.iter().position(|(_, name)| name == wanted) {
        return Ok(i);
    }
    let matches: Vec<usize> = agents
        .iter()
        .enumerate()
        .filter(|(_, (_, name))| name.eq_ignore_ascii_case(wanted))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => {
            let mut names: Vec<&str> = agents.iter().map(|(_, n)| n.as_str()).collect();
            names.sort_unstable();
            Err(format!(
                "No agent named '{wanted}'. Agents on this team: {}.",
                names.join(", ")
            ))
        }
        _ => Err(format!(
            "More than one agent is named '{wanted}' in different cases; use its id."
        )),
    }
}

/// Find an agent in the registry by id, exact name, or name in any case.
fn resolve_agent(state: &AppState, name_or_id: &str) -> Result<AgentEntry, String> {
    let all = state.kernel.registry.list();
    let pairs: Vec<(String, String)> = all
        .iter()
        .map(|e| (e.id.to_string(), e.name.clone()))
        .collect();
    let i = find_agent(&pairs, name_or_id)?;
    Ok(all[i].clone())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("Missing '{key}'."))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn refuse_if_frozen(state: &AppState) -> Result<(), String> {
    if state.frozen.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(
            "The system is frozen by the emergency stop. Nothing runs until the owner unfreezes it from the dashboard."
                .to_string(),
        );
    }
    Ok(())
}

// ── Identity and capabilities ──────────────────────────────────────────────

/// The FreEco tool an acting MCP tool stands in for. The acting agent's
/// manifest must grant it; tools absent here only read or talk, which any
/// team member may do.
fn required_capability(mcp_tool: &str) -> Option<&'static str> {
    match mcp_tool {
        "freeco_sandbox_exec" => Some("sandbox_exec"),
        "freeco_crm" => Some("crm_api"),
        "freeco_claim_task" => Some("task_claim"),
        "freeco_complete_task" => Some("task_complete"),
        "freeco_post_task" => Some("task_post"),
        _ => None,
    }
}

/// Whether a manifest's `capabilities.tools` grants `tool`.
///
/// The same comparison `tool_runner::execute_tool` enforces: both sides go
/// through `normalize_tool_name`, so a manifest that still lists an old name
/// (`docker_exec`) grants the tool now called `sandbox_exec`. `"*"` grants
/// everything. An empty list grants nothing here -- the kernel reads an empty
/// list as "all tools", but an external client gets only what is written down.
fn holds_tool(declared: &[String], tool: &str) -> bool {
    use freeco_types::tool_compat::normalize_tool_name;
    let wanted = normalize_tool_name(tool);
    declared
        .iter()
        .any(|t| t == "*" || normalize_tool_name(t.as_str()) == wanted)
}

/// Resolve the agent named by `X-Freeco-As-Agent`, before any tool runs.
/// Any existing agent may be named; a name that matches none is refused.
fn acting_agent(state: &AppState, caller: &Caller) -> Result<AgentEntry, String> {
    resolve_agent(state, &caller.as_agent).map_err(|e| {
        format!(
            "Cannot act as '{}' (from X-Freeco-As-Agent): {e}",
            caller.as_agent
        )
    })
}

async fn call_tool(
    state: &Arc<AppState>,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let me = acting_agent(state, caller)?;
    if let Some(tool) = required_capability(name) {
        if !holds_tool(&me.manifest.capabilities.tools, tool) {
            return Err(format!("agent {} does not hold tool {tool}", me.name));
        }
    }
    match name {
        "freeco_list_agents" => Ok(list_agents(state)),
        "freeco_list_tasks" => list_tasks(state, args).await,
        "freeco_post_task" => post_task(state, &me, args).await,
        "freeco_claim_task" => claim_task(state, &me, args).await,
        "freeco_complete_task" => complete_task(state, &me, args).await,
        "freeco_read_chat" => read_chat(state, args),
        "freeco_post_to_chat" => post_to_chat(state, caller, &me, args).await,
        "freeco_ask_agent" => ask_agent(state, caller, &me, args).await,
        "freeco_sandbox_exec" => sandbox_exec(state, &me, args).await,
        "freeco_crm" => crm(args).await,
        other => Err(format!("Unknown tool: {other}")),
    }
}

fn list_agents(state: &AppState) -> String {
    let mut agents = state.kernel.registry.list();
    agents.sort_by_key(|a| a.name.to_lowercase());
    let list: Vec<Value> = agents
        .iter()
        .map(|e| {
            json!({
                "id": e.id.to_string(),
                "name": e.name,
                "state": format!("{:?}", e.state),
                "model_provider": e.manifest.model.provider,
                "model": e.manifest.model.model,
            })
        })
        .collect();
    pretty(&Value::Array(list))
}

/// Agent ids on the board, with the names a reader needs beside them.
fn name_of(state: &AppState, id_or_name: &str) -> Option<String> {
    if id_or_name.is_empty() {
        return None;
    }
    id_or_name
        .parse::<AgentId>()
        .ok()
        .and_then(|id| state.kernel.registry.get(id))
        .map(|e| e.name)
}

async fn list_tasks(state: &AppState, args: &Value) -> Result<String, String> {
    let status = str_arg(args, "status");
    if let Some(s) = status {
        if !["pending", "in_progress", "completed"].contains(&s) {
            return Err(format!(
                "Unknown status '{s}'. Use pending, in_progress or completed."
            ));
        }
    }
    let mut tasks = state
        .kernel
        .memory
        .task_list(status)
        .await
        .map_err(|e| format!("Task list failed: {e}"))?;
    if tasks.is_empty() {
        return Ok("No tasks found.".to_string());
    }
    for t in &mut tasks {
        for field in ["assigned_to", "created_by"] {
            let raw = t[field].as_str().unwrap_or("").to_string();
            if let Some(n) = name_of(state, &raw) {
                t[format!("{field}_name")] = json!(n);
            }
        }
    }
    Ok(pretty(&Value::Array(tasks)))
}

async fn post_task(state: &AppState, me: &AgentEntry, args: &Value) -> Result<String, String> {
    let title = required(args, "title")?;
    let description = required(args, "description")?;
    // Stored as the agent's id: the board's claim matches assignees by id, so
    // a name here would leave the task unclaimable by the agent it names.
    let assignee = match str_arg(args, "assignee") {
        Some(a) => Some(resolve_agent(state, a)?),
        None => None,
    };
    let assignee_id = assignee.as_ref().map(|a| a.id.to_string());
    let task_id = state
        .kernel
        .memory
        .task_post(
            title,
            description,
            assignee_id.as_deref(),
            Some(me.id.to_string().as_str()),
        )
        .await
        .map_err(|e| format!("Task post failed: {e}"))?;
    Ok(pretty(&json!({
        "task_id": task_id,
        "title": title,
        "assigned_to": assignee.map(|a| a.name),
        "created_by": me.name,
    })))
}

async fn claim_task(state: &AppState, me: &AgentEntry, args: &Value) -> Result<String, String> {
    let my_id = me.id.to_string();
    match str_arg(args, "task_id") {
        None => match state
            .kernel
            .memory
            .task_claim(&my_id)
            .await
            .map_err(|e| format!("Task claim failed: {e}"))?
        {
            Some(task) => Ok(pretty(&task)),
            None => Ok(format!(
                "No pending task is assigned to {} or open to anyone.",
                me.name
            )),
        },
        Some(task_id) => claim_task_by_id(state, task_id, me)
            .await
            .map(|t| pretty(&t)),
    }
}

/// Claim one named task.
///
/// The board only has "claim the next task for this agent"; there is no claim
/// by id in the memory crate. This does the same update on the same table and
/// connection, guarded so it only takes a task that is still pending and is
/// either open or already assigned to this agent.
async fn claim_task_by_id(
    state: &AppState,
    task_id: &str,
    me: &AgentEntry,
) -> Result<Value, String> {
    let conn = state.kernel.memory.connection();
    let task_id = task_id.to_string();
    let my_id = me.id.to_string();
    let my_name = me.name.clone();
    tokio::task::spawn_blocking(move || {
        let db = conn
            .lock()
            .map_err(|e| format!("Task claim failed: {e}"))?;
        let row = db.query_row(
            "SELECT status, assigned_to, title, description, created_by, created_at
             FROM task_queue WHERE id = ?1",
            rusqlite::params![task_id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                ))
            },
        );
        let (status, assigned, title, description, created_by, created_at) = match row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(format!("No task with id {task_id}."))
            }
            Err(e) => return Err(format!("Task claim failed: {e}")),
        };
        if status != "pending" {
            return Err(format!(
                "Task {task_id} is {status}, not pending, so it cannot be claimed."
            ));
        }
        let mine = assigned.is_empty()
            || assigned == my_id
            || assigned.eq_ignore_ascii_case(&my_name);
        if !mine {
            return Err(format!(
                "Task {task_id} is assigned to another agent ({assigned}). Claim only open tasks or tasks assigned to {my_name}."
            ));
        }
        let changed = db
            .execute(
                "UPDATE task_queue SET status = 'in_progress', assigned_to = ?2
                 WHERE id = ?1 AND status = 'pending'",
                rusqlite::params![task_id, my_id],
            )
            .map_err(|e| format!("Task claim failed: {e}"))?;
        if changed == 0 {
            return Err(format!("Task {task_id} was claimed by someone else just now."));
        }
        Ok(json!({
            "id": task_id,
            "title": title,
            "description": description,
            "status": "in_progress",
            "assigned_to": my_id,
            "assigned_to_name": my_name,
            "created_by": created_by,
            "created_at": created_at,
        }))
    })
    .await
    .map_err(|e| format!("Task claim failed: {e}"))?
}

/// Complete a task -- only one this agent holds.
///
/// Claiming checked ownership and completing did not, so a client acting as
/// one agent could mark another agent's task done and overwrite its result.
async fn complete_task(state: &AppState, me: &AgentEntry, args: &Value) -> Result<String, String> {
    let task_id = required(args, "task_id")?;
    let result = required(args, "result")?;
    let conn = state.kernel.memory.connection();
    let (tid, my_id, my_name) = (task_id.to_string(), me.id.to_string(), me.name.clone());
    tokio::task::spawn_blocking(move || {
        let db = conn.lock().map_err(|e| format!("Task complete failed: {e}"))?;
        let row = db.query_row(
            "SELECT status, assigned_to FROM task_queue WHERE id = ?1",
            rusqlite::params![tid],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                ))
            },
        );
        let (status, assigned) = match row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(format!("No task with id {tid}.")),
            Err(e) => return Err(format!("Task complete failed: {e}")),
        };
        if status == "completed" {
            return Err(format!("Task {tid} is already completed."));
        }
        let mine = assigned == my_id || assigned.eq_ignore_ascii_case(&my_name);
        if !mine {
            return Err(format!(
                "Task {tid} is not held by {my_name}. Claim it first; only the agent holding a task can complete it."
            ));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("Task complete failed: {e}"))??;
    state
        .kernel
        .memory
        .task_complete(task_id, result)
        .await
        .map_err(|e| format!("Task complete failed: {e}"))?;
    Ok(format!("Task {task_id} marked as completed."))
}

fn read_chat(state: &AppState, args: &Value) -> Result<String, String> {
    let agent = resolve_agent(state, required(args, "agent")?)?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(READ_DEFAULT)
        .clamp(1, READ_MAX);
    let total = state.kernel.memory.canonical_len(agent.id).unwrap_or(0);
    let (_, messages) = state
        .kernel
        .memory
        .canonical_context(agent.id, Some(limit))
        .map_err(|e| format!("Could not read the conversation: {e}"))?;
    let items: Vec<Value> = messages
        .iter()
        .filter_map(|m| {
            let text = m.content.text_content();
            if text.trim().is_empty() {
                return None;
            }
            let role = match m.role {
                freeco_types::message::Role::User => "user",
                freeco_types::message::Role::Assistant => "assistant",
                freeco_types::message::Role::System => "system",
            };
            Some(json!({ "role": role, "text": text }))
        })
        .collect();
    Ok(pretty(&json!({
        "agent": agent.name,
        "total_messages": total,
        "messages": items,
    })))
}

/// The label on everything a client posts: which client, acting as which
/// agent. Always the acting agent, whatever conversation it is posted in.
fn chat_label(client: &str, acting_agent: &str) -> String {
    format!("[{client} as {acting_agent}] ")
}

async fn post_to_chat(
    state: &AppState,
    caller: &Caller,
    me: &AgentEntry,
    args: &Value,
) -> Result<String, String> {
    let agent = resolve_agent(state, required(args, "agent")?)?;
    let text = required(args, "text")?;
    if text.len() > MAX_MESSAGE_BYTES {
        return Err("Message too large (max 64 KB).".to_string());
    }
    let labelled = format!("{}{text}", chat_label(&caller.client, &me.name));
    state
        .kernel
        .memory
        .append_canonical(
            agent.id,
            &[freeco_types::message::Message::user(labelled.clone())],
            None,
        )
        .map_err(|e| format!("Could not post to the conversation: {e}"))?;
    // An open chat shows it at once; one opened later reads it from history.
    let live = crate::ws::broadcast_to_ws(
        agent.id,
        json!({ "type": "message", "content": labelled, "source": "mcp" }),
    )
    .await;
    Ok(format!(
        "Posted to {}'s conversation ({} open dashboard view(s) updated live).",
        agent.name, live
    ))
}

async fn ask_agent(
    state: &AppState,
    caller: &Caller,
    me: &AgentEntry,
    args: &Value,
) -> Result<String, String> {
    refuse_if_frozen(state)?;
    let target = resolve_agent(state, required(args, "agent")?)?;
    let message = required(args, "message")?.to_string();
    if message.len() > MAX_MESSAGE_BYTES {
        return Err("Message too large (max 64 KB).".to_string());
    }
    let sender_name = format!(
        "{} (external team member acting as {})",
        caller.client, me.name
    );
    let sender_id = format!("mcp:{}", me.id);
    let kernel = state.kernel.clone();
    let handle: Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle> =
        kernel.clone() as Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle>;
    let target_id = target.id;
    // Spawned so a timeout here stops the wait, not the agent's turn: its
    // reply still lands in its conversation.
    let turn = tokio::spawn(async move {
        kernel
            .send_message_with_handle_and_blocks(
                target_id,
                &message,
                Some(handle),
                None,
                Some(sender_id),
                Some(sender_name),
            )
            .await
            .map_err(|e| e.to_string())
    });
    match tokio::time::timeout(ASK_TIMEOUT, turn).await {
        Err(_) => Err(format!(
            "{} has not replied within 600 seconds. It is still working; read its reply later with freeco_read_chat.",
            target.name
        )),
        Ok(Err(e)) => Err(format!("{}'s turn failed: {e}", target.name)),
        Ok(Ok(Err(e))) => Err(format!("Message delivery failed: {e}")),
        Ok(Ok(Ok(result))) => {
            let cleaned = crate::ws::strip_think_tags(&result.response);
            if result.silent || cleaned.trim().is_empty() {
                Ok(format!("{} finished without a text reply.", target.name))
            } else {
                Ok(cleaned)
            }
        }
    }
}

async fn sandbox_exec(state: &AppState, me: &AgentEntry, args: &Value) -> Result<String, String> {
    refuse_if_frozen(state)?;
    let command = required(args, "command")?;
    let tier = str_arg(args, "tier").unwrap_or("workspace");
    if tier != "exec" && tier != "workspace" {
        return Err(format!("Unknown tier '{tier}'. Use 'exec' or 'workspace'."));
    }
    let workspace = me
        .manifest
        .workspace
        .clone()
        .ok_or_else(|| format!("{} has no workspace, so it has no sandbox.", me.name))?;
    if !state.kernel.config.docker.enabled {
        return Err(
            "The sandbox is not enabled in config.toml [docker]. Nothing was run.".to_string(),
        );
    }
    let input = json!({ "command": command, "tier": tier });
    let handle: Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle> =
        state.kernel.clone() as Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle>;
    let agent_id = me.id.to_string();
    freeco_kernel_runtime::tool_runner::tool_docker_exec(
        &input,
        Some(&state.kernel.config.docker),
        Some(workspace.as_path()),
        Some(agent_id.as_str()),
        Some(&handle),
    )
    .await
}

async fn crm(args: &Value) -> Result<String, String> {
    let mut input = json!({
        "method": args.get("method").and_then(Value::as_str).unwrap_or("GET"),
        "path": args.get("path").and_then(Value::as_str).unwrap_or(""),
    });
    if let Some(body) = args.get("body").filter(|b| !b.is_null()) {
        input["body"] = body.clone();
    }
    freeco_kernel_runtime::crm_tool::crm_api(&input).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(msg: Value) -> Value {
        match route(&msg) {
            Routed::Reply(v) => v,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_a_supported_version() {
        let r = reply(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }
        }));
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "freeco");
        assert_eq!(
            r["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_falls_back_to_the_preferred_version() {
        let r = reply(json!({
            "jsonrpc": "2.0", "id": "a", "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        }));
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn tools_list_has_all_ten_tools() {
        let r = reply(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 10);
        for expected in TOOL_NAMES {
            assert!(names.contains(expected), "{expected} missing");
        }
        for t in tools {
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn unknown_method_is_32601() {
        let r = reply(json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }));
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["id"], 3);
    }

    #[test]
    fn unknown_tool_is_a_protocol_error() {
        let r = reply(json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "rm_rf", "arguments": {} }
        }));
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn known_tool_is_routed_to_execution() {
        match route(&json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "freeco_list_agents" }
        })) {
            Routed::CallTool { id, name, args } => {
                assert_eq!(id, 5);
                assert_eq!(name, "freeco_list_agents");
                assert!(args.is_object());
            }
            other => panic!("expected CallTool, got {other:?}"),
        }
    }

    #[test]
    fn ping_answers_empty() {
        let r = reply(json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }));
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn notifications_get_202_and_no_body() {
        for method in ["notifications/initialized", "notifications/cancelled"] {
            assert!(matches!(
                route(&json!({ "jsonrpc": "2.0", "method": method })),
                Routed::NoReply
            ));
        }
        assert_eq!(finish(Vec::new(), false).status(), StatusCode::ACCEPTED);
        assert_eq!(finish(Vec::new(), true).status(), StatusCode::ACCEPTED);
    }

    #[test]
    fn a_request_gets_200() {
        let r = reply(json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }));
        assert_eq!(finish(vec![r], false).status(), StatusCode::OK);
    }

    #[test]
    fn tool_failures_are_results_with_is_error() {
        let v = tool_result(Err("nope".into()));
        assert_eq!(v["isError"], true);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "nope");
        assert_eq!(tool_result(Ok("ok".into()))["isError"], false);
    }

    #[test]
    fn bearer_refuses_everything_without_a_configured_token() {
        for configured in [None, Some(""), Some("   ")] {
            let e = check_bearer(configured, Some("Bearer anything")).unwrap_err();
            assert!(e.contains(TOKEN_ENV), "{e}");
            assert!(e.contains("secrets.env"), "{e}");
        }
    }

    #[test]
    fn bearer_missing_header_is_refused() {
        assert!(check_bearer(Some("s3cret"), None).is_err());
    }

    #[test]
    fn bearer_wrong_token_is_refused() {
        assert!(check_bearer(Some("s3cret"), Some("Bearer s3creT")).is_err());
        assert!(check_bearer(Some("s3cret"), Some("Bearer s3cre")).is_err());
        assert!(check_bearer(Some("s3cret"), Some("Basic s3cret")).is_err());
        assert!(check_bearer(Some("s3cret"), Some("s3cret")).is_err());
    }

    #[test]
    fn bearer_right_token_is_accepted() {
        assert!(check_bearer(Some("s3cret"), Some("Bearer s3cret")).is_ok());
        assert!(check_bearer(Some("s3cret"), Some("bearer s3cret")).is_ok());
    }

    #[test]
    fn client_labels_are_kept_plain() {
        assert_eq!(sanitize_client_label("Codex"), "Codex");
        assert_eq!(sanitize_client_label("<script>x</script>"), "scriptxscript");
        assert!(sanitize_client_label(&"a".repeat(100)).len() <= 32);
    }

    fn team() -> Vec<(String, String)> {
        vec![
            (
                "11111111-1111-4111-8111-111111111111".into(),
                "Developer".into(),
            ),
            (
                "22222222-2222-4222-8222-222222222222".into(),
                "FreEco.AI Executive Assistant, Concierge, CEO".into(),
            ),
            (
                "33333333-3333-4333-8333-333333333333".into(),
                "Tester".into(),
            ),
        ]
    }

    #[test]
    fn an_unknown_agent_is_refused() {
        let e = find_agent(&team(), "Nobody").unwrap_err();
        assert!(e.contains("No agent named 'Nobody'"), "{e}");
        assert!(e.contains("Tester"), "the error lists the team: {e}");
        assert!(find_agent(&team(), "").is_err());
        assert!(find_agent(&team(), "44444444-4444-4444-8444-444444444444").is_err());
    }

    #[test]
    fn an_existing_non_default_agent_is_accepted() {
        assert_eq!(find_agent(&team(), "Tester"), Ok(2));
        assert_eq!(find_agent(&team(), "tester"), Ok(2));
        assert_eq!(
            find_agent(&team(), "FreEco.AI Executive Assistant, Concierge, CEO"),
            Ok(1)
        );
        assert_eq!(
            find_agent(&team(), "22222222-2222-4222-8222-222222222222"),
            Ok(1)
        );
        assert_eq!(find_agent(&team(), DEFAULT_AGENT), Ok(0));
    }

    #[test]
    fn a_name_matching_several_agents_in_different_cases_is_refused() {
        let mut agents = team();
        agents.push((
            "55555555-5555-4555-8555-555555555555".into(),
            "TESTER".into(),
        ));
        // The exact spelling still finds its own agent...
        assert_eq!(find_agent(&agents, "Tester"), Ok(2));
        // ...but a third spelling is ambiguous.
        assert!(find_agent(&agents, "tester").is_err());
    }

    #[test]
    fn acting_tools_need_the_agents_own_capability() {
        assert_eq!(
            required_capability("freeco_sandbox_exec"),
            Some("sandbox_exec")
        );
        assert_eq!(required_capability("freeco_crm"), Some("crm_api"));
        assert_eq!(required_capability("freeco_claim_task"), Some("task_claim"));
        assert_eq!(
            required_capability("freeco_complete_task"),
            Some("task_complete")
        );
        assert_eq!(required_capability("freeco_post_task"), Some("task_post"));
        for talk in [
            "freeco_list_agents",
            "freeco_list_tasks",
            "freeco_read_chat",
            "freeco_post_to_chat",
            "freeco_ask_agent",
        ] {
            assert_eq!(required_capability(talk), None, "{talk}");
        }
    }

    #[test]
    fn capability_check_matches_tool_runner_normalization() {
        let legacy = vec!["docker_exec".to_string(), "file_read".to_string()];
        assert!(holds_tool(&legacy, "sandbox_exec"));
        assert!(!holds_tool(&legacy, "task_post"));
        assert!(!holds_tool(&legacy, "crm_api"));
        assert!(holds_tool(&["*".to_string()], "crm_api"));
        assert!(holds_tool(&["task_claim".to_string()], "task_claim"));
        assert!(!holds_tool(&[], "sandbox_exec"));
    }

    #[test]
    fn chat_posts_name_the_acting_agent() {
        assert_eq!(
            chat_label("Claude Code", "Developer"),
            "[Claude Code as Developer] "
        );
    }

    #[test]
    fn unset_token_is_refused() {
        assert!(check_bearer(None, None).is_err());
        assert!(check_bearer(None, Some("Bearer ")).is_err());
        assert!(check_bearer(Some(""), Some("Bearer ")).is_err());
    }

    #[tokio::test]
    async fn get_and_delete_are_405() {
        assert_eq!(
            method_not_allowed().await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}

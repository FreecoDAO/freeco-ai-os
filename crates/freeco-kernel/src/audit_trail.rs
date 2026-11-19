//! What the kernel writes to the audit trail about messages and tool calls.
//!
//! The audit trail recorded agent spawns, kills and the token counts of
//! messages sent through the non-streaming path, and nothing else agents
//! did. Chats in the dashboard (which stream), every tool call, and who a
//! message came from never reached it, so the Logs and Comms pages showed a
//! fraction of what happened. The pieces here fill that in.
//!
//! Message text and tool inputs are never written to the trail. It is
//! permanent and hash-chained, and both can hold personal data or secrets.
//! The trail says who, what kind, when and whether it worked; the
//! conversation itself stays in its session. A failure is written as its
//! kind ("LLM driver error", "Request failed"), never its full text, which
//! can quote a provider's response, a search query or a key.

use std::collections::HashMap;
use std::sync::Arc;

use freeco_kernel_runtime::audit::{AuditAction, AuditLog};
use freeco_kernel_runtime::hooks::{HookContext, HookHandler};
use freeco_types::agent::{AgentId, HookEvent};
use freeco_types::event::{AgentMessage, Event, EventPayload, EventTarget, MessageRole};
use freeco_types::message::TokenUsage;

/// Longest message preview put on the event bus for the Comms page.
const PREVIEW_BYTES: usize = 200;

/// Longest failure description kept in an audit outcome.
const OUTCOME_BYTES: usize = 80;

/// Make a value safe inside a `key=value, key=value` detail string.
fn detail_value(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            ',' | '=' | '\n' | '\r' => ' ',
            c => c,
        })
        .collect()
}

/// The audit detail for one message an agent handled: who it came from, when
/// known, and the tokens its turn used.
///
/// `from=` and `from_id=` come first and only when there is a sender, so an
/// entry without one reads exactly as entries always have.
pub fn message_detail(
    sender_id: Option<&str>,
    sender_name: Option<&str>,
    usage: &TokenUsage,
) -> String {
    let mut parts = Vec::new();
    if let Some(name) = sender_name.map(detail_value).filter(|s| !s.is_empty()) {
        parts.push(format!("from={name}"));
    }
    if let Some(id) = sender_id.map(detail_value).filter(|s| !s.is_empty()) {
        parts.push(format!("from_id={id}"));
    }
    parts.push(format!(
        "tokens_in={}, tokens_out={}",
        usage.input_tokens, usage.output_tokens
    ));
    parts.join(", ")
}

/// The audit detail for a message whose turn failed.
pub fn failed_message_detail(sender_id: Option<&str>, sender_name: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(name) = sender_name.map(detail_value).filter(|s| !s.is_empty()) {
        parts.push(format!("from={name}"));
    }
    if let Some(id) = sender_id.map(detail_value).filter(|s| !s.is_empty()) {
        parts.push(format!("from_id={id}"));
    }
    parts.push("agent loop failed".to_string());
    parts.join(", ")
}

/// Record one message an agent handled: the tokens its turn used, or the
/// error it ended with.
pub fn record_message(
    audit_log: &AuditLog,
    agent_id: AgentId,
    sender_id: Option<&str>,
    sender_name: Option<&str>,
    outcome: Result<&TokenUsage, String>,
) {
    match outcome {
        Ok(usage) => {
            audit_log.record(
                agent_id.to_string(),
                AuditAction::AgentMessage,
                message_detail(sender_id, sender_name, usage),
                "ok",
            );
        }
        Err(e) => {
            audit_log.record(
                agent_id.to_string(),
                AuditAction::AgentMessage,
                failed_message_detail(sender_id, sender_name),
                format!("error: {}", failure_kind(&e)),
            );
        }
    }
}

/// An agent-to-agent message as an event-bus record, for the Comms page and
/// its topology. Carries a short preview, never the whole text.
pub fn agent_message_event(from: AgentId, to: AgentId, content: &str, via: &str) -> Event {
    let mut metadata = HashMap::new();
    metadata.insert("via".to_string(), serde_json::json!(via));
    Event::new(
        from,
        EventTarget::Agent(to),
        EventPayload::Message(AgentMessage {
            content: freeco_types::truncate_str(content, PREVIEW_BYTES).to_string(),
            metadata,
            role: MessageRole::Agent,
        }),
    )
}

/// Which audit action a tool call is.
fn action_for_tool(tool: &str) -> AuditAction {
    match tool {
        "shell_exec" | "sandbox_exec" | "docker_exec" | "process_start" | "process_write" => {
            AuditAction::ShellExec
        }
        "file_read" | "file_write" | "file_list" | "create_directory" | "apply_patch" => {
            AuditAction::FileAccess
        }
        "memory_store"
        | "memory_recall"
        | "knowledge_add_entity"
        | "knowledge_add_relation"
        | "knowledge_query" => AuditAction::MemoryAccess,
        "web_fetch" | "web_search" | "a2a_discover" | "a2a_send" | "crm_api"
        | "accounting_api" => {
            AuditAction::NetworkAccess
        }
        t if t.starts_with("browser_") => AuditAction::NetworkAccess,
        _ => AuditAction::ToolInvoke,
    }
}

/// Whether one word of an error looks like a credential.
fn looks_secret(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    const MARKERS: [&str; 8] = [
        "sk-",
        "bearer",
        "token=",
        "key=",
        "password=",
        "secret=",
        "apikey",
        "api_key",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
        || (word.len() >= 32
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

/// Replace quoted text ('...', "...", `...`) with an ellipsis: it is what
/// errors quote back, such as a query, a path or a name. An apostrophe
/// inside a word ("can't") is kept.
fn without_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let apostrophe = c == '\''
            && i > 0
            && chars[i - 1].is_alphanumeric()
            && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
        if matches!(c, '\'' | '"' | '`') && !apostrophe {
            if let Some(close) = chars[i + 1..].iter().position(|&x| x == c) {
                out.push('\u{2026}');
                i += close + 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// What kind of failure an error is, safe for the permanent trail: its
/// first line up to the first ": ", with quoted text, URLs, paths,
/// addresses and anything that looks like a credential blanked.
///
/// Errors carry their detail after the colon -- "LLM driver error: <the
/// provider's response>", "Request failed: <url>" -- and that detail can
/// echo conversation text, a search query or a key. The trail cannot be
/// edited later, so it keeps the kind and the session keeps the rest.
pub fn failure_kind(error: &str) -> String {
    let first = error.lines().next().unwrap_or("").trim();
    let first = first
        .strip_prefix("Error:")
        .or_else(|| first.strip_prefix("error:"))
        .unwrap_or(first)
        .trim();
    let kind = first.split(": ").next().unwrap_or("").trim();
    let kind = without_quotes(kind);
    let mut cleaned: Vec<&str> = Vec::new();
    let mut after_bearer = false;
    for w in kind.split_whitespace() {
        // The word after "Bearer" is the token, whatever it looks like.
        let word = if after_bearer || looks_secret(w) || w.contains('@') {
            "<redacted>"
        } else if w.contains("://") {
            "<url>"
        } else if w.contains('/') || w.contains('\\') {
            "<path>"
        } else {
            w
        };
        after_bearer = w.eq_ignore_ascii_case("bearer");
        cleaned.push(word);
    }
    let out = freeco_types::truncate_str(&cleaned.join(" "), OUTCOME_BYTES).to_string();
    if out.is_empty() {
        "failed".to_string()
    } else {
        out
    }
}

/// Writes one audit entry for every tool call an agent finishes.
///
/// Registered on the kernel's hook registry, which both the streaming and
/// the non-streaming agent loops fire after each tool call. A call refused
/// for lack of the capability is recorded as a denied capability check.
pub struct ToolAuditHook {
    audit_log: Arc<AuditLog>,
}

impl ToolAuditHook {
    pub fn new(audit_log: Arc<AuditLog>) -> Self {
        Self { audit_log }
    }
}

impl HookHandler for ToolAuditHook {
    fn on_event(&self, ctx: &HookContext) -> Result<(), String> {
        if ctx.event != HookEvent::AfterToolCall {
            return Ok(());
        }
        let tool = ctx.data["tool_name"].as_str().unwrap_or("unknown");
        let is_error = ctx.data["is_error"].as_bool().unwrap_or(false);
        let content = ctx.data["result"].as_str().unwrap_or("");

        let (action, outcome) = if is_error && content.starts_with("Permission denied") {
            (
                AuditAction::CapabilityCheck,
                format!("denied: {}", failure_kind(content)),
            )
        } else if is_error {
            (
                action_for_tool(tool),
                format!("error: {}", failure_kind(content)),
            )
        } else {
            (action_for_tool(tool), "ok".to_string())
        };
        self.audit_log.record(
            ctx.agent_id,
            action,
            format!("tool={}", detail_value(tool)),
            outcome,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(i: u64, o: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: i,
            output_tokens: o,
        }
    }

    #[test]
    fn a_message_without_a_sender_reads_as_before() {
        assert_eq!(
            message_detail(None, None, &usage(12, 3)),
            "tokens_in=12, tokens_out=3"
        );
    }

    #[test]
    fn a_message_names_its_sender_safely() {
        assert_eq!(
            message_detail(Some("agent:42"), Some("Re,search=er"), &usage(1, 2)),
            "from=Re search er, from_id=agent:42, tokens_in=1, tokens_out=2"
        );
        assert_eq!(
            failed_message_detail(None, Some("Ana")),
            "from=Ana, agent loop failed"
        );
    }

    fn after_tool(agent: &str, tool: &str, result: &str, is_error: bool) -> HookContext<'static> {
        HookContext {
            agent_name: "a",
            agent_id: Box::leak(agent.to_string().into_boxed_str()),
            event: HookEvent::AfterToolCall,
            data: serde_json::json!({
                "tool_name": tool,
                "result": result,
                "is_error": is_error,
            }),
        }
    }

    #[test]
    fn every_finished_tool_call_is_recorded_without_its_input_or_output() {
        let log = Arc::new(AuditLog::new());
        let hook = ToolAuditHook::new(log.clone());
        hook.on_event(&after_tool("ag", "shell_exec", "secret output", false))
            .unwrap();
        hook.on_event(&after_tool(
            "ag",
            "web_fetch",
            "Request failed: 404\nbody with token=abc",
            true,
        ))
        .unwrap();
        hook.on_event(&after_tool(
            "ag",
            "file_write",
            "Permission denied: agent does not have capability to use tool 'file_write'",
            true,
        ))
        .unwrap();
        hook.on_event(&after_tool("ag", "mcp_github_search", "[]", false))
            .unwrap();

        let e = log.recent(10);
        assert_eq!(e.len(), 4);
        assert!(matches!(e[0].action, AuditAction::ShellExec));
        assert_eq!(e[0].detail, "tool=shell_exec");
        assert_eq!(e[0].outcome, "ok");
        assert!(matches!(e[1].action, AuditAction::NetworkAccess));
        assert_eq!(e[1].outcome, "error: Request failed");
        assert!(matches!(e[2].action, AuditAction::CapabilityCheck));
        assert_eq!(e[2].outcome, "denied: Permission denied");
        assert!(matches!(e[3].action, AuditAction::ToolInvoke));
        assert!(e.iter().all(|x| x.agent_id == "ag"));
        assert!(e.iter().all(|x| !x.outcome.contains("secret")));
        assert!(e.iter().all(|x| !x.outcome.contains("token=abc")));
    }

    #[test]
    fn a_failure_is_written_as_its_kind_without_its_detail() {
        // The detail after the colon is dropped, whatever it holds.
        assert_eq!(
            failure_kind("curl failed: Authorization: Bearer abc123 token=xyz"),
            "curl failed"
        );
        assert_eq!(
            failure_kind(
                r#"LLM driver error: API error 400: {"api_key": "abc123", "messages": "hi"}"#
            ),
            "LLM driver error"
        );
        assert_eq!(
            failure_kind("Error: Failed to resolve parent directory: No such file (os error 2)"),
            "Failed to resolve parent directory"
        );
        assert_eq!(failure_kind("Exit code 1: not found"), "Exit code 1");
        // Without a colon, quoted text, URLs, paths, addresses and keys go.
        assert_eq!(
            failure_kind("No results found for 'flights to my ex in Gstaad'."),
            "No results found for \u{2026}."
        );
        assert_eq!(
            failure_kind("can't open /home/ana/tax.pdf for ana@example.com"),
            "can't open <path> for <redacted>"
        );
        assert_eq!(
            failure_kind("bad key sk-live-1234 for https://api.example.com/v1?key=abc"),
            "bad key <redacted> for <redacted>"
        );
        assert_eq!(
            failure_kind("GET https://api.example.com/v1 timed out"),
            "GET <url> timed out"
        );
        assert_eq!(failure_kind(""), "failed");
    }

    #[test]
    fn a_failed_message_names_the_kind_of_failure_only() {
        let log = AuditLog::new();
        record_message(
            &log,
            AgentId::new(),
            None,
            None,
            Err("LLM driver error: API error 401: {\"error\": \"bad key sk-ant-123\"}".to_string()),
        );
        let e = log.recent(1);
        assert_eq!(e[0].outcome, "error: LLM driver error");
        assert_eq!(e[0].detail, "agent loop failed");
    }

    #[test]
    fn other_hook_events_write_nothing() {
        let log = Arc::new(AuditLog::new());
        let hook = ToolAuditHook::new(log.clone());
        let mut ctx = after_tool("ag", "shell_exec", "", false);
        ctx.event = HookEvent::BeforeToolCall;
        hook.on_event(&ctx).unwrap();
        assert!(log.is_empty());
    }

    #[test]
    fn the_bus_record_carries_a_preview_from_sender_to_receiver() {
        let from = AgentId::new();
        let to = AgentId::new();
        let long = "x".repeat(5000);
        let ev = agent_message_event(from, to, &long, "agent_send");
        assert_eq!(ev.source, from);
        assert!(matches!(ev.target, EventTarget::Agent(t) if t == to));
        match ev.payload {
            EventPayload::Message(m) => {
                assert_eq!(m.content.len(), PREVIEW_BYTES);
                assert_eq!(m.metadata["via"], "agent_send");
            }
            _ => panic!("not a message"),
        }
    }
}

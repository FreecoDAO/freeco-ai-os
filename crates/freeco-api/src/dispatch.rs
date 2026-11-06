//! The assistant popup's dispatcher: where does a new message belong?
//!
//! The popup used to deliver to its own agent's current conversation -- the
//! one any window last opened -- so a new task landed in whatever chat had
//! been used last, and related work was never found again. Its router only
//! asked "new topic or not", and its search knew one thread per agent.
//!
//! `POST /api/dispatch` looks at every agent and its recent conversations and
//! answers with one destination:
//!
//! - `here`     -- a follow-up: the conversation the popup is in
//! - `session`  -- an existing conversation where this work already lives
//! - `new_task` -- new work: a new conversation with the agent best suited
//! - `propose`  -- something to create first (project, company, team, hand);
//!   the popup asks, because those are tedious to undo
//!
//! Fails open to `here`: a housekeeping call that cannot reach a model must
//! never stop a message being sent.

use crate::routes::AppState;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use freeco_types::agent::AgentId;
use std::sync::Arc;

/// Conversations offered per agent, and in all.
const PER_AGENT: usize = 3;
const MAX_CONVERSATIONS: usize = 24;

#[derive(Debug, Clone)]
struct Agent {
    id: String,
    name: String,
    about: String,
}

#[derive(Debug, Clone)]
struct Conversation {
    agent: usize,
    session_id: String,
    topic: String,
}

/// The decision, as sent to the popup.
#[derive(Debug, PartialEq)]
struct Decision {
    action: &'static str,
    agent_id: Option<String>,
    agent_name: Option<String>,
    session_id: Option<String>,
    title: String,
    propose: Option<String>,
    reason: String,
}

impl Decision {
    fn here(reason: &str) -> Self {
        Decision {
            action: "here",
            agent_id: None,
            agent_name: None,
            session_id: None,
            title: String::new(),
            propose: None,
            reason: reason.to_string(),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "action": self.action,
            "agent_id": self.agent_id,
            "agent_name": self.agent_name,
            "session_id": self.session_id,
            "title": self.title,
            "propose": self.propose,
            "reason": self.reason,
        })
    }
}

const SYSTEM: &str = "You are the dispatcher of a personal AI office. A new message \
arrived in the assistant popup. Decide where it goes and answer with one line of JSON \
and nothing else: {\"go\":\"here\"|\"C<n>\"|\"A<n>\"|\"propose\",\"propose\":\"project\"|\
\"company\"|\"team\"|\"hand\"|null,\"title\":\"...\",\"reason\":\"...\"}.\n\
- here: it continues, corrects or follows up the popup's recent conversation. Short \
replies such as \"yes\", \"go on\", \"build it\" or \"why?\" are always here.\n\
- C<n>: it continues the work of that existing conversation.\n\
- A<n>: new work. Start a new conversation with that agent: a specialist when one \
clearly fits, otherwise the assistant/CEO.\n\
- propose: only when the user asks to set up a new project, company, team or \
automated hand, or the work clearly needs one; set propose accordingly.\n\
title: 2-5 words naming the task in the user's own words, like a heading. \
reason: one short sentence the user will read.";

/// For the CEO's popup: every destination is one of the CEO's own
/// conversations, or a new one with the CEO.
const SYSTEM_FOR_ONE: &str = "You are the CEO's dispatcher. A new message arrived in \
the CEO's popup. Decide which of the CEO's conversations it belongs in and answer with one \
line of JSON and nothing else: {\"go\":\"here\"|\"C<n>\"|\"A1\"|\"propose\",\"propose\":\
\"project\"|\"company\"|\"team\"|\"hand\"|null,\"title\":\"...\",\"reason\":\"...\"}.\n\
- here: it continues, corrects or follows up the popup's recent conversation. Short replies \
such as \"yes\", \"go on\", \"build it\" or \"why?\" are always here.\n\
- C<n>: it continues the work of that existing conversation of the CEO.\n\
- A1: new work -- start a new conversation with the CEO, named by title.\n\
- propose: only when the user asks to set up a new project, company, team or automated \
hand, or the work clearly needs one.\n\
title: 2-5 words naming the task in the user's own words, like a heading. \
reason: one short sentence the user will read.";

fn build_prompt(
    message: &str,
    context: &str,
    agents: &[Agent],
    conversations: &[Conversation],
) -> String {
    let mut p = String::new();
    p.push_str("Agents:\n");
    for (i, a) in agents.iter().enumerate() {
        p.push_str(&format!("A{}: {} -- {}\n", i + 1, a.name, a.about));
    }
    p.push_str("\nExisting conversations:\n");
    if conversations.is_empty() {
        p.push_str("(none)\n");
    }
    for (i, c) in conversations.iter().enumerate() {
        p.push_str(&format!(
            "C{}: with {} -- {}\n",
            i + 1,
            agents[c.agent].name,
            c.topic
        ));
    }
    p.push_str("\nThe popup's recent conversation:\n");
    p.push_str(if context.is_empty() {
        "(none)"
    } else {
        context
    });
    p.push_str("\n\nNew message: ");
    p.push_str(message);
    p
}

/// Turn the model's answer into a decision. Anything unusable is `here`.
fn decide(raw: &str, agents: &[Agent], conversations: &[Conversation]) -> Decision {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    // Models sometimes write a sentence around the JSON.
    let json = match (cleaned.find('{'), cleaned.rfind('}')) {
        (Some(a), Some(b)) if b > a => &cleaned[a..=b],
        _ => cleaned,
    };
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Decision::here("routing answer unreadable"),
    };
    let go = v["go"].as_str().unwrap_or("here").trim();
    let title: String = v["title"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(60)
        .collect();
    let reason: String = v["reason"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect();
    let index = |prefix: char| -> Option<usize> {
        go.strip_prefix(prefix)
            .or_else(|| go.strip_prefix(prefix.to_ascii_lowercase()))
            .and_then(|n| n.trim().parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
    };
    if let Some(c) = index('C').and_then(|i| conversations.get(i)) {
        let a = &agents[c.agent];
        return Decision {
            action: "session",
            agent_id: Some(a.id.clone()),
            agent_name: Some(a.name.clone()),
            session_id: Some(c.session_id.clone()),
            title: if title.is_empty() {
                c.topic.chars().take(60).collect()
            } else {
                title
            },
            propose: None,
            reason,
        };
    }
    if let Some(a) = index('A').and_then(|i| agents.get(i)) {
        return Decision {
            action: "new_task",
            agent_id: Some(a.id.clone()),
            agent_name: Some(a.name.clone()),
            session_id: None,
            title,
            propose: None,
            reason,
        };
    }
    if go.eq_ignore_ascii_case("propose") {
        let kind = v["propose"]
            .as_str()
            .filter(|k| ["project", "company", "team", "hand"].contains(k))
            .map(str::to_string);
        if kind.is_some() {
            return Decision {
                action: "propose",
                agent_id: None,
                agent_name: None,
                session_id: None,
                title,
                propose: kind,
                reason,
            };
        }
    }
    let mut d = Decision::here(&reason);
    d.title = title;
    d
}

/// POST /api/dispatch
///
/// Body: `{ "message": "...", "context": "recent popup turns",
///          "current_session_id": "..." }`
pub async fn dispatch(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let message = body["message"].as_str().unwrap_or("").trim().to_string();
    let context: String = body["context"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .rev()
        .take(2000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let current = body["current_session_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if message.is_empty() {
        return Json(Decision::here("empty message").to_json());
    }

    // The popup is the CEO: given its agent, only that agent's conversations
    // are destinations, and new work becomes a new conversation with it. The
    // CEO delegates to specialists itself. Routing straight to other agents
    // sent the owner's requests to whichever agent the model picked, and they
    // had to hunt for their own conversation afterwards.
    let only: Option<AgentId> = body["agent_id"].as_str().and_then(|s| s.parse().ok());
    let mut agents: Vec<Agent> = Vec::new();
    let mut ids: Vec<AgentId> = Vec::new();
    for e in state.kernel.registry.list() {
        if only.is_some_and(|o| o != e.id) {
            continue;
        }
        // Workers are copies of an agent, reached through it.
        if e.manifest
            .metadata
            .contains_key(freeco_kernel::kernel::WORKER_OF_KEY)
        {
            continue;
        }
        let about: String = e.manifest.description.chars().take(140).collect();
        agents.push(Agent {
            id: e.id.to_string(),
            name: e.name.clone(),
            about,
        });
        ids.push(e.id);
    }
    // With one agent, look further back through its own conversations.
    let per_agent = if only.is_some() {
        MAX_CONVERSATIONS
    } else {
        PER_AGENT
    };
    let mut conversations: Vec<Conversation> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let topics = state
            .kernel
            .memory
            .recent_session_topics(*id, per_agent + 1)
            .unwrap_or_default();
        for t in topics.into_iter().take(per_agent) {
            let sid = t["session_id"].as_str().unwrap_or("").to_string();
            // The popup's own conversation is "here", not somewhere else.
            if sid.is_empty() || sid == current || t["message_count"].as_u64() == Some(0) {
                continue;
            }
            let label = t["label"].as_str().unwrap_or("").trim();
            let last = t["last_user"].as_str().unwrap_or("").trim();
            let topic = match (label.is_empty(), last.is_empty()) {
                (false, false) => format!("\"{label}\" (last: {last})"),
                (false, true) => format!("\"{label}\""),
                (true, false) => format!("last: {last}"),
                (true, true) => continue,
            };
            conversations.push(Conversation {
                agent: i,
                session_id: sid,
                topic,
            });
        }
    }
    conversations.truncate(MAX_CONVERSATIONS);

    let prompt = build_prompt(&message, &context, &agents, &conversations);
    // Decided with the CEO's own model when routing for the CEO -- the popup
    // is the CEO and should think like it -- else with the default model.
    let answer = match only {
        Some(id) => {
            state
                .kernel
                .complete_once_as(id, SYSTEM_FOR_ONE, &prompt, 160)
                .await
        }
        None => state.kernel.complete_once(SYSTEM, &prompt, 160).await,
    };
    let decision = match answer {
        Ok(raw) => decide(&raw, &agents, &conversations),
        Err(e) => {
            tracing::warn!("dispatch unavailable, staying in this conversation: {e}");
            let mut d = Decision::here("routing unavailable");
            d.title = String::new();
            d
        }
    };
    Json(decision.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn office() -> (Vec<Agent>, Vec<Conversation>) {
        let agents = vec![
            Agent {
                id: "ceo".into(),
                name: "CEO".into(),
                about: "assistant".into(),
            },
            Agent {
                id: "crm".into(),
                name: "crm".into(),
                about: "customers".into(),
            },
        ];
        let conversations = vec![Conversation {
            agent: 0,
            session_id: "s-wa".into(),
            topic: "\"WhatsApp gateway\"".into(),
        }];
        (agents, conversations)
    }

    #[test]
    fn an_existing_conversation_is_named_with_its_agent() {
        let (a, c) = office();
        let d = decide(r#"{"go":"C1","title":"","reason":"same work"}"#, &a, &c);
        assert_eq!(d.action, "session");
        assert_eq!(d.session_id.as_deref(), Some("s-wa"));
        assert_eq!(d.agent_id.as_deref(), Some("ceo"));
    }

    #[test]
    fn new_work_goes_to_a_new_conversation_with_the_chosen_agent() {
        let (a, c) = office();
        let d = decide(
            "Sure: {\"go\":\"A2\",\"title\":\"Import leads\",\"reason\":\"CRM work\"}",
            &a,
            &c,
        );
        assert_eq!(d.action, "new_task");
        assert_eq!(d.agent_id.as_deref(), Some("crm"));
        assert_eq!(d.title, "Import leads");
    }

    #[test]
    fn anything_unusable_stays_here() {
        let (a, c) = office();
        assert_eq!(decide("no idea", &a, &c).action, "here");
        assert_eq!(decide(r#"{"go":"C9"}"#, &a, &c).action, "here");
        assert_eq!(
            decide(r#"{"go":"propose","propose":"galaxy"}"#, &a, &c).action,
            "here"
        );
        let d = decide(
            r#"{"go":"propose","propose":"project","title":"Grant"}"#,
            &a,
            &c,
        );
        assert_eq!(d.action, "propose");
        assert_eq!(d.propose.as_deref(), Some("project"));
    }
}

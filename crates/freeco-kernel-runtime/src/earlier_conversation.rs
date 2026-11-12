//! The part of a conversation the model is no longer sent still counts.
//!
//! An agent is sent only the newest messages of a conversation
//! (`max_history_messages`, 20 by default, tool calls and their results
//! included), so a few busy turns were enough to push the start of a chat out
//! of sight. The only summary meant to cover it was one per agent, cut to 500
//! characters and itself the first message the trim removed. A guest who gave
//! their name and room on turn one was asked for them again on turn eight.
//!
//! [`render`] turns the dropped messages into one note for the system prompt:
//!
//! - every dropped user message word for word, oldest first, within a budget
//!   scaled to the model's context window. When the budget is short, each
//!   message gets a fair share instead of the newest crowding out the rest;
//!   bare acknowledgements go first, then ordinary messages, and instructions
//!   ("always", "never", "from now on", "I prefer"...) last, because that is
//!   where standing requests live;
//! - the agent's own replies from that part as short heads, oldest first,
//!   with the tools each used, so it knows what it already did. When they do
//!   not all fit, the newest are kept and the rest are counted.
//!
//! No model call: it costs nothing, adds no delay, and the same history
//! renders the same note every turn, so nothing is paraphrased differently
//! from one turn to the next. Nothing stored is changed.

use crate::str_utils::safe_truncate_str;
use freeco_types::message::{ContentBlock, Message, MessageContent, Role};
use std::sync::OnceLock;

/// A reply head: enough to know what was answered, not the whole answer.
const REPLY_HEAD_CHARS: usize = 280;
/// Below this, a message is no longer worth carrying cut; older ones are
/// dropped instead, and counted.
const SHARE_FLOOR: usize = 240;

/// One user message with the replies and tools that followed it.
struct Exchange {
    user: Option<String>,
    replies: Vec<String>,
    tools: Vec<String>,
}

/// The words a user message carries; `None` for a tool result, which is the
/// agent's own work reported back, not something the user said.
fn user_words(message: &Message) -> Option<String> {
    let text = match &message.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => {
            if blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            {
                return None;
            }
            blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    ContentBlock::Image { .. } => Some("[image]"),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn exchanges(dropped: &[Message]) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    for message in dropped {
        match message.role {
            Role::System => {}
            Role::User => {
                if let Some(words) = user_words(message) {
                    out.push(Exchange {
                        user: Some(words),
                        replies: Vec::new(),
                        tools: Vec::new(),
                    });
                }
            }
            Role::Assistant => {
                if out.is_empty() {
                    out.push(Exchange {
                        user: None,
                        replies: Vec::new(),
                        tools: Vec::new(),
                    });
                }
                let current = out.last_mut().expect("pushed above");
                if let MessageContent::Blocks(blocks) = &message.content {
                    for block in blocks {
                        if let ContentBlock::ToolUse { name, .. } = block {
                            if !current.tools.contains(name) {
                                current.tools.push(name.clone());
                            }
                        }
                    }
                }
                let text = squash(&message.content.text_content());
                if !text.is_empty() {
                    current.replies.push(text);
                }
            }
        }
    }
    out
}

/// 2 for an instruction, 0 for a bare acknowledgement, 1 for anything else.
fn priority(text: &str) -> u8 {
    static RULE: OnceLock<regex_lite::Regex> = OnceLock::new();
    static ACK: OnceLock<regex_lite::Regex> = OnceLock::new();
    let rule = RULE.get_or_init(|| {
        regex_lite::Regex::new(
            r"(?i)\b(always|never|don't|dont|do not|must|make sure|ensure|from now on|going forward|next time|every time|remember|no more|only use|instead of|rather than|prefer|i want|i need|i like|i hate|i don't like|avoid|allergic|budget|deadline)\b",
        )
        .expect("valid pattern")
    });
    let ack = ACK.get_or_init(|| {
        regex_lite::Regex::new(
            r"(?i)^(ok|okay|k|thanks|thank you|thx|ty|cool|nice|great|good|perfect|yes|yep|yeah|no|nope|sure|continue|go on|go ahead|do it|proceed|next|done|got it|sounds good|looks good)[\s.!?,]*$",
        )
        .expect("valid pattern")
    });
    let t = text.trim();
    if t.is_empty() || ack.is_match(t) {
        0
    } else if rule.is_match(t) {
        2
    } else {
        1
    }
}

/// The user's messages within `budget` bytes: (carried, omitted count).
///
/// A short message is kept whole and a long one cut to its share, marked as
/// cut. Only when even [`SHARE_FLOOR`] each would not fit are messages
/// dropped: acknowledgements first, then the oldest ordinary ones, and
/// instructions only after both.
fn fit_user_messages(texts: Vec<String>, budget: usize) -> (Vec<String>, usize) {
    // An identical message repeated ("continue") is carried once, at its newest place.
    let mut items: Vec<String> = Vec::new();
    for text in texts.into_iter().rev() {
        if !items.contains(&text) {
            items.push(text);
        }
    }
    items.reverse();

    let total: usize = items.iter().map(String::len).sum();
    if total <= budget {
        return (items, 0);
    }

    let mut omitted = 0;
    while !items.is_empty() && items.len() * SHARE_FLOOR > budget {
        let drop_at = (0..=2u8)
            .find_map(|p| items.iter().position(|t| priority(t) == p))
            .unwrap_or(0);
        items.remove(drop_at);
        omitted += 1;
    }

    // Water-fill: the shortest are allotted first, and whatever a short
    // message does not use is shared among the longer ones.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| items[i].len());
    let mut allot = vec![0usize; items.len()];
    let mut remaining = budget;
    for (k, &i) in order.iter().enumerate() {
        let share = remaining / (items.len() - k);
        allot[i] = items[i].len().min(share);
        remaining -= allot[i];
    }
    let carried = items
        .iter()
        .zip(allot)
        .map(|(text, n)| {
            if n >= text.len() {
                text.clone()
            } else {
                format!(
                    "{}… [cut: first {} of {} characters]",
                    safe_truncate_str(text, n),
                    n,
                    text.len()
                )
            }
        })
        .collect();
    (carried, omitted)
}

fn reply_line(exchange: &Exchange) -> Option<String> {
    let reply = exchange.replies.last().map(String::as_str).unwrap_or("");
    let head = if reply.len() > REPLY_HEAD_CHARS {
        format!("{}…", safe_truncate_str(reply, REPLY_HEAD_CHARS - 1))
    } else {
        reply.to_string()
    };
    let used = if exchange.tools.is_empty() {
        String::new()
    } else {
        let mut names = exchange
            .tools
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if exchange.tools.len() > 6 {
            names.push_str(", …");
        }
        format!(" [used {names}]")
    };
    if head.is_empty() && used.is_empty() {
        return None;
    }
    let head = if head.is_empty() {
        "(no reply text)".to_string()
    } else {
        head
    };
    Some(format!("- {head}{used}"))
}

/// Budgets in bytes for (user messages, reply heads), scaled to the window.
fn budgets(context_window_tokens: usize) -> (usize, usize) {
    let chars = context_window_tokens.saturating_mul(4);
    (
        (chars * 3 / 100).clamp(2_000, 12_000),
        (chars * 15 / 1000).clamp(1_200, 6_000),
    )
}

/// The note for the messages a conversation's window dropped, or `None`
/// when they hold nothing worth carrying.
pub fn render(dropped: &[Message], context_window_tokens: usize) -> Option<String> {
    let exchanges = exchanges(dropped);
    if exchanges.is_empty() {
        return None;
    }
    let (user_budget, reply_budget) = budgets(context_window_tokens);

    let said: Vec<String> = exchanges.iter().filter_map(|e| e.user.clone()).collect();
    let said_count = said.len();
    let (carried, omitted_users) = fit_user_messages(said, user_budget);

    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    let mut omitted_replies = 0;
    for exchange in exchanges.iter().rev() {
        let Some(line) = reply_line(exchange) else {
            continue;
        };
        if used + line.len() + 1 > reply_budget {
            omitted_replies += 1;
            continue;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();

    if carried.is_empty() && lines.is_empty() {
        return None;
    }

    let mut note = String::from("## Earlier in this conversation\n");
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    note.push_str(&format!(
        "{} and your replies to them are no longer in the messages you are sent, so they are \
         carried here instead of being lost. The user's own words are below, verbatim and \
         oldest first: any instruction, preference, name, number or decision in them still \
         holds unless a later message changed it. Do not ask again for something answered here.\n",
        if said_count > 0 {
            plural(said_count, "An earlier message", "earlier messages")
        } else {
            "Earlier turns".to_string()
        }
    ));
    if !carried.is_empty() {
        note.push_str("\nWhat the user said:\n");
        if omitted_users > 0 {
            note.push_str(&format!(
                "- [{} not shown]\n",
                plural(omitted_users, "earlier message", "earlier messages")
            ));
        }
        for text in &carried {
            note.push_str("- ");
            note.push_str(&text.replace('\n', "\n  "));
            note.push('\n');
        }
    }
    if !lines.is_empty() {
        note.push_str("\nWhat you replied (shortened, oldest first):\n");
        if omitted_replies > 0 {
            note.push_str(&format!(
                "- [{} not shown]\n",
                plural(omitted_replies, "earlier reply", "earlier replies")
            ));
        }
        for line in &lines {
            note.push_str(line);
            note.push('\n');
        }
    }
    Some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: format!("call_{name}"),
                name: name.to_string(),
                input: serde_json::json!({}),
                provider_metadata: None,
            }]),
            ..Default::default()
        }
    }

    fn tool_result(text: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_x".to_string(),
                tool_name: "x".to_string(),
                content: text.to_string(),
                is_error: false,
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn nothing_dropped_is_no_note() {
        assert_eq!(render(&[], 128_000), None);
    }

    #[test]
    fn carries_what_the_user_said_word_for_word_and_what_was_done() {
        let dropped = vec![
            Message::user("My name is Ada Rossi, room 412. I'm allergic to nuts."),
            tool_call("calendar_create"),
            tool_result("SECRET-RESULT-TEXT"),
            Message::assistant("Booked the spa for 3 pm, Ada."),
            Message::user("ok"),
            Message::assistant("Anything else?"),
        ];
        let note = render(&dropped, 128_000).unwrap();
        assert!(note.contains("My name is Ada Rossi, room 412. I'm allergic to nuts."));
        assert!(note.contains("Booked the spa for 3 pm, Ada. [used calendar_create]"));
        assert!(note.contains("2 earlier messages"));
        // A tool result is the agent's own work, never presented as the user's words.
        assert!(!note.contains("SECRET-RESULT-TEXT"));
    }

    #[test]
    fn a_short_budget_keeps_instructions_and_drops_acknowledgements_first() {
        let mut texts = vec!["Never book anything before 9 am.".to_string()];
        for i in 0..30 {
            texts.push(format!("ordinary message number {i} {}", "x".repeat(300)));
            texts.push("thanks".to_string());
        }
        let (carried, omitted) = fit_user_messages(texts, 2_000);
        assert!(omitted > 0);
        assert!(carried
            .iter()
            .any(|t| t == "Never book anything before 9 am."));
        assert!(!carried.iter().any(|t| t == "thanks"));
        let total: usize = carried.iter().map(String::len).sum();
        // Within the budget, allowing for the cut markers.
        assert!(total <= 2_000 + carried.len() * 60, "{total}");
    }

    #[test]
    fn a_long_message_is_cut_to_its_share_and_says_so() {
        let long = "a".repeat(5_000);
        let (carried, omitted) =
            fit_user_messages(vec![long, "Keep it under CHF 200.".to_string()], 2_000);
        assert_eq!(omitted, 0);
        assert!(carried[0].contains("[cut: first"));
        assert_eq!(carried[1], "Keep it under CHF 200.");
    }

    #[test]
    fn the_note_stays_within_its_budget() {
        let mut dropped = Vec::new();
        for i in 0..200 {
            dropped.push(Message::user(format!("question {i} {}", "y".repeat(400))));
            dropped.push(Message::assistant(format!(
                "answer {i} {}",
                "z".repeat(400)
            )));
        }
        let note = render(&dropped, 8_000).unwrap();
        let (users, replies) = budgets(8_000);
        assert!(note.len() < users + replies + 2_000, "{}", note.len());
        assert!(note.contains("not shown"));
    }
}

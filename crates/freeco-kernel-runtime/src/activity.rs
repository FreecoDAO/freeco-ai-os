//! What every agent is doing right now.
//!
//! The dashboard's Live view shows "thinking", "running web_fetch for 6 s"
//! and "idle" as they happen. The audit log records what has finished; this
//! records what is still in progress. It listens to the agent loop's own
//! hooks (turn start, tool start, tool end, turn end), which fire in both the
//! streaming and the non-streaming loop, so the loop itself is unchanged.
//!
//! Nothing here is persisted and no tool input or output is kept: a step is
//! a kind, a tool name, two timestamps and whether it worked.

use crate::hooks::{HookContext, HookHandler};
use freeco_types::agent::HookEvent;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// Steps kept per agent, newest last. Enough for the Live view's strip and
/// timeline; older history is the audit log's job.
const MAX_STEPS: usize = 24;

/// A turn with no hook event for this long is taken as ended.
///
/// Some error exits leave the loop without firing `AgentLoopEnd` (a failed
/// LLM call, a blocked prompt). Without this an agent that failed once would
/// show as busy for ever. Ten minutes is longer than any single model call or
/// tool deadline, so a live agent is never cut short by it.
const STALE_MS: i64 = 10 * 60 * 1000;

/// Tools that hand work to another agent and wait for its answer. While one
/// runs, the agent is waiting on a helper rather than working itself.
const WAITING_TOOLS: [&str; 3] = ["agent_delegate", "agent_send", "a2a_send"];

/// What an agent is doing this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    /// No turn running.
    Idle,
    /// A turn is running and no tool is: the model is working.
    Thinking,
    /// A tool is running.
    Tool,
    /// The running tool hands work to another agent and waits for it.
    WaitingOnHelper,
}

/// One stretch of work: a model call or a tool call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    /// `"thinking"` or `"tool"`.
    pub kind: &'static str,
    /// The tool's name, for a tool step.
    pub name: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub started_ms: i64,
    /// `None` while the step is still running.
    pub ended_ms: Option<i64>,
    /// Whether the tool reported success. `None` for thinking, or a tool
    /// step that was cut off.
    pub ok: Option<bool>,
}

/// One agent's live state, as the API returns it.
#[derive(Debug, Clone, Serialize)]
pub struct AgentActivity {
    pub agent_id: String,
    pub agent_name: String,
    pub state: ActivityState,
    /// The tool running now, if any (the most recent when several are).
    pub tool: Option<String>,
    /// When the current state began, ms since the epoch. 0 if never active.
    pub since_ms: i64,
    /// Turns running at once. Above 1 when the agent works in several
    /// conversations in parallel.
    pub turns: u32,
    /// Recent steps, oldest first.
    pub steps: Vec<Step>,
}

#[derive(Default)]
struct Entry {
    agent_name: String,
    turns: u32,
    /// Tools in flight: name and start, oldest first.
    tools: Vec<(String, i64)>,
    last_event_ms: i64,
    steps: VecDeque<Step>,
}

impl Entry {
    fn push(&mut self, step: Step) {
        if self.steps.len() == MAX_STEPS {
            self.steps.pop_front();
        }
        self.steps.push_back(step);
    }

    /// Close the newest open step of `kind` (and `name`, when given).
    fn close(&mut self, kind: &str, name: Option<&str>, at: i64, ok: Option<bool>) {
        if let Some(step) = self.steps.iter_mut().rev().find(|s| {
            s.ended_ms.is_none()
                && s.kind == kind
                && name.is_none_or(|n| s.name.as_deref() == Some(n))
        }) {
            step.ended_ms = Some(at);
            step.ok = ok;
        }
    }

    fn close_all(&mut self, at: i64) {
        for s in self.steps.iter_mut().filter(|s| s.ended_ms.is_none()) {
            s.ended_ms = Some(at);
        }
        self.tools.clear();
        self.turns = 0;
    }

    fn open_thinking(&mut self, at: i64) {
        let already = self
            .steps
            .iter()
            .any(|s| s.ended_ms.is_none() && s.kind == "thinking");
        if !already {
            self.push(Step {
                kind: "thinking",
                name: None,
                started_ms: at,
                ended_ms: None,
                ok: None,
            });
        }
    }

    fn snapshot(&self, agent_id: &str) -> AgentActivity {
        let tool = self.tools.last().map(|(n, _)| n.clone());
        let state = match &tool {
            Some(n) if WAITING_TOOLS.contains(&n.as_str()) => ActivityState::WaitingOnHelper,
            Some(_) => ActivityState::Tool,
            None if self.turns > 0 => ActivityState::Thinking,
            None => ActivityState::Idle,
        };
        let since_ms = match state {
            ActivityState::Tool | ActivityState::WaitingOnHelper => {
                self.tools.last().map(|(_, t)| *t).unwrap_or(0)
            }
            ActivityState::Thinking => self
                .steps
                .iter()
                .rev()
                .find(|s| s.ended_ms.is_none() && s.kind == "thinking")
                .map(|s| s.started_ms)
                .unwrap_or(self.last_event_ms),
            ActivityState::Idle => self.steps.back().and_then(|s| s.ended_ms).unwrap_or(0),
        };
        AgentActivity {
            agent_id: agent_id.to_string(),
            agent_name: self.agent_name.clone(),
            state,
            tool,
            since_ms,
            turns: self.turns,
            steps: self.steps.iter().cloned().collect(),
        }
    }
}

/// Per-agent live state, fed by the agent loop's hooks.
#[derive(Default)]
pub struct ActivityTracker {
    agents: Mutex<HashMap<String, Entry>>,
}

impl ActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The hook events this tracker must be registered for.
    pub const EVENTS: [HookEvent; 4] = [
        HookEvent::BeforePromptBuild,
        HookEvent::BeforeToolCall,
        HookEvent::AfterToolCall,
        HookEvent::AgentLoopEnd,
    ];

    /// Record one hook event at `now_ms`.
    pub fn record(&self, ctx: &HookContext, now_ms: i64) {
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        let e = agents.entry(ctx.agent_id.to_string()).or_default();
        if !ctx.agent_name.is_empty() {
            e.agent_name = ctx.agent_name.to_string();
        }
        if e.turns > 0 && now_ms - e.last_event_ms > STALE_MS {
            let at = e.last_event_ms;
            e.close_all(at);
        }
        e.last_event_ms = now_ms;
        let tool = ctx.data["tool_name"].as_str().unwrap_or("").to_string();
        match ctx.event {
            HookEvent::BeforePromptBuild => {
                e.turns = e.turns.saturating_add(1);
                e.open_thinking(now_ms);
            }
            HookEvent::BeforeToolCall => {
                // A tool call outside a turn (a hook fired by a test or a
                // direct run) still counts as work in progress.
                if e.turns == 0 {
                    e.turns = 1;
                }
                e.close("thinking", None, now_ms, None);
                e.tools.push((tool.clone(), now_ms));
                e.push(Step {
                    kind: "tool",
                    name: Some(tool),
                    started_ms: now_ms,
                    ended_ms: None,
                    ok: None,
                });
            }
            HookEvent::AfterToolCall => {
                let ok = !ctx.data["is_error"].as_bool().unwrap_or(false);
                e.close("tool", Some(&tool), now_ms, Some(ok));
                if let Some(i) = e.tools.iter().rposition(|(n, _)| *n == tool) {
                    e.tools.remove(i);
                }
                if e.turns > 0 && e.tools.is_empty() {
                    e.open_thinking(now_ms);
                }
            }
            HookEvent::AgentLoopEnd => {
                e.turns = e.turns.saturating_sub(1);
                if e.turns == 0 {
                    e.close_all(now_ms);
                } else if e.tools.is_empty() {
                    // Another conversation of this agent is still running.
                    e.open_thinking(now_ms);
                }
            }
        }
    }

    /// Every agent seen so far, as of `now_ms`. An agent whose turn went
    /// quiet for longer than [`STALE_MS`] is reported idle.
    pub fn snapshot(&self, now_ms: i64) -> Vec<AgentActivity> {
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<AgentActivity> = agents
            .iter_mut()
            .map(|(id, e)| {
                if (e.turns > 0 || !e.tools.is_empty()) && now_ms - e.last_event_ms > STALE_MS {
                    let at = e.last_event_ms;
                    e.close_all(at);
                }
                e.snapshot(id)
            })
            .collect();
        out.sort_by(|a, b| a.agent_name.cmp(&b.agent_name));
        out
    }

    /// Drop an agent that no longer exists.
    pub fn forget(&self, agent_id: &str) {
        self.agents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(agent_id);
    }
}

impl HookHandler for ActivityTracker {
    fn on_event(&self, ctx: &HookContext) -> Result<(), String> {
        self.record(ctx, chrono::Utc::now().timestamp_millis());
        // Observe only: this must never block a tool call.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(event: HookEvent, tool: &str) -> HookContext<'static> {
        HookContext {
            agent_name: "Shopping",
            agent_id: "a1",
            event,
            data: serde_json::json!({ "tool_name": tool, "is_error": tool == "broken" }),
        }
    }

    fn one(t: &ActivityTracker, now: i64) -> AgentActivity {
        t.snapshot(now).pop().expect("one agent")
    }

    #[test]
    fn a_turn_goes_thinking_tool_thinking_idle() {
        let t = ActivityTracker::new();
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 1_000);
        assert_eq!(one(&t, 1_100).state, ActivityState::Thinking);

        t.record(&ctx(HookEvent::BeforeToolCall, "web_fetch"), 2_000);
        let a = one(&t, 2_500);
        assert_eq!(a.state, ActivityState::Tool);
        assert_eq!(a.tool.as_deref(), Some("web_fetch"));
        assert_eq!(a.since_ms, 2_000);

        t.record(&ctx(HookEvent::AfterToolCall, "web_fetch"), 3_000);
        assert_eq!(one(&t, 3_100).state, ActivityState::Thinking);

        t.record(&ctx(HookEvent::AgentLoopEnd, ""), 4_000);
        let a = one(&t, 4_100);
        assert_eq!(a.state, ActivityState::Idle);
        assert_eq!(a.since_ms, 4_000);
        let kinds: Vec<_> = a.steps.iter().map(|s| (s.kind, s.ended_ms)).collect();
        assert_eq!(
            kinds,
            vec![
                ("thinking", Some(2_000)),
                ("tool", Some(3_000)),
                ("thinking", Some(4_000))
            ]
        );
        assert_eq!(a.steps[1].ok, Some(true));
    }

    #[test]
    fn a_failed_tool_is_marked_and_delegation_is_waiting() {
        let t = ActivityTracker::new();
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 0);
        t.record(&ctx(HookEvent::BeforeToolCall, "broken"), 10);
        t.record(&ctx(HookEvent::AfterToolCall, "broken"), 20);
        assert_eq!(one(&t, 30).steps[1].ok, Some(false));

        t.record(&ctx(HookEvent::BeforeToolCall, "agent_delegate"), 40);
        assert_eq!(one(&t, 50).state, ActivityState::WaitingOnHelper);
    }

    /// An error exit skips `AgentLoopEnd`. The agent must not stay busy.
    #[test]
    fn a_turn_that_never_ends_goes_idle_after_the_stale_limit() {
        let t = ActivityTracker::new();
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 0);
        assert_eq!(one(&t, STALE_MS - 1).state, ActivityState::Thinking);
        let a = one(&t, STALE_MS + 1);
        assert_eq!(a.state, ActivityState::Idle);
        assert!(a.steps.iter().all(|s| s.ended_ms.is_some()));

        // And the next turn starts clean, not at two running turns.
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), STALE_MS + 10);
        t.record(&ctx(HookEvent::AgentLoopEnd, ""), STALE_MS + 20);
        assert_eq!(one(&t, STALE_MS + 30).state, ActivityState::Idle);
    }

    /// Two conversations of one agent at once: the first to finish must not
    /// mark the agent idle while the other is still running.
    #[test]
    fn parallel_turns_keep_the_agent_busy_until_both_end() {
        let t = ActivityTracker::new();
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 0);
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 5);
        assert_eq!(one(&t, 6).turns, 2);
        t.record(&ctx(HookEvent::AgentLoopEnd, ""), 10);
        assert_eq!(one(&t, 11).state, ActivityState::Thinking);
        t.record(&ctx(HookEvent::AgentLoopEnd, ""), 20);
        assert_eq!(one(&t, 21).state, ActivityState::Idle);
    }

    #[test]
    fn steps_are_capped() {
        let t = ActivityTracker::new();
        t.record(&ctx(HookEvent::BeforePromptBuild, ""), 0);
        for i in 0..100 {
            t.record(&ctx(HookEvent::BeforeToolCall, "web_search"), i * 10 + 1);
            t.record(&ctx(HookEvent::AfterToolCall, "web_search"), i * 10 + 2);
        }
        assert_eq!(one(&t, 2_000).steps.len(), MAX_STEPS);
    }

    #[test]
    fn the_tracker_never_blocks_a_tool() {
        let t = ActivityTracker::new();
        assert!(t
            .on_event(&ctx(HookEvent::BeforeToolCall, "shell_exec"))
            .is_ok());
    }
}

//! Conversations do not disappear: not on `/new`, not on a restart, not after
//! a crash.
//!
//! No model is called; these tests drive the session bookkeeping directly.

use freeco_kernel::FreecoKernel;
use freeco_types::agent::{AgentId, AgentManifest, SessionId};
use freeco_types::config::KernelConfig;
use freeco_types::message::Message;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn home(test: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "freeco-kept-{test}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn boot(home: &Path) -> FreecoKernel {
    let config = KernelConfig {
        home_dir: home.to_path_buf(),
        data_dir: home.join("data"),
        ..KernelConfig::default()
    };
    FreecoKernel::boot_with_config(config).expect("kernel boots")
}

fn spawn(kernel: &FreecoKernel, name: &str) -> AgentId {
    let manifest: AgentManifest = toml::from_str(&format!(
        r#"
name = "{name}"
module = "builtin:chat"
[model]
provider = "openai"
model = "gpt-4o-mini"
system_prompt = "test"
"#
    ))
    .unwrap();
    kernel.spawn_agent(manifest).unwrap()
}

/// Put a short exchange into a conversation.
fn talk(kernel: &FreecoKernel, session_id: SessionId, said: &str) {
    let mut session = kernel.memory.get_session(session_id).unwrap().unwrap();
    session.messages.push(Message::user(said));
    session.messages.push(Message::assistant("noted"));
    kernel.memory.save_session(&session).unwrap();
}

fn current(kernel: &FreecoKernel, agent: AgentId) -> SessionId {
    kernel.registry.get(agent).unwrap().session_id
}

/// `/new` -- from the chat, the WebSocket or a Telegram/WhatsApp bridge --
/// deleted the conversation the dashboard showed. It now starts another and
/// keeps the old one listed.
#[test]
fn a_new_conversation_keeps_the_old_one() {
    let home = home("new");
    let kernel = boot(&home);
    let agent = spawn(&kernel, "keeper");
    let old = current(&kernel, agent);
    talk(&kernel, old, "remember the blue folder");

    let new = kernel.start_new_session(agent).unwrap();
    assert_ne!(new, old);
    assert_eq!(current(&kernel, agent), new);
    let kept = kernel.memory.get_session(old).unwrap().expect("old kept");
    assert_eq!(kept.messages.len(), 2);
    let listed = kernel.list_agent_sessions(agent).unwrap();
    assert!(listed.iter().any(|s| s["session_id"] == old.0.to_string()));

    // Twice in a row does not pile up empty conversations.
    assert_eq!(kernel.start_new_session(agent).unwrap(), new);
}

/// A switch of conversation was saved only at a clean shutdown. After a
/// crash the agent came back on the conversation it had before.
#[test]
fn a_switched_conversation_survives_a_crash() {
    let home = home("crash");
    let agent;
    let switched;
    {
        let kernel = boot(&home);
        agent = spawn(&kernel, "survivor");
        let created = kernel
            .create_agent_session(agent, Some("second topic"))
            .unwrap();
        switched = SessionId(created["session_id"].as_str().unwrap().parse().unwrap());
        assert_eq!(current(&kernel, agent), switched);
        // No shutdown: the process dies here.
        std::mem::forget(kernel);
    }
    let kernel = boot(&home);
    assert_eq!(
        current(&kernel, agent),
        switched,
        "the agent comes back on the conversation it was switched to"
    );
}

/// A hand's agent is respawned at every boot. That deleted its
/// conversations and gave it a fresh empty one, so its chat came back blank.
#[test]
fn a_hand_keeps_its_conversation_across_a_restart() {
    let home = home("hand");
    let (agent, session);
    {
        let kernel = boot(&home);
        let instance = kernel
            .activate_hand("browser", HashMap::new(), None)
            .expect("browser hand activates");
        agent = instance.agent_id.unwrap();
        session = current(&kernel, agent);
        talk(&kernel, session, "check the shop page every morning");
        std::mem::forget(kernel);
    }
    let kernel = boot(&home);
    let instance = kernel
        .activate_hand("browser", HashMap::new(), None)
        .expect("browser hand activates again");
    let again = instance.agent_id.unwrap();
    assert_eq!(again, agent, "hands keep a stable id");
    assert_eq!(current(&kernel, again), session);
    let kept = kernel.memory.get_session(session).unwrap().expect("kept");
    assert_eq!(
        kept.messages[0].content.text_content(),
        "check the shop page every morning"
    );
    // The empty conversation the respawn made is not left behind.
    let listed = kernel.list_agent_sessions(again).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
}

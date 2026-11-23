//! Conversations run in parallel, and nothing typed into one is lost.
//!
//! Every test talks to a scripted, OpenAI-compatible model served on loopback,
//! so the whole kernel path runs -- registry, prompt, agent loop, tools,
//! session saves -- with no provider account and no network. The script keys
//! off a `MARK:<name>` line in each agent's system prompt, and every request
//! is timed, so a test can tell "ran at the same time" from "took turns".

use freeco_kernel::FreecoKernel;
use freeco_types::agent::{AgentId, AgentManifest};
use freeco_types::config::{DefaultModelConfig, KernelConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY_ENV: &str = "FREECO_PARALLEL_TEST_KEY";

/// One model request: whose it was, and when it started and ended.
#[derive(Debug, Clone)]
struct Call {
    who: String,
    start: Instant,
    end: Instant,
}

#[derive(Default)]
struct Script {
    calls: Mutex<Vec<Call>>,
    /// The system prompt of every request, by who sent it.
    prompts: Mutex<Vec<(String, String)>>,
    /// Every request body, by who sent it.
    bodies: Mutex<Vec<(String, serde_json::Value)>>,
}

impl Script {
    fn calls_of(&self, who: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.who == who)
            .cloned()
            .collect()
    }
}

fn overlaps(a: &Call, b: &Call) -> bool {
    a.start < b.end && b.start < a.end
}

/// What the scripted model answers, given who is asking and what it has seen.
fn reply_for(who: &str, body: &serde_json::Value) -> (Duration, serde_json::Value) {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let tool_result = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str().map(String::from));
    let last_user = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .and_then(|m| {
            m["content"].as_str().map(String::from).or_else(|| {
                m["content"].as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
        })
        .unwrap_or_default();

    let call = |name: &str, args: serde_json::Value| serde_json::json!({ "tool": { "name": name, "arguments": args.to_string() } });
    let text = |t: String| serde_json::json!({ "text": t });

    match (who, tool_result) {
        ("lead", None) => (
            Duration::from_millis(10),
            call(
                "agent_delegate",
                serde_json::json!({ "tasks": [
                    { "agent": "worker-a", "title": "alpha", "task": "Do alpha." },
                    { "agent": "worker-b", "title": "beta", "task": "Do beta." }
                ]}),
            ),
        ),
        ("lead", Some(result)) => (
            Duration::from_millis(10),
            text(format!("LEAD GOT {result}")),
        ),
        // Eight tasks whose answers are each far longer than a share of the
        // result: the worker echoes its prompt, task text included.
        ("wide", None) => (
            Duration::from_millis(10),
            call(
                "agent_delegate",
                serde_json::json!({ "tasks": (0..8).map(|i| serde_json::json!({
                    "agent": "worker-a",
                    "title": format!("part {i}"),
                    "task": format!("Part {i}. {}", "long ".repeat(3_000)),
                })).collect::<Vec<_>>() }),
            ),
        ),
        ("wide", Some(result)) => (
            Duration::from_millis(10),
            text(format!("WIDE GOT {result}")),
        ),
        ("ping", None) => (
            Duration::from_millis(10),
            call(
                "agent_send",
                serde_json::json!({ "agent_id": "pong", "message": "hello" }),
            ),
        ),
        ("ping", Some(result)) => (
            Duration::from_millis(10),
            text(format!("PING GOT {result}")),
        ),
        ("pong", None) => (
            Duration::from_millis(10),
            call(
                "agent_send",
                serde_json::json!({ "agent_id": "ping", "message": "asking you back" }),
            ),
        ),
        ("pong", Some(result)) => (
            Duration::from_millis(10),
            text(format!("PONG GOT {result}")),
        ),
        // Long chats: many quick turns.
        (w, _) if w.starts_with("fast") => (
            Duration::from_millis(10),
            text(format!("{who} answered: {last_user}")),
        ),
        // Workers and the echo agent: slow enough that overlap is unambiguous.
        (_, _) => (
            Duration::from_millis(600),
            text(format!("{who} answered: {last_user}")),
        ),
    }
}

fn completion_json(reply: &serde_json::Value) -> String {
    let message = if let Some(tool) = reply.get("tool") {
        serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": format!("call_{}", uuid::Uuid::new_v4().simple()),
                "type": "function",
                "function": { "name": tool["name"], "arguments": tool["arguments"] }
            }]
        })
    } else {
        serde_json::json!({ "role": "assistant", "content": reply["text"] })
    };
    let finish = if reply.get("tool").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 0,
        "model": "scripted",
        "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    })
    .to_string()
}

fn stream_body(reply: &serde_json::Value) -> String {
    let chunk = |delta: serde_json::Value, finish: Option<&str>| {
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "chatcmpl-test",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "scripted",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            })
        )
    };
    let mut out = String::new();
    if let Some(tool) = reply.get("tool") {
        out.push_str(&chunk(
            serde_json::json!({ "role": "assistant", "tool_calls": [{
                "index": 0,
                "id": format!("call_{}", uuid::Uuid::new_v4().simple()),
                "type": "function",
                "function": { "name": tool["name"], "arguments": tool["arguments"] }
            }]}),
            None,
        ));
        out.push_str(&chunk(serde_json::json!({}), Some("tool_calls")));
    } else {
        out.push_str(&chunk(
            serde_json::json!({ "role": "assistant", "content": reply["text"] }),
            None,
        ));
        out.push_str(&chunk(serde_json::json!({}), Some("stop")));
    }
    out.push_str(&format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "chatcmpl-test", "object": "chat.completion.chunk", "created": 0,
            "model": "scripted", "choices": [],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        })
    ));
    out.push_str("data: [DONE]\n\n");
    out
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<serde_json::Value> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + len {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&buf[header_end..header_end + len]).ok()
}

/// Serve the scripted model on a loopback port; returns its base URL.
async fn serve(script: Arc<Script>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let script = Arc::clone(&script);
            tokio::spawn(async move {
                let Some(body) = read_request(&mut stream).await else {
                    return;
                };
                let system = body["messages"]
                    .as_array()
                    .and_then(|m| m.iter().find(|m| m["role"] == "system"))
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let who = system
                    .split("MARK:")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("unknown")
                    .to_string();
                script
                    .prompts
                    .lock()
                    .unwrap()
                    .push((who.clone(), system.clone()));
                script
                    .bodies
                    .lock()
                    .unwrap()
                    .push((who.clone(), body.clone()));
                let start = Instant::now();
                // A provider that refuses the request, as a bad key, a blocked
                // prompt or an outage does.
                if who == "failing" {
                    let payload = r#"{"error":{"message":"scripted failure","type":"invalid_request_error"}}"#;
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    script.calls.lock().unwrap().push(Call {
                        who,
                        start,
                        end: Instant::now(),
                    });
                    return;
                }
                let (delay, reply) = reply_for(&who, &body);
                tokio::time::sleep(delay).await;
                let streaming = body["stream"].as_bool().unwrap_or(false);
                let (content_type, payload) = if streaming {
                    ("text/event-stream", stream_body(&reply))
                } else {
                    ("application/json", completion_json(&reply))
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                script.calls.lock().unwrap().push(Call {
                    who,
                    start,
                    end: Instant::now(),
                });
            });
        }
    });
    format!("http://{addr}/v1")
}

fn boot(test: &str) -> Arc<FreecoKernel> {
    boot_with(test, |_| {})
}

/// `boot`, with a change to the configuration first.
fn boot_with(test: &str, change: impl FnOnce(&mut KernelConfig)) -> Arc<FreecoKernel> {
    std::env::set_var(KEY_ENV, "sk-test");
    let home = std::env::temp_dir().join(format!(
        "freeco-parallel-{test}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&home).unwrap();
    let mut config = KernelConfig {
        home_dir: home.clone(),
        data_dir: home.join("data"),
        default_model: DefaultModelConfig {
            provider: "openai".to_string(),
            model: "scripted".to_string(),
            api_key_env: KEY_ENV.to_string(),
            base_url: None,
            subprocess_timeout_secs: None,
        },
        ..KernelConfig::default()
    };
    change(&mut config);
    let kernel = Arc::new(FreecoKernel::boot_with_config(config).expect("kernel boots"));
    kernel.set_self_handle();
    kernel
}

fn spawn(kernel: &FreecoKernel, name: &str, base_url: &str, tools: &[&str]) -> AgentId {
    spawn_with(kernel, name, base_url, tools, "")
}

/// `spawn`, with extra top-level manifest lines such as a tool profile.
fn spawn_with(
    kernel: &FreecoKernel,
    name: &str,
    base_url: &str,
    tools: &[&str],
    extra: &str,
) -> AgentId {
    let tools = tools
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest: AgentManifest = toml::from_str(&format!(
        r#"
name = "{name}"
module = "builtin:chat"
{extra}
[model]
provider = "openai"
model = "scripted"
base_url = "{base_url}"
api_key_env = "{KEY_ENV}"
system_prompt = "MARK:{name} You are a scripted test agent."
[capabilities]
tools = [{tools}]
agent_message = ["*"]
memory_read = ["*"]
memory_write = ["self.*"]
"#
    ))
    .expect("manifest parses");
    kernel.spawn_agent(manifest).expect("agent spawns")
}

fn user_texts(kernel: &FreecoKernel, session: freeco_types::agent::SessionId) -> Vec<String> {
    kernel
        .memory
        .get_session(session)
        .unwrap()
        .map(|s| {
            s.messages
                .iter()
                .filter(|m| m.role == freeco_types::message::Role::User)
                .map(|m| m.content.text_content())
                .collect()
        })
        .unwrap_or_default()
}

/// The bug: streaming took no lock, so a dashboard reply running while a
/// channel message arrived for the same conversation saved over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_conversation_takes_turns_and_keeps_every_message() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("turns");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let session = kernel.registry.get(agent).unwrap().session_id;

    let (mut rx, streamed) = kernel
        .send_message_streaming(agent, "from the dashboard", None, None, None, None)
        .expect("streaming starts");
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let k2 = Arc::clone(&kernel);
    let channel = tokio::spawn(async move { k2.send_message(agent, "from whatsapp").await });

    streamed.await.unwrap().expect("streamed run succeeds");
    channel.await.unwrap().expect("channel run succeeds");
    drain.await.unwrap();

    let texts = user_texts(&kernel, session);
    assert!(
        texts.iter().any(|t| t.contains("from the dashboard")),
        "dashboard message kept: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("from whatsapp")),
        "channel message kept: {texts:?}"
    );
    let calls = script.calls_of("echo");
    assert_eq!(calls.len(), 2, "one model call per message: {calls:?}");
    assert!(
        !overlaps(&calls[0], &calls[1]),
        "two runs in one conversation must take turns"
    );
    assert!(!kernel.is_conversation_busy(session));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_conversations_of_one_agent_run_at_the_same_time() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("sessions");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let current = kernel.registry.get(agent).unwrap().session_id;
    let other = kernel
        .memory
        .create_session_with_label(agent, Some("second chat"))
        .unwrap()
        .id;

    let (a, b) = tokio::join!(
        kernel.send_message_in_session(agent, current, "in the first chat", None, None),
        kernel.send_message_in_session(agent, other, "in the second chat", None, None),
    );
    a.expect("first conversation answers");
    b.expect("second conversation answers");

    let calls = script.calls_of("echo");
    assert_eq!(calls.len(), 2);
    assert!(
        overlaps(&calls[0], &calls[1]),
        "different conversations must not wait for each other"
    );
    assert_eq!(user_texts(&kernel, current), vec!["in the first chat"]);
    assert_eq!(user_texts(&kernel, other), vec!["in the second chat"]);
    assert_eq!(
        kernel.registry.get(agent).unwrap().session_id,
        current,
        "addressing a conversation must not switch the agent's current one"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegate_runs_tasks_in_parallel_and_files_them_under_the_project() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("delegate");
    // `agent_send` alone grants `agent_delegate`.
    let lead = spawn(&kernel, "lead", &url, &["agent_send"]);
    let worker_a = spawn(&kernel, "worker-a", &url, &[]);
    let worker_b = spawn(&kernel, "worker-b", &url, &[]);

    let org = kernel.memory.org();
    let project = org
        .create_project("Launch", None, Some("Ship the launch."))
        .unwrap();
    let lead_session = kernel.registry.get(lead).unwrap().session_id;
    org.assign_session(&lead_session.0.to_string(), Some(&project.id), None)
        .unwrap();

    let reply = tokio::time::timeout(
        Duration::from_secs(30),
        kernel.send_message(lead, "split this up"),
    )
    .await
    .expect("delegation finishes")
    .expect("lead run succeeds")
    .response;

    assert!(reply.contains("2 of 2 tasks finished"), "{reply}");
    assert!(reply.contains("worker-a answered"), "{reply}");
    assert!(reply.contains("worker-b answered"), "{reply}");

    let a = script.calls_of("worker-a");
    let b = script.calls_of("worker-b");
    assert_eq!((a.len(), b.len()), (1, 1));
    assert!(overlaps(&a[0], &b[0]), "delegated tasks must run at once");

    let filed = org.sessions_for_project(&project.id).unwrap();
    assert_eq!(filed.len(), 3, "the lead's chat and both tasks: {filed:?}");
    for worker in [worker_a, worker_b] {
        assert!(
            filed.iter().any(|s| s["agent_id"] == worker.to_string()
                && s["label"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Task from lead")),
            "{worker} task is filed under the project: {filed:?}"
        );
        // A task gets a conversation of its own; the worker's chat is untouched.
        let current = kernel.registry.get(worker).unwrap().session_id;
        assert!(user_texts(&kernel, current).is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn asking_back_up_the_chain_is_refused_not_deadlocked() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("cycle");
    let ping = spawn(&kernel, "ping", &url, &["agent_send"]);
    spawn(&kernel, "pong", &url, &["agent_send"]);

    let reply = tokio::time::timeout(Duration::from_secs(30), kernel.send_message(ping, "go"))
        .await
        .expect("a cycle must not hang")
        .expect("ping run succeeds")
        .response;
    assert!(reply.contains("PING GOT"), "{reply}");
    assert!(reply.contains("already busy"), "{reply}");
}

/// A chat names its conversation. Another agent's is refused; one reset or
/// deleted in another window falls back to the current one, so what the user
/// typed is still answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_addressed_conversation_is_checked_before_use() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("addressed");
    let mine = spawn(&kernel, "echo", &url, &[]);
    let theirs = spawn(&kernel, "other", &url, &[]);
    let their_session = kernel.registry.get(theirs).unwrap().session_id;

    let refused = kernel
        .send_message_in_session(mine, their_session, "not yours", None, None)
        .await;
    assert!(refused.is_err(), "another agent's conversation is refused");
    assert!(user_texts(&kernel, their_session).is_empty());

    let gone = kernel
        .memory
        .create_session_with_label(mine, Some("short-lived"))
        .unwrap()
        .id;
    kernel.memory.delete_session(gone).unwrap();
    kernel
        .send_message_in_session(mine, gone, "after a reset", None, None)
        .await
        .expect("a vanished conversation falls back to the current one");
    let current = kernel.registry.get(mine).unwrap().session_id;
    assert_eq!(user_texts(&kernel, current), vec!["after a reset"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_waiting_on_a_reset_conversation_goes_to_the_new_one() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("reset-wait");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let old = kernel.registry.get(agent).unwrap().session_id;

    let k1 = Arc::clone(&kernel);
    let first = tokio::spawn(async move { k1.send_message(agent, "first").await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let k2 = Arc::clone(&kernel);
    let second = tokio::spawn(async move {
        k2.send_message_in_session(agent, old, "second", None, None)
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    // The user starts over while the first reply runs and the second waits.
    kernel.reset_session(agent).unwrap();
    let new = kernel.registry.get(agent).unwrap().session_id;
    assert_ne!(new, old);

    first.await.unwrap().expect("the running reply finishes");
    second
        .await
        .unwrap()
        .expect("the waiting message is still answered");
    assert_eq!(
        user_texts(&kernel, new),
        vec!["second"],
        "the waiting message goes to the conversation the agent is in now"
    );
    assert!(
        !user_texts(&kernel, old).iter().any(|t| t == "second"),
        "it is not saved into the conversation that was reset"
    );

    // Addressed to the reset conversation afterwards, a message is redirected
    // too, even though the first reply saved that conversation again.
    kernel
        .send_message_in_session(agent, old, "third", None, None)
        .await
        .unwrap();
    assert_eq!(user_texts(&kernel, new), vec!["second", "third"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_message_waiting_on_a_reset_conversation_says_so() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("reset-stream");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let old = kernel.registry.get(agent).unwrap().session_id;

    let k1 = Arc::clone(&kernel);
    let first = tokio::spawn(async move { k1.send_message(agent, "first").await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (mut rx, streamed) = kernel
        .send_message_streaming_in_session(agent, old, "streamed", None, None)
        .expect("streaming starts");
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tokio::time::sleep(Duration::from_millis(150)).await;
    kernel.reset_session(agent).unwrap();
    let new = kernel.registry.get(agent).unwrap().session_id;

    first.await.unwrap().unwrap();
    let err = streamed
        .await
        .unwrap()
        .expect_err("the sender is told, not answered in a deleted chat");
    assert!(
        err.to_string()
            .contains(freeco_kernel::kernel::CONVERSATION_RESET_WHILE_WAITING),
        "{err}"
    );
    drain.await.unwrap();
    assert!(!user_texts(&kernel, old).iter().any(|t| t == "streamed"));
    assert!(user_texts(&kernel, new).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pinned_message_keeps_its_sender() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("sender");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let session = kernel.registry.get(agent).unwrap().session_id;

    kernel
        .send_message_in_session_from(
            agent,
            session,
            "hello",
            None,
            None,
            Some("+41790000000".to_string()),
            Some("Guest Room 12".to_string()),
        )
        .await
        .unwrap();
    let prompts = script.prompts.lock().unwrap().clone();
    assert!(
        prompts
            .iter()
            .any(|(who, p)| who == "echo" && p.contains("Message from: Guest Room 12")),
        "the sender reaches the prompt: {prompts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_messaging_profile_can_delegate() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("profile");
    // No tool list: the tools come from the profile, which has `agent_send`.
    let lead = spawn_with(&kernel, "lead", &url, &[], "profile = \"messaging\"");
    spawn(&kernel, "worker-a", &url, &[]);
    spawn(&kernel, "worker-b", &url, &[]);

    let reply = tokio::time::timeout(
        Duration::from_secs(30),
        kernel.send_message(lead, "split this up"),
    )
    .await
    .expect("delegation finishes")
    .expect("lead run succeeds")
    .response;
    assert!(reply.contains("2 of 2 tasks finished"), "{reply}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eight_long_answers_all_come_back_whole() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("wide");
    let lead = spawn(&kernel, "wide", &url, &["agent_send"]);
    spawn(&kernel, "worker-a", &url, &[]);

    let reply = tokio::time::timeout(
        Duration::from_secs(60),
        kernel.send_message(lead, "eight parts"),
    )
    .await
    .expect("delegation finishes")
    .expect("lead run succeeds")
    .response;
    let json = reply
        .strip_prefix("WIDE GOT ")
        .expect("the lead saw the result");
    let result: serde_json::Value =
        serde_json::from_str(json).expect("the result reaches the lead as whole JSON");
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 8, "every task is in the result");
    for r in results {
        assert_eq!(r["ok"], true, "{r}");
        assert!(r["conversation"].as_str().is_some(), "{r}");
        assert!(r["reply"].as_str().unwrap().contains("cut here"), "{r}");
    }
    assert!(json.len() < 30_000, "{} bytes", json.len());
}

/// The last system prompt sent for `who`.
fn last_prompt(script: &Script, who: &str) -> String {
    script
        .prompts
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(w, _)| w == who)
        .map(|(_, p)| p.clone())
        .unwrap_or_default()
}

/// A reply that fails must not take the user's message with it: it showed in
/// the chat while typed and was gone on reload, and the agent never knew.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_reply_keeps_the_message() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("failed");
    let agent = spawn(&kernel, "failing", &url, &[]);
    let session = kernel.registry.get(agent).unwrap().session_id;

    let err = kernel
        .send_message(agent, "please book the 7 pm table")
        .await;
    assert!(err.is_err(), "the scripted provider refuses");

    let (mut rx, streamed) = kernel
        .send_message_streaming(agent, "and a taxi at 6:30", None, None, None, None)
        .expect("streaming starts");
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    assert!(
        streamed.await.unwrap().is_err(),
        "the streamed reply fails too"
    );
    drain.await.unwrap();

    let texts = user_texts(&kernel, session);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("please book the 7 pm table")),
        "the message of a failed reply is kept: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("and a taxi at 6:30")),
        "the message of a failed streamed reply is kept: {texts:?}"
    );
}

/// Stop ends the run before anything was saved at the end of it; the
/// message it was answering stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_reply_keeps_the_message() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("stopped");
    let agent = spawn(&kernel, "echo", &url, &[]);
    let session = kernel.registry.get(agent).unwrap().session_id;

    let (mut rx, streamed) = kernel
        .send_message_streaming(
            agent,
            "write the welcome note for room 12",
            None,
            None,
            None,
            None,
        )
        .expect("streaming starts");
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    // Stop once the model has been asked: the kernel saves the message before
    // that call, and the scripted reply then takes 600 ms, so the run is
    // certainly still going. A fixed sleep raced the run's setup on slow
    // runners (Windows CI) and stopped it before the message was saved.
    let asked = Instant::now();
    while bodies_of(&script, "echo", 0).is_empty() {
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "the model was never asked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(kernel.stop_agent_run(agent).unwrap(), "a run was going");
    let _ = streamed.await;
    drain.await.unwrap();

    let texts = user_texts(&kernel, session);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("write the welcome note for room 12")),
        "the message of a stopped reply is kept: {texts:?}"
    );
}

/// The model is sent only the newest messages. What drops out of that window
/// reaches it as a note, the user's words verbatim, so a long chat does not
/// forget its start, and no extra model call is made for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_chat_keeps_its_start_in_view() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("longchat");
    let agent = spawn_with(&kernel, "fast-chat", &url, &[], "max_history_messages = 6");
    let session = kernel.registry.get(agent).unwrap().session_id;

    kernel
        .send_message(
            agent,
            "My name is Ada Rossi and my room is 412. Never call before 9 am.",
        )
        .await
        .expect("first turn");
    for i in 0..7 {
        kernel
            .send_message(agent, &format!("question {i}"))
            .await
            .expect("later turn");
    }

    let prompt = last_prompt(&script, "fast-chat");
    assert!(
        prompt.contains("## Earlier in this conversation"),
        "the dropped part is carried: {prompt}"
    );
    assert!(
        prompt.contains("My name is Ada Rossi and my room is 412. Never call before 9 am."),
        "the user's words are carried verbatim: {prompt}"
    );
    assert!(
        prompt.contains("fast-chat answered: My name is Ada Rossi"),
        "the agent's own reply is carried in short: {prompt}"
    );
    // The newest messages are still sent as messages, not repeated in the note.
    assert!(!prompt.contains("- question 6"), "{prompt}");
    // One model call per turn: carrying the start costs nothing extra.
    assert_eq!(script.calls_of("fast-chat").len(), 8);
    assert_eq!(script.calls_of("unknown").len(), 0, "no summarising calls");
    // And the stored conversation is whole.
    assert_eq!(user_texts(&kernel, session).len(), 8);
}

/// A team runs like a lead with teammates: the lead gets the goal and its
/// team by name, hands parts to the members, and they work at once, each in
/// a conversation filed under the team.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_team_run_hands_work_to_its_members() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("team");
    let lead = spawn(&kernel, "lead", &url, &["agent_send"]);
    let worker_a = spawn(&kernel, "worker-a", &url, &["agent_send"]);
    let worker_b = spawn(&kernel, "worker-b", &url, &[]);

    let org = kernel.memory.org();
    let project = org
        .create_project("Launch", None, Some("Ship the launch."))
        .unwrap();
    let team = org.create_team("Launch crew", Some(&project.id)).unwrap();
    // Joined first, but the role says who leads.
    org.add_team_member(&team.id, &worker_a.to_string(), Some("research"))
        .unwrap();
    org.add_team_member(&team.id, &lead.to_string(), Some("lead"))
        .unwrap();
    org.add_team_member(&team.id, &worker_b.to_string(), None)
        .unwrap();

    let run = kernel
        .start_team_run(&team.id, "Prepare the launch plan.")
        .expect("team run starts");
    assert_eq!(run["agent_id"], lead.to_string());
    let lead_session = freeco_types::agent::SessionId(
        uuid::Uuid::parse_str(run["session_id"].as_str().unwrap()).unwrap(),
    );

    // The run goes on in the background; wait for the lead's answer.
    let deadline = Instant::now() + Duration::from_secs(30);
    let answer = loop {
        let session = kernel.memory.get_session(lead_session).unwrap();
        let last = session.and_then(|s| {
            s.messages
                .iter()
                .rev()
                .find(|m| m.role == freeco_types::message::Role::Assistant)
                .map(|m| m.content.text_content())
        });
        if let Some(text) = last.filter(|t| t.contains("LEAD GOT")) {
            break text;
        }
        assert!(Instant::now() < deadline, "the team run finishes");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(answer.contains("worker-a answered"), "{answer}");
    assert!(answer.contains("worker-b answered"), "{answer}");

    let lead_prompt = last_prompt(&script, "lead");
    assert!(
        lead_prompt.contains("Team \"Launch crew\""),
        "{lead_prompt}"
    );
    assert!(lead_prompt.contains("You lead this team"), "{lead_prompt}");
    assert!(
        lead_prompt.contains("- worker-a — research"),
        "{lead_prompt}"
    );
    assert!(lead_prompt.contains("Ship the launch."), "{lead_prompt}");
    let kickoff = user_texts(&kernel, lead_session);
    assert!(
        kickoff[0].contains("Prepare the launch plan.")
            && kickoff[0].contains("worker-a, worker-b"),
        "{kickoff:?}"
    );

    // Members are told who leads and who their teammates are.
    let worker_prompt = last_prompt(&script, "worker-a");
    assert!(
        worker_prompt.contains("lead leads this team"),
        "{worker_prompt}"
    );
    assert!(worker_prompt.contains("- worker-b"), "{worker_prompt}");

    let a = script.calls_of("worker-a");
    let b = script.calls_of("worker-b");
    assert_eq!((a.len(), b.len()), (1, 1));
    assert!(overlaps(&a[0], &b[0]), "members work at the same time");

    let filed = org.sessions_for_project(&project.id).unwrap();
    assert_eq!(filed.len(), 3, "the lead's chat and both tasks: {filed:?}");
    for s in &filed {
        let scope = org
            .session_scope(s["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            scope.1.as_deref(),
            Some(team.id.as_str()),
            "filed under the team: {s}"
        );
    }
    // Running a team never pulls the lead out of the chat it was in.
    assert_ne!(kernel.registry.get(lead).unwrap().session_id, lead_session);
}

/// The Teams page draws a line for every hand-off, so each one must be on
/// record with who asked whom and how it ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_hand_off_is_recorded_with_how_it_ended() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("handoffs");
    let lead = spawn(&kernel, "lead", &url, &["agent_send"]);
    spawn(&kernel, "worker-a", &url, &[]);
    spawn(&kernel, "worker-b", &url, &[]);

    tokio::time::timeout(
        Duration::from_secs(30),
        kernel.send_message(lead, "split this up"),
    )
    .await
    .expect("delegation finishes")
    .expect("lead run succeeds");

    let trail = kernel.recent_handoffs(10);
    assert_eq!(trail.len(), 2, "{trail:?}");
    for h in &trail {
        assert_eq!(h.kind, "delegate");
        assert_eq!(h.status, "done", "{h:?}");
        assert_eq!(h.from_name.as_deref(), Some("lead"));
        assert_eq!(h.from_agent.as_deref(), Some(lead.to_string().as_str()));
        assert!(h.finished_at.is_some());
        assert!(h.session_id.is_some(), "a task has its own conversation");
    }
    let mut to: Vec<&str> = trail.iter().map(|h| h.to_name.as_str()).collect();
    to.sort();
    assert_eq!(to, vec!["worker-a", "worker-b"]);
    assert!(
        kernel.busy_conversations().is_empty(),
        "nothing is working now"
    );
}

/// A colleague's message must not read like the user typing: the recipient
/// is told who sent it and that it carries no approval of the user's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_from_another_agent_says_it_is_not_from_the_user() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("peer-mark");
    let ping = spawn(&kernel, "ping", &url, &["agent_send"]);
    let pong = spawn(&kernel, "pong", &url, &["agent_send"]);

    tokio::time::timeout(Duration::from_secs(30), kernel.send_message(ping, "go"))
        .await
        .expect("a cycle must not hang")
        .expect("ping run succeeds");

    let pong_session = kernel.registry.get(pong).unwrap().session_id;
    let received = user_texts(&kernel, pong_session);
    assert!(
        received
            .iter()
            .any(|t| t.contains("Message from the agent \"ping\"")
                && t.contains("not from the user")
                && t.contains("hello")),
        "{received:?}"
    );
    // What the user typed to ping stays unmarked.
    let ping_session = kernel.registry.get(ping).unwrap().session_id;
    let typed = user_texts(&kernel, ping_session);
    assert!(typed.iter().any(|t| t == "go"), "{typed:?}");
    assert!(
        !typed.iter().any(|t| t.contains("not from the user")),
        "{typed:?}"
    );

    // ping -> pong finished; pong asking back up the chain was refused.
    let trail = kernel.recent_handoffs(10);
    let sent = trail
        .iter()
        .find(|h| h.from_name.as_deref() == Some("ping"))
        .expect("ping's message is on record");
    assert_eq!((sent.kind, sent.status), ("message", "done"));
    let back = trail
        .iter()
        .find(|h| h.from_name.as_deref() == Some("pong"))
        .expect("pong's attempt is on record");
    assert_eq!(back.status, "failed", "{back:?}");
}

/// The tool names one request offered the model.
fn offered_tools(body: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The requests `who` sent from position `from` in the record on.
fn bodies_of(script: &Script, who: &str, from: usize) -> Vec<serde_json::Value> {
    script.bodies.lock().unwrap()[from..]
        .iter()
        .filter(|(w, _)| w == who)
        .map(|(_, b)| b.clone())
        .collect()
}

/// Write one of the owner's private files into an agent's state directory.
fn write_owner_file(kernel: &FreecoKernel, agent: AgentId, file: &str, text: &str) {
    let dir = kernel
        .registry
        .get(agent)
        .unwrap()
        .manifest
        .state_dir
        .expect("agents have a state directory");
    std::fs::write(dir.join(file), text).unwrap();
}

/// A WhatsApp guest is a different person from the owner: their own
/// conversation, none of the owner's profile, memory or chats, and only the
/// tools `[guests] tools` lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_gets_their_own_conversation_and_nothing_of_the_owners() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot("guest");
    let desk = spawn(
        &kernel,
        "fast-desk",
        &url,
        &["web_fetch", "shell_exec", "file_read", "agent_send"],
    );
    write_owner_file(&kernel, desk, "USER.md", "OWNER-PROFILE lives in Geneva.");
    write_owner_file(&kernel, desk, "MEMORY.md", "OWNER-MEMORY: banker is Ms X.");
    let handle = kernel.clone() as Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle>;

    kernel
        .send_message(desk, "owner private note")
        .await
        .unwrap();
    let owner = bodies_of(&script, "fast-desk", 0);
    let owner_text = owner.last().unwrap().to_string();
    assert!(
        owner_text.contains("OWNER-PROFILE"),
        "the test files are read"
    );
    assert!(offered_tools(owner.last().unwrap()).contains(&"shell_exec".to_string()));

    let seen = script.bodies.lock().unwrap().len();
    let reply = kernel
        .send_message_with_handle_and_blocks(
            desk,
            "guest asks for a table",
            Some(Arc::clone(&handle)),
            None,
            Some("+41790000000".to_string()),
            Some("Gast".to_string()),
        )
        .await
        .unwrap();
    assert!(reply.response.contains("guest asks"), "{}", reply.response);

    let guest = bodies_of(&script, "fast-desk", seen);
    assert_eq!(guest.len(), 1);
    let guest_text = guest[0].to_string();
    for private in ["OWNER-PROFILE", "OWNER-MEMORY", "owner private note"] {
        assert!(!guest_text.contains(private), "{private} reached a guest");
    }
    assert!(guest_text.contains("Guest conversation"), "{guest_text}");
    // The default `[guests] tools` the agent also has: web_fetch, and the
    // read-only skill tools every agent gets.
    assert_eq!(
        offered_tools(&guest[0]),
        vec!["skill_describe", "skill_list", "web_fetch"]
    );

    // Each in their own conversation, and the owner's stays put.
    let main = kernel.registry.get(desk).unwrap().session_id;
    assert_eq!(user_texts(&kernel, main), vec!["owner private note"]);
    let theirs = kernel.conversation_for(desk, "+41790000000").unwrap();
    assert_ne!(theirs, main);
    assert_eq!(user_texts(&kernel, theirs), vec!["guest asks for a table"]);

    // And nothing the guest said comes back to the owner as "earlier".
    let seen = script.bodies.lock().unwrap().len();
    kernel.send_message(desk, "owner again").await.unwrap();
    let later = bodies_of(&script, "fast-desk", seen);
    assert!(!later[0].to_string().contains("guest asks"), "{}", later[0]);
}

/// Asking another agent is no way around the limits: the agent a guest's
/// conversation asks runs as that guest too, in a conversation of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_guest_stays_a_guest_through_other_agents() {
    let script = Arc::new(Script::default());
    let url = serve(Arc::clone(&script)).await;
    let kernel = boot_with("guest-chain", |c| {
        c.guests.tools = vec!["agent_send".to_string()];
    });
    let ping = spawn(&kernel, "ping", &url, &["agent_send", "file_read"]);
    let pong = spawn(&kernel, "pong", &url, &["agent_send", "shell_exec"]);
    write_owner_file(&kernel, pong, "USER.md", "PONG-OWNER-PROFILE");
    let handle = kernel.clone() as Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle>;

    // The owner talks with pong first, so its own conversation has history.
    tokio::time::timeout(
        Duration::from_secs(30),
        kernel.send_message(pong, "owner private note"),
    )
    .await
    .expect("the owner's run finishes")
    .unwrap();

    let seen = script.bodies.lock().unwrap().len();
    tokio::time::timeout(
        Duration::from_secs(30),
        kernel.send_message_with_handle_and_blocks(
            ping,
            "guest says go",
            Some(handle),
            None,
            Some("telegram:42".to_string()),
            Some("Guest".to_string()),
        ),
    )
    .await
    .expect("the guest's chain finishes")
    .unwrap();

    let asked = bodies_of(&script, "pong", seen);
    assert!(!asked.is_empty(), "ping asked pong");
    for body in &asked {
        let text = body.to_string();
        assert!(text.contains("Guest conversation"), "pong ran as the owner");
        assert!(!text.contains("PONG-OWNER-PROFILE"), "{text}");
        assert!(!text.contains("owner private note"), "{text}");
        assert_eq!(offered_tools(body), vec!["agent_send"]);
    }
    for body in bodies_of(&script, "ping", seen) {
        assert_eq!(offered_tools(&body), vec!["agent_send"]);
    }

    // pong's own conversation with the owner was not touched (tool results
    // are stored as user turns with no text).
    let main = kernel.registry.get(pong).unwrap().session_id;
    let typed: Vec<String> = user_texts(&kernel, main)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    assert_eq!(typed, vec!["owner private note"]);
    let theirs = kernel.conversation_for(pong, "telegram:42").unwrap();
    assert!(
        user_texts(&kernel, theirs)
            .iter()
            .any(|t| t.contains("Message from the agent \"ping\"")),
        "pong heard from ping in the guest's conversation"
    );
}

/// An agent reads its own team's board through the kernel, by column, and
/// hands work in for review; an agent on another team cannot read it.
#[tokio::test]
async fn an_agent_reads_only_its_own_team_board_and_hands_in_for_review() {
    use freeco_kernel_runtime::kernel_handle::KernelHandle;
    let kernel = boot("team-board");
    let org = kernel.memory.org();
    let web = org.create_team("Web", None).unwrap();
    let ops = org.create_team("Ops", None).unwrap();
    org.add_team_member(&web.id, "agent-web", None).unwrap();
    org.add_team_member(&ops.id, "agent-ops", None).unwrap();
    let task = kernel
        .memory
        .task_post_on_board("Ship", "", None, Some("user:lena"), Some(&web.id), &[])
        .await
        .unwrap();

    let board = kernel
        .team_task_list(Some("agent-web"), "web", Some("todo"))
        .await
        .unwrap();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0]["column"], "todo");
    assert!(kernel
        .team_task_list(Some("agent-ops"), "Web", None)
        .await
        .is_err());
    assert!(kernel
        .team_task_list(Some("agent-web"), "Web", Some("sideways"))
        .await
        .is_err());
    assert!(kernel
        .team_task_list(Some("agent-web"), "No such team", None)
        .await
        .is_err());

    let claimed = kernel
        .memory
        .task_claim("agent-web")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed["id"], task.as_str());
    kernel
        .task_hand_in(&task, "draft ready", Some("agent-web"))
        .await
        .unwrap();
    let review = kernel
        .team_task_list(Some("agent-web"), &web.id, Some("review"))
        .await
        .unwrap();
    assert_eq!(review.len(), 1);
    assert_eq!(review[0]["result"], "draft ready");
    kernel.shutdown();
}

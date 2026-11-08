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
                let start = Instant::now();
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
    std::env::set_var(KEY_ENV, "sk-test");
    let home = std::env::temp_dir().join(format!(
        "freeco-parallel-{test}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&home).unwrap();
    let config = KernelConfig {
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
    let kernel = Arc::new(FreecoKernel::boot_with_config(config).expect("kernel boots"));
    kernel.set_self_handle();
    kernel
}

fn spawn(kernel: &FreecoKernel, name: &str, base_url: &str, tools: &[&str]) -> AgentId {
    let tools = tools
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest: AgentManifest = toml::from_str(&format!(
        r#"
name = "{name}"
module = "builtin:chat"
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

    let started = Instant::now();
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
    assert!(started.elapsed() < Duration::from_millis(1100));
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

//! WhatsApp Web gateway — embedded Node.js process management.
//!
//! Embeds the gateway JS at compile time, extracts it to `~/.freeco-ai/whatsapp-gateway/`,
//! runs `npm install` if needed, and spawns `node index.js` as a managed child process
//! that auto-restarts on crash.

use crate::config::freeco_ai_home;
use freeco_types::agent::AgentId;
use freeco_types::event::{Event, EventPayload, EventTarget, SystemEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Gateway source files embedded at compile time.
const GATEWAY_INDEX_JS: &str = include_str!("../../../packages/whatsapp-gateway/index.js");
const GATEWAY_PACKAGE_JSON: &str = include_str!("../../../packages/whatsapp-gateway/package.json");

/// Default port for the WhatsApp Web gateway.
const DEFAULT_GATEWAY_PORT: u16 = 3009;

/// Crashes in a row before giving up.
const MAX_RESTARTS: u32 = 10;

/// Restart backoff delays in seconds, one per attempt: about 17 minutes in
/// all, long enough to ride out a network outage or a laptop waking up.
const RESTART_DELAYS: [u64; MAX_RESTARTS as usize] = [5, 10, 20, 30, 60, 60, 120, 120, 300, 300];

/// A gateway that ran this long before exiting had been working, so its crash
/// starts a fresh run of attempts. Without this, a crash a day would use up
/// the attempts within two weeks and the gateway would stay down for good.
const STABLE_RUN: Duration = Duration::from_secs(600);

/// Crashes in a row after which the owner and the supervising agent are told,
/// while restarts carry on.
const ALERT_AFTER_CRASHES: u32 = 3;

// The early alert must come before the gateway is given up on.
const _: () = assert!(ALERT_AFTER_CRASHES < MAX_RESTARTS);

/// Get the gateway installation directory.
fn gateway_dir() -> PathBuf {
    freeco_ai_home().join("whatsapp-gateway")
}

/// Compute a simple hash of content for change detection.
fn content_hash(content: &str) -> String {
    // Use a simple FNV-style hash — no crypto needed, just change detection.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Write a file only if its content hash differs from the existing file.
/// Returns `true` if the file was written (content changed).
fn write_if_changed(path: &std::path::Path, content: &str) -> std::io::Result<bool> {
    let hash_path = path.with_extension("hash");
    let new_hash = content_hash(content);

    // Check existing hash
    if let Ok(existing_hash) = std::fs::read_to_string(&hash_path) {
        if existing_hash.trim() == new_hash {
            return Ok(false); // No change
        }
    }

    std::fs::write(path, content)?;
    std::fs::write(&hash_path, &new_hash)?;
    Ok(true)
}

/// Ensure the gateway files are extracted and npm dependencies installed.
///
/// Returns the gateway directory path on success, or an error message.
async fn ensure_gateway_installed() -> Result<PathBuf, String> {
    let dir = gateway_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create gateway dir: {e}"))?;

    let index_path = dir.join("index.js");
    let package_path = dir.join("package.json");

    // Write files only if content changed (avoids unnecessary npm install)
    let index_changed = write_if_changed(&index_path, GATEWAY_INDEX_JS)
        .map_err(|e| format!("Write index.js: {e}"))?;
    let package_changed = write_if_changed(&package_path, GATEWAY_PACKAGE_JSON)
        .map_err(|e| format!("Write package.json: {e}"))?;

    let node_modules = dir.join("node_modules");
    let needs_install = !install_is_complete(&node_modules) || package_changed;

    if needs_install {
        info!("Installing WhatsApp gateway npm dependencies...");

        // Start from an empty folder, so nothing an interrupted install left
        // behind survives into this one.
        if node_modules.exists() {
            std::fs::remove_dir_all(&node_modules)
                .map_err(|e| format!("Remove incomplete node_modules: {e}"))?;
        }

        // Determine npm command (npm.cmd on Windows, npm elsewhere)
        let npm_cmd = if cfg!(windows) { "npm.cmd" } else { "npm" };

        let output = tokio::process::Command::new(npm_cmd)
            .arg("install")
            .arg("--production")
            .current_dir(&dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("npm install failed to start: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("npm install failed: {stderr}"));
        }

        std::fs::write(
            install_marker(&node_modules),
            content_hash(GATEWAY_PACKAGE_JSON),
        )
        .map_err(|e| format!("Write install marker: {e}"))?;
        info!("WhatsApp gateway npm dependencies installed");
    } else if index_changed {
        info!("WhatsApp gateway index.js updated (binary upgrade)");
    }

    Ok(dir)
}

/// The file a completed `npm install` leaves in `node_modules`, holding the hash
/// of the package.json it installed.
fn install_marker(node_modules: &std::path::Path) -> PathBuf {
    node_modules.join(".freeco-install-complete")
}

/// Whether `node_modules` holds a finished install of the embedded package.json.
///
/// An install cut off part-way (a restart mid-download) leaves the folder
/// half-filled and no marker. Checking for the folder alone skipped the
/// install from then on, and the gateway crashed on every start with a
/// missing module. The marker, not the folder, decides.
fn install_is_complete(node_modules: &std::path::Path) -> bool {
    std::fs::read_to_string(install_marker(node_modules))
        .map(|hash| hash.trim() == content_hash(GATEWAY_PACKAGE_JSON))
        .unwrap_or(false)
}

/// Check if Node.js is available on the system.
async fn node_available() -> bool {
    let node_cmd = if cfg!(windows) { "node.exe" } else { "node" };
    tokio::process::Command::new(node_cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start the WhatsApp Web gateway as a managed child process.
///
/// This function:
/// 1. Checks if Node.js is available
/// 2. Extracts and installs the gateway files
/// 3. Spawns `node index.js` with appropriate env vars
/// 4. Sets `WHATSAPP_WEB_GATEWAY_URL` so the daemon finds it
/// 5. Monitors the process and restarts on crash (up to 3 times)
///
/// The PID is stored in the kernel's `whatsapp_gateway_pid` for shutdown cleanup.
pub async fn start_whatsapp_gateway(kernel: &Arc<super::kernel::FreecoKernel>) {
    // Only start if WhatsApp is configured
    let wa_config = match &kernel.config.channels.whatsapp {
        Some(cfg) => cfg.clone(),
        None => return,
    };

    // Check for Node.js
    if !node_available().await {
        warn!(
            "WhatsApp Web gateway requires Node.js >= 18 but `node` was not found. \
             Install Node.js to enable WhatsApp Web integration."
        );
        return;
    }

    // Extract and install
    let gateway_path = match ensure_gateway_installed().await {
        Ok(p) => p,
        Err(e) => {
            warn!("WhatsApp Web gateway setup failed: {e}");
            return;
        }
    };

    let port = DEFAULT_GATEWAY_PORT;
    let api_listen = &kernel.config.api_listen;
    let freeco_ai_url = format!("http://{api_listen}");
    let gateway_token = uuid::Uuid::new_v4().to_string();
    let api_key = kernel.config.api_key.trim().to_string();
    let default_agent = wa_config
        .default_agent
        .as_deref()
        .unwrap_or("assistant")
        .to_string();

    // Auto-set the env var so the rest of the system finds the gateway
    std::env::set_var(
        "WHATSAPP_WEB_GATEWAY_URL",
        format!("http://127.0.0.1:{port}"),
    );
    std::env::set_var("WHATSAPP_GATEWAY_TOKEN", &gateway_token);
    info!("WHATSAPP_WEB_GATEWAY_URL set to http://127.0.0.1:{port}");

    // The gateway has one allowlist, its WHATSAPP_ALLOWED_GROUPS and
    // WHATSAPP_ALLOWED_NUMBERS variables. `[channels.whatsapp] allowed_users`
    // in config.toml is folded into them, so the owner can keep the list in
    // config.toml without a second check existing anywhere.
    let (allowed_groups, allowed_numbers) = allowlist_env(
        &wa_config.allowed_users,
        std::env::var("WHATSAPP_ALLOWED_GROUPS").ok().as_deref(),
        std::env::var("WHATSAPP_ALLOWED_NUMBERS").ok().as_deref(),
    );

    // Spawn with crash monitoring
    let kernel_weak = Arc::downgrade(kernel);
    let gateway_pid = Arc::clone(&kernel.whatsapp_gateway_pid);

    tokio::spawn(async move {
        let mut restarts = 0u32;

        loop {
            let node_cmd = if cfg!(windows) { "node.exe" } else { "node" };

            info!("Starting WhatsApp Web gateway (attempt {})", restarts + 1);

            let started = Instant::now();
            let mut command = tokio::process::Command::new(node_cmd);
            if let Some(groups) = &allowed_groups {
                command.env("WHATSAPP_ALLOWED_GROUPS", groups);
            }
            if let Some(numbers) = &allowed_numbers {
                command.env("WHATSAPP_ALLOWED_NUMBERS", numbers);
            }
            let child = command
                .arg("index.js")
                .current_dir(&gateway_path)
                .env("WHATSAPP_GATEWAY_PORT", port.to_string())
                .env("FREECO_AI_URL", &freeco_ai_url)
                .env("FREECO_AI_DEFAULT_AGENT", &default_agent)
                .env("WHATSAPP_GATEWAY_TOKEN", &gateway_token)
                .env("FREECO_AI_API_KEY", &api_key)
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .spawn();

            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to spawn WhatsApp gateway: {e}");
                    return;
                }
            };

            // Store PID for shutdown cleanup
            if let Some(pid) = child.id() {
                if let Ok(mut guard) = gateway_pid.lock() {
                    *guard = Some(pid);
                }
                info!("WhatsApp Web gateway started (PID {pid})");
            }

            // Wait for process exit
            let last_exit;
            match child.wait().await {
                Ok(status) => {
                    // Clear stored PID
                    if let Ok(mut guard) = gateway_pid.lock() {
                        *guard = None;
                    }

                    // Check if kernel is still alive (not shutting down)
                    let kernel = match kernel_weak.upgrade() {
                        Some(k) => k,
                        None => {
                            info!("WhatsApp gateway exited (kernel dropped)");
                            return;
                        }
                    };

                    if kernel.supervisor.is_shutting_down() {
                        info!("WhatsApp gateway stopped (daemon shutting down)");
                        return;
                    }

                    if status.success() {
                        info!("WhatsApp gateway exited cleanly");
                        return;
                    }

                    if started.elapsed() >= STABLE_RUN {
                        restarts = 0;
                    }
                    warn!(
                        "WhatsApp gateway crashed (exit: {status}), restart {}/{MAX_RESTARTS}",
                        restarts + 1
                    );
                    last_exit = status.to_string();
                }
                Err(e) => {
                    if let Ok(mut guard) = gateway_pid.lock() {
                        *guard = None;
                    }
                    warn!("WhatsApp gateway wait error: {e}");
                    last_exit = format!("wait error: {e}");
                }
            }

            restarts += 1;
            if restarts >= MAX_RESTARTS {
                warn!("WhatsApp gateway exceeded max restarts ({MAX_RESTARTS}), giving up");
                alert_owner_and_supervisor(&kernel_weak, gave_up_alert(&last_exit));
                return;
            }
            if restarts == ALERT_AFTER_CRASHES {
                alert_owner_and_supervisor(
                    &kernel_weak,
                    still_retrying_alert(restarts, &last_exit),
                );
            }

            // Backoff before restart
            let delay = RESTART_DELAYS
                .get(restarts as usize - 1)
                .copied()
                .unwrap_or(20);
            info!("Restarting WhatsApp gateway in {delay}s...");
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    });
}

/// The gateway's allowlist variables with `[channels.whatsapp] allowed_users`
/// folded in: entries ending in `@g.us` are groups, the rest numbers. What
/// the environment already lists is kept. `None` leaves a variable as the
/// environment has it (unset means: no groups, every direct chat).
fn allowlist_env(
    allowed_users: &[String],
    env_groups: Option<&str>,
    env_numbers: Option<&str>,
) -> (Option<String>, Option<String>) {
    let merge = |env: Option<&str>, extra: Vec<&str>| -> Option<String> {
        if extra.is_empty() {
            return None;
        }
        let mut all: Vec<String> = env
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        for e in extra {
            if !all.iter().any(|a| a == e) {
                all.push(e.to_string());
            }
        }
        Some(all.join(","))
    };
    let entries: Vec<&str> = allowed_users
        .iter()
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        .collect();
    let (groups, numbers): (Vec<&str>, Vec<&str>) =
        entries.into_iter().partition(|u| u.ends_with("@g.us"));
    (merge(env_groups, groups), merge(env_numbers, numbers))
}

/// The alert for a gateway that keeps crashing but is still being restarted.
fn still_retrying_alert(crashes: u32, last_exit: &str) -> String {
    format!(
        "WhatsApp gateway alert: it has crashed {crashes} times in a row (last exit: {last_exit}) \
         and is being restarted automatically, up to {MAX_RESTARTS} times. While it is down, \
         WhatsApp messages are neither received nor answered. Nothing needs doing yet; if it \
         keeps failing, a second alert says what to do."
    )
}

/// The alert for a gateway that FreEco.ai has stopped restarting.
fn gave_up_alert(last_exit: &str) -> String {
    format!(
        "WhatsApp gateway alert: it crashed {MAX_RESTARTS} times in a row and FreEco.ai has \
         stopped restarting it (last exit: {last_exit}). WhatsApp messages are neither received \
         nor answered until it runs again. What to do: \
         1) Read the gateway's error in the daemon log (stderr). \"EADDRINUSE ... 3009\" means \
         an older gateway is still running: stop the node process listening on 127.0.0.1:3009. \
         2) Restart FreEco.ai; the gateway starts with it. \
         3) If the log says the device was removed or logged out, open Channels > WhatsApp and \
         scan a new QR code."
    )
}

/// Tell the owner and the supervising agent that the gateway is in trouble.
///
/// The supervising agent is the workspace's CEO -- the agent tagged `ceo`,
/// which the dashboard assistant talks to -- so the owner reads the alert in
/// that conversation and the CEO can act on it or pass it on. It is also
/// published as a health-check event, so triggers and the event feed see it.
fn alert_owner_and_supervisor(
    kernel_weak: &std::sync::Weak<super::kernel::FreecoKernel>,
    text: String,
) {
    warn!("{text}");
    let Some(kernel) = kernel_weak.upgrade() else {
        return;
    };
    tokio::spawn(async move {
        let event = Event::new(
            AgentId::new(),
            EventTarget::System,
            EventPayload::System(SystemEvent::HealthCheck {
                status: text.clone(),
            }),
        );
        kernel.publish_event(event).await;

        // The CEO is the agent tagged `ceo`. The tag is added when the
        // dashboard first asks for its assistant, so right after a restart it
        // can be missing; then fall back to the canonical assistant, as
        // GET /api/assistant does.
        let agents = kernel.registry.list();
        let canonical = AgentId::from_string("freeco-concierge");
        let ceo = agents
            .iter()
            .find(|e| e.tags.iter().any(|t| t.eq_ignore_ascii_case("ceo")))
            .or_else(|| agents.iter().find(|e| e.id == canonical))
            .or_else(|| {
                agents
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case("freeco-concierge"))
            })
            .map(|e| e.id);
        match ceo {
            Some(agent_id) => {
                if let Err(e) = kernel.send_message(agent_id, &text).await {
                    warn!("Could not alert the supervising agent about the WhatsApp gateway: {e}");
                }
            }
            None => warn!("No supervising (CEO) agent to alert about the WhatsApp gateway"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_files_not_empty() {
        assert_ne!(GATEWAY_INDEX_JS, "");
        assert_ne!(GATEWAY_PACKAGE_JSON, "");
        assert!(GATEWAY_INDEX_JS.contains("WhatsApp"));
        assert!(GATEWAY_PACKAGE_JSON.contains("@freeco/whatsapp-gateway"));
    }

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash("hello world");
        let h2 = content_hash("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_changes_on_different_input() {
        let h1 = content_hash("version 1");
        let h2 = content_hash("version 2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_gateway_dir_under_freeco_home() {
        let dir = gateway_dir();
        let home = freeco_ai_home();
        assert!(dir.ends_with("whatsapp-gateway"));
        assert_eq!(dir.parent(), Some(home.as_path()));
    }

    #[test]
    fn test_write_if_changed_creates_new_file() {
        let tmp = std::env::temp_dir().join("freeco_test_gateway");
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("test_write.js");
        let hash_path = path.with_extension("hash");

        // Clean up any previous runs
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&hash_path);

        // First write should return true (new file)
        let changed = write_if_changed(&path, "console.log('v1')").unwrap();
        assert!(changed);
        assert!(path.exists());
        assert!(hash_path.exists());

        // Same content should return false
        let changed = write_if_changed(&path, "console.log('v1')").unwrap();
        assert!(!changed);

        // Different content should return true
        let changed = write_if_changed(&path, "console.log('v2')").unwrap();
        assert!(changed);

        // Clean up
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&hash_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn test_install_counts_only_with_marker() {
        let tmp = std::env::temp_dir().join("freeco_test_gateway_install");
        let node_modules = tmp.join("node_modules");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(node_modules.join("@whiskeysockets").join("baileys")).unwrap();

        // The folder an interrupted install leaves behind does not count
        assert!(!install_is_complete(&node_modules));

        // Nor does a marker written for a different package.json
        std::fs::write(install_marker(&node_modules), "0000000000000000").unwrap();
        assert!(!install_is_complete(&node_modules));

        std::fs::write(
            install_marker(&node_modules),
            content_hash(GATEWAY_PACKAGE_JSON),
        )
        .unwrap();
        assert!(install_is_complete(&node_modules));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_default_gateway_port() {
        assert_eq!(DEFAULT_GATEWAY_PORT, 3009);
    }

    #[test]
    fn test_restart_backoff_delays() {
        assert_eq!(MAX_RESTARTS, 10);
        assert!(RESTART_DELAYS.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn allowed_users_fold_into_the_gateways_one_allowlist() {
        let users = vec![
            "+41 79 123 45 67".to_string(),
            "120363000000000000@g.us".to_string(),
            " ".to_string(),
        ];
        let (groups, numbers) = allowlist_env(&users, None, Some("+4900"));
        assert_eq!(groups.as_deref(), Some("120363000000000000@g.us"));
        assert_eq!(numbers.as_deref(), Some("+4900,+41 79 123 45 67"));

        // Nothing in config.toml: the environment is left exactly as it is.
        assert_eq!(allowlist_env(&[], Some("*"), Some("+4900")), (None, None));
        // Listed in both: not repeated.
        let (_, numbers) = allowlist_env(&["+4900".to_string()], None, Some("+4900"));
        assert_eq!(numbers.as_deref(), Some("+4900"));
    }

    #[test]
    fn test_alerts_say_what_happened_and_what_to_do() {
        let retrying = still_retrying_alert(3, "exit code: 1");
        assert!(retrying.contains("3 times in a row"));
        assert!(retrying.contains("exit code: 1"));

        let gave_up = gave_up_alert("exit code: 1");
        assert!(gave_up.contains("stopped restarting"));
        assert!(gave_up.contains("EADDRINUSE"));
        assert!(gave_up.contains("Restart FreEco.ai"));
        assert!(gave_up.contains("scan a new QR code"));
    }
}

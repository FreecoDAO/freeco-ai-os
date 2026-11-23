//! WhatsApp Web gateway — embedded Node.js process management.
//!
//! Embeds the gateway JS at compile time, extracts it to `~/.freeco-ai/whatsapp-gateway/`,
//! runs `npm install` if needed, and spawns `node index.js` as a managed child process
//! that auto-restarts on crash.

use crate::config::freeco_ai_home;
use freeco_channels::whatsapp_allowlist::WhatsAppAllowlist;
use freeco_types::agent::AgentId;
use freeco_types::config::WhatsAppConfig;
use freeco_types::event::{Event, EventPayload, EventTarget, SystemEvent};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Gateway source files embedded at compile time.
const GATEWAY_INDEX_JS: &str = include_str!("../../../packages/whatsapp-gateway/index.js");
const GATEWAY_PACKAGE_JSON: &str = include_str!("../../../packages/whatsapp-gateway/package.json");
/// The allowlist module `index.js` imports; without it the gateway cannot start.
const GATEWAY_ALLOWLIST_JS: &str = include_str!("../../../packages/whatsapp-gateway/allowlist.js");

/// Default port for the WhatsApp Web gateway.
const DEFAULT_GATEWAY_PORT: u16 = 3009;

/// Where the daemon's own gateway listens (`DEFAULT_GATEWAY_PORT` on loopback).
pub const LOCAL_GATEWAY_URL: &str = "http://127.0.0.1:3009";

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
    let allowlist_changed = write_if_changed(&dir.join("allowlist.js"), GATEWAY_ALLOWLIST_JS)
        .map_err(|e| format!("Write allowlist.js: {e}"))?;
    let index_changed = index_changed || allowlist_changed;
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

/// What the gateway process is doing, for the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayProcess {
    /// No `[channels.whatsapp]` section: nothing to run.
    NotConfigured,
    /// Checking Node.js / installing dependencies / spawning.
    Starting,
    /// `node` was not found.
    NodeMissing,
    /// Extracting the files or `npm install` failed.
    InstallFailed,
    /// The Node process is up (whether WhatsApp is linked is the gateway's
    /// own `/login/status`).
    Running,
    /// It crashed and is waiting to be started again.
    Restarting,
    /// It crashed too often; FreEco.ai stopped restarting it.
    GaveUp,
    /// It exited cleanly or was stopped.
    Stopped,
}

impl GatewayProcess {
    /// The name the API and the dashboard use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Starting => "starting",
            Self::NodeMissing => "node_missing",
            Self::InstallFailed => "install_failed",
            Self::Running => "running",
            Self::Restarting => "restarting",
            Self::GaveUp => "gave_up",
            Self::Stopped => "stopped",
        }
    }
}

/// The gateway process state and its crashes in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayStatus {
    pub process: GatewayProcess,
    pub crashes: u32,
}

static STATUS: Mutex<GatewayStatus> = Mutex::new(GatewayStatus {
    process: GatewayProcess::NotConfigured,
    crashes: 0,
});

/// What the gateway process is doing right now.
pub fn gateway_status() -> GatewayStatus {
    *STATUS.lock().unwrap_or_else(|p| p.into_inner())
}

fn set_status(process: GatewayProcess, crashes: u32) {
    *STATUS.lock().unwrap_or_else(|p| p.into_inner()) = GatewayStatus { process, crashes };
}

/// Everything a gateway process is started with, except its per-start token.
/// A change in any of it means the running gateway must be restarted.
#[derive(Clone, PartialEq, Eq)]
struct GatewaySettings {
    freeco_ai_url: String,
    api_key: String,
    default_agent: String,
    allowed_numbers: String,
    allowed_groups: String,
}

/// The settings for `[channels.whatsapp]`. The allowlist comes from the
/// config alone (`allowed_users` + `owner_number`) -- the same list the daemon
/// checks again on receipt and on every send -- and is always passed
/// explicitly, so the gateway never inherits a different one from the
/// daemon's environment.
fn gateway_settings(api_listen: &str, api_key: &str, wa: &WhatsAppConfig) -> GatewaySettings {
    let allowlist = WhatsAppAllowlist::from_config(&wa.allowed_users, wa.owner_number.as_deref());
    GatewaySettings {
        freeco_ai_url: local_api_url(api_listen),
        api_key: api_key.trim().to_string(),
        default_agent: wa
            .default_agent
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or("assistant")
            .to_string(),
        allowed_numbers: allowlist.numbers_env(),
        allowed_groups: allowlist.groups_env(),
    }
}

/// The URL the gateway reaches the API on. An API listening on every
/// interface (`0.0.0.0` / `[::]`) is reached on loopback: `0.0.0.0` is not
/// an address one can connect to everywhere (Windows refuses it).
fn local_api_url(api_listen: &str) -> String {
    let listen = api_listen.trim();
    let local = if let Some(port) = listen.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{port}")
    } else if let Some(port) = listen.strip_prefix("[::]:") {
        format!("127.0.0.1:{port}")
    } else {
        listen.to_string()
    };
    format!("http://{local}")
}

/// The running supervisor: the task that keeps one gateway process alive.
struct Supervisor {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    settings: GatewaySettings,
}

static SUPERVISOR: Mutex<Option<Supervisor>> = Mutex::new(None);

/// One start/restart/stop at a time.
fn apply_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Start the WhatsApp Web gateway at boot, if `[channels.whatsapp]` is configured.
pub async fn start_whatsapp_gateway(kernel: &Arc<super::kernel::FreecoKernel>) {
    apply_whatsapp_config(kernel, kernel.config.channels.whatsapp.clone()).await;
}

/// Make the gateway match `wa_config`: start it if it is not running, restart
/// it if its settings (the allowlist above all) changed, stop it if WhatsApp
/// is no longer configured, and leave it alone if nothing changed.
///
/// Called at boot and on every channel reload, so WhatsApp set up from the
/// dashboard starts its gateway without a daemon restart, and an allowlist
/// edited there reaches the gateway at once.
///
/// The gateway process:
/// 1. needs Node.js; its files are extracted and `npm install`ed if needed,
/// 2. runs `node index.js` with its settings as env vars,
/// 3. sets `WHATSAPP_WEB_GATEWAY_URL` so the daemon finds it,
/// 4. is restarted on crash (up to `MAX_RESTARTS` in a row, with backoff).
///
/// The PID is stored in the kernel's `whatsapp_gateway_pid` for shutdown cleanup.
pub async fn apply_whatsapp_config(
    kernel: &Arc<super::kernel::FreecoKernel>,
    wa_config: Option<WhatsAppConfig>,
) {
    let _one_at_a_time = apply_lock().lock().await;

    let wanted = wa_config
        .as_ref()
        .map(|wa| gateway_settings(&kernel.config.api_listen, &kernel.config.api_key, wa));

    let previous = {
        let mut guard = SUPERVISOR.lock().unwrap_or_else(|p| p.into_inner());
        let unchanged = match (guard.as_ref(), wanted.as_ref()) {
            (Some(running), Some(wanted)) => {
                running.settings == *wanted && !running.task.is_finished()
            }
            _ => false,
        };
        if unchanged {
            return;
        }
        guard.take()
    };
    if let Some(previous) = previous {
        info!("Stopping the WhatsApp Web gateway to apply new settings");
        let _ = previous.stop.send(true);
        let _ = previous.task.await;
    }

    let (Some(settings), Some(wa)) = (wanted, wa_config) else {
        set_status(GatewayProcess::NotConfigured, 0);
        return;
    };

    let allowlist = WhatsAppAllowlist::from_config(&wa.allowed_users, wa.owner_number.as_deref());
    for bad in allowlist.rejected() {
        warn!(
            "[channels.whatsapp] entry {bad:?} is neither an international number (+41 79 ...) \
             nor a group JID (...@g.us); it allows nothing"
        );
    }
    if allowlist.is_empty() {
        warn!(
            "[channels.whatsapp] allows no chat (allowed_users and owner_number are empty): \
             every WhatsApp message is ignored and every send refused"
        );
    }
    for legacy in ["WHATSAPP_ALLOWED_NUMBERS", "WHATSAPP_ALLOWED_GROUPS"] {
        if std::env::var(legacy).is_ok_and(|v| !v.trim().is_empty()) {
            warn!(
                "{legacy} is set in the daemon's environment and ignored: the WhatsApp \
                 allowlist is [channels.whatsapp] allowed_users in config.toml"
            );
        }
    }

    set_status(GatewayProcess::Starting, 0);

    // Check for Node.js
    if !node_available().await {
        warn!(
            "WhatsApp Web gateway requires Node.js >= 20 but `node` was not found. \
             Install Node.js to enable WhatsApp Web integration."
        );
        set_status(GatewayProcess::NodeMissing, 0);
        return;
    }

    // Extract and install
    let gateway_path = match ensure_gateway_installed().await {
        Ok(p) => p,
        Err(e) => {
            warn!("WhatsApp Web gateway setup failed: {e}");
            set_status(GatewayProcess::InstallFailed, 0);
            return;
        }
    };

    let port = DEFAULT_GATEWAY_PORT;
    let gateway_token = uuid::Uuid::new_v4().to_string();

    // Auto-set the env var so the rest of the system finds the gateway
    std::env::set_var(
        "WHATSAPP_WEB_GATEWAY_URL",
        format!("http://127.0.0.1:{port}"),
    );
    std::env::set_var("WHATSAPP_GATEWAY_TOKEN", &gateway_token);
    info!("WHATSAPP_WEB_GATEWAY_URL set to http://127.0.0.1:{port}");

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(supervise(
        Arc::downgrade(kernel),
        Arc::clone(&kernel.whatsapp_gateway_pid),
        gateway_path,
        port,
        gateway_token,
        settings.clone(),
        stop_rx,
    ));
    *SUPERVISOR.lock().unwrap_or_else(|p| p.into_inner()) = Some(Supervisor {
        stop: stop_tx,
        task,
        settings,
    });
}

/// Keep one gateway process running until told to stop, the daemon shuts
/// down, or it crashed `MAX_RESTARTS` times in a row.
async fn supervise(
    kernel_weak: std::sync::Weak<super::kernel::FreecoKernel>,
    gateway_pid: Arc<Mutex<Option<u32>>>,
    gateway_path: PathBuf,
    port: u16,
    gateway_token: String,
    settings: GatewaySettings,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut restarts = 0u32;

    loop {
        let node_cmd = if cfg!(windows) { "node.exe" } else { "node" };

        info!("Starting WhatsApp Web gateway (attempt {})", restarts + 1);

        let started = Instant::now();
        let child = tokio::process::Command::new(node_cmd)
            .arg("index.js")
            .current_dir(&gateway_path)
            .env("WHATSAPP_GATEWAY_PORT", port.to_string())
            .env("FREECO_AI_URL", &settings.freeco_ai_url)
            .env("FREECO_AI_DEFAULT_AGENT", &settings.default_agent)
            .env("WHATSAPP_GATEWAY_TOKEN", &gateway_token)
            .env("FREECO_AI_API_KEY", &settings.api_key)
            // Always set, even empty: the one list is the config's.
            .env("WHATSAPP_ALLOWED_NUMBERS", &settings.allowed_numbers)
            .env("WHATSAPP_ALLOWED_GROUPS", &settings.allowed_groups)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to spawn WhatsApp gateway: {e}");
                set_status(GatewayProcess::Stopped, restarts);
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
        set_status(GatewayProcess::Running, restarts);

        // Wait for the process to exit, or for a stop (new settings).
        let waited = tokio::select! {
            status = child.wait() => Some(status),
            _ = stop_rx.changed() => None,
        };
        let Some(waited) = waited else {
            let _ = child.kill().await;
            if let Ok(mut guard) = gateway_pid.lock() {
                *guard = None;
            }
            set_status(GatewayProcess::Stopped, 0);
            info!("WhatsApp gateway stopped");
            return;
        };

        let last_exit;
        match waited {
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
                        set_status(GatewayProcess::Stopped, 0);
                        return;
                    }
                };

                if kernel.supervisor.is_shutting_down() {
                    info!("WhatsApp gateway stopped (daemon shutting down)");
                    set_status(GatewayProcess::Stopped, 0);
                    return;
                }

                if status.success() {
                    info!("WhatsApp gateway exited cleanly");
                    set_status(GatewayProcess::Stopped, 0);
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
            set_status(GatewayProcess::GaveUp, restarts);
            alert_owner_and_supervisor(&kernel_weak, gave_up_alert(&last_exit));
            return;
        }
        if restarts == ALERT_AFTER_CRASHES {
            alert_owner_and_supervisor(&kernel_weak, still_retrying_alert(restarts, &last_exit));
        }

        // Backoff before restart; a stop during the wait ends it.
        let delay = restart_delay(restarts);
        set_status(GatewayProcess::Restarting, restarts);
        info!("Restarting WhatsApp gateway in {delay}s...");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
            _ = stop_rx.changed() => {
                set_status(GatewayProcess::Stopped, 0);
                return;
            }
        }
    }
}

/// Seconds to wait before restart number `restarts` (1-based).
fn restart_delay(restarts: u32) -> u64 {
    RESTART_DELAYS
        .get((restarts as usize).saturating_sub(1))
        .copied()
        .unwrap_or(20)
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
        assert_eq!(
            LOCAL_GATEWAY_URL,
            format!("http://127.0.0.1:{DEFAULT_GATEWAY_PORT}")
        );
    }

    #[test]
    fn test_restart_backoff_delays() {
        assert_eq!(MAX_RESTARTS, 10);
        assert!(RESTART_DELAYS.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn gateway_gets_the_configs_allowlist_and_nothing_else() {
        let wa = WhatsAppConfig {
            allowed_users: vec![
                "+41 79 123 45 67".to_string(),
                "120363000000000000@g.us".to_string(),
                " ".to_string(),
            ],
            owner_number: Some("0041 79 000 00 00".to_string()),
            default_agent: Some("concierge".to_string()),
            ..Default::default()
        };
        let s = gateway_settings("127.0.0.1:4200", " key ", &wa);
        assert_eq!(s.allowed_numbers, "+41790000000,+41791234567");
        assert_eq!(s.allowed_groups, "120363000000000000@g.us");
        assert_eq!(s.default_agent, "concierge");
        assert_eq!(s.api_key, "key");
        assert_eq!(s.freeco_ai_url, "http://127.0.0.1:4200");

        // Empty config: empty lists (deny all), passed as such, not left unset.
        let empty = gateway_settings("127.0.0.1:4200", "", &WhatsAppConfig::default());
        assert_eq!(empty.allowed_numbers, "");
        assert_eq!(empty.allowed_groups, "");
        assert_eq!(empty.default_agent, "assistant");
    }

    #[test]
    fn a_changed_allowlist_changes_the_settings_so_the_gateway_restarts() {
        let before = gateway_settings("127.0.0.1:4200", "", &WhatsAppConfig::default());
        let after = gateway_settings(
            "127.0.0.1:4200",
            "",
            &WhatsAppConfig {
                allowed_users: vec!["+41791234567".to_string()],
                ..Default::default()
            },
        );
        assert!(before != after);
        assert!(before == gateway_settings("127.0.0.1:4200", "", &WhatsAppConfig::default()));
    }

    #[test]
    fn gateway_reaches_an_api_on_every_interface_through_loopback() {
        assert_eq!(local_api_url("0.0.0.0:4200"), "http://127.0.0.1:4200");
        assert_eq!(local_api_url("[::]:4200"), "http://127.0.0.1:4200");
        assert_eq!(local_api_url("127.0.0.1:4300"), "http://127.0.0.1:4300");
        assert_eq!(local_api_url("192.168.1.5:4200"), "http://192.168.1.5:4200");
    }

    #[test]
    fn the_allowlist_module_is_embedded_with_the_gateway() {
        assert!(GATEWAY_INDEX_JS.contains("from './allowlist.js'"));
        assert!(GATEWAY_ALLOWLIST_JS.contains("export function normalizeChat"));
    }

    #[test]
    fn restart_delays_follow_the_table() {
        assert_eq!(restart_delay(1), 5);
        assert_eq!(restart_delay(MAX_RESTARTS), 300);
        assert_eq!(restart_delay(0), 5);
        assert_eq!(restart_delay(MAX_RESTARTS + 1), 20);
    }

    #[test]
    fn gateway_states_have_stable_names() {
        assert_eq!(GatewayProcess::Running.as_str(), "running");
        assert_eq!(GatewayProcess::Restarting.as_str(), "restarting");
        assert_eq!(GatewayProcess::GaveUp.as_str(), "gave_up");
        assert_eq!(GatewayProcess::NotConfigured.as_str(), "not_configured");
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

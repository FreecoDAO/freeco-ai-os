//! Running the containers that back service hands.
//!
//! A hand with a `[container]` section is a service rather than a capability:
//! it is started when the hand is activated, stopped when it is paused, and its
//! health is something the product can report instead of the user discovering
//! it by hand.
//!
//! Everything here shells out to `podman` or `docker`. That is deliberate --
//! the socket APIs differ between the two while the CLI arguments used here are
//! identical, so one code path covers both.

use crate::{HandContainer, HandError};
use std::time::Duration;

/// Seconds allowed for a single runtime command before it is abandoned.
///
/// `docker version` blocks indefinitely while Docker Desktop is starting rather
/// than failing, and every caller inherits that hang. A probe that cannot fail
/// is worse than one that returns false.
const RUNTIME_TIMEOUT: Duration = Duration::from_secs(20);
// Every runtime command below is spawned with `kill_on_drop(true)`. A timeout
// drops the future but not the process it started: without this, a `podman`
// call that hung after the PC slept kept running in the background, holding
// Podman's lock, so every later call -- the watchdog's included -- queued
// behind it and the daemon reported the runtime down while it was up.

/// Make sure an image is local before `run` is asked to start it.
///
/// `run` pulls a missing image itself, inside whatever time limit it was given.
/// A service image is often several gigabytes, and the single-container path
/// gave `run` ninety seconds: a service whose image was not yet local could
/// never start, and every retry began the download again and was killed again.
/// Pulling separately, with a limit sized for a download, lets `run` keep the
/// short limit that suits what it actually does.
async fn ensure_image(rt: &str, image: &str) -> Result<(), HandError> {
    let present = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["image", "inspect", image])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;
    if matches!(present, Ok(Ok(s)) if s.success()) {
        return Ok(());
    }
    tracing::info!(image, "pulling image before first start");
    let out = tokio::time::timeout(
        Duration::from_secs(45 * 60),
        tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["pull", image])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| HandError::Config(format!("pulling {image} took longer than 45 minutes")))?
    .map_err(|e| HandError::Config(format!("could not run {rt} pull: {e}")))?;
    if !out.status.success() {
        return Err(HandError::Config(format!(
            "could not pull {image}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// When the daemon last restarted WSL, in seconds since the Unix epoch.
/// Windows-only, like the one function that uses it.
#[cfg(windows)]
static LAST_WSL_RESTART: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Minimum spacing between two WSL restarts.
#[cfg(windows)]
const WSL_RESTART_SPACING_SECS: u64 = 600;

/// Last resort when the Podman VM will not come back: restart WSL itself.
///
/// After Windows wakes from Modern Standby the VM can be left in a state no
/// `podman machine` command clears: the VM runs and sshd listens inside it,
/// but WSL's localhost forwarding is gone, so `machine start` fails with
/// "machine is not listening on ssh port" and `stop`/`start` repeats the
/// failure. Restarting WSL is the one step that has cleared it every time it
/// was observed. It also stops any other WSL distribution, which the owner
/// accepted for this. Services come back on their own: podman-restart.service
/// restarts containers with a restart policy, and the watchdog restarts
/// service hands within a minute.
///
/// Rate-limited so a VM that is broken for some other reason cannot make the
/// daemon restart WSL in a loop.
#[cfg(windows)]
async fn restart_wsl_then_start() -> bool {
    use std::sync::atomic::Ordering;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last = LAST_WSL_RESTART.load(Ordering::Relaxed);
    if now.saturating_sub(last) < WSL_RESTART_SPACING_SECS {
        tracing::warn!(
            "Podman VM still unreachable; WSL was restarted {}s ago, not restarting it again yet",
            now.saturating_sub(last)
        );
        return false;
    }
    LAST_WSL_RESTART.store(now, Ordering::Relaxed);
    tracing::warn!(
        "Podman VM unreachable after starting and restarting it; restarting WSL to clear \
         its port forwarding (typical after the PC wakes from sleep)"
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new("wsl")
            .kill_on_drop(true)
            .arg("--shutdown")
            .status(),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (started, stderr) = podman_machine(&["start"], 300).await;
    let ok = started && probe("podman").await;
    if ok {
        tracing::info!("Podman VM recovered after restarting WSL");
    } else {
        tracing::warn!(error = %stderr.trim(), "Podman VM still unreachable after restarting WSL");
    }
    ok
}

#[cfg(not(windows))]
async fn restart_wsl_then_start() -> bool {
    false
}

/// Which runtime to use: docker when its daemon answers, else podman.
///
/// Cached, because this is consulted on every service operation and probing
/// twice per call would add two process spawns to each one.
pub async fn runtime() -> Option<&'static str> {
    use std::sync::OnceLock;
    // Only a *success* is cached. Caching the failure made the daemon
    // permanently blind: boot once while the VM is down and every later hand
    // activation answered "no container runtime found" even after the user
    // started Podman, until the daemon was restarted. The failing path costs
    // two process spawns, which is the right price for being able to recover.
    static RUNTIME: OnceLock<&'static str> = OnceLock::new();
    if let Some(hit) = RUNTIME.get() {
        return Some(*hit);
    }
    for candidate in ["docker", "podman"] {
        if probe(candidate).await {
            let _ = RUNTIME.set(candidate);
            return Some(candidate);
        }
    }

    // Podman on Windows and macOS runs inside a VM that stops when the host
    // sleeps or reboots, and stays stopped. The CLI is still installed and on
    // PATH, so "podman is missing" and "podman's VM is not running" look
    // identical from a failed `podman version` -- and the second is the common
    // one, four days of downtime here before anyone noticed.
    if start_podman_machine().await && probe("podman").await {
        let _ = RUNTIME.set("podman");
        return Some("podman");
    }
    None
}

/// The resolver file every container needs, and which nothing else recreates.
///
/// Podman bind-mounts the VM's `/etc/resolv.conf` into each container, so if it
/// is missing *no container can start at all* -- the error names resolv.conf and
/// says nothing about DNS, which makes it read like a broken image.
///
/// It goes missing because the VM is configured with `generateResolvConf =
/// false`, which is deliberate: WSL otherwise writes a single nameserver
/// pointing at its own NAT proxy, and when that proxy hiccups every image pull
/// fails with "Temporary failure in name resolution". Three nameservers with a
/// short timeout fixed that. The cost is that WSL no longer maintains the file,
/// and it did not survive a VM restart.
const VM_RESOLV_CONF: &str =
    "nameserver 1.1.1.1\\nnameserver 8.8.8.8\\nnameserver 10.255.255.254\\noptions timeout:2 attempts:2\\n";

/// Id of the missing-resolver issue.
pub const VM_ISSUE_RESOLVER: &str = "resolver";
/// Id of the disabled-restart-service issue.
pub const VM_ISSUE_RESTART_SERVICE: &str = "restart_service";

/// A problem in the Podman VM whose fix needs `sudo` inside the VM.
///
/// Both known problems used to be repaired silently: the watchdog and the
/// container start paths ran `sudo` in the VM whenever they noticed one.
/// Gaining root in the VM is the user's decision, not the daemon's, so the
/// daemon now only finds and reports them, and the fix runs from an explicit
/// click through `repair_vm_issue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmIssue {
    pub id: &'static str,
    pub title: &'static str,
    pub problem: &'static str,
    pub what_fixing_changes: &'static str,
    /// The literal command the fix runs, shown to the user before they approve.
    pub command: String,
}

impl VmIssue {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "title": self.title,
            "problem": self.problem,
            "what_fixing_changes": self.what_fixing_changes,
            "command": self.command,
        })
    }
}

/// The one command each fix runs, and nothing else.
///
/// Unknown ids get `None`, so no request can choose what runs -- the id picks
/// between two fixed commands, it is never interpolated into one. `sudo -n` so
/// a VM without passwordless sudo fails at once instead of hanging on a prompt
/// nobody can see.
pub fn vm_fix_command(id: &str) -> Option<String> {
    match id {
        VM_ISSUE_RESOLVER => Some(format!(
            "sudo -n sh -c 'printf \"{VM_RESOLV_CONF}\" > /etc/resolv.conf'"
        )),
        VM_ISSUE_RESTART_SERVICE => {
            Some("sudo -n systemctl --global enable podman-restart.service".to_string())
        }
        _ => None,
    }
}

/// Run a script in the Podman VM and return its stdout, or `None` if it could
/// not run, failed, or timed out.
async fn vm_ssh(script: &str, timeout: Duration) -> Option<String> {
    let out = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args(["machine", "ssh", script])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Report a missing `/etc/resolv.conf` in the Podman VM. Read-only.
///
/// `None` when the file is present, when the runtime is not Podman, and when
/// the check itself could not run -- a failed check is not evidence of a
/// problem, and reporting one would put a false warning in front of the user.
pub async fn check_vm_resolver() -> Option<VmIssue> {
    if runtime().await != Some("podman") {
        return None;
    }
    let present = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args(["machine", "ssh", "test -f /etc/resolv.conf"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;
    match present {
        // `test -f` exits 1 when the file is missing; any other result is
        // either present or a check that did not run.
        Ok(Ok(s)) if s.code() == Some(1) => Some(VmIssue {
            id: VM_ISSUE_RESOLVER,
            title: "Podman VM has no DNS resolver",
            problem: "The Podman VM is missing /etc/resolv.conf, so no container can start: \
                      every start fails with an error naming resolv.conf.",
            what_fixing_changes: "Writes /etc/resolv.conf inside the Podman VM with three \
                                  nameservers (1.1.1.1, 8.8.8.8 and the WSL gateway), using \
                                  sudo in the VM. Nothing on Windows changes.",
            command: vm_fix_command(VM_ISSUE_RESOLVER).unwrap_or_default(),
        }),
        _ => None,
    }
}

/// Report that the Podman VM will not bring service containers back after it
/// restarts. Read-only.
///
/// Service hands run their containers with `restart: unless-stopped`, and on
/// rootless Podman that policy is only honoured at boot by
/// `podman-restart.service` -- which ships disabled. So every time the VM
/// stopped (host reboot, sleep, `podman machine stop`) each service came back
/// exited and stayed down until someone started it by hand. That, not the
/// volumes, is why a configured CRM kept failing to survive a reboot. Verified
/// on this machine: once enabled, all ten service containers returned on their
/// own about ninety seconds after a VM restart.
///
/// The fix enables it globally for user sessions rather than with `systemctl
/// --user`: in this VM `~/.config/systemd` is owned by root, which refuses the
/// per-user enable and even the symlink it would create.
pub async fn check_vm_restart_service() -> Option<VmIssue> {
    if runtime().await != Some("podman") {
        return None;
    }
    // `|| true` because is-enabled exits non-zero for "disabled", and that is
    // exactly the answer this needs to read.
    let state = vm_ssh(
        "systemctl --global is-enabled podman-restart.service || true",
        RUNTIME_TIMEOUT,
    )
    .await?;
    // Only a definite answer is reported. An empty or unexpected reply is a
    // check that did not work, not a disabled service.
    match state.trim() {
        "disabled" | "masked" => Some(VmIssue {
            id: VM_ISSUE_RESTART_SERVICE,
            title: "Services will not restart after a reboot",
            problem: "podman-restart.service is disabled in the Podman VM, so CRM, voice and \
                      other service containers stay stopped after the VM or the computer \
                      restarts.",
            what_fixing_changes: "Enables podman-restart.service for all user sessions inside \
                                  the Podman VM, using sudo in the VM, so containers set to \
                                  restart come back on their own. Nothing on Windows changes.",
            command: vm_fix_command(VM_ISSUE_RESTART_SERVICE).unwrap_or_default(),
        }),
        _ => None,
    }
}

/// Every known Podman VM problem, for the dashboard and the watchdog.
pub async fn vm_issues() -> Vec<VmIssue> {
    let mut issues = Vec::new();
    if let Some(issue) = check_vm_resolver().await {
        issues.push(issue);
    }
    if let Some(issue) = check_vm_restart_service().await {
        issues.push(issue);
    }
    issues
}

/// Run the fix for one issue. Only ever called from an explicit user action.
///
/// The command comes from `vm_fix_command`, never from the caller, and the
/// issue is checked again afterwards so "fixed" means the problem is gone, not
/// merely that a command exited zero.
pub async fn repair_vm_issue(id: &str) -> Result<String, String> {
    let command = vm_fix_command(id).ok_or_else(|| format!("unknown VM issue '{id}'"))?;
    if runtime().await != Some("podman") {
        return Err("the container runtime is not Podman, so there is no VM to fix".into());
    }
    tracing::warn!(issue = id, "running user-approved Podman VM fix: {command}");
    if vm_ssh(&command, Duration::from_secs(60)).await.is_none() {
        return Err(format!(
            "the fix did not succeed: `{command}` failed inside the Podman VM"
        ));
    }
    let remaining = if id == VM_ISSUE_RESOLVER {
        check_vm_resolver().await
    } else {
        check_vm_restart_service().await
    };
    match remaining {
        None => Ok(format!("Fixed. Ran `{command}` in the Podman VM.")),
        Some(_) => Err(format!("`{command}` ran, but the problem is still there")),
    }
}

/// What a container start says when the VM has no resolver.
///
/// Shown to whoever started it -- the dashboard or an agent calling
/// `hand_activate` -- so it names the cause and the fix instead of a bare
/// container error.
fn resolver_missing_message() -> String {
    "a container could not start because the Podman VM has no /etc/resolv.conf, so nothing \
     inside it can resolve DNS. Nothing was changed. To restore it, open Settings > Services \
     and press Fix on \"Podman VM has no DNS resolver\" (it rewrites /etc/resolv.conf inside \
     the Podman VM using sudo), then start this again."
        .to_string()
}

/// Try to bring a stopped runtime back, for callers that noticed it is down.
///
/// Returns whether a runtime answers afterwards.
pub async fn try_start_runtime() -> bool {
    start_podman_machine().await && probe("podman").await
}

/// Start the Podman VM if one exists and is stopped.
///
/// Returns whether a start was attempted and reported success. Does nothing on
/// Linux, where `podman machine` has no VM to manage.
/// Whether the Podman VM's SSH forward accepts connections from this machine.
///
/// This is the one fact that separates a VM that is genuinely unreachable from
/// a `podman` command that merely failed. After sleep, WSL can lose its port
/// forwarding: the VM runs, sshd listens inside it, and the port on Windows
/// refuses -- that needs a restart. But the `podman version` probe also fails
/// for other reasons while the VM is perfectly healthy (the daemon once
/// reported no runtime with twelve containers running), and acting on that
/// alone stopped a working VM and took every service down with it, repeatedly.
///
/// `Some(true)`: reachable, so do nothing destructive. `Some(false)`: refused,
/// a restart is justified. `None`: the port could not be determined.
async fn machine_port_open() -> Option<bool> {
    let out = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args([
                "system",
                "connection",
                "list",
                "--format",
                "{{.URI}} {{.Default}}",
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Prefer the default connection; otherwise the first one listed.
    let line = text
        .lines()
        .find(|l| l.trim_end().ends_with("true"))
        .or_else(|| text.lines().next())?;
    // ssh://user@127.0.0.1:52898/run/user/1000/podman/podman.sock
    let after_host = line.split("127.0.0.1:").nth(1)?;
    let port: u16 = after_host
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    let reachable = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    Some(reachable)
}

async fn start_podman_machine() -> bool {
    let listed = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args(["machine", "list", "--format", "{{.Name}}"])
            .output(),
    )
    .await;
    let has_machine = matches!(listed, Ok(Ok(ref o)) if o.status.success()
        && !String::from_utf8_lossy(&o.stdout).trim().is_empty());
    if !has_machine {
        return false;
    }
    tracing::info!("container runtime unreachable; starting the Podman machine");
    // Starting a stopped VM takes tens of seconds.
    let (started, stderr) = podman_machine(&["start"], 300).await;
    if started {
        return true;
    }

    // "Already running" is not proof the runtime is usable, and treating it as
    // a harmless error is what kept this machine's runtime dead for two weeks.
    // The state seen here: the WSL VM was up, but the SSH port forward the CLI
    // connects through had died and its port had been taken by another
    // process, so every podman command was refused while `machine start`
    // answered "already running". The watchdog retried that every ten minutes
    // and could never succeed.
    if !stderr.to_lowercase().contains("already running") {
        // A start that fails outright is what the post-sleep state looks
        // like: "machine did not transition into running state: ssh error:
        // machine is not listening on ssh port". Confirm it before acting.
        tracing::warn!(error = %stderr.trim(), "podman machine start failed");
        // Only a port confirmed refused justifies restarting WSL; an answering
        // port, or one that cannot be determined, does not.
        if machine_port_open().await != Some(false) {
            tracing::warn!("the VM's port is not confirmed unreachable; not restarting WSL");
            return false;
        }
        return restart_wsl_then_start().await;
    }
    if probe("podman").await {
        return true;
    }

    // Before stopping anything: is the VM actually unreachable, or did the
    // probe fail for some other reason? Only a refused port justifies taking
    // down a VM that may be serving every container the user relies on.
    if machine_port_open().await != Some(false) {
        tracing::warn!(
            "Podman did not answer, but its VM is reachable (or its port is \
             unknown); leaving the VM running rather than stopping it"
        );
        return false;
    }

    // Cycling the VM re-establishes the forward, and `start` reassigns a
    // conflicting port. It stops every container, but only after the runtime
    // has already stopped answering, so nothing reachable is lost:
    // `podman-restart.service` brings back containers with a restart policy
    // when the VM starts, and the watchdog restarts active service hands
    // within a minute.
    tracing::warn!(
        "Podman reports its VM running but its API does not answer; the port \
         forward is dead. Restarting the VM to re-establish it."
    );
    let _ = podman_machine(&["stop"], 120).await;
    if podman_machine(&["start"], 300).await.0 && probe("podman").await {
        return true;
    }
    restart_wsl_then_start().await
}

/// Run `podman machine <args>`, returning whether it succeeded and its stderr.
async fn podman_machine(args: &[&str], timeout_secs: u64) -> (bool, String) {
    let run = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .arg("machine")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await;
    match run {
        Ok(Ok(out)) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ),
        Ok(Err(e)) => (false, e.to_string()),
        Err(_) => (false, "timed out".to_string()),
    }
}

/// What the product can say about the container runtime right now.
///
/// Separate from `runtime()` because a status check must never be answered from
/// cache: the point is to notice that something which was working has stopped.
pub async fn runtime_status() -> RuntimeStatus {
    for candidate in ["docker", "podman"] {
        if probe(candidate).await {
            return RuntimeStatus::Running { name: candidate };
        }
    }
    let listed = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args(["machine", "list", "--format", "{{.Name}}"])
            .output(),
    )
    .await;
    let has_machine = matches!(listed, Ok(Ok(ref o)) if o.status.success()
        && !String::from_utf8_lossy(&o.stdout).trim().is_empty());
    if has_machine {
        RuntimeStatus::Stopped
    } else {
        RuntimeStatus::NotInstalled
    }
}

/// The three states worth telling apart, because each has a different fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// A runtime is answering.
    Running { name: &'static str },
    /// Podman is installed but its VM is not running — `podman machine start`.
    Stopped,
    /// Nothing to start; the user has to install something.
    NotInstalled,
}

impl RuntimeStatus {
    pub fn as_json(&self) -> serde_json::Value {
        match self {
            RuntimeStatus::Running { name } => serde_json::json!({
                "state": "running", "runtime": name,
            }),
            RuntimeStatus::Stopped => serde_json::json!({
                "state": "stopped",
                "runtime": "podman",
                "detail": "Podman is installed but its virtual machine is not running.",
                "fix": "podman machine start",
            }),
            RuntimeStatus::NotInstalled => serde_json::json!({
                "state": "not_installed",
                "detail": "No container runtime found.",
                "fix": "Install Podman or Docker Desktop.",
            }),
        }
    }
}

async fn probe(runtime: &str) -> bool {
    let cmd = tokio::process::Command::new(runtime)
        .kill_on_drop(true)
        .arg("version")
        .arg("--format")
        .arg("{{.Server.Version}}")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(
        tokio::time::timeout(RUNTIME_TIMEOUT, cmd).await,
        Ok(Ok(s)) if s.success()
    )
}

/// The container name for a hand's service.
///
/// Keyed by hand and tenant, **never by instance**. An instance id is minted
/// fresh on every daemon boot, so naming containers after it meant a restart
/// could never recognise the container it had started last time: it created a
/// new one beside the old, and the service the user had configured was replaced
/// by an empty one. That is the whole reason "my CRM resets after a reboot"
/// happened, and no amount of volume fixing addresses it -- a new container
/// with the right volume still has to be set up again if the old one is still
/// holding the port.
///
/// With a stable name, `start` finds the existing container running and reuses
/// it. Restarting the daemon becomes a no-op for services, which is what a
/// service is supposed to be.
///
/// Two tenants still get separate containers, because the tenant is in the
/// name and it is durable in a way the instance is not.
pub fn container_name(hand_id: &str, tenant: Option<&str>) -> String {
    match tenant {
        Some(t) => format!("freeco-{hand_id}-{t}"),
        None => format!("freeco-{hand_id}"),
    }
}

/// Where a stack's compose file and generated `.env` live.
///
/// Per hand and tenant, matching the container and volume names. Keyed by
/// instance it produced a new directory -- and so a newly generated `.env` with
/// fresh database passwords -- on every restart, which is a second way the same
/// service came back not itself.
pub fn stack_dir(hand_id: &str, tenant: Option<&str>) -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".freeco-ai")
        .join("hands")
        .join(container_name(hand_id, tenant))
}

/// Resolve one environment value against the key store.
///
/// Two forms, because two things are needed. A whole value of `secret:NAME` is
/// the simple case. `${NAME}` substitution exists for values that *embed* a
/// secret -- a Postgres URL is one string containing a password, and splitting
/// it into parts is not an option when the application expects `PG_DATABASE_URL`.
///
/// An unresolved `${NAME}` is left as written rather than blanked, so a missing
/// secret surfaces as a connection refused naming the variable, instead of a
/// database silently accepting an empty password.
pub fn resolve_env_value(raw: &str, resolve_secret: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(key) = raw.strip_prefix("secret:") {
        return resolve_secret(key).unwrap_or_default();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match resolve_secret(key) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push_str("${");
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Network name for a stack instance. Per instance, so two tenants' databases
/// are not on the same network.
pub fn network_name(hand_id: &str, tenant: Option<&str>) -> String {
    project_name(hand_id, tenant)
}

/// The durable name of a volume for a hand's data.
///
/// Keyed by the **hand and its tenant, never the instance**. An instance is
/// bookkeeping that is recreated on every daemon boot; the data is not. Keying
/// volumes by instance meant each restart mounted a fresh empty volume and
/// silently abandoned the previous one — a user who spent an evening
/// configuring OmniRoute found it empty the next morning, six times over, with
/// six orphaned volumes holding six configurations nobody could reach. Nothing
/// was deleted, which is why it produced no error: the data was simply no
/// longer attached to anything.
///
/// The tenant stays in the name because two tenants must not share a database.
/// That was the reason instance ids were used originally, and it is satisfied
/// by the tenant alone, which is durable.
pub fn volume_name(hand_id: &str, tenant: Option<&str>, declared: &str) -> String {
    match tenant {
        Some(t) => format!("freeco-{hand_id}-{t}-{declared}"),
        None => format!("freeco-{hand_id}-{declared}"),
    }
}

/// Container name for one service within a stack.
pub fn service_container_name(hand_id: &str, tenant: Option<&str>, service: &str) -> String {
    format!("{}-{service}", project_name(hand_id, tenant))
}

/// Order services so nothing starts before what it depends on.
///
/// Returns an error naming the cycle rather than looping forever or silently
/// dropping a service: a stack missing its database starts and then fails in a
/// way that looks like a broken image.
fn startup_order(services: &[crate::HandService]) -> Result<Vec<usize>, HandError> {
    let mut done: Vec<usize> = Vec::new();
    let mut placed = vec![false; services.len()];
    while done.len() < services.len() {
        let mut progressed = false;
        for (i, s) in services.iter().enumerate() {
            if placed[i] {
                continue;
            }
            let ready = s.depends_on.iter().all(|d| {
                services
                    .iter()
                    .position(|o| &o.name == d)
                    .is_some_and(|j| placed[j])
            });
            if ready {
                placed[i] = true;
                done.push(i);
                progressed = true;
            }
        }
        if !progressed {
            let stuck: Vec<&str> = services
                .iter()
                .enumerate()
                .filter(|(i, _)| !placed[*i])
                .map(|(_, s)| s.name.as_str())
                .collect();
            return Err(HandError::Config(format!(
                "circular or unknown depends_on among: {}",
                stuck.join(", ")
            )));
        }
    }
    Ok(done)
}

/// Poll a service's `health_cmd` inside its container until it succeeds.
async fn wait_service_healthy(rt: &str, name: &str, cmd: &[String], budget_secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(budget_secs);
    while tokio::time::Instant::now() < deadline {
        let mut c = tokio::process::Command::new(rt);
        c.kill_on_drop(true);
        c.arg("exec").arg(name);
        for part in cmd {
            c.arg(part);
        }
        let ok = matches!(
            tokio::time::timeout(Duration::from_secs(15), c.status()).await,
            Ok(Ok(s)) if s.success()
        );
        if ok {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
}

/// Bring up a natively-defined stack: one network, containers in dependency
/// order, each waited for before its dependents start.
async fn start_native_stack(
    spec: &HandContainer,
    hand_id: &str,
    tenant: Option<&str>,
    resolve_secret: &(impl Fn(&str) -> Option<String> + ?Sized),
) -> Result<String, HandError> {
    let rt = runtime().await.ok_or_else(|| {
        HandError::Config(
            "no container runtime found. Start Docker Desktop, or start Podman with \
             `podman machine start`"
                .into(),
        )
    })?;
    let project = project_name(hand_id, tenant);
    let net = network_name(hand_id, tenant);

    // Idempotent: an existing network from a previous start is fine.
    let _ = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["network", "create", &net])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;

    for idx in startup_order(&spec.services)? {
        let svc = &spec.services[idx];
        let name = service_container_name(hand_id, tenant, &svc.name);

        if is_running(&name).await {
            continue;
        }
        let _ = tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["rm", "-f", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        ensure_image(rt, &svc.image).await?;

        let mut cmd = tokio::process::Command::new(rt);
        cmd.kill_on_drop(true);
        cmd.args(["run", "-d", "--name", &name, "--restart", &spec.restart]);
        // The alias is what makes `postgres://user@db:5432` resolve inside the
        // application container.
        cmd.args(["--network", &net, "--network-alias", &svc.name]);
        for p in &svc.ports {
            cmd.arg("-p").arg(p);
        }
        for v in &svc.volumes {
            // Namespaced so two instances do not share one database volume.
            cmd.arg("-v").arg(volume_name(hand_id, tenant, v));
        }
        for (k, v) in &svc.env {
            let resolved = resolve_env_value(v, &|key| resolve_secret(key));
            cmd.arg("-e").arg(format!("{k}={resolved}"));
        }
        cmd.arg(&svc.image);
        // Command arguments carry `${VAR}` and `secret:NAME` references just as
        // env values do, and only env was being resolved. Redis was started
        // with `--requirepass ${VOICE_REDIS_PASSWORD}` *as a literal string*,
        // so its password was the seven characters of the placeholder while
        // every client was handed the real value. The service came up, looked
        // healthy, and rejected every connection with "invalid username-password
        // pair" -- a config bug that reads as a broken product.
        for part in &svc.command {
            cmd.arg(resolve_env_value(part, &|key| resolve_secret(key)));
        }

        let out = tokio::time::timeout(Duration::from_secs(900), cmd.output())
            .await
            .map_err(|_| HandError::Config(format!("starting {name} timed out")))?
            .map_err(|e| HandError::Config(format!("could not run {rt}: {e}")))?;

        // A missing resolver in the VM stops every container, and the error
        // names resolv.conf rather than DNS. It is not repaired here: the fix
        // runs sudo inside the Podman VM, which is the user's decision, so this
        // fails with the cause and the dashboard fix instead.
        if !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("resolv.conf") {
            return Err(HandError::Config(resolver_missing_message()));
        }

        if !out.status.success() {
            return Err(HandError::Config(format!(
                "{name} failed to start: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        // The health command needs the same resolution: `redis-cli -a
        // ${VOICE_REDIS_PASSWORD} ping` authenticates with the placeholder
        // otherwise, so a healthy service is reported unhealthy.
        let health_cmd: Vec<String> = svc
            .health_cmd
            .iter()
            .map(|part| resolve_env_value(part, &|key| resolve_secret(key)))
            .collect();
        if !health_cmd.is_empty()
            && !wait_service_healthy(rt, &name, &health_cmd, spec.health_timeout_secs).await
        {
            return Err(HandError::Config(format!(
                "{name} started but never became ready. Check its logs: {rt} logs {name}"
            )));
        }
    }

    if let Some(path) = &spec.health_path {
        let port = service_port(spec).ok_or_else(|| {
            HandError::Config(format!(
                "{hand_id} declares health_path {path} but no port to reach it on"
            ))
        })?;
        let url = format!("http://127.0.0.1:{port}{path}");
        if !wait_healthy(&url, spec.health_timeout_secs).await {
            return Err(HandError::Config(format!(
                "{project} started but {url} did not respond within {}s",
                spec.health_timeout_secs
            )));
        }
    }
    Ok(project)
}

/// Bring up a compose stack and wait for it to serve.
///
/// The project name is keyed by hand and tenant, not by instance id. `compose
/// down` targets a project, so two tenants sharing one would tear down each
/// other's databases -- but an instance id is minted afresh on every restore,
/// and keying on it gave each daemon restart a new project on empty volumes.
async fn start_stack(
    spec: &HandContainer,
    hand_id: &str,
    tenant: Option<&str>,
    resolve_secret: impl Fn(&str) -> Option<String>,
) -> Result<String, HandError> {
    let rt = runtime().await.ok_or_else(|| {
        HandError::Config(
            "no container runtime found. Start Docker Desktop, or start Podman with \
             `podman machine start`"
                .into(),
        )
    })?;
    let compose = spec.compose.as_ref().expect("caller checked is_stack");
    let dir = stack_dir(hand_id, tenant);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("docker-compose.yml"), compose)?;

    // Secrets go in a compose `.env` rather than on the command line, where
    // every process on the machine could read them from the process list.
    if !spec.env.is_empty() {
        let mut lines = String::new();
        for (k, v) in &spec.env {
            let resolved = match v.strip_prefix("secret:") {
                Some(key) => resolve_secret(key).unwrap_or_default(),
                None => v.clone(),
            };
            lines.push_str(&format!("{k}={resolved}\n"));
        }
        let env_path = dir.join(".env");
        std::fs::write(&env_path, lines)?;
        harden_file(&env_path);
    }

    let project = project_name(hand_id, tenant);
    let out = tokio::time::timeout(
        Duration::from_secs(900), // first run pulls images; a CRM stack is gigabytes
        tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["compose", "-p", &project, "up", "-d"])
            .current_dir(&dir)
            .output(),
    )
    .await
    .map_err(|_| HandError::Config(format!("{rt} compose up timed out for {hand_id}")))?
    .map_err(|e| HandError::Config(format!("could not run {rt} compose: {e}")))?;
    if !out.status.success() {
        return Err(HandError::Config(format!(
            "{rt} compose up failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    if let Some(path) = &spec.health_path {
        let port = service_port(spec).ok_or_else(|| {
            HandError::Config(format!(
                "{hand_id} declares health_path {path} but no port: add a `ports` entry, \
                 or `PORT` under [container.env]"
            ))
        })?;
        let url = format!("http://127.0.0.1:{port}{path}");
        if !wait_healthy(&url, spec.health_timeout_secs).await {
            return Err(HandError::Config(format!(
                "{project} started but {url} did not respond within {}s. \
                 Check its logs: {rt} compose -p {project} logs",
                spec.health_timeout_secs
            )));
        }
    }
    Ok(project)
}

/// Project name for a hand's stack. Durable, for the same reason as
/// `container_name`.
pub fn project_name(hand_id: &str, tenant: Option<&str>) -> String {
    container_name(hand_id, tenant)
}

/// Restrict a generated file to the current user.
///
/// The compose `.env` holds real database passwords, so it must not be left
/// readable to other accounts on the machine.
fn harden_file(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path; // Windows inherits the user-profile ACL
    }
}

/// Is this hand's container running right now?
pub async fn is_running(name: &str) -> bool {
    let Some(rt) = runtime().await else {
        return false;
    };
    let out = tokio::process::Command::new(rt)
        .kill_on_drop(true)
        .args([
            "ps",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Names}}",
        ])
        .output();
    match tokio::time::timeout(RUNTIME_TIMEOUT, out).await {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim() == name),
        _ => false,
    }
}

/// Start the container for a hand instance, and wait until it actually serves.
///
/// Returns once the health check passes, or an error saying which stage failed.
/// "Started" on its own is not a useful claim: a container that logs `Ready`
/// and serves nothing is the failure this is written to catch.
pub async fn start(
    spec: &HandContainer,
    hand_id: &str,
    // Scopes the data volumes and every name. Durable, unlike an instance id,
    // which is why no instance id is taken here at all.
    tenant: Option<&str>,
    resolve_secret: impl Fn(&str) -> Option<String>,
) -> Result<String, HandError> {
    // A stack is a different lifecycle: compose owns the containers, so start,
    // stop and liveness all have to go through it rather than through names we
    // made up.
    if spec.is_native_stack() {
        return start_native_stack(spec, hand_id, tenant, &resolve_secret).await;
    }
    if spec.is_stack() {
        return start_stack(spec, hand_id, tenant, resolve_secret).await;
    }

    let rt = runtime()
        .await
        .ok_or_else(|| HandError::Config(
            "no container runtime found. Start Docker Desktop, or start Podman with `podman machine start`".into(),
        ))?;

    let name = container_name(hand_id, tenant);
    // One set of containers per hand. Restoring a hand mints a new instance id,
    // so without this each daemon restart leaves the previous set running.
    if is_running(&name).await {
        return Ok(name);
    }

    // Remove any stopped container of the same name, or `run` fails on the
    // leftover rather than starting.
    let _ = tokio::process::Command::new(rt)
        .kill_on_drop(true)
        .args(["rm", "-f", &name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    ensure_image(rt, &spec.image).await?;

    let mut cmd = tokio::process::Command::new(rt);
    cmd.kill_on_drop(true);
    cmd.args(["run", "-d", "--name", &name, "--restart", &spec.restart]);

    if spec.host_network {
        cmd.arg("--network").arg("host");
    } else {
        for p in &spec.ports {
            cmd.arg("-p").arg(p);
        }
    }
    for v in &spec.volumes {
        // Namespaced per instance, as the stack path already does. Passed
        // through raw, a named volume like `data:/app/data` claims the global
        // name `data`, which any other hand declaring the same obvious name
        // would then share -- two services silently reading each other's
        // database. A host path (starting with / or a drive letter) is left
        // alone: that one the author meant literally.
        let looks_like_host_path =
            v.starts_with('/') || v.starts_with('.') || v.chars().nth(1) == Some(':');
        if looks_like_host_path {
            cmd.arg("-v").arg(v);
        } else {
            cmd.arg("-v").arg(volume_name(hand_id, tenant, v));
        }
    }
    for (k, v) in &spec.env {
        // A value written as `secret:NAME` is looked up in the key store, so
        // tokens stay out of the hand definition and out of git.
        let resolved = match v.strip_prefix("secret:") {
            Some(key) => resolve_secret(key).unwrap_or_default(),
            None => v.clone(),
        };
        cmd.arg("-e").arg(format!("{k}={resolved}"));
    }
    cmd.arg(&spec.image);

    let out = tokio::time::timeout(Duration::from_secs(90), cmd.output())
        .await
        .map_err(|_| HandError::Config(format!("{rt} run timed out")))?
        .map_err(|e| HandError::Config(format!("could not run {rt}: {e}")))?;

    // Same as the stack path: a VM missing /etc/resolv.conf stops every
    // container. Reported with the dashboard fix rather than repaired here.
    if !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("resolv.conf") {
        return Err(HandError::Config(resolver_missing_message()));
    }
    if !out.status.success() {
        return Err(HandError::Config(format!(
            "{rt} run failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    if let Some(path) = &spec.health_path {
        // No derivable port is a definition error, and saying so beats waiting
        // the full health budget to report a timeout that names nothing.
        let port = service_port(spec).ok_or_else(|| {
            HandError::Config(format!(
                "{name} declares health_path {path} but no port: add a `ports` entry, \
                 or `PORT` under [container.env] for a host-network hand"
            ))
        })?;
        let url = format!("http://127.0.0.1:{port}{path}");
        if !wait_healthy(&url, spec.health_timeout_secs).await {
            return Err(HandError::Config(format!(
                "{name} started but {url} did not respond within {}s. \
                 Check its logs: {rt} logs {name}",
                spec.health_timeout_secs
            )));
        }
    }
    Ok(name)
}

/// Stop the container or stack, leaving its volumes intact.
///
/// `compose down` without `-v` keeps named volumes, so pausing a CRM does not
/// destroy the customer data in it. Removing that data has to be a separate,
/// deliberate act.
pub async fn stop_native_stack(
    spec: &HandContainer,
    hand_id: &str,
    tenant: Option<&str>,
) -> Result<(), HandError> {
    let Some(rt) = runtime().await else {
        return Ok(());
    };
    // Reverse dependency order, so an application is stopped before the
    // database it is talking to.
    let mut order = startup_order(&spec.services)?;
    order.reverse();
    for idx in order {
        let name = service_container_name(hand_id, tenant, &spec.services[idx].name);
        let _ = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(rt)
                .kill_on_drop(true)
                .args(["stop", &name])
                .status(),
        )
        .await;
        // Removed, not just stopped: the container is recreated on next start,
        // while the named volumes -- the actual data -- are left untouched.
        let _ = tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["rm", "-f", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = tokio::process::Command::new(rt)
        .kill_on_drop(true)
        .args(["network", "rm", &network_name(hand_id, tenant)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    Ok(())
}

pub async fn stop(hand_id: &str, tenant: Option<&str>, is_stack: bool) -> Result<(), HandError> {
    let Some(rt) = runtime().await else {
        return Ok(()); // nothing to stop if no runtime
    };
    if is_stack {
        let project = project_name(hand_id, tenant);
        let dir = stack_dir(hand_id, tenant);
        let _ = tokio::time::timeout(
            Duration::from_secs(180),
            tokio::process::Command::new(rt)
                .kill_on_drop(true)
                .args(["compose", "-p", &project, "down"])
                .current_dir(&dir)
                .status(),
        )
        .await;
        return Ok(());
    }
    let name = container_name(hand_id, tenant);
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(rt)
            .kill_on_drop(true)
            .args(["stop", &name])
            .status(),
    )
    .await;
    Ok(())
}

/// The host side of the first published port, for health checks and for
/// telling the user where the service is.
pub fn first_host_port(spec: &HandContainer) -> Option<String> {
    spec.ports
        .first()
        .and_then(|p| p.split(':').next())
        .map(|s| s.to_string())
}

/// The port this service answers on from the host.
///
/// A host-network hand publishes nothing -- there is no mapping to read,
/// because the container shares the host's stack and simply listens. OmniRoute
/// is exactly this: `host_network = true`, no `ports`, and `PORT` in its env.
/// Reading only `ports` gave an empty string, so the health URL came out as
/// `http://127.0.0.1:/v1/models`, which cannot succeed -- the first activation
/// of the only service hand would have waited its full 180 second budget and
/// then rolled back, reporting a timeout rather than a missing port.
pub fn service_port(spec: &HandContainer) -> Option<String> {
    first_host_port(spec).or_else(|| spec.env.get("PORT").cloned())
}

async fn wait_healthy(url: &str, budget_secs: u64) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        // Redirects are the answer, not a step on the way to it: a service that
        // sends one is already serving. Following it would test wherever it
        // points, which for a first-run wizard is a different page entirely.
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(budget_secs);
    while tokio::time::Instant::now() < deadline {
        // Any HTTP response means something is listening and answering, which
        // is what this checks. Requiring 2xx failed Akaunting, which redirects
        // its root to a setup wizard until the first run is complete: the
        // service was up, answering 302, and reported as never having started
        // for ten minutes before being rolled back. A 401 or a 404 says the
        // same thing -- the process is alive and handling requests. Only a
        // connection failure means it is not.
        //
        // `services.rs` had this right and this file disagreed with it; the
        // rule now matches in both places.
        if client.get(url).send().await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
}

/// How long a status check waits for a service's health path. Status is
/// read by people and by agents; a wedged service must not hang either.
pub const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// One HTTP probe of a service hand's `health_path`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HealthProbe {
    pub url: String,
    /// HTTP status code; `None` when nothing answered.
    pub status: Option<u16>,
    /// Time to the response (or to the failure), in milliseconds.
    pub latency_ms: u64,
    /// Answered with a status below 500. A redirect, 401 or 404 still means
    /// the process is up and handling requests (the activation rule); a 5xx
    /// means it is up but failing.
    pub ok: bool,
    /// Why nothing answered (connection refused, timeout, ...).
    pub error: Option<String>,
}

/// The loopback URL of a service hand's health path, or `None` when it
/// declares no `health_path` or has no numeric port to reach it on.
pub fn health_url(spec: &HandContainer) -> Option<String> {
    let path = spec.health_path.as_deref()?.trim();
    let port: u16 = service_port(spec)?.trim().parse().ok()?;
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Some(format!("http://127.0.0.1:{port}{path}"))
}

/// Is `url` an `http://` URL on this machine's loopback interface?
fn is_loopback_http(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "http" {
        return false;
    }
    let host = u.host_str().unwrap_or("");
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// GET `url` once, with `timeout`, and report status and latency. Only
/// loopback URLs are probed: this runs from a status call any agent can
/// make, and must not become a way to reach other hosts.
pub async fn probe_health(url: &str, timeout: Duration) -> HealthProbe {
    let started = std::time::Instant::now();
    let failed = |error: String| HealthProbe {
        url: url.to_string(),
        status: None,
        latency_ms: started.elapsed().as_millis() as u64,
        ok: false,
        error: Some(error),
    };
    if !is_loopback_http(url) {
        return failed("not a loopback http URL; refused".to_string());
    }
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        // A proxy from the environment would carry a loopback request elsewhere.
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => return failed(e.to_string()),
    };
    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            HealthProbe {
                url: url.to_string(),
                status: Some(status),
                latency_ms: started.elapsed().as_millis() as u64,
                ok: status < 500,
                error: None,
            }
        }
        Err(e) => failed(if e.is_timeout() {
            format!("no answer within {} ms", timeout.as_millis())
        } else if e.is_connect() {
            "connection refused or unreachable".to_string()
        } else {
            e.to_string()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HandContainer {
        HandContainer {
            image: "example/thing:1".into(),
            services: vec![],
            compose: None,
            ports: vec!["20128:20128".into()],
            volumes: vec![],
            env: Default::default(),
            health_path: Some("/health".into()),
            health_timeout_secs: 30,
            host_network: false,
            restart: "unless-stopped".into(),
            mcp_url: None,
        }
    }

    /// A restart must reuse the same container, not create a rival.
    ///
    /// This test previously asserted the opposite -- that two instances get
    /// distinct names -- which is exactly what made a service come back empty
    /// after every reboot: the new instance could not see the container the old
    /// one had left running, so it built a second one beside it.
    #[test]
    fn a_hand_keeps_one_container_across_restarts() {
        assert_eq!(container_name("crm", None), container_name("crm", None));
        assert_eq!(container_name("crm", None), "freeco-crm");
    }

    /// Tenants still get their own, because the tenant is durable.
    #[test]
    fn tenants_get_separate_containers() {
        assert_ne!(
            container_name("crm", Some("acme")),
            container_name("crm", Some("globex"))
        );
    }

    #[test]
    fn host_port_is_the_left_half() {
        assert_eq!(first_host_port(&spec()).as_deref(), Some("20128"));
    }

    #[test]
    fn no_ports_means_no_host_port() {
        let mut s = spec();
        s.ports.clear();
        assert_eq!(first_host_port(&s), None);
    }

    /// A host-network hand publishes no ports and still serves one. OmniRoute
    /// is shaped exactly like this, and reading `ports` alone produced an empty
    /// port in the health URL -- a check that could never pass.
    #[test]
    fn host_network_port_comes_from_the_env() {
        let mut s = spec();
        s.ports.clear();
        s.host_network = true;
        s.env.insert("PORT".into(), "20128".into());
        assert_eq!(service_port(&s).as_deref(), Some("20128"));
    }

    /// A published mapping still wins: it is what the host can actually reach,
    /// while `PORT` is what the process listens on inside the container.
    #[test]
    fn published_ports_win_over_the_env() {
        let mut s = spec();
        s.env.insert("PORT".into(), "3000".into());
        assert_eq!(service_port(&s).as_deref(), Some("20128"));
    }

    #[test]
    fn no_port_anywhere_is_reported_as_missing() {
        let mut s = spec();
        s.ports.clear();
        assert_eq!(service_port(&s), None);
    }

    fn svc(name: &str, deps: &[&str]) -> crate::HandService {
        crate::HandService {
            name: name.into(),
            image: "x".into(),
            env: Default::default(),
            ports: vec![],
            volumes: vec![],
            command: vec![],
            health_cmd: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// An application started before its database crashes on first boot, so
    /// order is not cosmetic.
    #[test]
    fn dependencies_start_first() {
        let services = vec![svc("app", &["db"]), svc("db", &[])];
        let order = startup_order(&services).unwrap();
        let names: Vec<&str> = order.iter().map(|i| services[*i].name.as_str()).collect();
        assert_eq!(names, vec!["db", "app"]);
    }

    /// A cycle must be reported, not spun on forever or silently truncated.
    #[test]
    fn circular_dependencies_are_an_error() {
        let services = vec![svc("a", &["b"]), svc("b", &["a"])];
        let err = startup_order(&services).unwrap_err();
        assert!(format!("{err}").contains("circular"));
    }

    /// A dependency naming a service that does not exist is the same failure:
    /// it can never become ready, so waiting for it would hang forever.
    #[test]
    fn unknown_dependencies_are_an_error() {
        let services = vec![svc("app", &["nonexistent"])];
        assert!(startup_order(&services).is_err());
    }

    /// A password inside a connection string has to be substituted: the
    /// application wants one `PG_DATABASE_URL`, so the secret cannot be a
    /// separate variable.
    #[test]
    fn secrets_substitute_inside_a_value() {
        let get = |k: &str| (k == "PW").then(|| "hunter2".to_string());
        assert_eq!(
            resolve_env_value("postgres://user:${PW}@db:5432/app", &get),
            "postgres://user:hunter2@db:5432/app"
        );
    }

    #[test]
    fn a_whole_value_secret_still_works() {
        let get = |k: &str| (k == "TOKEN").then(|| "abc".to_string());
        assert_eq!(resolve_env_value("secret:TOKEN", &get), "abc");
    }

    /// Left as written, not blanked. An empty password is one a database
    /// accepts, turning a missing secret into a silent misconfiguration
    /// instead of a connection error that names the variable.
    #[test]
    fn an_unresolved_placeholder_is_left_visible() {
        let get = |_: &str| None;
        assert_eq!(
            resolve_env_value("postgres://user:${MISSING}@db/app", &get),
            "postgres://user:${MISSING}@db/app"
        );
    }

    /// Data must outlive the instance that happened to be running.
    ///
    /// Instances are recreated on every daemon boot. Keying volumes by instance
    /// meant each restart mounted a fresh empty one and abandoned the previous
    /// -- an evening of configuring OmniRoute was lost six times this way, with
    /// six orphaned volumes and no error, because nothing was deleted: the data
    /// was simply no longer attached to anything.
    #[test]
    fn volume_names_do_not_change_between_instances() {
        let a = volume_name("omniroute", None, "data");
        let b = volume_name("omniroute", None, "data");
        assert_eq!(a, b, "the same hand must reuse the same volume");
        assert!(!a.contains('-') || !a.matches('-').count() > 3);
        assert_eq!(a, "freeco-omniroute-data");
    }

    /// Two tenants must not share one database. This is the reason instance ids
    /// were used originally, and the tenant satisfies it while staying durable.
    #[test]
    fn tenants_get_separate_volumes() {
        let acme = volume_name("crm", Some("acme"), "db-data");
        let globex = volume_name("crm", Some("globex"), "db-data");
        assert_ne!(acme, globex);
        assert_eq!(acme, "freeco-crm-acme-db-data");
    }

    /// Different declared volumes within one hand stay distinct.
    #[test]
    fn declared_volumes_stay_distinct() {
        assert_ne!(
            volume_name("crm", None, "db-data"),
            volume_name("crm", None, "server-local-data")
        );
    }

    /// A stack takes a different path through start/stop entirely, so the
    /// distinction has to be visible from the definition alone.
    #[test]
    fn a_compose_definition_is_a_stack() {
        let mut s = spec();
        assert!(!s.is_stack());
        s.compose = Some("services: {}".into());
        assert!(s.is_stack());
    }

    /// `compose down` targets a project name. Two instances sharing one would
    /// tear down each other's containers -- and for a CRM or a ledger that
    /// means someone else's database going away.
    #[test]
    fn stack_projects_are_per_tenant_not_per_instance() {
        assert_ne!(
            project_name("crm", Some("acme")),
            project_name("crm", Some("globex"))
        );
        // And stable for one tenant, so a restart adopts the running stack.
        assert_eq!(
            project_name("crm", Some("acme")),
            project_name("crm", Some("acme"))
        );
    }

    /// The compose file and its generated `.env` follow the same key. Keyed by
    /// instance, every restart wrote a new `.env` with fresh database
    /// passwords, so the stack came back unable to open its own database.
    #[test]
    fn stack_dirs_are_stable_per_tenant() {
        assert_eq!(
            stack_dir("crm", Some("acme")),
            stack_dir("crm", Some("acme"))
        );
        assert_ne!(
            stack_dir("crm", Some("acme")),
            stack_dir("crm", Some("globex"))
        );
    }

    /// A tenant must not see another tenant's services.
    ///
    /// This is the whole point of the field: a CRM instance holds one
    /// customer's data, and "list my services" returning someone else's is a
    /// data leak rather than a display bug.
    #[test]
    fn tenants_do_not_see_each_others_instances() {
        use crate::HandInstance;
        use std::collections::HashMap;
        let acme = HandInstance::new("crm", "agent", HashMap::new(), None).for_tenant("acme");
        assert!(acme.belongs_to(Some("acme")));
        assert!(!acme.belongs_to(Some("globex")));
        // An owned instance is not visible to an untenanted request either,
        // or single-tenant callers would see every tenant's services.
        assert!(!acme.belongs_to(None));
    }

    /// A local install has no tenants; its own services must stay visible.
    #[test]
    fn ownerless_instances_are_visible_to_everyone() {
        use crate::HandInstance;
        use std::collections::HashMap;
        let local = HandInstance::new("omniroute", "agent", HashMap::new(), None);
        assert_eq!(local.tenant_id, None);
        assert!(local.belongs_to(None));
        assert!(local.belongs_to(Some("acme")));
    }

    /// The fix endpoint passes a request's id straight here, so this is the
    /// whole guarantee that no request can choose what runs with sudo.
    #[test]
    fn vm_fix_command_knows_only_its_two_fixed_commands() {
        for id in [
            "",
            "RESOLVER",
            "resolver ",
            "resolver; rm -rf /",
            "../resolver",
            "rm -rf /",
        ] {
            assert_eq!(
                vm_fix_command(id),
                None,
                "id {id:?} must not map to a command"
            );
        }
        assert_eq!(
            vm_fix_command(VM_ISSUE_RESTART_SERVICE).as_deref(),
            Some("sudo -n systemctl --global enable podman-restart.service")
        );
        let resolver = vm_fix_command(VM_ISSUE_RESOLVER).expect("resolver fix");
        assert!(
            resolver.starts_with("sudo -n "),
            "must not wait on a password prompt"
        );
        assert!(
            resolver.ends_with("> /etc/resolv.conf'"),
            "must write only resolv.conf"
        );
    }

    /// Whatever starts a hand -- the dashboard or an agent -- is told the cause
    /// and where the fix is, not handed a bare container error.
    #[test]
    fn resolver_failure_names_the_cause_and_the_dashboard_fix() {
        let msg = resolver_missing_message();
        assert!(msg.contains("/etc/resolv.conf"));
        assert!(msg.contains("Settings > Services"));
        assert!(msg.contains("Nothing was changed"));
    }

    #[test]
    fn health_url_is_loopback_with_the_service_port() {
        assert_eq!(
            health_url(&spec()).as_deref(),
            Some("http://127.0.0.1:20128/health")
        );
        let mut s = spec();
        s.health_path = Some("healthz".into());
        assert_eq!(
            health_url(&s).as_deref(),
            Some("http://127.0.0.1:20128/healthz")
        );
        // Host-network hand: the port comes from PORT.
        let mut s = spec();
        s.ports.clear();
        s.env.insert("PORT".into(), "3200".into());
        assert_eq!(
            health_url(&s).as_deref(),
            Some("http://127.0.0.1:3200/health")
        );
        // No health path, or no usable port: nothing to probe.
        let mut s = spec();
        s.health_path = None;
        assert!(health_url(&s).is_none());
        let mut s = spec();
        s.ports = vec!["evil.example:80".into()];
        assert!(health_url(&s).is_none());
    }

    /// Serve one canned HTTP response on a loopback port; returns the port.
    async fn one_shot_server(response: &'static str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        port
    }

    #[tokio::test]
    async fn probe_reports_status_and_latency() {
        let port =
            one_shot_server("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        let url = format!("http://127.0.0.1:{port}/health");
        let probe = probe_health(&url, Duration::from_secs(3)).await;
        assert_eq!(probe.status, Some(204));
        assert!(probe.ok);
        assert!(probe.error.is_none());
        assert!(probe.latency_ms < 3000);
        assert_eq!(probe.url, url);
    }

    #[tokio::test]
    async fn probe_counts_redirects_as_up_and_5xx_as_failing() {
        let port = one_shot_server(
            "HTTP/1.1 302 Found\r\nLocation: /setup\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let probe = probe_health(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(3)).await;
        assert_eq!(probe.status, Some(302));
        assert!(probe.ok);

        let port = one_shot_server(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let probe = probe_health(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(3)).await;
        assert_eq!(probe.status, Some(503));
        assert!(!probe.ok);
    }

    #[tokio::test]
    async fn probe_reports_nothing_listening() {
        // Bind and drop to get a port nothing listens on.
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let probe = probe_health(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(2)).await;
        assert_eq!(probe.status, None);
        assert!(!probe.ok);
        assert!(probe.error.is_some());
    }

    #[tokio::test]
    async fn probe_times_out_quickly() {
        // Accepts the connection and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _held = listener.accept().await;
            tokio::time::sleep(Duration::from_millis(800)).await;
        });
        let probe =
            probe_health(&format!("http://127.0.0.1:{port}/"), Duration::from_millis(300)).await;
        assert_eq!(probe.status, None);
        assert!(!probe.ok);
        assert!(probe.latency_ms < 5000);
    }

    #[tokio::test]
    async fn probe_refuses_non_loopback_urls() {
        for url in [
            "http://example.com/health",
            "http://10.0.0.1:80/",
            "https://127.0.0.1:443/",
            "http://127.0.0.1.evil.example/",
            "file:///etc/passwd",
        ] {
            let probe = probe_health(url, Duration::from_millis(200)).await;
            assert_eq!(probe.status, None, "{url}");
            assert!(probe.error.as_deref().unwrap_or("").contains("loopback"), "{url}");
        }
    }
}

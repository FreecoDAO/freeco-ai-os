//! Docker container sandbox — OS-level isolation for agent code execution.
//!
//! Provides secure command execution inside Docker containers with strict
//! resource limits, network isolation, and capability dropping.

use freeco_types::config::DockerSandboxConfig;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, warn};

/// A running sandbox container.
#[derive(Debug, Clone)]
pub struct SandboxContainer {
    pub container_id: String,
    pub agent_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Hosting a service, so the pool must not reap it.
    pub is_service: bool,
    /// What this container is for. Decides whether the pool may reap it.
    pub tier: freeco_types::config::SandboxTier,
    /// The binary that created this container.
    ///
    /// Carried per container rather than read from config at each call: a
    /// container created by one runtime has to be exec'd into and removed by
    /// the same one, and the config can change between those calls.
    pub runtime: String,
}

/// Result of executing a command in the sandbox.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// SECURITY: Sanitize container name — alphanumeric + dash only.
fn sanitize_container_name(name: &str) -> Result<String, String> {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        return Err("Container name cannot be empty".into());
    }
    if sanitized.len() > 63 {
        return Err("Container name too long (max 63 chars)".into());
    }
    Ok(sanitized)
}

/// SECURITY: Validate Docker image name — only allow safe characters.
fn validate_image_name(image: &str) -> Result<(), String> {
    if image.is_empty() {
        return Err("Docker image name cannot be empty".into());
    }
    // Allow: alphanumeric, dots, colons, slashes, dashes, underscores, and
    // `@` for content-addressed image digests.
    if !image
        .chars()
        .all(|c| c.is_alphanumeric() || ".:/-_@".contains(c))
    {
        return Err(format!("Invalid Docker image name: {image}"));
    }
    Ok(())
}

/// Validate a command destined for the sandbox container.
///
/// Deliberately does NOT filter shell metacharacters. That filter exists for
/// `subprocess_sandbox`, which runs commands on the user's own machine, where
/// limiting `&&`, `;` and `${}` genuinely reduces the blast radius of injected
/// input.
///
/// Inside this sandbox the reasoning inverts. The container is the security
/// boundary: no network, all capabilities dropped, no privilege escalation,
/// read-only root, capped memory/CPU/PIDs, and destroyed afterwards. Running
/// arbitrary shell in there is the entire point of having it.
///
/// The filter also bought nothing. Anyone who can set the command can put
/// whatever they like in a *single* command; chaining is a convenience, not an
/// escalation. So it stopped no attack while blocking ordinary work -- an
/// agent could not run `which git && git --version`, a Python one-liner with a
/// semicolon, or anything using `${VAR}`. A control that blocks legitimate use
/// and prevents nothing is worse than no control: it pushes work back onto the
/// host, which is exactly what the sandbox was built to avoid.
fn validate_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("Command cannot be empty".into());
    }
    Ok(())
}

/// Return the host identity that owns a mounted workspace.
///
/// A sandbox runs without `CAP_DAC_OVERRIDE`, so container root cannot access a
/// restrictive (for example `0700`) workspace owned by the host user. Running
/// as that workspace owner keeps mounts usable without restoring that capability
/// or broadening host directory permissions.
fn workspace_owner(workspace: &Path) -> Result<Option<String>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(workspace)
            .map_err(|error| format!("Failed to inspect Docker workspace: {error}"))?;
        Ok(Some(format!("{}:{}", metadata.uid(), metadata.gid())))
    }

    #[cfg(not(unix))]
    {
        let _ = workspace;
        Ok(None)
    }
}

/// Check if Docker is available on this system.
/// Probe one runtime, bounded.
///
/// While Docker Desktop is starting, or when its backend is wedged,
/// `docker version` blocks indefinitely rather than failing. This probe had no
/// timeout, so every caller inherited the hang: an agent asking to run one
/// command would sit there forever with nothing reported. A probe that cannot
/// fail is worse than one that returns false -- false is recoverable and says
/// so. Five seconds is far longer than a healthy daemon needs and short enough
/// that a person notices nothing.
pub async fn is_docker_available(runtime: &str) -> bool {
    let probe = crate::quiet_command::quiet_async(runtime)
        .arg("version")
        .arg("--format")
        .arg("{{.Server.Version}}")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match tokio::time::timeout(Duration::from_secs(5), probe).await {
        Ok(Ok(output)) => output.status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            warn!("{runtime} did not respond within 5s; treating it as unavailable");
            false
        }
    }
}

/// Which container runtime to shell out to: `docker` or `podman`.
///
/// Docker was assumed everywhere, so the sandbox did not exist at all for
/// anyone running Podman -- the rootless default on Fedora and RHEL. The agent
/// was told to work in a container it had no way to start, leaving the host as
/// the only route, which is precisely what the sandbox exists to prevent.
///
/// Docker is preferred when its daemon answers, so existing installs are
/// untouched; Podman is used when it does not. Both take identical arguments
/// for everything here, so only the executable name changes.
///
/// Cached: this runs on every sandboxed tool call, and probing each time would
/// add two process spawns to all of them. The timeout above applies to each
/// probe, so a wedged Docker costs five seconds once rather than forever.
pub async fn container_runtime() -> Option<&'static str> {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Option<&'static str>> = OnceLock::new();
    if let Some(cached) = RUNTIME.get() {
        return *cached;
    }
    let mut found = None;
    for candidate in ["docker", "podman"] {
        if is_docker_available(candidate).await {
            found = Some(candidate);
            break;
        }
    }
    let _ = RUNTIME.set(found);
    found
}
/// Check whether an image is already in the local Docker cache.
///
/// `docker run` silently pulls a missing image, which on a metered or slow
/// connection means an unannounced multi-hundred-megabyte download. We check
/// first so the user is told what it costs and decides when to pay it.
pub async fn is_image_present(runtime: &str, image: &str) -> bool {
    match crate::quiet_command::quiet_async(runtime)
        .arg("image")
        .arg("inspect")
        .arg(image)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

/// Resolve a mutable image tag to the immutable digest it currently points at.
///
/// Reusable sandboxes require a digest, because a tag can be repointed under a
/// long-lived container and nothing would notice. Requiring the *user* to write
/// the digest is what made that rule unusable in practice: the configured image
/// was a tag, so `docker_exec` refused every call on every tier, and agents
/// could not run a command, install a tool, or touch a container at all.
///
/// Resolving it here keeps the guarantee — the container still runs a specific,
/// content-addressed image — while removing the manual step. The digest is read
/// at the moment of use, so repointing the tag takes effect on the next sandbox
/// rather than silently mid-life.
///
/// Returns `None` when the image is absent or the runtime reports no digest, in
/// which case the caller must refuse rather than fall back to the tag.
pub async fn resolve_image_digest(runtime: &str, image: &str) -> Option<String> {
    if is_immutable_image_reference(image) {
        return Some(image.to_string());
    }
    // `RepoDigests` first -- that is the manifest digest the registry and the
    // runtime both address the image by.
    //
    // Not `.Id`: that is the *config blob* digest, a different hash entirely.
    // Pasting it after `repo@sha256:` produces a reference that looks right and
    // resolves to nothing, so the sandbox reported the image as "not downloaded
    // yet" for an image sitting in local storage. On this machine `.Id` is
    // 73169df1... while the real repo digest is 3dc62ec6... -- no relation.
    let inspect = |format: &str| {
        let mut c = crate::quiet_command::quiet_async(runtime);
        c.arg("image")
            .arg("inspect")
            .arg(image)
            .arg("--format")
            .arg(format)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        c
    };

    if let Ok(out) = inspect("{{if .RepoDigests}}{{index .RepoDigests 0}}{{end}}")
        .output()
        .await
    {
        if out.status.success() {
            let reference = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if is_immutable_image_reference(&reference) {
                return Some(reference);
            }
        }
    }

    // An image built locally and never pushed has no repo digest at all. Its
    // id is still content-addressed and still immutable, and the runtime
    // accepts it as a reference, so it satisfies the same guarantee -- the
    // container cannot silently become a different image.
    let out = inspect("{{.Id}}").output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let bare = id.strip_prefix("sha256:").unwrap_or(&id);
    if bare.len() != 64 || !bare.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("sha256:{bare}"))
}

/// Create and start a sandbox container for an agent.
pub async fn create_sandbox(
    config: &DockerSandboxConfig,
    agent_id: &str,
    workspace: &Path,
    writable_workspace: bool,
) -> Result<SandboxContainer, String> {
    validate_image_name(&config.image)?;
    let workspace_str = workspace
        .to_str()
        .ok_or_else(|| "Workspace path is not valid UTF-8".to_string())?;
    validate_bind_mount(workspace_str, &config.blocked_mounts)?;

    // Refuse rather than trigger a surprise download. Once the image is
    // cached this check costs milliseconds and never fires again.
    if !is_image_present(&config.runtime, &config.image).await {
        return Err(format!(
            "Sandbox image '{}' is not downloaded yet, and nothing was run. \
             Fetching it needs a one-time download, so it is not started behind \
             your back. \
             Run `{} pull {}` when you are ready, or set docker.image in \
             ~/.freeco-ai/config.toml to an image you already have.",
            config.image, config.runtime, config.image
        ));
    }
    let container_name = sandbox_container_name(config, agent_id, workspace)?;
    let workspace_user = workspace_owner(workspace)?;
    let runtime_bin = container_runtime().await.unwrap_or("docker");

    // What this sandbox is: the pool's config hash, plus the tier and whether
    // the workspace is writable, which change the container's limits and mounts
    // without necessarily changing that hash.
    let fingerprint = format!(
        "{:016x}-{:?}-{}",
        config_hash(config),
        config.tier,
        if writable_workspace { "rw" } else { "ro" }
    );

    // Pick up the sandbox an earlier process left behind instead of colliding
    // with it. See `SANDBOX_FINGERPRINT_LABEL` for why this has to exist.
    if let Some(existing) = inspect_existing_sandbox(runtime_bin, &container_name).await {
        let same = existing.fingerprint.as_deref() == Some(fingerprint.as_str());
        if same && (existing.running || start_existing_sandbox(runtime_bin, &container_name).await)
        {
            debug!(container = %container_name, "Reusing existing sandbox");
            return Ok(SandboxContainer {
                container_id: existing.id,
                agent_id: agent_id.to_string(),
                created_at: chrono::Utc::now(),
                is_service: config.service,
                tier: config.tier,
                runtime: config.runtime.clone(),
            });
        }
        // A different sandbox under this name -- made from another image or
        // config, unlabelled from before labels existed -- or one that will not
        // start. The workspace is a bind mount, so removing the container loses
        // nothing that was meant to persist.
        remove_sandbox_by_name(runtime_bin, &container_name).await?;
    }

    let mut cmd = crate::quiet_command::quiet_async(runtime_bin);
    cmd.arg("run").arg("-d").arg("--name").arg(&container_name);
    cmd.arg("--label")
        .arg(format!("{SANDBOX_FINGERPRINT_LABEL}={fingerprint}"));

    // Package-manager caches somewhere writable. The root filesystem is
    // read-only and so is `$HOME`, so `npm install` failed outright (exit 1)
    // trying to write `~/.npm`, and pip ran with its cache silently disabled.
    // A writable workspace keeps the cache on disk, where it survives the
    // container and a real front-end install fits; otherwise it goes to the
    // tmpfs, which is small and lost on stop but is all that sandbox has.
    let cache_root = if writable_workspace {
        format!("{}/.cache", config.workdir)
    } else {
        "/tmp/.cache".to_string()
    };
    cmd.arg("-e").arg(format!("XDG_CACHE_HOME={cache_root}"));
    cmd.arg("-e")
        .arg(format!("npm_config_cache={cache_root}/npm"));
    cmd.arg("-e").arg(format!("PIP_CACHE_DIR={cache_root}/pip"));
    if let Some(workspace_user) = workspace_user {
        cmd.arg("--user").arg(workspace_user);
    }

    // Resource limits, taken from the tier unless the config set them by hand.
    //
    // The defaults were one size for every purpose: 512 MB and 100 PIDs, which
    // is right for running a command and cannot build a project or start a
    // database. A tier says what the sandbox is for and the limits follow, so
    // they cannot drift into a combination that makes no sense.
    let memory = if config.memory_limit == "512m" {
        config.tier.memory_limit().to_string()
    } else {
        config.memory_limit.clone()
    };
    let pids = if config.pids_limit == 100 {
        config.tier.pids_limit()
    } else {
        config.pids_limit
    };
    cmd.arg("--memory").arg(&memory);
    cmd.arg("--cpus").arg(config.cpu_limit.to_string());
    cmd.arg("--pids-limit").arg(pids.to_string());

    // Security: drop ALL capabilities, prevent privilege escalation
    cmd.arg("--cap-drop").arg("ALL");
    cmd.arg("--security-opt").arg("no-new-privileges");

    // Add back specific capabilities if configured
    for cap in &config.cap_add {
        // Validate: only allow known capability names (alphanumeric + underscore)
        if cap.chars().all(|c| c.is_alphanumeric() || c == '_') {
            cmd.arg("--cap-add").arg(cap);
        } else {
            warn!("Skipping invalid capability: {cap}");
        }
    }

    // Read-only root filesystem
    if config.read_only_root {
        cmd.arg("--read-only");
    }

    // Network isolation
    let network = if config.network == "none" {
        config.tier.network().to_string()
    } else {
        config.network.clone()
    };
    cmd.arg("--network").arg(&network);

    // Publish ports, and ask Docker to keep the container alive, when this
    // sandbox is hosting a service. Without the port mapping a service starts
    // and nothing can reach it, which is why a sandbox could not host one.
    if config.service {
        cmd.arg("--restart").arg(&config.restart_policy);
    }
    for mapping in &config.ports {
        cmd.arg("-p").arg(mapping);
    }

    // tmpfs mounts
    for tmpfs_mount in &config.tmpfs {
        cmd.arg("--tmpfs").arg(tmpfs_mount);
    }

    // The default is a read-only workspace. A writable mount is only used for
    // an explicitly authorized persistent development workspace.
    let ws_str = workspace.display().to_string();
    let mount_mode = if writable_workspace { "rw" } else { "ro" };
    cmd.arg("-v")
        .arg(format!("{ws_str}:{}:{mount_mode}", config.workdir));

    // Working directory
    cmd.arg("-w").arg(&config.workdir);

    // Image + command to keep container alive
    cmd.arg(&config.image).arg("sleep").arg("infinity");

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    debug!(container = %container_name, image = %config.image, "Creating Docker sandbox");

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run docker: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Docker create failed: {}", stderr.trim()));
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(SandboxContainer {
        container_id,
        agent_id: agent_id.to_string(),
        created_at: chrono::Utc::now(),
        is_service: config.service,
        tier: config.tier,
        runtime: config.runtime.clone(),
    })
}

/// Execute a command inside an existing sandbox container.
pub async fn exec_in_sandbox(
    container: &SandboxContainer,
    command: &str,
    timeout: Duration,
) -> Result<ExecResult, String> {
    validate_command(command)?;

    let mut cmd = crate::quiet_command::quiet_async(container_runtime().await.unwrap_or("docker"));
    cmd.arg("exec")
        .arg(&container.container_id)
        .arg("sh")
        .arg("-c")
        .arg(command);

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    debug!(container = %container.container_id, "Executing in Docker sandbox");

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("Docker exec timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("Docker exec failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    // Truncate large outputs (char-boundary safe to avoid UTF-8 panics)
    let max_output = 50_000;
    let stdout = if stdout.len() > max_output {
        let safe_end = crate::str_utils::safe_truncate_str(&stdout, max_output);
        format!("{}... [truncated, {} total bytes]", safe_end, stdout.len())
    } else {
        stdout
    };
    let stderr = if stderr.len() > max_output {
        let safe_end = crate::str_utils::safe_truncate_str(&stderr, max_output);
        format!("{}... [truncated, {} total bytes]", safe_end, stderr.len())
    } else {
        stderr
    };

    Ok(ExecResult {
        stdout,
        stderr,
        exit_code,
    })
}

/// Stop and remove a sandbox container.
/// Stop a sandbox without deleting it.
///
/// Reaping used to remove the container outright, which threw away everything
/// installed in it. A workspace that has just spent ten minutes on `pnpm
/// install` or `cargo build` is worth returning to, and re-creating it means
/// paying for that again on a connection that may not survive it.
///
/// Stopping keeps the container filesystem, so the next call starts it back up
/// with the work still there. Files under the mounted workspace were always
/// safe - that is a host bind mount - but everything installed into the image
/// layers was not.
pub async fn hibernate_sandbox(container: &SandboxContainer) -> Result<(), String> {
    debug!(container = %container.container_id, "Hibernating sandbox");

    let output = crate::quiet_command::quiet_async(&container.runtime)
        .arg("stop")
        .arg(&container.container_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to hibernate container: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to hibernate container: {stderr}"));
    }
    Ok(())
}

/// Label recording what a sandbox container was created as.
///
/// A sandbox's name is fixed per agent and workspace, and the container outlives
/// the process that made it: a daemon restart, a rebuild, and the idle
/// hibernation above all leave it in place while the in-memory pool forgets it.
/// The next call then ran `run --name <same name>` and failed with "name is
/// already in use", so every agent lost its sandbox after each restart and after
/// thirty idle minutes. The label is how a later call tells "the same sandbox,
/// pick it up" from "same name, different sandbox, replace it" without trusting
/// the name alone.
const SANDBOX_FINGERPRINT_LABEL: &str = "freeco.sandbox.fingerprint";

/// A container already holding a sandbox's name.
struct ExistingSandbox {
    id: String,
    running: bool,
    fingerprint: Option<String>,
}

/// Look up a container by name. `None` when no such container exists.
///
/// Reads the JSON form rather than a `--format` template, so nothing depends on
/// how quotes inside a template survive being passed as an argument on Windows.
async fn inspect_existing_sandbox(runtime: &str, name: &str) -> Option<ExistingSandbox> {
    let output = crate::quiet_command::quiet_async(runtime)
        .arg("container")
        .arg("inspect")
        .arg(name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_existing_sandbox(&output.stdout)
}

/// Read a container's id, run state and sandbox fingerprint from `container
/// inspect` JSON.
///
/// Kept apart from the command so the parsing -- which decides between reusing a
/// sandbox and deleting it -- can be tested without a container runtime.
fn parse_existing_sandbox(inspect_json: &[u8]) -> Option<ExistingSandbox> {
    let parsed: serde_json::Value = serde_json::from_slice(inspect_json).ok()?;
    let container = parsed.get(0)?;
    let id = container.get("Id")?.as_str()?.to_string();
    let running = container
        .pointer("/State/Running")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let fingerprint = container
        .pointer("/Config/Labels")
        .and_then(|labels| labels.get(SANDBOX_FINGERPRINT_LABEL))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(ExistingSandbox {
        id,
        running,
        fingerprint,
    })
}

/// Start a stopped sandbox, such as one the idle cleanup hibernated.
async fn start_existing_sandbox(runtime: &str, name: &str) -> bool {
    matches!(
        crate::quiet_command::quiet_async(runtime)
            .arg("start")
            .arg(name)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await,
        Ok(status) if status.success()
    )
}

/// Remove a container by name, for when the name is held by a sandbox that
/// cannot be reused.
async fn remove_sandbox_by_name(runtime: &str, name: &str) -> Result<(), String> {
    let output = crate::quiet_command::quiet_async(runtime)
        .arg("rm")
        .arg("-f")
        .arg(name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to remove stale sandbox {name}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to remove stale sandbox {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub async fn destroy_sandbox(container: &SandboxContainer) -> Result<(), String> {
    debug!(container = %container.container_id, "Destroying Docker sandbox");

    let output = crate::quiet_command::quiet_async(container_runtime().await.unwrap_or("docker"))
        .arg("rm")
        .arg("-f")
        .arg(&container.container_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to destroy container: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(container = %container.container_id, "Docker rm failed: {}", stderr.trim());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Container Pool (Gap 5) — reuse containers across sessions
// ---------------------------------------------------------------------------

use dashmap::DashMap;
use std::sync::Arc;

/// Pool entry for a reusable container.
#[derive(Debug, Clone)]
struct PoolEntry {
    container: SandboxContainer,
    config_hash: u64,
    last_used: std::time::Instant,
}

/// The result of looking up a reusable sandbox container.
#[derive(Debug)]
pub enum PoolAcquire {
    Available(SandboxContainer),
    Missing,
    CoolingDown,
    Incompatible(SandboxContainer),
}

/// Container pool for reusing Docker containers.
#[derive(Debug, Clone)]
pub struct ContainerPool {
    entries: Arc<DashMap<String, PoolEntry>>,
    reservations: Arc<DashMap<String, ()>>,
}

/// An exclusive reservation for one reusable sandbox key.
///
/// A key stays reserved while a command is running, preventing simultaneous
/// calls for one agent workspace from creating duplicate named containers.
pub struct ContainerReservation {
    pool: ContainerPool,
    pool_key: String,
}

impl ContainerPool {
    /// Create a new container pool.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            reservations: Arc::new(DashMap::new()),
        }
    }

    /// Reserve one agent workspace for a reusable sandbox command.
    pub fn reserve(&self, pool_key: &str) -> Result<ContainerReservation, String> {
        if self.reservations.insert(pool_key.to_string(), ()).is_some() {
            return Err(
                "A Docker sandbox command is already running for this agent workspace. \
                 Wait for it to finish before starting another."
                    .to_string(),
            );
        }
        Ok(ContainerReservation {
            pool: self.clone(),
            pool_key: pool_key.to_string(),
        })
    }

    /// Acquire a container from the pool matching the config hash.
    pub fn acquire(&self, pool_key: &str, config_hash: u64, cool_secs: u64) -> PoolAcquire {
        let Some(entry) = self.entries.get(pool_key) else {
            return PoolAcquire::Missing;
        };
        if entry.config_hash != config_hash {
            drop(entry);
            return self
                .entries
                .remove(pool_key)
                .map(|(_, entry)| PoolAcquire::Incompatible(entry.container))
                .unwrap_or(PoolAcquire::Missing);
        }
        if entry.last_used.elapsed().as_secs() < cool_secs {
            return PoolAcquire::CoolingDown;
        }
        drop(entry);
        self.entries
            .remove(pool_key)
            .map(|(_, entry)| PoolAcquire::Available(entry.container))
            .unwrap_or(PoolAcquire::Missing)
    }

    /// Release a container back to the pool.
    pub fn release(&self, pool_key: String, container: SandboxContainer, config_hash: u64) {
        self.entries.insert(
            pool_key,
            PoolEntry {
                container,
                config_hash,
                last_used: std::time::Instant::now(),
            },
        );
    }

    /// Cleanup containers older than max_age or idle longer than idle_timeout.
    pub async fn cleanup(&self, idle_timeout_secs: u64, max_age_secs: u64) {
        let candidate_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| pool_entry_is_expired(entry.value(), idle_timeout_secs, max_age_secs))
            .map(|entry| entry.key().clone())
            .collect();

        for key in candidate_keys {
            let Some(container) = self.take_expired(&key, idle_timeout_secs, max_age_secs) else {
                continue;
            };
            // Hibernate rather than destroy. An idle workspace is not a
            // finished one, and deleting it discards every dependency
            // installed into it - which then has to be downloaded again.
            debug!(container_id = %container.container_id, "Hibernating idle pool container");
            let _ = hibernate_sandbox(&container).await;
        }
    }

    /// Number of containers in the pool.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn take_expired(
        &self,
        pool_key: &str,
        idle_timeout_secs: u64,
        max_age_secs: u64,
    ) -> Option<SandboxContainer> {
        self.entries
            .remove_if(pool_key, |_, entry| {
                pool_entry_is_expired(entry, idle_timeout_secs, max_age_secs)
            })
            .map(|(_, entry)| entry.container)
    }
}

impl ContainerReservation {
    /// Look up the reserved workspace's reusable container.
    pub fn acquire(&self, config_hash: u64, cool_secs: u64) -> PoolAcquire {
        self.pool.acquire(&self.pool_key, config_hash, cool_secs)
    }

    /// Return a successfully used container to the pool.
    pub fn release(&self, container: SandboxContainer, config_hash: u64) {
        self.pool
            .release(self.pool_key.clone(), container, config_hash);
    }
}

impl Drop for ContainerReservation {
    fn drop(&mut self) {
        self.pool.reservations.remove(&self.pool_key);
    }
}

impl Default for ContainerPool {
    fn default() -> Self {
        Self::new()
    }
}

fn pool_entry_is_expired(entry: &PoolEntry, idle_timeout_secs: u64, max_age_secs: u64) -> bool {
    // A service is never reaped. Idle and max-age exist so a forgotten
    // execution sandbox does not hold memory forever, but applied to a hosted
    // service they delete a running database after thirty idle minutes -
    // idleness is the normal state of a service that is waiting for a request.
    if entry.container.is_service {
        return false;
    }
    if !entry.container.tier.reapable() {
        return false;
    }
    let age_secs = chrono::Utc::now()
        .signed_duration_since(entry.container.created_at)
        .to_std()
        .map(|age| age.as_secs())
        .unwrap_or(0);
    entry.last_used.elapsed().as_secs() > idle_timeout_secs || age_secs > max_age_secs
}

/// Build a stable, workspace-specific container name.
///
/// The workspace suffix prevents one agent's independent workspaces from
/// colliding on Docker's globally unique container names.
fn sandbox_container_name(
    config: &DockerSandboxConfig,
    agent_id: &str,
    workspace: &Path,
) -> Result<String, String> {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in workspace.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    sanitize_container_name(&format!(
        "{}-{}-{hash:012x}",
        config.container_prefix,
        crate::str_utils::safe_truncate_str(agent_id, 8)
    ))
}

/// Returns true only for content-addressed OCI image references.
///
/// Mutable tags are acceptable for short-lived local sandboxes, but a reused
/// container or writable workspace must be reproducible and auditable.
pub fn is_immutable_image_reference(image: &str) -> bool {
    // `repo@sha256:<64 hex>` -- a registry image addressed by manifest digest.
    if let Some((_, digest)) = image.rsplit_once("@sha256:") {
        return digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    // `sha256:<64 hex>` -- an image addressed by its own id, which is what a
    // locally built image that was never pushed has instead of a repo digest.
    // It is content-addressed in exactly the same sense: the reference names
    // one specific image and cannot be repointed at another. Rejecting it
    // meant a locally built sandbox could never satisfy the rule, so
    // `docker_exec` refused every call on this machine.
    if let Some(digest) = image.strip_prefix("sha256:") {
        return digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    false
}

// ---------------------------------------------------------------------------
// Bind Mount Validation (Gap 5) — prevent mounting sensitive host paths
// ---------------------------------------------------------------------------

/// Default blocked mount paths (always blocked regardless of config).
const BLOCKED_MOUNT_PATHS: &[&str] = &[
    "/etc",
    "/proc",
    "/sys",
    "/dev",
    "/var/run/docker.sock",
    "/root",
    "/boot",
];

/// Validate a bind mount path for security.
///
/// Blocks:
/// - Sensitive system paths (/etc, /proc, /sys, Docker socket)
/// - Non-absolute paths
/// - Symlink escape attempts
/// - Paths in the configured blocked_mounts list
pub fn validate_bind_mount(path: &str, blocked: &[String]) -> Result<(), String> {
    let p = std::path::Path::new(path);

    // Must be absolute (Docker bind mounts use Unix paths, so check for '/' prefix
    // in addition to platform-native is_absolute check)
    if !p.is_absolute() && !path.starts_with('/') {
        return Err(format!("Bind mount path must be absolute: {path}"));
    }

    // Check for path traversal
    for component in p.components() {
        if let std::path::Component::ParentDir = component {
            return Err(format!("Bind mount path contains '..': {path}"));
        }
    }

    // Check default blocked paths
    for blocked_path in BLOCKED_MOUNT_PATHS {
        if path.starts_with(blocked_path) {
            return Err(format!(
                "Bind mount to '{blocked_path}' is blocked for security"
            ));
        }
    }

    // Check user-configured blocked paths
    for bp in blocked {
        if path.starts_with(bp.as_str()) {
            return Err(format!("Bind mount to '{bp}' is blocked by configuration"));
        }
    }

    // Check for symlink escape (best-effort — canonicalize if path exists).
    //
    // Both sides are canonicalized. Comparing a resolved candidate against an
    // unresolved blocked path is the bug this replaces: on macOS a temporary
    // directory is `/var/folders/...`, which resolves to `/private/var/...`,
    // so the resolved candidate never shares a prefix with the configured
    // blocked path and a symlink aliasing a blocked directory was accepted.
    // The same hole opens on any system where the blocked path is itself
    // reached through a symlink — macOS just makes it the default case, which
    // is why only that platform's tests caught it.
    if p.exists() {
        match p.canonicalize() {
            Ok(canonical) => {
                let canonical_str = canonical.to_string_lossy();
                // Resolve a blocked path for comparison, falling back to the
                // literal value when it does not exist on this machine — a
                // blocked path that is not present still has to be honoured.
                let resolved_blocked = |bp: &str| -> String {
                    std::path::Path::new(bp)
                        .canonicalize()
                        .map(|c| c.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| bp.to_string())
                };
                for blocked_path in BLOCKED_MOUNT_PATHS {
                    if canonical_str.starts_with(blocked_path)
                        || canonical_str.starts_with(&resolved_blocked(blocked_path))
                    {
                        return Err(format!(
                            "Bind mount resolves to blocked path via symlink: {} → {}",
                            path, canonical_str
                        ));
                    }
                }
                for blocked_path in blocked {
                    if canonical_str.starts_with(blocked_path.as_str())
                        || canonical_str.starts_with(&resolved_blocked(blocked_path))
                    {
                        return Err(format!(
                            "Bind mount resolves to configured blocked path via symlink: {} → {}",
                            path, canonical_str
                        ));
                    }
                }
            }
            Err(_) => {
                // Can't canonicalize — path doesn't exist yet, allow it
            }
        }
    }

    Ok(())
}

/// Hash a Docker sandbox config for pool matching.
pub fn config_hash(config: &DockerSandboxConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.image.hash(&mut hasher);
    config.container_prefix.hash(&mut hasher);
    config.network.hash(&mut hasher);
    config.memory_limit.hash(&mut hasher);
    config.workdir.hash(&mut hasher);
    config.cpu_limit.to_bits().hash(&mut hasher);
    config.timeout_secs.hash(&mut hasher);
    config.read_only_root.hash(&mut hasher);
    config.cap_add.hash(&mut hasher);
    config.tmpfs.hash(&mut hasher);
    config.pids_limit.hash(&mut hasher);
    config.persistent_workspace.hash(&mut hasher);
    config.persistent_workspace_agents.hash(&mut hasher);
    config.blocked_mounts.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_container_name_valid() {
        let result = sanitize_container_name("freeco-ai-sandbox-abc123").unwrap();
        assert_eq!(result, "freeco-ai-sandbox-abc123");
    }

    #[test]
    fn test_sanitize_container_name_special_chars() {
        let result = sanitize_container_name("test;rm -rf /").unwrap();
        assert!(!result.contains(';'));
        assert!(!result.contains(' '));
    }

    #[test]
    fn test_sanitize_container_name_empty() {
        assert!(sanitize_container_name("").is_err());
    }

    #[test]
    fn test_sanitize_container_name_too_long() {
        let long = "a".repeat(100);
        assert!(sanitize_container_name(&long).is_err());
    }

    #[test]
    fn test_validate_image_name_valid() {
        assert!(validate_image_name("python:3.12-slim").is_ok());
        assert!(validate_image_name(
            "python:3.12-slim@sha256:229a2c5bfa27522db7815ea81f9bed70af17ccb9de9fc7ad142b1877b5830d36"
        )
        .is_ok());
        assert!(validate_image_name("ubuntu:22.04").is_ok());
        assert!(validate_image_name("registry.example.com/my-image:latest").is_ok());
    }

    #[test]
    fn test_validate_image_name_empty() {
        assert!(validate_image_name("").is_err());
    }

    #[test]
    fn test_validate_image_name_invalid() {
        assert!(validate_image_name("image;rm -rf /").is_err());
        assert!(validate_image_name("image`whoami`").is_err());
        assert!(validate_image_name("image$(id)").is_err());
    }

    #[test]
    fn test_validate_command_valid() {
        assert!(validate_command("python script.py").is_ok());
        assert!(validate_command("ls -la /workspace").is_ok());
    }

    /// Ordinary shell must work inside the sandbox. These previously all
    /// failed as "potential injection", which made the sandbox unusable for
    /// real work: an agent could not chain two commands, write a Python
    /// one-liner, or reference an environment variable. The container is the
    /// boundary, and it is the container's job to contain them.
    #[test]
    fn ordinary_shell_is_allowed_in_the_sandbox() {
        assert!(validate_command("echo hello | grep h").is_ok());
        assert!(validate_command("which git && git --version").is_ok());
        assert!(validate_command("pip install pyyaml; python3 check.py").is_ok());
        assert!(validate_command("echo ${HOME}").is_ok());
        assert!(validate_command("echo `whoami`").is_ok());
        assert!(validate_command("cat a.txt > b.txt").is_ok());
    }

    #[test]
    fn test_validate_command_empty() {
        assert!(validate_command("").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_workspace_owner_uses_workspace_metadata() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let metadata = std::fs::metadata(temp.path()).unwrap();
        assert_eq!(
            workspace_owner(temp.path()).unwrap(),
            Some(format!("{}:{}", metadata.uid(), metadata.gid()))
        );
    }

    #[tokio::test]
    async fn test_docker_available() {
        // Just verify it doesn't panic — result depends on Docker installation
        let _ = is_docker_available("docker").await;
    }

    #[test]
    fn test_config_defaults() {
        let config = DockerSandboxConfig::default();
        // On by default: code execution belongs in an isolated container, and
        // a sandbox nobody switches on protects nobody. This is deliberate --
        // if it ever flips back to false, that is a regression, not a tidy-up.
        assert!(config.enabled);
        assert_eq!(
            config.image,
            "python:3.12-slim@sha256:229a2c5bfa27522db7815ea81f9bed70af17ccb9de9fc7ad142b1877b5830d36"
        );
        assert_eq!(config.container_prefix, "freeco-ai-sandbox");
        assert_eq!(config.workdir, "/workspace");
        assert_eq!(config.network, "none");
        assert_eq!(config.memory_limit, "512m");
        assert_eq!(config.cpu_limit, 1.0);
        assert_eq!(config.timeout_secs, 60);
        assert!(config.read_only_root);
        assert!(config.cap_add.is_empty());
        assert_eq!(config.tmpfs, vec!["/tmp:size=64m"]);
        assert_eq!(config.pids_limit, 100);
        assert_eq!(config.mode, freeco_types::config::DockerSandboxMode::All);
        assert_eq!(config.scope, freeco_types::config::DockerScope::Session);
        assert_eq!(config.reuse_cool_secs, 0);
        assert!(!config.persistent_workspace);
        assert!(config.persistent_workspace_agents.is_empty());
    }

    #[test]
    fn test_exec_result_fields() {
        let result = ExecResult {
            stdout: "hello".to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello");
    }

    // ── Container Pool tests ──────────────────────────────────────────

    #[test]
    fn test_container_pool_empty() {
        let pool = ContainerPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_container_pool_release_acquire() {
        let pool = ContainerPool::new();
        let container = SandboxContainer {
            container_id: "test123".to_string(),
            agent_id: "agent1".to_string(),
            created_at: chrono::Utc::now(),
            is_service: false,
            tier: Default::default(),
            runtime: "docker".to_string(),
        };
        pool.release("agent:one".to_string(), container, 12345);
        assert_eq!(pool.len(), 1);

        // Acquire with same hash — should succeed (cool_secs=0 for test)
        let acquired = pool.acquire("agent:one", 12345, 0);
        assert!(matches!(
            acquired,
            PoolAcquire::Available(container) if container.container_id == "test123"
        ));
        assert!(pool.is_empty());
    }

    #[test]
    fn test_container_pool_hash_mismatch() {
        let pool = ContainerPool::new();
        let container = SandboxContainer {
            container_id: "test123".to_string(),
            agent_id: "agent1".to_string(),
            created_at: chrono::Utc::now(),
            is_service: false,
            tier: Default::default(),
            runtime: "docker".to_string(),
        };
        pool.release("agent:one".to_string(), container, 12345);

        // A changed config must return the old container for destruction before
        // a replacement with the same stable name is created.
        let acquired = pool.acquire("agent:one", 99999, 0);
        assert!(matches!(
            acquired,
            PoolAcquire::Incompatible(container) if container.container_id == "test123"
        ));
        assert!(pool.is_empty());
    }

    #[test]
    fn test_container_pool_does_not_cross_agent_boundaries() {
        let pool = ContainerPool::new();
        let container = SandboxContainer {
            container_id: "test123".to_string(),
            agent_id: "agent1".to_string(),
            created_at: chrono::Utc::now(),
            is_service: false,
            tier: Default::default(),
            runtime: "docker".to_string(),
        };
        pool.release("agent:one".to_string(), container, 12345);
        assert!(matches!(
            pool.acquire("agent:two", 12345, 0),
            PoolAcquire::Missing
        ));
    }

    #[test]
    fn test_container_pool_reservation_prevents_concurrent_workspace_execution() {
        let pool = ContainerPool::new();
        let reservation = pool.reserve("agent:one").unwrap();
        assert!(pool.reserve("agent:one").is_err());
        drop(reservation);
        assert!(pool.reserve("agent:one").is_ok());
    }

    #[test]
    fn test_container_pool_cooling_does_not_create_a_duplicate() {
        let pool = ContainerPool::new();
        let container = SandboxContainer {
            container_id: "test123".to_string(),
            agent_id: "agent1".to_string(),
            created_at: chrono::Utc::now(),
            is_service: false,
            tier: Default::default(),
            runtime: "docker".to_string(),
        };
        pool.release("agent:one".to_string(), container, 12345);
        assert!(matches!(
            pool.acquire("agent:one", 12345, 60),
            PoolAcquire::CoolingDown
        ));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_max_age_uses_the_container_creation_time() {
        let entry = PoolEntry {
            container: SandboxContainer {
                container_id: "test123".to_string(),
                agent_id: "agent1".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::seconds(2),
                is_service: false,
                tier: Default::default(),
                runtime: "docker".to_string(),
            },
            config_hash: 12345,
            last_used: std::time::Instant::now(),
        };
        assert!(pool_entry_is_expired(&entry, 60, 1));
    }

    #[test]
    fn test_container_pool_keeps_entry_that_is_no_longer_expired() {
        let pool = ContainerPool::new();
        pool.release(
            "agent:one".to_string(),
            SandboxContainer {
                container_id: "test123".to_string(),
                agent_id: "agent1".to_string(),
                created_at: chrono::Utc::now(),
                is_service: false,
                tier: Default::default(),
                runtime: "docker".to_string(),
            },
            12345,
        );

        assert!(pool.take_expired("agent:one", 60, 60).is_none());
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_container_names_are_unique_per_workspace() {
        let config = DockerSandboxConfig::default();
        let first =
            sandbox_container_name(&config, "agent1", Path::new("/tmp/workspace-one")).unwrap();
        let second =
            sandbox_container_name(&config, "agent1", Path::new("/tmp/workspace-two")).unwrap();
        assert!(first.starts_with("freeco-ai-sandbox-agent1-"));
        assert_ne!(first, second);
    }

    #[test]
    fn immutable_image_references_require_sha256_digest() {
        assert!(is_immutable_image_reference(
            "registry.example/freeco@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!is_immutable_image_reference("python:3.12-slim"));
        assert!(!is_immutable_image_reference(
            "registry.example/freeco@sha256:not-a-digest"
        ));
    }

    /// A locally built image has no repo digest, only its own id. That is
    /// content-addressed too, and rejecting it meant `docker_exec` refused
    /// every call on a machine whose sandbox image was built rather than
    /// pulled.
    #[test]
    fn bare_image_id_counts_as_immutable() {
        assert!(is_immutable_image_reference(
            "sha256:73169df13d621e9be57cb0064ee40e5cf7d0b81005e758a50386fa87e8ab4d0a"
        ));
        assert!(!is_immutable_image_reference("sha256:short"));
        assert!(!is_immutable_image_reference(
            "sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
        ));
    }

    /// A sandbox this code created: labelled and running, so it is reused.
    #[test]
    fn reads_a_labelled_running_sandbox() {
        let json = br#"[{"Id":"abc123","State":{"Running":true},"Config":{"Labels":{"freeco.sandbox.fingerprint":"00ff-Workspace-rw"}}}]"#;
        let found = parse_existing_sandbox(json).expect("parsed");
        assert_eq!(found.id, "abc123");
        assert!(found.running);
        assert_eq!(found.fingerprint.as_deref(), Some("00ff-Workspace-rw"));
    }

    /// A sandbox from before labels existed, stopped by the idle cleanup. With
    /// no fingerprint it can never match, so it is replaced rather than adopted.
    #[test]
    fn unlabelled_stopped_sandbox_has_no_fingerprint() {
        let json = br#"[{"Id":"old","State":{"Running":false},"Config":{"Labels":null}}]"#;
        let found = parse_existing_sandbox(json).expect("parsed");
        assert!(!found.running);
        assert_eq!(found.fingerprint, None);
    }

    /// No container, or output that is not inspect JSON, means nothing is there.
    /// It must never read as a container to delete.
    #[test]
    fn absent_or_garbage_inspect_output_is_none() {
        assert!(parse_existing_sandbox(b"[]").is_none());
        assert!(parse_existing_sandbox(b"Error: no such container").is_none());
    }

    // ── Bind Mount Validation tests ──────────────────────────────────

    #[test]
    fn test_validate_bind_mount_valid() {
        assert!(validate_bind_mount("/home/user/workspace", &[]).is_ok());
        assert!(validate_bind_mount("/tmp/sandbox", &[]).is_ok());
    }

    #[test]
    fn test_validate_bind_mount_non_absolute() {
        assert!(validate_bind_mount("relative/path", &[]).is_err());
    }

    #[test]
    fn test_validate_bind_mount_blocked_paths() {
        assert!(validate_bind_mount("/etc/passwd", &[]).is_err());
        assert!(validate_bind_mount("/proc/self", &[]).is_err());
        assert!(validate_bind_mount("/sys/kernel", &[]).is_err());
        assert!(validate_bind_mount("/var/run/docker.sock", &[]).is_err());
    }

    #[test]
    fn test_validate_bind_mount_traversal() {
        assert!(validate_bind_mount("/home/user/../etc/passwd", &[]).is_err());
    }

    #[test]
    fn test_validate_bind_mount_custom_blocked() {
        let blocked = vec!["/data/secrets".to_string()];
        assert!(validate_bind_mount("/data/secrets/vault", &blocked).is_err());
        assert!(validate_bind_mount("/data/public", &blocked).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_bind_mount_custom_blocked_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_dir = temp.path().join("blocked");
        let alias = temp.path().join("workspace");
        std::fs::create_dir(&blocked_dir).unwrap();
        std::os::unix::fs::symlink(&blocked_dir, &alias).unwrap();

        let blocked = vec![blocked_dir.display().to_string()];
        assert!(validate_bind_mount(alias.to_str().unwrap(), &blocked).is_err());
    }

    #[test]
    fn test_config_hash_deterministic() {
        let c1 = DockerSandboxConfig::default();
        let c2 = DockerSandboxConfig::default();
        assert_eq!(config_hash(&c1), config_hash(&c2));
    }

    #[test]
    fn test_config_hash_different_images() {
        let c1 = DockerSandboxConfig::default();
        let c2 = DockerSandboxConfig {
            image: "node:20-slim".to_string(),
            ..Default::default()
        };
        assert_ne!(config_hash(&c1), config_hash(&c2));
    }

    #[test]
    fn test_config_hash_changes_when_writable_workspace_access_changes() {
        let c1 = DockerSandboxConfig::default();
        let mut c2 = c1.clone();
        c2.persistent_workspace_agents.push("agent-1".to_string());
        assert_ne!(config_hash(&c1), config_hash(&c2));
    }
}

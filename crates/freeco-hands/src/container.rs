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
async fn start_podman_machine() -> bool {
    let listed = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new("podman")
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
    // Starting a stopped VM takes tens of seconds; an already-running one
    // returns immediately with an error we can ignore.
    matches!(
        tokio::time::timeout(
            Duration::from_secs(300),
            tokio::process::Command::new("podman")
                .args(["machine", "start"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await,
        Ok(Ok(s)) if s.success()
    )
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

/// The container name for a hand instance.
///
/// Includes the instance id so two instances of the same hand -- a CRM per
/// tenant, say -- do not collide on a name.
pub fn container_name(hand_id: &str, instance_id: &str) -> String {
    let short: String = instance_id.chars().take(8).collect();
    format!("freeco-{hand_id}-{short}")
}

/// Where a stack's compose file and generated `.env` live.
///
/// Per instance, not per hand, so two tenants running the same hand do not
/// overwrite each other's configuration.
pub fn stack_dir(hand_id: &str, instance_id: &str) -> std::path::PathBuf {
    let short: String = instance_id.chars().take(8).collect();
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".freeco-ai")
        .join("hands")
        .join(format!("{hand_id}-{short}"))
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
pub fn network_name(hand_id: &str, instance_id: &str) -> String {
    project_name(hand_id, instance_id)
}

/// Container name for one service within a stack.
pub fn service_container_name(hand_id: &str, instance_id: &str, service: &str) -> String {
    format!("{}-{service}", project_name(hand_id, instance_id))
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
    instance_id: &str,
    resolve_secret: &(impl Fn(&str) -> Option<String> + ?Sized),
) -> Result<String, HandError> {
    let rt = runtime().await.ok_or_else(|| {
        HandError::Config(
            "no container runtime found. Start Docker Desktop, or start Podman with \
             `podman machine start`"
                .into(),
        )
    })?;
    let project = project_name(hand_id, instance_id);
    let net = network_name(hand_id, instance_id);

    // Idempotent: an existing network from a previous start is fine.
    let _ = tokio::time::timeout(
        RUNTIME_TIMEOUT,
        tokio::process::Command::new(rt)
            .args(["network", "create", &net])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;

    for idx in startup_order(&spec.services)? {
        let svc = &spec.services[idx];
        let name = service_container_name(hand_id, instance_id, &svc.name);

        if is_running(&name).await {
            continue;
        }
        let _ = tokio::process::Command::new(rt)
            .args(["rm", "-f", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        let mut cmd = tokio::process::Command::new(rt);
        cmd.args(["run", "-d", "--name", &name, "--restart", &spec.restart]);
        // The alias is what makes `postgres://user@db:5432` resolve inside the
        // application container.
        cmd.args(["--network", &net, "--network-alias", &svc.name]);
        for p in &svc.ports {
            cmd.arg("-p").arg(p);
        }
        for v in &svc.volumes {
            // Namespaced so two instances do not share one database volume.
            cmd.arg("-v").arg(format!("{project}-{v}"));
        }
        for (k, v) in &svc.env {
            let resolved = resolve_env_value(v, &|key| resolve_secret(key));
            cmd.arg("-e").arg(format!("{k}={resolved}"));
        }
        cmd.arg(&svc.image);
        for part in &svc.command {
            cmd.arg(part);
        }

        let out = tokio::time::timeout(Duration::from_secs(900), cmd.output())
            .await
            .map_err(|_| HandError::Config(format!("starting {name} timed out")))?
            .map_err(|e| HandError::Config(format!("could not run {rt}: {e}")))?;
        if !out.status.success() {
            return Err(HandError::Config(format!(
                "{name} failed to start: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        if !svc.health_cmd.is_empty()
            && !wait_service_healthy(rt, &name, &svc.health_cmd, spec.health_timeout_secs).await
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
/// The project name carries the instance id for the same reason container
/// names do: `compose down` targets a project, so two instances sharing one
/// would tear down each other's databases.
async fn start_stack(
    spec: &HandContainer,
    hand_id: &str,
    instance_id: &str,
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
    let dir = stack_dir(hand_id, instance_id);
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

    let project = project_name(hand_id, instance_id);
    let out = tokio::time::timeout(
        Duration::from_secs(900), // first run pulls images; a CRM stack is gigabytes
        tokio::process::Command::new(rt)
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

/// Compose project name for a hand instance.
pub fn project_name(hand_id: &str, instance_id: &str) -> String {
    let short: String = instance_id.chars().take(8).collect();
    format!("freeco-{hand_id}-{short}")
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
        .args(["ps", "--filter", &format!("name={name}"), "--format", "{{.Names}}"])
        .output();
    match tokio::time::timeout(RUNTIME_TIMEOUT, out).await {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == name),
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
    instance_id: &str,
    resolve_secret: impl Fn(&str) -> Option<String>,
) -> Result<String, HandError> {
    // A stack is a different lifecycle: compose owns the containers, so start,
    // stop and liveness all have to go through it rather than through names we
    // made up.
    if spec.is_native_stack() {
        return start_native_stack(spec, hand_id, instance_id, &resolve_secret).await;
    }
    if spec.is_stack() {
        return start_stack(spec, hand_id, instance_id, resolve_secret).await;
    }

    let rt = runtime()
        .await
        .ok_or_else(|| HandError::Config(
            "no container runtime found. Start Docker Desktop, or start Podman with `podman machine start`".into(),
        ))?;

    let name = container_name(hand_id, instance_id);
    if is_running(&name).await {
        return Ok(name);
    }

    // Remove any stopped container of the same name, or `run` fails on the
    // leftover rather than starting.
    let _ = tokio::process::Command::new(rt)
        .args(["rm", "-f", &name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let mut cmd = tokio::process::Command::new(rt);
    cmd.args(["run", "-d", "--name", &name, "--restart", &spec.restart]);

    if spec.host_network {
        cmd.arg("--network").arg("host");
    } else {
        for p in &spec.ports {
            cmd.arg("-p").arg(p);
        }
    }
    for v in &spec.volumes {
        cmd.arg("-v").arg(v);
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
    instance_id: &str,
) -> Result<(), HandError> {
    let Some(rt) = runtime().await else {
        return Ok(());
    };
    // Reverse dependency order, so an application is stopped before the
    // database it is talking to.
    let mut order = startup_order(&spec.services)?;
    order.reverse();
    for idx in order {
        let name = service_container_name(hand_id, instance_id, &spec.services[idx].name);
        let _ = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(rt).args(["stop", &name]).status(),
        )
        .await;
        // Removed, not just stopped: the container is recreated on next start,
        // while the named volumes -- the actual data -- are left untouched.
        let _ = tokio::process::Command::new(rt)
            .args(["rm", "-f", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = tokio::process::Command::new(rt)
        .args(["network", "rm", &network_name(hand_id, instance_id)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    Ok(())
}

pub async fn stop(hand_id: &str, instance_id: &str, is_stack: bool) -> Result<(), HandError> {
    let Some(rt) = runtime().await else {
        return Ok(()); // nothing to stop if no runtime
    };
    if is_stack {
        let project = project_name(hand_id, instance_id);
        let dir = stack_dir(hand_id, instance_id);
        let _ = tokio::time::timeout(
            Duration::from_secs(180),
            tokio::process::Command::new(rt)
                .args(["compose", "-p", &project, "down"])
                .current_dir(&dir)
                .status(),
        )
        .await;
        return Ok(());
    }
    let name = container_name(hand_id, instance_id);
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(rt).args(["stop", &name]).status(),
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
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(budget_secs);
    while tokio::time::Instant::now() < deadline {
        if let Ok(r) = client.get(url).send().await {
            if r.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
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

    /// Two instances of one hand must not collide on a container name, or
    /// starting a second tenant's CRM would adopt the first tenant's container.
    #[test]
    fn instances_get_distinct_container_names() {
        let a = container_name("crm", "11111111-aaaa-bbbb-cccc-dddddddddddd");
        let b = container_name("crm", "22222222-aaaa-bbbb-cccc-dddddddddddd");
        assert_ne!(a, b);
        assert!(a.starts_with("freeco-crm-"));
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
    fn stack_projects_are_per_instance() {
        let a = project_name("crm", "11111111-aaaa-bbbb-cccc-dddddddddddd");
        let b = project_name("crm", "22222222-aaaa-bbbb-cccc-dddddddddddd");
        assert_ne!(a, b);
        assert!(a.starts_with("freeco-crm-"));
    }

    /// The compose file and its generated `.env` must not be shared either:
    /// the `.env` holds that instance's database password.
    #[test]
    fn stack_dirs_are_per_instance() {
        let a = stack_dir("crm", "11111111-aaaa-bbbb-cccc-dddddddddddd");
        let b = stack_dir("crm", "22222222-aaaa-bbbb-cccc-dddddddddddd");
        assert_ne!(a, b);
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

    /// Two tenants running the same hand get separate containers.
    #[test]
    fn same_hand_different_tenants_get_separate_containers() {
        use crate::HandInstance;
        use std::collections::HashMap;
        let a = HandInstance::new("crm", "agent", HashMap::new(), None).for_tenant("acme");
        let b = HandInstance::new("crm", "agent", HashMap::new(), None).for_tenant("globex");
        let na = container_name(&a.hand_id, &a.instance_id.to_string());
        let nb = container_name(&b.hand_id, &b.instance_id.to_string());
        assert_ne!(na, nb, "one tenant would adopt the other's container");
    }
}

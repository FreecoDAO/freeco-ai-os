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
    static RUNTIME: OnceLock<Option<&'static str>> = OnceLock::new();
    if let Some(hit) = RUNTIME.get() {
        return *hit;
    }
    let mut found = None;
    for candidate in ["docker", "podman"] {
        if probe(candidate).await {
            found = Some(candidate);
            break;
        }
    }
    let _ = RUNTIME.set(found);
    found
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
        let port = first_host_port(spec).unwrap_or_default();
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

/// Stop the container, leaving it removable and its volumes intact.
pub async fn stop(hand_id: &str, instance_id: &str) -> Result<(), HandError> {
    let Some(rt) = runtime().await else {
        return Ok(()); // nothing to stop if no runtime
    };
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
            ports: vec!["20128:20128".into()],
            volumes: vec![],
            env: Default::default(),
            health_path: Some("/health".into()),
            health_timeout_secs: 30,
            host_network: false,
            restart: "unless-stopped".into(),
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
}

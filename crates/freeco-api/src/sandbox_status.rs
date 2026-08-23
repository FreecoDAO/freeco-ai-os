//! Sandbox visibility.
//!
//! The Docker sandbox is the boundary that keeps agent-written code off the
//! user's machine, and until now it was completely invisible: no endpoint, no
//! panel, no way to tell whether it was on, whether Docker was running, or
//! whether the image was even downloaded. A security control nobody can see is
//! one nobody can trust, and when it silently was not working there was no way
//! to find out except by reading the source.
//!
//! This exposes the whole truth in one call: is it enabled, is Docker up, is
//! the image present, and what limits are actually applied.

use crate::routes::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use std::sync::Arc;

/// GET /api/sandbox/status
///
/// Reports what is true right now rather than what is configured. `ready` is
/// the single field the UI needs: it is only true when the sandbox would
/// actually run something, which requires all three of enabled, Docker
/// running, and the image already downloaded.
pub async fn sandbox_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config = &state.kernel.config.docker;

    // Report what a container would actually get, not the raw config.
    //
    // These fields used to read config.memory_limit and friends directly,
    // which are the pre-tier defaults. With a workspace tier configured the
    // panel still said "512 MB, no network, 60s" while a real container got
    // 4 GB and a network - so the one place a user checks whether the sandbox
    // can build anything was telling them it could not.
    //
    // The same precedence as create_sandbox: an explicitly tuned config wins,
    // otherwise the tier decides.
    let effective_memory = if config.memory_limit == "512m" {
        config.tier.memory_limit().to_string()
    } else {
        config.memory_limit.clone()
    };
    let effective_pids = if config.pids_limit == 100 {
        config.tier.pids_limit()
    } else {
        config.pids_limit
    };
    let effective_network = if config.network == "none" {
        config.tier.network().to_string()
    } else {
        config.network.clone()
    };
    let effective_timeout = if config.timeout_secs == 60 {
        config.tier.timeout_secs()
    } else {
        config.timeout_secs
    };

    let docker_running =
        freeco_kernel_runtime::docker_sandbox::is_docker_available(&config.runtime).await;
    // Only probe the image when Docker answers; otherwise the inspect call
    // just times out and we would report "image missing" for the wrong reason.
    let image_present = if docker_running {
        freeco_kernel_runtime::docker_sandbox::is_image_present(&config.runtime, &config.image)
            .await
    } else {
        false
    };

    let ready = config.enabled && docker_running && image_present;

    // Say what to do next, in the order it has to be done. A status panel that
    // only reports "not ready" leaves the user guessing which of three things
    // is wrong.
    let next_step = if !config.enabled {
        Some("Sandbox is switched off. Set docker.enabled = true in ~/.freeco-ai/config.toml.")
    } else if !docker_running {
        Some("Docker is not running. Start Docker Desktop, or install it from https://docker.com/products/docker-desktop.")
    } else if !image_present {
        Some("The sandbox image has not been downloaded yet. It is a one-time download, and it is not started automatically so it cannot surprise a metered connection.")
    } else {
        None
    };

    Json(serde_json::json!({
        "enabled": config.enabled,
        "docker_running": docker_running,
        "image": config.image,
        "image_present": image_present,
        "ready": ready,
        "next_step": next_step,
        // The protections actually in force, so the user can see the boundary
        // rather than take it on faith.
        "protections": {
            "tier": match config.tier {
                freeco_types::config::SandboxTier::Exec => "exec",
                freeco_types::config::SandboxTier::Workspace => "workspace",
                freeco_types::config::SandboxTier::Service => "service",
            },
            "network": effective_network,
            "network_blocked": effective_network == "none",
            "memory_limit": effective_memory,
            "cpu_limit": config.cpu_limit,
            "pids_limit": effective_pids,
            "read_only_root": config.read_only_root,
            "timeout_secs": effective_timeout,
            "capabilities_dropped": "ALL",
            "no_new_privileges": true,
        },
    }))
}

/// POST /api/sandbox/pull — download the sandbox image on request.
///
/// Deliberately explicit. `docker run` would pull the image silently on first
/// use, which on a metered connection is an unannounced several-hundred-
/// megabyte download. Making it a button the user presses keeps that decision
/// theirs.
pub async fn sandbox_pull(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let image = state.kernel.config.docker.image.clone();
    let runtime = state.kernel.config.docker.runtime.clone();

    if !freeco_kernel_runtime::docker_sandbox::is_docker_available(&runtime).await {
        return Json(serde_json::json!({
            "ok": false,
            "error": "No container runtime is running, so the image cannot be downloaded. Start Docker Desktop, or start Podman with `podman machine start`, and try again.",
        }));
    }

    // Whichever runtime is actually available -- the sandbox supports both,
    // and assuming Docker left Podman users unable to pull the image at all.
    let runtime = freeco_kernel_runtime::docker_sandbox::container_runtime()
        .await
        .unwrap_or("docker");
    match freeco_kernel_runtime::quiet_command::quiet_async(runtime)
        .arg("pull")
        .arg(&image)
        .output()
        .await
    {
        Ok(out) if out.status.success() => Json(serde_json::json!({
            "ok": true,
            "image": image,
            "message": format!("{image} is downloaded. The sandbox is ready."),
        })),
        Ok(out) => Json(serde_json::json!({
            "ok": false,
            "error": String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("could not run {runtime} pull: {e}"),
        })),
    }
}

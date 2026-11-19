//! One-click service provisioning — Freeco installs and runs the services it
//! needs, instead of assuming the user already has them.
//!
//! `POST /api/services/{id}/install` starts a background task that:
//! 1. checks Docker is available (and says how to get it if not);
//! 2. materialises the service (git clone, or an embedded compose file) into
//!    `~/.freeco-ai/services/{id}/`;
//! 3. runs `docker compose up -d`, streaming progress;
//! 4. waits for the service's health endpoint;
//! 5. registers it as an MCP server so agents can use it immediately.
//!
//! `GET /api/services` lists the catalog with live status; `GET
//! /api/services/status` returns progress for the running install.

use crate::routes::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

/// A service Freeco can install and run on the user's behalf.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub purpose: &'static str,
    /// Git repo to clone when the compose file needs the repo's other files.
    pub repo: Option<&'static str>,
    /// Self-contained compose file, when no repo checkout is needed.
    pub compose: Option<&'static str>,
    /// URL polled to decide the service is up.
    pub health_url: &'static str,
    /// MCP endpoint to auto-register once healthy (None = needs a wrapper/key).
    pub mcp_url: Option<&'static str>,
    /// Rough disk/download cost so the user is never surprised.
    pub approx_download_gb: f32,
    /// Extra step the user must do themselves (e.g. create an API key).
    pub manual_step: Option<&'static str>,
}

/// Minimal, self-contained compose for Twenty CRM, on host port 3200 — NOT
/// 3000. dograh's `ui` service publishes 3000, so a user who installs voice and
/// CRM would hit a port collision and the second install would fail to bind.
const TWENTY_COMPOSE: &str = r#"services:
  twenty-db:
    image: postgres:16
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: freeco
    volumes: [twenty-db-data:/var/lib/postgresql/data]
  twenty:
    image: twentycrm/twenty:latest
    depends_on: [twenty-db]
    environment:
      PG_DATABASE_URL: postgres://postgres:freeco@twenty-db:5432/default
      SERVER_URL: http://localhost:3200
      APP_SECRET: freeco-local-dev-secret-change-me
    ports: ["3200:3000"]
    volumes: [twenty-data:/app/packages/twenty-server/.local-storage]
volumes:
  twenty-db-data:
  twenty-data:
"#;

/// Minimal, self-contained compose for Akaunting.
const AKAUNTING_COMPOSE: &str = r#"services:
  akaunting-db:
    image: mariadb:11
    environment:
      MYSQL_ROOT_PASSWORD: freeco
      MYSQL_DATABASE: akaunting
      MYSQL_USER: akaunting
      MYSQL_PASSWORD: freeco
    volumes: [akaunting-db-data:/var/lib/mysql]
  akaunting:
    image: akaunting/akaunting:latest
    depends_on: [akaunting-db]
    environment:
      DB_HOST: akaunting-db
      DB_DATABASE: akaunting
      DB_USERNAME: akaunting
      DB_PASSWORD: freeco
      APP_URL: http://localhost:8080
    ports: ["8080:80"]
    volumes: [akaunting-data:/var/www/html]
volumes:
  akaunting-db-data:
  akaunting-data:
"#;

pub const SERVICE_CATALOG: &[ServiceProfile] = &[
    ServiceProfile {
        id: "dograh",
        name: "dograh — real-time voice AI",
        purpose: "Live voice calls, barge-in, telephony and voice-campaign workflows for outreach.",
        repo: Some("https://github.com/FreecoDAO/dograh"),
        compose: None,
        health_url: "http://localhost:8000",
        // dograh is MCP-native, so it connects with no extra wrapper or key.
        // It mounts the MCP server under its API prefix
        // (`app.mount(f"{API_PREFIX}/mcp", mcp_app)`) → /api/v1/mcp. Pointing at
        // a bare /mcp registers an endpoint that *looks* connected but is dead.
        mcp_url: Some("http://localhost:8000/api/v1/mcp"),
        approx_download_gb: 3.5,
        manual_step: None,
    },
    ServiceProfile {
        id: "crm",
        name: "Twenty — CRM",
        purpose:
            "Where donors, grant givers, VCs and partners live: people, companies, notes, pipeline.",
        repo: None,
        compose: Some(TWENTY_COMPOSE),
        health_url: "http://localhost:3200",
        mcp_url: None,
        approx_download_gb: 2.0,
        manual_step: Some(
            "Open http://localhost:3200, create your workspace, then Settings → APIs & Webhooks → \
             create an API key and paste it in Connect below. Never paste it into a chat.",
        ),
    },
    ServiceProfile {
        id: "accounting",
        name: "Akaunting — accounting",
        purpose: "Auditable books: income/expense, invoices, and reports for grant funders.",
        repo: None,
        compose: Some(AKAUNTING_COMPOSE),
        health_url: "http://localhost:8080",
        mcp_url: None,
        approx_download_gb: 1.5,
        manual_step: Some(
            "Open http://localhost:8080 and finish the first-run wizard (company + admin user), \
             then enter that login in Connect below. Agents get read-only access to the books \
             through the accounting_api tool. Never paste the password into a chat.",
        ),
    },
];

pub fn service_profile(id: &str) -> Option<&'static ServiceProfile> {
    SERVICE_CATALOG.iter().find(|s| s.id == id)
}

/// Live progress of a service install (mirrors the local-AI status shape).
#[derive(Debug, Clone, Serialize, Default)]
pub struct ServiceStatus {
    /// idle | checking | fetching | starting | waiting | connecting | done | needs-docker | error
    pub phase: String,
    pub detail: String,
    pub percent: i32,
    pub service: String,
    pub running: bool,
}

pub type SharedServiceStatus = Arc<tokio::sync::RwLock<ServiceStatus>>;

async fn set_status(s: &SharedServiceStatus, phase: &str, detail: String, percent: i32) {
    let mut w = s.write().await;
    w.phase = phase.to_string();
    w.detail = detail;
    w.percent = percent;
}

async fn run(cmd: &str, args: &[&str], cwd: Option<&std::path::Path>) -> Result<String, String> {
    let mut c = tokio::process::Command::new(cmd);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c
        .output()
        .await
        .map_err(|e| format!("could not run {cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Create a usable `.env` for a cloned service from its `.env.example`,
/// replacing obvious change-me placeholders with generated secrets. Never
/// overwrites an existing `.env` (the user may have tuned it).
///
/// Checks the repo root and `api/` (dograh keeps its example in `api/`).
fn ensure_env_file(dir: &std::path::Path) -> Result<(), String> {
    let env_path = dir.join(".env");
    if env_path.exists() {
        return Ok(());
    }
    let example = [
        dir.join(".env.example"),
        dir.join("api").join(".env.example"),
    ]
    .into_iter()
    .find(|p| p.exists());
    let Some(example) = example else {
        return Ok(()); // nothing to template from — compose may still have defaults
    };
    let content =
        std::fs::read_to_string(&example).map_err(|e| format!("read .env.example: {e}"))?;

    // A random-but-deterministic-per-install secret. Not cryptographic-grade
    // key material, but far better than shipping "change-me" into a running
    // service that a user may later expose.
    let secret = format!(
        "freeco-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );

    let patched: String = content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') || !line.contains('=') {
                return line.to_string();
            }
            let lower = line.to_ascii_lowercase();
            let looks_secret = lower.contains("secret") || lower.contains("password");
            let placeholder = lower.contains("change-me") || lower.contains("changeme");
            if looks_secret && placeholder {
                if let Some((key, _)) = line.split_once('=') {
                    return format!("{key}=\"{secret}\"");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write(&env_path, patched).map_err(|e| format!("write .env: {e}"))?;
    Ok(())
}

/// Is a container runtime installed and answering?
///
/// Asks for whichever runtime is actually present rather than for Docker by
/// name. This ran `docker version` literally, so on a machine with podman --
/// which is the supported runtime here, and the only one that has ever worked
/// on this Windows box -- it always failed. Every Install button on the
/// Services page is bound to `:disabled="!dockerReady"`, so the whole page was
/// switched off with a message telling the user to install Docker Desktop
/// while a working runtime sat right there running their other containers.
pub async fn container_runtime_ready() -> bool {
    freeco_hands::container::runtime().await.is_some()
}

async fn health_ok(url: &str) -> bool {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let Ok(client) = client else { return false };
    // Any HTTP response means something is listening and serving.
    client.get(url).send().await.is_ok()
}

/// GET /api/services — catalog plus live status of each service.
pub async fn list_services(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let docker = container_runtime_ready().await;
    let configured: std::collections::HashSet<&str> = state
        .kernel
        .config
        .mcp_servers
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    let mut items = Vec::new();
    for s in SERVICE_CATALOG {
        let running = health_ok(s.health_url).await;
        items.push(serde_json::json!({
            "id": s.id,
            "name": s.name,
            "purpose": s.purpose,
            "approx_download_gb": s.approx_download_gb,
            "manual_step": s.manual_step,
            "health_url": s.health_url,
            "running": running,
            // The CRM connects through a stored key, not an MCP entry.
            "connected": configured.contains(s.id)
                || (s.id == "crm" && crate::crm::is_connected())
                || (s.id == "accounting" && crate::crm::accounting_is_connected()),
            "connect_with_key": s.id == "crm" || s.id == "accounting",
            // Akaunting logs in with an e-mail and password, not an API key.
            "connect_with_login": s.id == "accounting",
            "auto_connects": s.mcp_url.is_some(),
        }));
    }
    // Name the runtime that was found, so the page can say "using Podman"
    // rather than leaving the user to guess which of the two it picked -- and
    // so "nothing is running" can be told apart from "Docker is not installed",
    // which are different problems with different fixes.
    let runtime = freeco_hands::container::runtime().await;
    // The full state, not just a boolean: "not installed" and "installed but
    // its VM is stopped" need different things from the user, and answering
    // both with "install Docker Desktop" sent someone to a download page while
    // a working runtime sat there stopped.
    let runtime_status = freeco_hands::container::runtime_status().await;
    // Podman VM problems whose fix needs sudo in the VM. Reported here so the
    // page can offer each fix as a button; the daemon never runs one on its own.
    let vm_issues: Vec<serde_json::Value> = freeco_hands::container::vm_issues()
        .await
        .iter()
        .map(|issue| issue.as_json())
        .collect();
    Json(serde_json::json!({
        "docker_ready": docker,
        "runtime": runtime,
        "runtime_status": runtime_status.as_json(),
        "vm_issues": vm_issues,
        "services": items,
    }))
}

/// Reject a state-changing request that a web page could have forged.
///
/// With dashboard auth off, every loopback request is trusted, and a page open
/// in the user's browser reaches 127.0.0.1 as loopback. A cross-site "simple"
/// POST -- a form, or a fetch with no custom headers -- is sent without a CORS
/// preflight, so the CORS allowlist never sees it: CORS stops the page reading
/// the response, not the request arriving. Three checks close that:
///
/// 1. `Content-Type: application/json`. A browser only sends that cross-site
///    after a preflight, and the preflight is refused.
/// 2. `Host` names a loopback address. Without this, DNS rebinding defeats the
///    origin check: a page on evil.com rebinds its name to 127.0.0.1 and sends
///    Origin and Host both as `evil.com:port`, which agree with each other.
/// 3. If `Origin` is present it must be exactly `http://<Host>`. Browsers send
///    it on POSTs; curl and agents usually do not, and they are not the threat
///    this guards against.
pub fn same_origin_json(headers: &HeaderMap) -> Result<(), (StatusCode, &'static str)> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_json = content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "this endpoint accepts only Content-Type: application/json",
        ));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_loopback_authority(host) {
        return Err((
            StatusCode::FORBIDDEN,
            "the request is not addressed to this machine's loopback interface",
        ));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
            return Err((
                StatusCode::FORBIDDEN,
                "the request came from a different origin",
            ));
        }
    }
    Ok(())
}

/// Whether a `Host` value (`host` or `host:port`) names this machine.
fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, _)) => inner,
            None => return false,
        }
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(name, _)| name)
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// POST /api/services/vm-fix/{id} — run the fix for one reported Podman VM
/// problem, because the user pressed its Fix button.
///
/// The only place a fix runs. The id selects one of two fixed commands and is
/// never part of one, and the request must pass `same_origin_json` so a web
/// page cannot press the button on the user's behalf.
pub async fn vm_fix(
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err((status, reason)) = same_origin_json(&headers) {
        return (
            status,
            Json(serde_json::json!({ "ok": false, "error": reason })),
        );
    }
    if freeco_hands::container::vm_fix_command(&id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": format!("unknown VM issue '{id}'") })),
        );
    }
    match freeco_hands::container::repair_vm_issue(&id).await {
        Ok(message) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "message": message })),
        ),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "ok": false, "error": error })),
        ),
    }
}

/// GET /api/services/status — progress of the running install.
pub async fn service_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let s = state.services.read().await.clone();
    Json(serde_json::json!(s))
}

/// POST /api/services/{id}/install — install, run and connect a service.
pub async fn install_service(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(profile) = service_profile(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("unknown service '{id}'")})),
        );
    };

    {
        let mut s = state.services.write().await;
        if s.running {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "another service install is already running"})),
            );
        }
        s.running = true;
        s.service = profile.id.to_string();
        s.phase = "checking".into();
        s.detail = format!("Preparing {}...", profile.name);
        s.percent = -1;
    }

    let status = state.services.clone();
    let home = state.kernel.config.home_dir.clone();
    tokio::spawn(async move {
        let result = provision(&status, profile, &home).await;
        let mut s = status.write().await;
        s.running = false;
        match result {
            Ok(()) => {}
            Err(e) => {
                s.phase = "error".into();
                s.detail = e;
                s.percent = -1;
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "started", "service": profile.id})),
    )
}

async fn provision(
    status: &SharedServiceStatus,
    profile: &'static ServiceProfile,
    home: &std::path::Path,
) -> Result<(), String> {
    // 1. Docker
    if !container_runtime_ready().await {
        set_status(
            status,
            "needs-docker",
            "No container runtime is running. Start Podman (`podman machine start`) or Docker \
             Desktop, then press Install again — everything else is automatic."
                .into(),
            -1,
        )
        .await;
        return Ok(());
    }

    let dir = home.join("services").join(profile.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create service dir: {e}"))?;

    // 2. Materialise the service definition.
    if let Some(compose) = profile.compose {
        set_status(
            status,
            "fetching",
            format!("Writing {} configuration...", profile.name),
            -1,
        )
        .await;
        std::fs::write(dir.join("docker-compose.yaml"), compose)
            .map_err(|e| format!("write compose: {e}"))?;
    } else if let Some(repo) = profile.repo {
        if dir.join(".git").exists() {
            set_status(
                status,
                "fetching",
                format!("Updating {}...", profile.name),
                -1,
            )
            .await;
            let _ = run("git", &["pull", "--ff-only"], Some(&dir)).await;
        } else {
            set_status(
                status,
                "fetching",
                format!(
                    "Downloading {} (about {:.1} GB including images)...",
                    profile.name, profile.approx_download_gb
                ),
                -1,
            )
            .await;
            run(
                "git",
                &["clone", "--depth", "1", repo, &dir.to_string_lossy()],
                None,
            )
            .await?;
        }
    }

    // 2b. Repo-based services ship a `.env.example` and refuse to start (or come
    // up degraded) without a real `.env`. Materialise one with generated secrets
    // so "one click" genuinely works instead of failing inside Docker.
    if profile.repo.is_some() {
        ensure_env_file(&dir)?;
    }

    // 3. Start it, with whichever runtime is present.
    let rt = freeco_hands::container::runtime().await.ok_or_else(|| {
        "no container runtime found. Start Docker Desktop, or start Podman with \
         `podman machine start`"
            .to_string()
    })?;
    set_status(
        status,
        "starting",
        format!(
            "Starting {} with {rt} (first run pulls images)...",
            profile.name
        ),
        -1,
    )
    .await;
    run(rt, &["compose", "up", "-d"], Some(&dir))
        .await
        .map_err(|e| format!("could not start {}: {e}", profile.name))?;

    // 4. Wait for health.
    set_status(
        status,
        "waiting",
        format!("Waiting for {} to come up...", profile.name),
        -1,
    )
    .await;
    let mut up = false;
    for i in 0..90 {
        if health_ok(profile.health_url).await {
            up = true;
            break;
        }
        if i % 10 == 0 {
            set_status(
                status,
                "waiting",
                format!(
                    "Still starting {} ({}s)... first run downloads images, this can take a few minutes.",
                    profile.name,
                    i * 2
                ),
                -1,
            )
            .await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    if !up {
        return Err(format!(
            "{} was started but did not answer at {} within 3 minutes. \
             Check `docker compose logs` in {}.",
            profile.name,
            profile.health_url,
            dir.display()
        ));
    }

    // 5. Connect it (MCP-native services need nothing else).
    if let Some(mcp_url) = profile.mcp_url {
        set_status(
            status,
            "connecting",
            format!("Connecting {} to your agents...", profile.name),
            -1,
        )
        .await;
        let config_path = home.join("config.toml");
        crate::routes::register_mcp_http_server(&config_path, profile.id, mcp_url)
            .map_err(|e| format!("connected but could not save MCP entry: {e}"))?;
    }

    let done_msg = match profile.manual_step {
        Some(step) => format!(
            "{} is running at {}. One step left: {}",
            profile.name, profile.health_url, step
        ),
        None => format!(
            "{} is running and connected to your agents. Restart FreEco.ai to load its tools.",
            profile.name
        ),
    };
    set_status(status, "done", done_msg, 100).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dograh mounts MCP under its API prefix (`/api/v1/mcp`). Pointing at
    /// `/mcp` registers an endpoint that looks connected but is dead.
    #[test]
    fn dograh_mcp_url_uses_api_prefix() {
        let dograh = service_profile("dograh").expect("dograh in catalog");
        let url = dograh.mcp_url.expect("dograh is MCP-native");
        assert!(url.ends_with("/api/v1/mcp"), "got {url}");
    }

    /// dograh publishes host port 3000 for its UI, so the CRM must not also
    /// bind 3000 or the second install fails.
    #[test]
    fn crm_does_not_collide_with_dograh_ui_port() {
        let crm = service_profile("crm").expect("crm in catalog");
        assert!(
            !crm.health_url.contains(":3000"),
            "CRM must avoid port 3000"
        );
        let compose = crm.compose.expect("crm ships a compose file");
        assert!(
            !compose.contains("\"3000:3000\""),
            "CRM must not publish 3000"
        );
    }

    /// The Services card, its Connect step and the read-only accounting_api
    /// tool must all point at the same Akaunting.
    #[test]
    fn accounting_service_and_tool_agree() {
        let acc = service_profile("accounting").expect("accounting in catalog");
        assert_eq!(
            acc.health_url,
            freeco_kernel_runtime::accounting_tool::BASE_URL
        );
        assert!(acc.manual_step.unwrap_or("").contains("Connect"));
    }

    /// Every catalog entry must be materialisable: either a repo to clone or
    /// an embedded compose file.
    #[test]
    fn every_service_is_materialisable() {
        for s in SERVICE_CATALOG {
            assert!(
                s.repo.is_some() || s.compose.is_some(),
                "{} has no repo and no compose",
                s.id
            );
            assert!(s.health_url.starts_with("http"), "{} bad health_url", s.id);
        }
    }

    /// `.env` generation replaces change-me secrets and never clobbers a
    /// user-tuned file.
    #[test]
    fn env_file_is_generated_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env.example"),
            "# comment\nAPP_SECRET=\"change-me-please\"\nLOG_LEVEL=\"DEBUG\"\n",
        )
        .unwrap();

        ensure_env_file(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert!(
            !written.contains("change-me"),
            "placeholder must be replaced"
        );
        assert!(
            written.contains("LOG_LEVEL=\"DEBUG\""),
            "non-secrets preserved"
        );
        assert!(written.contains("# comment"), "comments preserved");

        // Second run must not overwrite.
        std::fs::write(dir.path().join(".env"), "USER_TUNED=1\n").unwrap();
        ensure_env_file(dir.path()).unwrap();
        let again = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert_eq!(again, "USER_TUNED=1\n");
    }

    fn headers(pairs: &[(&'static str, &str)]) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        for (name, value) in pairs {
            h.insert(
                axum::http::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
        }
        h
    }

    fn rejected_with(pairs: &[(&'static str, &str)]) -> Option<axum::http::StatusCode> {
        super::same_origin_json(&headers(pairs))
            .err()
            .map(|(status, _)| status)
    }

    #[test]
    fn vm_fix_guard_rejects_a_form_post_content_type() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/x-www-form-urlencoded"),
                ("host", "127.0.0.1:50051"),
            ]),
            Some(axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
    }

    #[test]
    fn vm_fix_guard_rejects_a_missing_content_type() {
        assert_eq!(
            rejected_with(&[("host", "127.0.0.1:50051")]),
            Some(axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
    }

    /// DNS rebinding: evil.com resolves to 127.0.0.1, and the browser sends
    /// Origin and Host both as evil.com. They agree with each other, so only
    /// the loopback-Host rule stops this.
    #[test]
    fn vm_fix_guard_rejects_dns_rebinding() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json"),
                ("host", "evil.com:50051"),
                ("origin", "http://evil.com:50051"),
            ]),
            Some(axum::http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn vm_fix_guard_rejects_a_foreign_origin_on_the_loopback_host() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:50051"),
                ("origin", "https://evil.com"),
            ]),
            Some(axum::http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn vm_fix_guard_rejects_a_loopback_origin_on_another_port() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:50051"),
                ("origin", "http://127.0.0.1:4200"),
            ]),
            Some(axum::http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn vm_fix_guard_accepts_the_dashboard_on_127_0_0_1() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:50051"),
                ("origin", "http://127.0.0.1:50051"),
            ]),
            None
        );
    }

    #[test]
    fn vm_fix_guard_accepts_the_dashboard_on_localhost() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json; charset=utf-8"),
                ("host", "localhost:50051"),
                ("origin", "http://localhost:50051"),
            ]),
            None
        );
    }

    /// curl and agents send no Origin; the loopback Host and JSON body are
    /// still required of them.
    #[test]
    fn vm_fix_guard_accepts_a_loopback_request_without_origin() {
        assert_eq!(
            rejected_with(&[
                ("content-type", "application/json"),
                ("host", "[::1]:50051"),
            ]),
            None
        );
    }

    /// `server.rs` registers `/api/services/{id}/install` and then
    /// `/api/services/vm-fix/{id}`: the same shape, a parameter against a
    /// static segment. axum panics when a route is inserted, so a conflict
    /// would abort daemon start-up while every other test passes -- no test
    /// builds the full router. This registers the pair in the same order and
    /// checks each request reaches its own handler, so a fix request can never
    /// fall through to the install endpoint.
    #[tokio::test]
    async fn vm_fix_route_coexists_with_and_wins_over_the_install_route() {
        use tower::ServiceExt;
        let router: axum::Router<()> = axum::Router::new()
            .route(
                "/api/services/{id}/install",
                axum::routing::post(|| async { axum::http::StatusCode::ACCEPTED }),
            )
            .route(
                "/api/services/vm-fix/{id}",
                axum::routing::post(|| async { axum::http::StatusCode::IM_A_TEAPOT }),
            );
        for (path, expected) in [
            (
                "/api/services/vm-fix/resolver",
                axum::http::StatusCode::IM_A_TEAPOT,
            ),
            (
                "/api/services/dograh/install",
                axum::http::StatusCode::ACCEPTED,
            ),
        ] {
            let status = router
                .clone()
                .oneshot(
                    axum::http::Request::post(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(status, expected, "{path} reached the wrong handler");
        }
    }
}

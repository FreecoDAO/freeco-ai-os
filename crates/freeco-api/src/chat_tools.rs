//! The working surface around a conversation: its files, what changed, what it
//! produced, a terminal into its sandbox, its browser, what it runs in the
//! background, and the conversation itself -- rename, fork, archive, delete.
//!
//! A chat that only showed messages hid everything an agent actually did: the
//! files it wrote were reachable only by asking it, its sandbox only through
//! it, and a long conversation could not be named, copied or put away. Each
//! route here answers one of those, and each stays inside the agent's own
//! workspace or sandbox: nothing here reads or runs anything on the host that
//! the agent itself could not.

use crate::routes::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use freeco_types::agent::AgentId;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Largest file returned inline as text.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
/// Largest file offered for download.
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Directories never walked: dependency and build caches, not the agent's work.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    "dist",
    ".next",
    ".mypy_cache",
    ".pytest_cache",
];
/// Where agents conventionally put what they make for the user.
const ARTIFACT_DIRS: &[&str] = &["output", "outputs", "artifacts", "exports", "reports"];
/// Formats that are deliverables wherever they sit.
const ARTIFACT_EXTS: &[&str] = &[
    "html", "pdf", "png", "jpg", "jpeg", "gif", "svg", "webp", "csv", "xlsx", "docx", "pptx", "md",
    "mp3", "wav", "mp4",
];

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Early-exit responses are boxed: `Response` is large, and returning it by
/// value through every `Result` here is what clippy's `result_large_err` flags.
type Deny = Box<Response>;

fn agent_id(id: &str) -> Result<AgentId, Deny> {
    id.parse::<AgentId>()
        .map_err(|_| Box::new(err(StatusCode::BAD_REQUEST, "invalid agent id")))
}

/// The agent's workspace root, or a response explaining why there is none.
fn workspace_of(state: &AppState, id: AgentId) -> Result<PathBuf, Deny> {
    let entry = state
        .kernel
        .registry
        .get(id)
        .ok_or_else(|| Box::new(err(StatusCode::NOT_FOUND, "agent not found")))?;
    entry
        .manifest
        .workspace
        .clone()
        .ok_or_else(|| Box::new(err(StatusCode::NOT_FOUND, "this agent has no workspace")))
}

/// Resolve a path the user asked for, confined to the workspace.
fn confined(root: &std::path::Path, rel: &str) -> Result<PathBuf, Deny> {
    let rel = if rel.trim().is_empty() { "." } else { rel };
    freeco_kernel_runtime::workspace_sandbox::resolve_sandbox_path(rel, root)
        .map_err(|e| Box::new(err(StatusCode::FORBIDDEN, e)))
}

/// `p` relative to the workspace root, with forward slashes.
///
/// Paths arrive in two forms: canonical ones from the confinement check and
/// plain ones from the directory walk. On Windows canonical paths carry a
/// `\\?\` prefix, so comparing one form against the other never matched and
/// the panel showed full host paths -- and, since every path then had a slash
/// in it, root files were misfiled. Both forms are tried on both sides.
fn rel_to(root: &std::path::Path, p: &std::path::Path) -> String {
    let canon_root = root.canonicalize().ok();
    let canon_p = p.canonicalize().ok();
    let rel = p
        .strip_prefix(root)
        .ok()
        .or_else(|| canon_root.as_deref().and_then(|r| p.strip_prefix(r).ok()))
        .or_else(|| {
            let cp = canon_p.as_deref()?;
            cp.strip_prefix(canon_root.as_deref().unwrap_or(root)).ok()
        })
        .unwrap_or(p);
    rel.to_string_lossy().replace('\\', "/")
}

fn modified_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Every file under `root`, skipping caches, capped so a huge tree cannot stall
/// the request.
fn walk(root: &std::path::Path, cap: usize) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else {
                out.push((path, meta));
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

// ── Files ──────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub download: bool,
    /// Show an HTML file as a page rather than as its source.
    #[serde(default)]
    pub open: bool,
}

/// The policy an agent's HTML page runs under when opened.
///
/// A map or report an agent builds needs its scripts and its tiles from the
/// web, so plain `sandbox` (no scripts) left it blank. It still gets an
/// opaque origin, so it cannot read the dashboard's storage, and `connect-src
/// https:` keeps it from calling FreEco's own API on plain-http localhost.
const OPEN_PAGE_CSP: &str = "sandbox allow-scripts allow-popups; \
default-src https: data: blob: 'unsafe-inline'; connect-src https:";

/// GET /api/agents/{id}/workspace/tree?path= — one directory of the workspace.
pub async fn workspace_tree(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let root = match workspace_of(&state, id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let dir = match confined(&root, &q.path) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return err(StatusCode::NOT_FOUND, "no such directory in the workspace");
    };
    let mut items: Vec<serde_json::Value> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(serde_json::json!({
                "name": e.file_name().to_string_lossy(),
                "path": rel_to(&root, &e.path()),
                "is_dir": meta.is_dir(),
                "size": if meta.is_dir() { 0 } else { meta.len() },
                "modified": modified_secs(&meta),
            }))
        })
        .collect();
    // Folders first, then by name, as every file browser does.
    items.sort_by(|a, b| {
        let da = a["is_dir"].as_bool().unwrap_or(false);
        let db = b["is_dir"].as_bool().unwrap_or(false);
        db.cmp(&da).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Json(serde_json::json!({ "path": rel_to(&root, &dir), "entries": items })).into_response()
}

/// GET /api/agents/{id}/workspace/file?path=[&download=true]
///
/// Text up to 1 MB comes back inline for viewing; anything else, or any file
/// with `download=true`, comes back as the file itself.
pub async fn workspace_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let root = match workspace_of(&state, id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let path = match confined(&root, &q.path) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return err(StatusCode::NOT_FOUND, "no such file in the workspace");
    };
    if meta.is_dir() {
        return err(StatusCode::BAD_REQUEST, "that is a directory");
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mime = mime_guess(&name);

    let is_text = mime.starts_with("text/") || mime == "application/json";
    // `open` serves the file itself (a link a person clicks wants the file,
    // not JSON); only an HTML page gets to run its scripts.
    let open_page = q.open && !q.download && mime.starts_with("text/html");
    if q.download || q.open || meta.len() > MAX_TEXT_BYTES || !is_text {
        if meta.len() > MAX_DOWNLOAD_BYTES {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "file is larger than 200 MB");
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file");
        };
        let disposition = if q.download { "attachment" } else { "inline" };
        // The name is reduced to safe characters so it cannot break the header.
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || ".-_".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        return (
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("{disposition}; filename=\"{safe}\""),
                ),
                // Never let a file from an agent's workspace run as a page on
                // the dashboard's origin.
                (
                    header::CONTENT_SECURITY_POLICY,
                    if open_page { OPEN_PAGE_CSP } else { "sandbox" }.to_string(),
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response();
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Json(serde_json::json!({
        "path": rel_to(&root, &path),
        "size": meta.len(),
        "modified": modified_secs(&meta),
        "content": text,
    }))
    .into_response()
}

fn mime_guess(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" | "log" | "md" | "rs" | "py" | "js" | "ts" | "toml" | "yaml" | "yml" | "sh"
        | "css" | "sql" | "ini" | "cfg" | "env" | "csv" | "xml" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

// ── Changes and artifacts ──────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ChangesQuery {
    pub hours: Option<u64>,
}

/// GET /api/agents/{id}/workspace/changes?hours=24 — files changed recently,
/// newest first. What the agent actually touched, without asking it.
pub async fn workspace_changes(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ChangesQuery>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let root = match workspace_of(&state, id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let hours = q.hours.unwrap_or(24).clamp(1, 24 * 30);
    let since = SystemTime::now()
        .checked_sub(Duration::from_secs(hours * 3600))
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut changed: Vec<(u64, serde_json::Value)> = walk(&root, 20_000)
        .into_iter()
        .filter_map(|(p, meta)| {
            let m = modified_secs(&meta);
            (m >= since).then(|| {
                (
                    m,
                    serde_json::json!({
                        "path": rel_to(&root, &p),
                        "size": meta.len(),
                        "modified": m,
                    }),
                )
            })
        })
        .collect();
    changed.sort_by_key(|c| std::cmp::Reverse(c.0));
    changed.truncate(500);
    Json(serde_json::json!({
        "hours": hours,
        "changes": changed.into_iter().map(|c| c.1).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// GET /api/agents/{id}/artifacts — what the agent made for the user: anything
/// in an output folder, and documents, images and media anywhere else.
pub async fn artifacts(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let root = match workspace_of(&state, id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let mut found: Vec<(u64, serde_json::Value)> = walk(&root, 20_000)
        .into_iter()
        .filter_map(|(p, meta)| {
            let rel = rel_to(&root, &p);
            let first = rel.split('/').next().unwrap_or("").to_lowercase();
            let ext = rel.rsplit('.').next().unwrap_or("").to_lowercase();
            // Identity files are how the agent is configured, not its output.
            let is_identity = !rel.contains('/')
                && rel.ends_with(".md")
                && rel.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            let is_artifact = ARTIFACT_DIRS.contains(&first.as_str())
                || (ARTIFACT_EXTS.contains(&ext.as_str()) && !is_identity);
            is_artifact.then(|| {
                let m = modified_secs(&meta);
                (
                    m,
                    serde_json::json!({
                        "path": rel,
                        "size": meta.len(),
                        "modified": m,
                        "kind": ext,
                    }),
                )
            })
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.truncate(300);
    Json(serde_json::json!({ "artifacts": found.into_iter().map(|f| f.1).collect::<Vec<_>>() }))
        .into_response()
}

// ── Terminal ───────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct TerminalRequest {
    pub command: String,
    /// "exec" (throwaway, no network) or "workspace" (writable, networked).
    pub tier: Option<String>,
}

/// POST /api/agents/{id}/terminal — run a command in the agent's own sandbox.
///
/// The same path the agent's `sandbox_exec` tool takes: its container, its
/// workspace mounted at /workspace, its limits. Never the host. This is what
/// lets a person look at what the agent sees, or finish a step by hand,
/// without a second, differently isolated route into the machine.
pub async fn terminal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TerminalRequest>,
) -> Response {
    let id_str = id.clone();
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let root = match workspace_of(&state, id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if req.command.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "command is empty");
    }
    if !state.kernel.config.docker.enabled {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "the sandbox is not enabled in config.toml [docker]",
        );
    }
    let mut input = serde_json::json!({ "command": req.command });
    if let Some(t) = req.tier.as_deref() {
        input["tier"] = serde_json::json!(t);
    }
    let handle: Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle> =
        state.kernel.clone() as Arc<dyn freeco_kernel_runtime::kernel_handle::KernelHandle>;
    match freeco_kernel_runtime::tool_runner::tool_docker_exec(
        &input,
        Some(&state.kernel.config.docker),
        Some(root.as_path()),
        Some(id_str.as_str()),
        Some(&handle),
    )
    .await
    {
        Ok(output) => Json(serde_json::json!({ "ok": true, "output": output })).into_response(),
        Err(e) => Json(serde_json::json!({ "ok": false, "output": e })).into_response(),
    }
}

// ── Browser ────────────────────────────────────────────────────────────────

/// GET /api/agents/{id}/browser/screenshot — what the agent's browser shows now.
pub async fn browser_screenshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if agent_id(&id).is_err() {
        return err(StatusCode::BAD_REQUEST, "invalid agent id");
    }
    if !state.kernel.browser_ctx.has_session(&id) {
        return Json(serde_json::json!({
            "active": false,
            "message": "This agent has no browser open right now.",
        }))
        .into_response();
    }
    let resp = state
        .kernel
        .browser_ctx
        .send_command(
            &id,
            freeco_kernel_runtime::browser::BrowserCommand::Screenshot,
        )
        .await;
    match resp {
        Ok(r) if r.success => {
            Json(serde_json::json!({ "active": true, "data": r.data })).into_response()
        }
        Ok(r) => err(
            StatusCode::BAD_GATEWAY,
            r.error.unwrap_or_else(|| "screenshot failed".into()),
        ),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

// ── Background work ────────────────────────────────────────────────────────

/// GET /api/agents/{id}/background — what the agent runs without being asked:
/// long-lived processes and scheduled jobs.
pub async fn background(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let aid = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let processes: Vec<serde_json::Value> = state
        .kernel
        .process_manager
        .list(&id)
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "command": p.command,
                "alive": p.alive,
                "uptime_secs": p.uptime_secs,
            })
        })
        .collect();
    let jobs = state.kernel.cron_scheduler.list_jobs(aid);
    Json(serde_json::json!({ "processes": processes, "scheduled": jobs })).into_response()
}

// ── The conversation itself ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct LabelRequest {
    pub label: Option<String>,
}

/// PUT /api/agents/{id}/conversation/label — name the conversation.
pub async fn conversation_label(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<LabelRequest>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let label = req
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    if let Some(l) = label {
        if l.chars().count() > 120 {
            return err(
                StatusCode::BAD_REQUEST,
                "names are limited to 120 characters",
            );
        }
    }
    match state.kernel.memory.set_canonical_label(id, label) {
        Ok(()) => Json(serde_json::json!({ "label": label })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn archive_dir(state: &AppState) -> PathBuf {
    state
        .kernel
        .config
        .home_dir
        .join("archive")
        .join("conversations")
}

/// Write the conversation to a file before it is cleared, so archive and
/// delete never lose it: both are recoverable from the archive list.
fn snapshot(state: &AppState, id: AgentId, reason: &str) -> Result<PathBuf, String> {
    let canonical = state
        .kernel
        .memory
        .load_canonical(id)
        .map_err(|e| e.to_string())?;
    let agent_name = state
        .kernel
        .registry
        .get(id)
        .map(|e| e.name.clone())
        .unwrap_or_default();
    let dir = archive_dir(state);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("{id}-{stamp}-{reason}.json"));
    let body = serde_json::json!({
        "agent_id": id.to_string(),
        "agent_name": agent_name,
        "label": canonical.label,
        "reason": reason,
        "archived_at": chrono::Utc::now().to_rfc3339(),
        "summary": canonical.compacted_summary,
        "messages": canonical.messages,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Empty the conversation the chat shows.
///
/// `reset_session` alone resets the other session store, not the per-agent
/// conversation the chat reads -- so archive and delete reported success and
/// left every message in place, and a restore would have appended the archived
/// messages onto the ones still there. Both are cleared here.
fn clear_conversation(state: &AppState, id: AgentId) -> Result<(), String> {
    state
        .kernel
        .memory
        .delete_canonical_session(id)
        .map_err(|e| e.to_string())?;
    state.kernel.reset_session(id).map_err(|e| e.to_string())
}

/// POST /api/agents/{id}/conversation/archive — put the conversation away and
/// start fresh. Kept in the archive; restorable.
pub async fn conversation_archive(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let path = match snapshot(&state, id, "archived") {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not archive: {e}"),
            )
        }
    };
    if let Err(e) = clear_conversation(&state, id) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("archived, but could not clear: {e}"),
        );
    }
    Json(serde_json::json!({ "archived_to": path.display().to_string() })).into_response()
}

/// DELETE /api/agents/{id}/conversation — clear the conversation.
///
/// A copy goes to the archive first. Deleting a conversation is usually
/// wanted, and occasionally regretted within the minute; the copy costs a few
/// kilobytes and turns the second case from "gone" into "restore".
pub async fn conversation_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    // No copy, no delete. The copy is the promise that makes deleting safe; a
    // failed copy that was ignored made the deletion permanent.
    let path = match snapshot(&state, id, "deleted") {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("not deleted: could not keep a copy first ({e})"),
            )
        }
    };
    if let Err(e) = clear_conversation(&state, id) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not delete: {e}"),
        );
    }
    Json(serde_json::json!({
        "deleted": true,
        "recoverable_from": path.display().to_string(),
    }))
    .into_response()
}

/// GET /api/agents/{id}/conversation/archives — conversations put away.
pub async fn conversation_archives(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if agent_id(&id).is_err() {
        return err(StatusCode::BAD_REQUEST, "invalid agent id");
    }
    let dir = archive_dir(&state);
    let mut items: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !name.starts_with(&id) || !name.ends_with(".json") {
                        return None;
                    }
                    let raw = std::fs::read(e.path()).ok()?;
                    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
                    Some(serde_json::json!({
                        "file": name,
                        "label": v["label"],
                        "reason": v["reason"],
                        "archived_at": v["archived_at"],
                        "message_count": v["messages"].as_array().map(|a| a.len()).unwrap_or(0),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| b["archived_at"].as_str().cmp(&a["archived_at"].as_str()));
    Json(serde_json::json!({ "archives": items })).into_response()
}

/// POST /api/agents/{id}/conversation/archives/{file}/restore — bring an
/// archived conversation back. The current one is archived first, so a
/// restore cannot itself lose anything.
pub async fn conversation_restore(
    State(state): State<Arc<AppState>>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    let aid = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    // The name must be one this agent's archive produced: no separators, no
    // traversal, and prefixed with the agent's own id.
    if file.contains(['/', '\\'])
        || file.contains("..")
        || !file.starts_with(&id)
        || !file.ends_with(".json")
    {
        return err(StatusCode::BAD_REQUEST, "not an archive of this agent");
    }
    let path = archive_dir(&state).join(&file);
    let raw = match std::fs::read(&path) {
        Ok(r) => r,
        Err(_) => return err(StatusCode::NOT_FOUND, "no such archive"),
    };
    let v: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("archive is unreadable: {e}"),
            )
        }
    };
    let messages: Vec<freeco_types::message::Message> =
        match serde_json::from_value(v["messages"].clone()) {
            Ok(m) => m,
            Err(e) => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("archive is unreadable: {e}"),
                )
            }
        };
    // The current conversation is kept before it is replaced; if it cannot be,
    // nothing is replaced.
    if let Err(e) = snapshot(&state, aid, "replaced-by-restore") {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("not restored: could not keep the current conversation first ({e})"),
        );
    }
    if let Err(e) = clear_conversation(&state, aid) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = state.kernel.memory.append_canonical(aid, &messages, None) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = state
        .kernel
        .memory
        .set_canonical_label(aid, v["label"].as_str());
    Json(serde_json::json!({ "restored": messages.len() })).into_response()
}

#[derive(serde::Deserialize)]
pub struct ForkRequest {
    pub name: Option<String>,
}

/// POST /api/agents/{id}/conversation/fork — continue the conversation in a
/// copy of the agent, leaving the original untouched.
///
/// For trying a different direction without losing the one already taken. The
/// copy gets the same configuration, a fresh workspace, and the conversation so
/// far, so it picks up where the original stood.
pub async fn conversation_fork(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ForkRequest>,
) -> Response {
    let id = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(source) = state.kernel.registry.get(id) else {
        return err(StatusCode::NOT_FOUND, "agent not found");
    };
    let canonical = match state.kernel.memory.load_canonical(id) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    // A readable, unique name: "<agent> (fork 2)" and so on.
    let base = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} (fork)", source.name));
    if base.contains(['/', '\\']) || base.chars().any(|c| c.is_control()) {
        return err(
            StatusCode::BAD_REQUEST,
            "names cannot contain slashes or control characters",
        );
    }
    let mut name = base.clone();
    let mut n = 2;
    while state.kernel.registry.find_by_name(&name).is_some() {
        name = format!("{base} {n}");
        n += 1;
    }
    let mut manifest = source.manifest.clone();
    manifest.name = name.clone();
    // A fork is a new agent: it gets its own workspace and state rather than
    // sharing the original's, which would let the two overwrite each other.
    manifest.workspace = None;
    manifest.state_dir = None;
    drop(source);
    let new_id = match state.kernel.spawn_agent(manifest) {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not create the fork: {e}"),
            )
        }
    };
    if let Err(e) = state
        .kernel
        .memory
        .append_canonical(new_id, &canonical.messages, None)
    {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("fork created, but the conversation could not be copied: {e}"),
        );
    }
    let label = canonical.label.map(|l| format!("{l} (fork)"));
    let _ = state
        .kernel
        .memory
        .set_canonical_label(new_id, label.as_deref());
    Json(serde_json::json!({ "agent_id": new_id.to_string(), "name": name })).into_response()
}

// ── Output style ───────────────────────────────────────────────────────────

/// GET /api/output-styles — the styles an agent can be switched to.
pub async fn output_styles() -> impl IntoResponse {
    let list: Vec<serde_json::Value> = freeco_kernel::kernel::OUTPUT_STYLES
        .iter()
        .map(|(id, text)| serde_json::json!({ "id": id, "description": text }))
        .collect();
    Json(serde_json::json!({ "styles": list }))
}

#[derive(serde::Deserialize)]
pub struct StyleRequest {
    pub style: String,
}

/// PUT /api/agents/{id}/output-style — how this agent writes its replies.
/// POST /api/agents/{id}/tasks — `{ "title": "…", "description": "…" }`.
///
/// Puts a task on the shared board, assigned to this agent and marked as the
/// owner's, so a message sent as "New task" is work the board can track, not
/// only a conversation. The board was reachable only by agents' own tools.
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let aid = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let title = body["title"].as_str().unwrap_or("").trim();
    let description = body["description"].as_str().unwrap_or("").trim();
    if title.is_empty() {
        return err(StatusCode::BAD_REQUEST, "a task needs a title");
    }
    let title: String = title.chars().take(200).collect();
    match state
        .kernel
        .memory
        .task_post(&title, description, Some(&aid.to_string()), Some("owner"))
        .await
    {
        Ok(task_id) => {
            Json(serde_json::json!({ "task_id": task_id, "title": title })).into_response()
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not create the task: {e}"),
        ),
    }
}

pub async fn set_output_style(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<StyleRequest>,
) -> Response {
    let aid = match agent_id(&id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !freeco_kernel::kernel::OUTPUT_STYLES
        .iter()
        .any(|(s, _)| *s == req.style)
    {
        return err(StatusCode::BAD_REQUEST, "unknown output style");
    }
    let updated = state.kernel.registry.update_metadata(
        aid,
        "output_style".to_string(),
        serde_json::json!(req.style),
    );
    if let Err(e) = updated {
        return err(StatusCode::NOT_FOUND, e.to_string());
    }
    state.kernel.persist_manifest_to_disk(aid);
    Json(serde_json::json!({ "style": req.style })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An opened agent page runs scripts, but stays off the dashboard's
    /// origin and cannot reach FreEco's API on plain-http localhost.
    #[test]
    fn an_opened_page_is_sandboxed_away_from_the_dashboard() {
        assert!(OPEN_PAGE_CSP.starts_with("sandbox "));
        assert!(!OPEN_PAGE_CSP.contains("allow-same-origin"));
        assert!(OPEN_PAGE_CSP.contains("connect-src https:"));
        assert!(!OPEN_PAGE_CSP.contains("http:"));
    }

    #[test]
    fn mime_types_are_sensible() {
        assert!(mime_guess("notes.md").starts_with("text/"));
        assert_eq!(mime_guess("report.pdf"), "application/pdf");
        assert_eq!(mime_guess("unknown.bin"), "application/octet-stream");
    }

    #[test]
    fn walking_skips_dependency_caches() {
        let tmp = std::env::temp_dir().join(format!("ct-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(tmp.join("src/main.rs"), "x").unwrap();
        let names: Vec<String> = walk(&tmp, 100)
            .into_iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&"index.js".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Walked paths are plain, the root may be canonical (`\\?\C:\...` on
    /// Windows): both must give the same workspace-relative path.
    #[test]
    fn relative_paths_match_whichever_form_the_path_is_in() {
        let tmp = std::env::temp_dir().join(format!("ct-rel-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("output")).unwrap();
        std::fs::write(tmp.join("output").join("a.csv"), "x").unwrap();
        let plain = tmp.join("output").join("a.csv");
        let canon = plain.canonicalize().unwrap();
        assert_eq!(rel_to(&tmp, &plain), "output/a.csv");
        assert_eq!(rel_to(&tmp, &canon), "output/a.csv");
        assert_eq!(rel_to(&tmp.canonicalize().unwrap(), &plain), "output/a.csv");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

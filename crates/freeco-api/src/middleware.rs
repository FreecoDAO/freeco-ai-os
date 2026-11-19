//! Production middleware for the Freeco API server.
//!
//! Provides:
//! - Request ID generation and propagation
//! - Per-endpoint structured request logging
//! - In-memory rate limiting (per IP)
//! - Authentication, and the role every change needs (see [`Caller`])

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;
use freeco_kernel::auth::Action;
use freeco_types::auth::UserRole;
use freeco_types::config::{KernelConfig, UserConfig};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

/// Request ID header name (standard).
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Middleware: inject a unique request ID and log the request/response.
pub async fn request_logging(request: Request<Body>, next: Next) -> Response<Body> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let method = request.method().clone();
    let uri = request.uri().path().to_string();
    let start = Instant::now();

    let mut response = next.run(request).await;

    let elapsed = start.elapsed();
    let status = response.status().as_u16();

    info!(
        request_id = %request_id,
        method = %method,
        path = %uri,
        status = status,
        latency_ms = elapsed.as_millis() as u64,
        "API request"
    );

    // Inject the request ID into the response
    if let Ok(header_val) = request_id.parse() {
        response.headers_mut().insert(REQUEST_ID_HEADER, header_val);
    }

    response
}

/// Authentication state passed to the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    pub api_key: String,
    pub auth_enabled: bool,
    /// From [`crate::session_auth::session_secret`]; nothing else.
    pub session_secret: String,
    /// Set from `FREECO_AI_ALLOW_NO_AUTH=1` (or the legacy
    /// `OPENFANG_ALLOW_NO_AUTH=1`) to permit running without an api_key on a
    /// non-loopback bind. Off by default so empty keys fail closed.
    pub allow_no_auth: bool,
    /// `auth.username`: the admin account, always the owner.
    pub admin_username: String,
    /// `[[users]]`, to resolve a session's current role.
    pub users: Arc<Vec<UserConfig>>,
}

impl AuthState {
    /// Build the auth state from the kernel config, with the signing secret
    /// from the same function login and every handler use.
    pub fn from_config(config: &KernelConfig, allow_no_auth: bool) -> Self {
        Self {
            api_key: config.api_key.trim().to_string(),
            auth_enabled: config.auth.enabled,
            session_secret: crate::session_auth::session_secret(config),
            allow_no_auth,
            admin_username: config.auth.username.clone(),
            users: Arc::new(config.users.clone()),
        }
    }
}

/// How a request proved who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// Dashboard login off and the request came from this machine (or
    /// `FREECO_AI_ALLOW_NO_AUTH=1`): the single local user.
    Local,
    /// `Authorization: Bearer`, `X-API-Key` or `?token=` with the API key.
    ApiKey,
    /// A signed dashboard session cookie.
    Session,
}

/// Who is calling, and with which role. The auth middleware puts one on every
/// request it lets through; handlers take it as an extractor:
///
/// ```ignore
/// async fn handler(caller: Caller) -> Response {
///     if let Err(denied) = caller.require(UserRole::Admin) { return denied; }
///     ...
/// }
/// ```
///
/// The local user (login off) and an API-key holder are the owner; a session
/// has the lower of the role in its cookie and its account's current role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The signed-in account, for a session. `None` for local and API-key
    /// callers, which are not a named account.
    pub username: Option<String>,
    /// The role this request acts with.
    pub role: UserRole,
    /// How it authenticated.
    pub method: AuthMethod,
}

impl Caller {
    pub(crate) fn owner(method: AuthMethod) -> Self {
        Self {
            username: None,
            role: UserRole::Owner,
            method,
        }
    }

    /// Whether this caller has at least `min`.
    pub fn has(&self, min: UserRole) -> bool {
        self.role >= min
    }

    /// `Ok` when this caller has at least `min`, otherwise the 403 to return.
    pub fn require(&self, min: UserRole) -> Result<(), Response<Body>> {
        require_role(self.role, min)
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Caller {
    type Rejection = Response<Body>;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        // Absent only on a public route, which the middleware passes through
        // without identifying anyone.
        parts
            .extensions
            .get::<Caller>()
            .cloned()
            .ok_or_else(|| json_error(StatusCode::UNAUTHORIZED, "Not signed in"))
    }
}

fn json_error(status: StatusCode, msg: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "error": msg }).to_string()))
        .unwrap_or_default()
}

/// `Ok` when `role` is at least `min`, otherwise a 403 naming the role needed.
pub fn require_role(role: UserRole, min: UserRole) -> Result<(), Response<Body>> {
    if role >= min {
        Ok(())
    } else {
        Err(json_error(
            StatusCode::FORBIDDEN,
            &format!("This needs the {min} role; you are signed in as {role}"),
        ))
    }
}

/// The role a request needs, or `None` for a read.
///
/// Reads (GET, HEAD, OPTIONS) need only a valid sign-in, as before. Every
/// change needs at least the role of the kernel [`Action`] it is:
/// - chatting (sending a message, uploading for it, starting or routing a
///   chat) is [`Action::ChatWithAgent`];
/// - accounts are [`Action::ManageUsers`], the system config
///   [`Action::ModifyConfig`];
/// - anything else is [`Action::ChangeSystem`].
pub fn required_role_for_request(method: &axum::http::Method, path: &str) -> Option<UserRole> {
    use axum::http::Method;
    if *method == Method::GET || *method == Method::HEAD || *method == Method::OPTIONS {
        return None;
    }
    if is_team_work(method, path) {
        // The handler decides from the caller's role on that team
        // (`team_access`); here it only has to be an account that may change
        // anything at all.
        return Some(UserRole::User);
    }
    let action = if path == "/api/users" || path.starts_with("/api/users/") {
        Action::ManageUsers
    } else if path == "/api/config" || path.starts_with("/api/config/") {
        Action::ModifyConfig
    } else if is_chat(method, path) {
        Action::ChatWithAgent
    } else {
        Action::ChangeSystem
    };
    Some(action.required_role())
}

/// Whether a change is work on one team that a team lead or member may do:
/// adding or removing a member, and posting, moving, assigning or deleting a
/// card on the team's board. Everything else under a team (creating,
/// renaming, running it, setting a member's role) stays with admins.
fn is_team_work(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    let Some(rest) = path.strip_prefix("/api/org/teams/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    match parts.as_slice() {
        [_, "members"] | [_, "tasks"] => *method == Method::POST,
        [_, "members", _] => *method == Method::DELETE,
        [_, "tasks", _] => *method == Method::PUT || *method == Method::DELETE,
        _ => false,
    }
}

/// Whether a change is part of chatting with an agent.
fn is_chat(method: &axum::http::Method, path: &str) -> bool {
    if *method != axum::http::Method::POST {
        return false;
    }
    if path == "/api/sessions/route" {
        return true;
    }
    // Replying in a Chamber conversation; crate::access decides where.
    if let Some(rest) = path.strip_prefix("/api/chamber/conversations/") {
        return rest
            .strip_suffix("/reply")
            .is_some_and(|id| !id.contains('/'));
    }
    let Some(rest) = path.strip_prefix("/api/agents/") else {
        return false;
    };
    let Some((_id, action)) = rest.split_once('/') else {
        return false;
    };
    matches!(action, "message" | "message/stream" | "upload" | "sessions")
}

/// Whether an explicit environment opt-in permits unauthenticated non-loopback
/// access. The FreEco.ai variable takes precedence over the legacy name.
pub fn allow_no_auth_from_env() -> bool {
    std::env::var("FREECO_AI_ALLOW_NO_AUTH")
        .or_else(|_| std::env::var("OPENFANG_ALLOW_NO_AUTH"))
        .map(|v| is_true_env_flag(&v))
        .unwrap_or(false)
}

fn is_true_env_flag(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "on")
}

/// Headers a reverse proxy adds when it relays a request for someone else.
const FORWARDING_HEADERS: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-real-ip",
    "cf-connecting-ip",
    "true-client-ip",
];

/// Whether a request was made on this machine, rather than relayed by a
/// proxy running on it.
///
/// A reverse proxy on the same host (Caddy, nginx, a tunnel) connects from
/// 127.0.0.1, so every request from the internet it passes on looked local:
/// it skipped the API key when none was set, could turn dashboard login off
/// and set the first password. A relayed request names who it is for in one
/// of [`FORWARDING_HEADERS`]; such a request is not this machine's.
pub fn from_this_device(peer: std::net::IpAddr, headers: &axum::http::HeaderMap) -> bool {
    peer.is_loopback() && !FORWARDING_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// Bearer token authentication middleware.
///
/// When `api_key` is non-empty (after trimming), requests to non-public
/// endpoints must include `Authorization: Bearer <api_key>`.
///
/// When `api_key` is empty (no key configured) the server defaults to
/// fail-closed for any request that does NOT originate from loopback.
/// Loopback traffic (127.0.0.1 / ::1) is always allowed through with no
/// key so single-user local setups keep zero-config UX. To explicitly
/// run a no-auth server on a LAN/WAN address, set
/// `FREECO_AI_ALLOW_NO_AUTH=1` (or the legacy `OPENFANG_ALLOW_NO_AUTH=1`);
/// this opts out of fail-closed and is reported loudly at startup.
///
/// When dashboard auth is enabled, session cookies are also accepted, and a
/// session may make a change only with the role
/// [`required_role_for_request`] names (403 otherwise). Every request let
/// through, except on public routes, carries a [`Caller`].
pub async fn auth(
    axum::extract::State(auth_state): axum::extract::State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // SECURITY: Capture method early for method-aware public endpoint checks.
    let method = request.method().clone();

    let is_loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| from_this_device(ci.0.ip(), request.headers()))
        .unwrap_or(false); // SECURITY: default-deny; unknown origin is NOT loopback

    // Owned, so the request can still be given its `Caller` below.
    let path = request.uri().path().to_string();
    let path = path.as_str();
    // Shutdown is loopback-only (CLI on same machine). Skip token auth only
    // when the request is from loopback.
    if path == "/api/shutdown" && is_loopback {
        return next.run(request).await;
    }

    // Public endpoints that don't require auth (dashboard needs these).
    // SECURITY: /api/agents is GET-only (listing). POST (spawn) requires auth.
    // SECURITY: Public endpoints are GET-only unless explicitly noted.
    // POST/PUT/DELETE to any endpoint ALWAYS requires auth to prevent
    // unauthenticated writes (cron job creation, skill install, etc.).
    let is_get = method == axum::http::Method::GET;
    let is_public = path == "/"
        || path == "/logo.png"
        || path == "/favicon.ico"
        || (path == "/.well-known/agent.json" && is_get)
        || (path.starts_with("/a2a/") && is_get)
        || path == "/api/health"
        || path == "/api/version"
        || (path.starts_with("/api/cron/") && is_get)
        || path.starts_with("/api/providers/github-copilot/oauth/")
        || path == "/api/auth/login"
        || path == "/api/auth/logout"
        || path == "/api/auth/set-password"
        || path == "/api/auth/dismiss-setup"
        || path == "/api/auth/disable"
        || (path == "/api/auth/check" && is_get)
        // The MCP endpoint authenticates itself with FREECO_MCP_TOKEN and
        // refuses everything while that is unset. Both use the Authorization
        // header, so checking the API key here as well rejected every MCP
        // call whenever an api_key or dashboard login was configured.
        || path == "/mcp";

    if is_public {
        return next.run(request).await;
    }

    // If no API key configured and no dashboard login is active, fail closed
    // for anything that did not come from loopback. Opting out of this
    // behavior requires setting `FREECO_AI_ALLOW_NO_AUTH=1`, which is logged
    // loudly at startup.
    //
    // See issue #1034 (B1/B2): empty api_key previously bypassed auth for
    // all origins, exposing agent config, channel tokens, and LLM keys on
    // any LAN-reachable bind.
    let api_key_trimmed = auth_state.api_key.trim().to_string();
    if api_key_trimmed.is_empty() && !auth_state.auth_enabled {
        if is_loopback || auth_state.allow_no_auth {
            request
                .extensions_mut()
                .insert(Caller::owner(AuthMethod::Local));
            return next.run(request).await;
        }
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "Bearer")
            .body(Body::from(
                serde_json::json!({
                    "error": "API key required for non-loopback requests. Set FREECO_AI_API_KEY or bind to 127.0.0.1."
                })
                .to_string(),
            ))
            .unwrap_or_default();
    }
    let api_key = api_key_trimmed.as_str();

    // Check Authorization: Bearer <token> header, then fallback to X-API-Key
    let bearer_token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let api_token = bearer_token.or_else(|| {
        request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
    });

    // SECURITY: Use constant-time comparison to prevent timing attacks.
    //
    // With dashboard login on and no api_key, `api_key` is empty -- and an
    // empty token has the same length, so `Bearer ` with nothing after it
    // compared equal and got in. An empty key matches nothing.
    let header_auth = api_token.map(|token| {
        use subtle::ConstantTimeEq;
        if api_key.is_empty() || token.len() != api_key.len() {
            return false;
        }
        token.as_bytes().ct_eq(api_key.as_bytes()).into()
    });

    // Also check ?token= query parameter (for EventSource/SSE clients that
    // cannot set custom headers, same approach as WebSocket auth).
    let query_token_decoded = request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")))
        .map(crate::percent_decode);

    // SECURITY: Use constant-time comparison to prevent timing attacks.
    let query_auth = query_token_decoded.as_deref().map(|token| {
        use subtle::ConstantTimeEq;
        if api_key.is_empty() || token.len() != api_key.len() {
            return false;
        }
        token.as_bytes().ct_eq(api_key.as_bytes()).into()
    });

    // Accept if either auth method matches
    if header_auth == Some(true) || query_auth == Some(true) {
        request
            .extensions_mut()
            .insert(Caller::owner(AuthMethod::ApiKey));
        return next.run(request).await;
    }

    // Check session cookie (dashboard login sessions). The session's role
    // decides whether it may make this change.
    if auth_state.auth_enabled {
        let claims =
            crate::session_auth::extract_session_cookie(request.headers()).and_then(|token| {
                crate::session_auth::verify_session_claims(&token, &auth_state.session_secret)
            });
        if let Some(claims) = claims {
            let role = crate::session_auth::effective_role(
                &claims,
                &auth_state.admin_username,
                &auth_state.users,
            );
            if let Some(min) = required_role_for_request(&method, path) {
                if let Err(denied) = require_role(role, min) {
                    return denied;
                }
            }
            request.extensions_mut().insert(Caller {
                username: Some(claims.username),
                role,
                method: AuthMethod::Session,
            });
            return next.run(request).await;
        }
    }

    // Determine error message: was a credential provided but wrong, or missing entirely?
    let credential_provided = header_auth.is_some() || query_auth.is_some();
    let error_msg = if credential_provided {
        "Invalid API key"
    } else {
        "Missing Authorization: Bearer <api_key> header"
    };

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("www-authenticate", "Bearer")
        .body(Body::from(
            serde_json::json!({"error": error_msg}).to_string(),
        ))
        .unwrap_or_default()
}

/// Security headers middleware — applied to ALL API responses.
pub async fn security_headers(request: Request<Body>, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
    // The dashboard handler (webchat_page) sets its own nonce-based CSP.
    // For all other responses (API endpoints), apply a strict default.
    if !headers.contains_key("content-security-policy") {
        headers.insert(
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'"
                .parse()
                .unwrap(),
        );
    }
    headers.insert(
        "referrer-policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    headers.insert(
        "cache-control",
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    headers.insert(
        "strict-transport-security",
        "max-age=63072000; includeSubDomains".parse().unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Method, Request};
    use axum::routing::get;
    use axum::Router;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    #[test]
    fn test_request_id_header_constant() {
        assert_eq!(REQUEST_ID_HEADER, "x-request-id");
    }

    #[test]
    fn allow_no_auth_flag_is_opt_in() {
        for value in ["1", "true", "TRUE", "yes", "on", " on "] {
            assert!(is_true_env_flag(value));
        }
        for value in ["", "0", "false", "truE", "onward"] {
            assert!(!is_true_env_flag(value));
        }
    }

    fn auth_state_empty() -> AuthState {
        AuthState {
            api_key: String::new(),
            auth_enabled: false,
            session_secret: String::new(),
            allow_no_auth: false,
            admin_username: "admin".to_string(),
            users: Arc::new(Vec::new()),
        }
    }

    fn auth_state_with_key(key: &str) -> AuthState {
        AuthState {
            api_key: key.to_string(),
            auth_enabled: false,
            session_secret: key.to_string(),
            allow_no_auth: false,
            admin_username: "admin".to_string(),
            users: Arc::new(Vec::new()),
        }
    }

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn router(state: AuthState) -> Router {
        Router::new()
            .route("/api/agents/1", get(ok_handler))
            .route("/api/logs/stream", get(ok_handler))
            .route_layer(axum::middleware::from_fn_with_state(state, auth))
    }

    fn req_from(ip: &str) -> Request<Body> {
        let addr: SocketAddr = format!("{ip}:40000").parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/agents/1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    #[tokio::test]
    async fn empty_key_allows_loopback() {
        let app = router(auth_state_empty());
        let resp = app.oneshot(req_from("127.0.0.1")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A reverse proxy on this machine connects from 127.0.0.1 on behalf of
    /// someone on the internet; what it relays is not this machine's.
    #[tokio::test]
    async fn empty_key_blocks_a_request_relayed_by_a_local_proxy() {
        for header in [
            "x-forwarded-for",
            "forwarded",
            "x-real-ip",
            "cf-connecting-ip",
        ] {
            let app = router(auth_state_empty());
            let mut req = req_from("127.0.0.1");
            req.headers_mut()
                .insert(header, "203.0.113.5".parse().unwrap());
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{header}");
        }
    }

    #[tokio::test]
    async fn empty_key_blocks_lan_origin() {
        // Issue #1034 B1: previously 192.168/10/... could hit every non-public
        // endpoint when api_key was unset. Must now be 401.
        let app = router(auth_state_empty());
        let resp = app.oneshot(req_from("192.168.1.50")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_key_blocks_public_origin() {
        let app = router(auth_state_empty());
        let resp = app.oneshot(req_from("203.0.113.5")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_key_blocks_unknown_connect_info() {
        // Paranoia: if ConnectInfo is missing for any reason, we must fail
        // closed, not open.
        let app = router(auth_state_empty());
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/agents/1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Dashboard login on, no api_key: the key is empty, and an empty bearer
    /// token has the same length -- it compared equal and got in.
    #[tokio::test]
    async fn an_empty_token_does_not_match_an_empty_key() {
        let mut s = auth_state_empty();
        s.auth_enabled = true;
        for (header, uri) in [
            (Some("Bearer "), "/api/agents/1"),
            (None, "/api/agents/1?token="),
        ] {
            let app = router(s.clone());
            let addr: SocketAddr = "203.0.113.5:40000".parse().unwrap();
            let mut b = Request::builder().method(Method::GET).uri(uri);
            if let Some(h) = header {
                b = b.header("authorization", h);
            }
            let mut req = b.body(Body::empty()).unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri} {header:?}");
        }
    }

    #[tokio::test]
    async fn empty_key_with_allow_no_auth_opens_everything() {
        let mut s = auth_state_empty();
        s.allow_no_auth = true;
        let app = router(s);
        let resp = app.oneshot(req_from("10.0.0.9")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn configured_key_rejects_missing_token_from_loopback() {
        let app = router(auth_state_with_key("secret"));
        let resp = app.oneshot(req_from("127.0.0.1")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn configured_key_accepts_bearer() {
        let app = router(auth_state_with_key("secret"));
        let addr: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/agents/1")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn configured_key_rejects_log_stream_without_token() {
        let app = router(auth_state_with_key("secret"));
        let addr: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/logs/stream")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn configured_key_accepts_log_stream_query_token() {
        let app = router(auth_state_with_key("secret"));
        let addr: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/logs/stream?token=secret")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Roles on changes ──────────────────────────────────────────────────

    const SECRET: &str = "$argon2id$the-admin-hash";

    fn account(name: &str, role: &str) -> UserConfig {
        UserConfig {
            name: name.to_string(),
            role: role.to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    /// Dashboard login on, no API key, the owner `admin` plus `[[users]]`.
    fn auth_state_login(api_key: &str) -> AuthState {
        let config = KernelConfig {
            api_key: api_key.to_string(),
            auth: freeco_types::config::AuthConfig {
                enabled: true,
                password_hash: SECRET.to_string(),
                ..Default::default()
            },
            users: vec![
                account("alice", "admin"),
                account("uma", "user"),
                account("kim", "kid"),
                account("val", "viewer"),
            ],
            ..Default::default()
        };
        AuthState::from_config(&config, false)
    }

    async fn whoami(caller: Caller) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "user": caller.username,
            "role": caller.role,
        }))
    }

    async fn admin_only(caller: Caller) -> Response<Body> {
        match caller.require(UserRole::Admin) {
            Ok(()) => Response::new(Body::from("ok")),
            Err(denied) => denied,
        }
    }

    fn role_router(state: AuthState) -> Router {
        use axum::routing::post;
        Router::new()
            .route("/api/agents", get(whoami).post(ok_handler))
            .route("/api/agents/{id}/message", post(ok_handler))
            .route("/api/users", post(ok_handler))
            .route("/api/config/set", post(ok_handler))
            .route("/api/reports", get(admin_only))
            .route_layer(axum::middleware::from_fn_with_state(state, auth))
    }

    fn request(method: Method, uri: &str, cookie: Option<String>) -> Request<Body> {
        let addr: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            b = b.header("cookie", format!("freeco_session={c}"));
        }
        let mut req = b.body(Body::empty()).unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    fn session(state: &AuthState, user: &str, role: UserRole) -> String {
        crate::session_auth::create_session_token(user, role, &state.session_secret, 1)
    }

    async fn status(state: &AuthState, method: Method, uri: &str, cookie: Option<String>) -> u16 {
        role_router(state.clone())
            .oneshot(request(method, uri, cookie))
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[test]
    fn reads_need_no_role_and_changes_need_the_actions_role() {
        let r = |m: Method, p: &str| required_role_for_request(&m, p);
        assert_eq!(r(Method::GET, "/api/users"), None);
        assert_eq!(r(Method::GET, "/api/agents"), None);
        assert_eq!(r(Method::POST, "/api/users"), Some(UserRole::Owner));
        assert_eq!(r(Method::DELETE, "/api/users/bob"), Some(UserRole::Owner));
        assert_eq!(r(Method::POST, "/api/config/set"), Some(UserRole::Owner));
        assert_eq!(
            r(Method::POST, "/api/agents/x/message"),
            Some(UserRole::Kid)
        );
        assert_eq!(
            r(Method::POST, "/api/agents/x/message/stream"),
            Some(UserRole::Kid)
        );
        assert_eq!(r(Method::POST, "/api/sessions/route"), Some(UserRole::Kid));
        // A Chamber reply is chatting; crate::access decides where.
        assert_eq!(
            r(Method::POST, "/api/chamber/conversations/x/reply"),
            Some(UserRole::Kid)
        );
        assert_eq!(
            r(Method::POST, "/api/chamber/conversations/x/y/reply"),
            Some(UserRole::Admin)
        );
        assert_eq!(r(Method::GET, "/api/chamber/conversations"), None);
        assert_eq!(r(Method::POST, "/api/agents"), Some(UserRole::Admin));
        assert_eq!(r(Method::DELETE, "/api/agents/x"), Some(UserRole::Admin));
        assert_eq!(r(Method::PUT, "/api/budget"), Some(UserRole::Admin));
        assert_eq!(
            r(Method::PATCH, "/api/agents/x/config"),
            Some(UserRole::Admin)
        );
        // Not a chat route just because it mentions one.
        assert_eq!(
            r(Method::POST, "/api/agents/x/y/message"),
            Some(UserRole::Admin)
        );
        assert_eq!(r(Method::POST, "/api/usersx"), Some(UserRole::Admin));
    }

    /// Team work needs only a user account at the door; the handler checks
    /// the caller's role on the team. The rest of a team stays admin-only.
    #[test]
    fn team_work_is_left_to_the_team_role() {
        let r = |m: Method, p: &str| required_role_for_request(&m, p);
        let user = Some(UserRole::User);
        let admin = Some(UserRole::Admin);
        assert_eq!(r(Method::POST, "/api/org/teams/t1/members"), user);
        assert_eq!(r(Method::DELETE, "/api/org/teams/t1/members/a1"), user);
        assert_eq!(r(Method::POST, "/api/org/teams/t1/tasks"), user);
        assert_eq!(r(Method::PUT, "/api/org/teams/t1/tasks/k1"), user);
        assert_eq!(r(Method::DELETE, "/api/org/teams/t1/tasks/k1"), user);
        // Not team work.
        assert_eq!(r(Method::PUT, "/api/org/teams/t1/members"), admin);
        assert_eq!(r(Method::PUT, "/api/org/teams/t1/members/a1"), admin);
        assert_eq!(r(Method::POST, "/api/org/teams"), admin);
        assert_eq!(r(Method::PUT, "/api/org/teams/t1"), admin);
        assert_eq!(r(Method::POST, "/api/org/teams/t1/run"), admin);
        assert_eq!(r(Method::POST, "/api/org/teams//tasks"), admin);
        assert_eq!(r(Method::POST, "/api/org/teams/t1/tasks/k1/x"), admin);
        assert_eq!(r(Method::POST, "/api/org/companies"), admin);
        assert_eq!(r(Method::GET, "/api/org/teams/t1/tasks"), None);
    }

    #[tokio::test]
    async fn a_non_admin_session_gets_403_on_a_change() {
        let s = auth_state_login("");
        for user in [
            ("uma", UserRole::User),
            ("kim", UserRole::Kid),
            ("val", UserRole::Viewer),
        ] {
            let cookie = session(&s, user.0, user.1);
            assert_eq!(
                status(&s, Method::POST, "/api/agents", Some(cookie)).await,
                403,
                "{user:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_admin_or_owner_session_may_make_the_change() {
        let s = auth_state_login("");
        let admin = session(&s, "alice", UserRole::Admin);
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(admin.clone())).await,
            200
        );
        let owner = session(&s, "admin", UserRole::Owner);
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(owner.clone())).await,
            200
        );
        // Accounts and config are the owner's alone.
        assert_eq!(
            status(&s, Method::POST, "/api/users", Some(admin.clone())).await,
            403
        );
        assert_eq!(
            status(&s, Method::POST, "/api/config/set", Some(admin)).await,
            403
        );
        assert_eq!(
            status(&s, Method::POST, "/api/users", Some(owner.clone())).await,
            200
        );
        assert_eq!(
            status(&s, Method::POST, "/api/config/set", Some(owner)).await,
            200
        );
    }

    #[tokio::test]
    async fn users_and_kids_may_chat_but_viewers_may_not() {
        let s = auth_state_login("");
        let uri = "/api/agents/abc/message";
        let uma = session(&s, "uma", UserRole::User);
        let kim = session(&s, "kim", UserRole::Kid);
        let val = session(&s, "val", UserRole::Viewer);
        assert_eq!(status(&s, Method::POST, uri, Some(uma)).await, 200);
        assert_eq!(status(&s, Method::POST, uri, Some(kim)).await, 200);
        assert_eq!(status(&s, Method::POST, uri, Some(val)).await, 403);
    }

    #[tokio::test]
    async fn reads_keep_working_for_every_role() {
        let s = auth_state_login("");
        let val = session(&s, "val", UserRole::Viewer);
        assert_eq!(status(&s, Method::GET, "/api/agents", Some(val)).await, 200);
        assert_eq!(status(&s, Method::GET, "/api/agents", None).await, 401);
    }

    #[tokio::test]
    async fn a_token_cannot_claim_more_than_the_account_has() {
        let s = auth_state_login("");
        // uma is a `user` in config; a cookie claiming owner acts as user.
        let cookie = session(&s, "uma", UserRole::Owner);
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(cookie)).await,
            403
        );
        // A name not in config at all is a viewer.
        let cookie = session(&s, "mallory", UserRole::Owner);
        assert_eq!(
            status(&s, Method::POST, "/api/agents/x/message", Some(cookie)).await,
            403
        );
    }

    /// Sessions issued before cookies carried a role keep working for the
    /// owner, and give anyone else only their account's role.
    #[tokio::test]
    async fn a_legacy_session_gets_the_accounts_current_role() {
        use base64::Engine;
        let s = auth_state_login("");
        let legacy = |user: &str| {
            use hmac::{Hmac, Mac};
            let expiry = chrono::Utc::now().timestamp() + 3600;
            let payload = format!("{user}:{expiry}");
            let mut mac =
                Hmac::<sha2::Sha256>::new_from_slice(s.session_secret.as_bytes()).unwrap();
            mac.update(payload.as_bytes());
            let sig = hex::encode(mac.finalize().into_bytes());
            base64::engine::general_purpose::STANDARD.encode(format!("{payload}:{sig}"))
        };
        assert_eq!(
            status(&s, Method::POST, "/api/users", Some(legacy("admin"))).await,
            200
        );
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(legacy("uma"))).await,
            403
        );
        assert_eq!(
            status(
                &s,
                Method::POST,
                "/api/agents/x/message",
                Some(legacy("uma"))
            )
            .await,
            200
        );
    }

    #[tokio::test]
    async fn the_caller_extractor_reports_identity_and_role() {
        let s = auth_state_login("");
        let cookie = session(&s, "kim", UserRole::Kid);
        let resp = role_router(s.clone())
            .oneshot(request(Method::GET, "/api/agents", Some(cookie)))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["user"], "kim");
        assert_eq!(v["role"], "kid");

        // A read route can still demand a role through the helper.
        let kim = session(&s, "kim", UserRole::Kid);
        assert_eq!(
            status(&s, Method::GET, "/api/reports", Some(kim)).await,
            403
        );
        let alice = session(&s, "alice", UserRole::Admin);
        assert_eq!(
            status(&s, Method::GET, "/api/reports", Some(alice)).await,
            200
        );
    }

    /// The API key and the login-off local user act as the owner.
    #[tokio::test]
    async fn the_api_key_and_the_local_user_are_the_owner() {
        let s = auth_state_login("the-key");
        let addr: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/api/users")
            .header("authorization", "Bearer the-key")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        let resp = role_router(s.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // With an API key set, sessions are signed with it and still checked.
        let uma = session(&s, "uma", UserRole::User);
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(uma)).await,
            403
        );
        let alice = session(&s, "alice", UserRole::Admin);
        assert_eq!(
            status(&s, Method::POST, "/api/agents", Some(alice)).await,
            200
        );

        let local = auth_state_empty();
        let addr: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/agents")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        let resp = role_router(local).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["role"], "owner");
    }
}

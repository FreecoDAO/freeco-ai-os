//! Embedded WebChat UI served as static HTML.
//!
//! The production dashboard is assembled at compile time from separate
//! HTML/CSS/JS files under `static/` using `include_str!()`. This keeps
//! single-binary deployment while allowing organized source files.
//!
//! Features:
//! - Alpine.js SPA with hash-based routing (10 panels)
//! - Dark/light theme toggle with system preference detection
//! - Responsive layout with collapsible sidebar
//! - Markdown rendering + syntax highlighting (bundled locally)
//! - WebSocket real-time chat with HTTP fallback
//! - Agent management, workflows, memory browser, audit log, and more

use axum::http::header;
use axum::response::IntoResponse;

/// Nonce placeholder in compile-time HTML, replaced at request time.
const NONCE_PLACEHOLDER: &str = "__NONCE__";

/// Compile-time ETag based on the crate version.
/// Not used for the dashboard page (nonce prevents caching) but retained
/// for potential future use by static asset handlers.
#[allow(dead_code)]
const ETAG: &str = concat!("\"freeco-", env!("CARGO_PKG_VERSION"), "\"");

/// Embedded logo PNG for single-binary deployment.
const LOGO_PNG: &[u8] = include_bytes!("../static/logo.png");

/// Embedded favicon ICO for browser tabs.
const FAVICON_ICO: &[u8] = include_bytes!("../static/favicon.ico");

/// GET /logo.png — Serve the Freeco logo.
pub async fn logo_png() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        LOGO_PNG,
    )
}

/// GET /favicon.ico — Serve the Freeco favicon.
pub async fn favicon_ico() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/x-icon"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        FAVICON_ICO,
    )
}

/// Self-hosted web fonts, so the dashboard makes no request to Google.
///
/// The page used to load Inter and Geist Mono from fonts.googleapis.com and
/// fonts.gstatic.com, which sent the viewer's IP address and browser to Google
/// on every single page load -- the only third-party request the dashboard
/// made. Both families are SIL Open Font License 1.1, so they ship here.
///
/// A fixed table rather than a directory read: the name in the URL only ever
/// selects one of these compiled-in files, so no request can reach the disk.
const FONT_FILES: &[(&str, &[u8])] = &[
    (
        "geist-mono-cyrillic-ext.woff2",
        include_bytes!("../static/fonts/geist-mono-cyrillic-ext.woff2"),
    ),
    (
        "geist-mono-cyrillic.woff2",
        include_bytes!("../static/fonts/geist-mono-cyrillic.woff2"),
    ),
    (
        "geist-mono-latin-ext.woff2",
        include_bytes!("../static/fonts/geist-mono-latin-ext.woff2"),
    ),
    (
        "geist-mono-latin.woff2",
        include_bytes!("../static/fonts/geist-mono-latin.woff2"),
    ),
    (
        "geist-mono-vietnamese.woff2",
        include_bytes!("../static/fonts/geist-mono-vietnamese.woff2"),
    ),
    (
        "inter-cyrillic-ext.woff2",
        include_bytes!("../static/fonts/inter-cyrillic-ext.woff2"),
    ),
    (
        "inter-cyrillic.woff2",
        include_bytes!("../static/fonts/inter-cyrillic.woff2"),
    ),
    (
        "inter-greek-ext.woff2",
        include_bytes!("../static/fonts/inter-greek-ext.woff2"),
    ),
    (
        "inter-greek.woff2",
        include_bytes!("../static/fonts/inter-greek.woff2"),
    ),
    (
        "inter-latin-ext.woff2",
        include_bytes!("../static/fonts/inter-latin-ext.woff2"),
    ),
    (
        "inter-latin.woff2",
        include_bytes!("../static/fonts/inter-latin.woff2"),
    ),
    (
        "inter-vietnamese.woff2",
        include_bytes!("../static/fonts/inter-vietnamese.woff2"),
    ),
];

/// The @font-face rules pointing at the files above.
const FONTS_CSS: &str = include_str!("../static/fonts/fonts.css");

/// GET /fonts/{name} — serve one of the compiled-in font files.
pub async fn font_file(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    if name == "fonts.css" {
        return (
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            FONTS_CSS,
        )
            .into_response();
    }
    match FONT_FILES.iter().find(|(n, _)| *n == name) {
        Some((_, bytes)) => (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            *bytes,
        )
            .into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

/// Embedded PWA manifest for installable web app support.
const MANIFEST_JSON: &str = include_str!("../static/manifest.json");

/// Embedded service worker for PWA support.
const SW_JS: &str = include_str!("../static/sw.js");

/// GET /manifest.json — Serve the PWA web app manifest.
pub async fn manifest_json() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        MANIFEST_JSON,
    )
}

/// GET /sw.js — Serve the PWA service worker.
pub async fn sw_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SW_JS,
    )
}

/// GET / — Serve the Freeco Dashboard single-page application.
///
/// Generates a unique CSP nonce on every request and injects it into both
/// the `<script>` tags and the `Content-Security-Policy` header. This
/// replaces `'unsafe-inline'` so only our own scripts execute.
pub async fn webchat_page() -> impl IntoResponse {
    let nonce = uuid::Uuid::new_v4().to_string();
    let html = WEBCHAT_HTML.replace(NONCE_PLACEHOLDER, &nonce);
    let csp = format!(
        "default-src 'self'; \
         script-src 'self' 'nonce-{nonce}' 'unsafe-eval' https://cdn.jsdelivr.net; \
         style-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; \
         img-src 'self' data: blob:; \
         connect-src 'self' ws://localhost:* ws://127.0.0.1:* wss://localhost:* wss://127.0.0.1:* https://cdn.jsdelivr.net https://api.github.com; \
         font-src 'self' https://cdn.jsdelivr.net; \
         media-src 'self' blob:; \
         frame-src 'self' blob:; \
         object-src 'none'; \
         base-uri 'self'; \
         form-action 'self'"
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (
                header::HeaderName::from_static("content-security-policy"),
                csp,
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        html,
    )
}

/// The embedded HTML/CSS/JS for the Freeco Dashboard.
///
/// Assembled at compile time from organized static files.
/// All vendor libraries (Alpine.js, marked.js, highlight.js) are bundled
/// locally — no CDN dependency. Alpine.js is included LAST because it
/// immediately processes x-data directives and fires alpine:init on load.
/// KaTeX is loaded dynamically from jsdelivr CDN when needed for LaTeX rendering.
const WEBCHAT_HTML: &str = concat!(
    include_str!("../static/index_head.html"),
    "<style>\n",
    include_str!("../static/css/theme.css"),
    "\n",
    include_str!("../static/css/layout.css"),
    "\n",
    include_str!("../static/css/components.css"),
    "\n",
    include_str!("../static/vendor/github-dark.min.css"),
    "\n</style>\n",
    include_str!("../static/index_body.html"),
    // Vendor libs: marked + highlight first (used by app.js), then Chart.js
    "<script nonce=\"__NONCE__\">\n",
    include_str!("../static/vendor/marked.min.js"),
    "\n</script>\n",
    "<script nonce=\"__NONCE__\">\n",
    include_str!("../static/vendor/highlight.min.js"),
    "\n</script>\n",
    "<script nonce=\"__NONCE__\">\n",
    include_str!("../static/vendor/chart.umd.min.js"),
    "\n</script>\n",
    // App code
    "<script nonce=\"__NONCE__\">\n",
    include_str!("../static/js/api.js"),
    "\n",
    include_str!("../static/js/app.js"),
    "\n",
    include_str!("../static/js/pages/overview.js"),
    "\n",
    include_str!("../static/js/katex.js"),
    "\n",
    // Shared chat behaviour, before the pages that mix it in.
    include_str!("../static/js/shared/inline-approvals.js"),
    "\n",
    include_str!("../static/js/pages/chat.js"),
    "\n",
    include_str!("../static/js/pages/agents.js"),
    "\n",
    include_str!("../static/js/pages/projects.js"),
    "\n",
    include_str!("../static/js/pages/teams.js"),
    "\n",
    include_str!("../static/js/pages/chamber.js"),
    "\n",
    include_str!("../static/js/pages/sidenav.js"),
    "\n",
    include_str!("../static/js/pages/live.js"),
    "\n",
    include_str!("../static/js/pages/workflows.js"),
    "\n",
    include_str!("../static/js/pages/workflow-builder.js"),
    "\n",
    include_str!("../static/js/pages/channels.js"),
    "\n",
    include_str!("../static/js/pages/skills.js"),
    "\n",
    include_str!("../static/js/pages/hands.js"),
    "\n",
    include_str!("../static/js/pages/scheduler.js"),
    "\n",
    include_str!("../static/js/pages/settings.js"),
    "\n",
    include_str!("../static/js/pages/usage.js"),
    "\n",
    include_str!("../static/js/pages/sessions.js"),
    "\n",
    include_str!("../static/js/pages/logs.js"),
    "\n",
    include_str!("../static/js/pages/wizard.js"),
    "\n",
    include_str!("../static/js/pages/approvals.js"),
    "\n",
    include_str!("../static/js/pages/comms.js"),
    "\n",
    include_str!("../static/js/pages/runtime.js"),
    "\n",
    include_str!("../static/js/pages/assistant.js"),
    "\n</script>\n",
    // Alpine.js MUST be last — it processes x-data and fires alpine:init
    "<script nonce=\"__NONCE__\">\n",
    include_str!("../static/vendor/alpine.min.js"),
    "\n</script>\n",
    "</body></html>"
);

//! Cross-site request forgery protection for state-changing requests.
//!
//! With dashboard auth off, every loopback request is trusted, and a page open
//! in the browser reaches 127.0.0.1 as loopback like any other client. A form
//! on any website can post to this daemon; CORS does not stop the request
//! arriving, only the reply being read -- which is no comfort when the request
//! itself shuts the daemon down, disables auth, or installs a service.
//!
//! One endpoint (`/api/services/vm-fix/{id}`) was guarded by hand. That left
//! every other state-changing handler open, so the guard moves here and applies
//! to all of them.
//!
//! The rule, in the order a request meets it:
//!
//! 1. Only state-changing methods are checked. A GET changes nothing.
//! 2. When the daemon listens on loopback, the request must be addressed to a
//!    loopback authority. This is what defeats DNS rebinding, where a hostile
//!    name resolves to 127.0.0.1. A daemon bound to a LAN address or behind a
//!    reverse proxy is legitimately reached by other names, so there the rule
//!    is skipped and rules 3 and 4 do the work.
//! 3. An `Origin` header, when present, must match that authority. A browser
//!    always sends one on cross-site requests, so this catches them.
//! 4. Without an `Origin`, the content type must not be one a plain HTML form
//!    can produce. A form can send `application/x-www-form-urlencoded`,
//!    `multipart/form-data` or `text/plain` with no preflight and no `Origin`
//!    on older browsers; it cannot send `application/json`.
//!
//! Rule 4 is what lets `curl` keep working -- it sends JSON and no `Origin` --
//! while a form post from another site is refused. Uploads are multipart and
//! come from the dashboard, so they arrive with a matching `Origin` and pass at
//! rule 3.

use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Authorities that mean "this machine".
fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // Bracketed IPv6, optionally with a port: `[::1]` or `[::1]:4200`.
        rest.split(']').next().unwrap_or("")
    } else if authority.matches(':').count() > 1 {
        // Bare IPv6 such as `::1`. Its colons are part of the address, so
        // splitting at the last one would read `1` as a port.
        authority
    } else {
        // Name or IPv4, optionally with a port.
        authority.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Content types a plain HTML form can send without a CORS preflight.
fn is_form_submittable(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    mime.eq_ignore_ascii_case("application/x-www-form-urlencoded")
        || mime.eq_ignore_ascii_case("multipart/form-data")
        || mime.eq_ignore_ascii_case("text/plain")
}

/// Decide whether a request may change state. Pure, so it can be tested
/// without standing up a server.
pub fn check(
    method: &Method,
    headers: &HeaderMap,
    listens_on_loopback: bool,
) -> Result<(), (StatusCode, &'static str)> {
    let changes_state = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if !changes_state {
        return Ok(());
    }

    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if listens_on_loopback && !is_loopback_authority(host) {
        return Err((
            StatusCode::FORBIDDEN,
            "this request is not addressed to the loopback interface",
        ));
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) => {
            // Scheme-agnostic: the dashboard is served over http on loopback,
            // but a future https listener must not start failing its own posts.
            let matches = origin == format!("http://{host}") || origin == format!("https://{host}");
            if !matches {
                return Err((StatusCode::FORBIDDEN, "cross-origin request refused"));
            }
            Ok(())
        }
        None if is_form_submittable(content_type) => Err((
            StatusCode::FORBIDDEN,
            "form-style content types are refused without a matching Origin",
        )),
        None => Ok(()),
    }
}

/// Whether the daemon listens only on loopback; set once when the router is
/// built. Defaults to true, the stricter setting.
static LISTENS_ON_LOOPBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Record the listen address, so the loopback-Host rule applies only where it
/// means something. Required behind a reverse proxy: there every request
/// arrives with the public Host, and the rule refused all of them.
pub fn set_listens_on_loopback(loopback: bool) {
    LISTENS_ON_LOOPBACK.store(loopback, std::sync::atomic::Ordering::Relaxed);
}

/// Axum middleware wrapper around [`check`].
pub async fn guard(request: Request, next: Next) -> Response {
    let loopback = LISTENS_ON_LOOPBACK.load(std::sync::atomic::Ordering::Relaxed);
    if let Err((status, message)) = check(request.method(), request.headers(), loopback) {
        return (status, message).into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn reads_are_never_blocked() {
        assert!(check(&Method::GET, &headers(&[("host", "evil.example")]), true).is_ok());
    }

    #[test]
    fn curl_with_json_and_no_origin_is_allowed() {
        let h = headers(&[
            ("host", "127.0.0.1:50051"),
            ("content-type", "application/json"),
        ]);
        assert!(check(&Method::POST, &h, true).is_ok());
    }

    #[test]
    fn the_dashboards_own_post_is_allowed() {
        let h = headers(&[
            ("host", "127.0.0.1:50051"),
            ("origin", "http://127.0.0.1:50051"),
            ("content-type", "application/json"),
        ]);
        assert!(check(&Method::POST, &h, true).is_ok());
    }

    /// The case this exists for: a page on another site posting to the daemon.
    #[test]
    fn a_cross_site_form_post_is_refused() {
        let h = headers(&[
            ("host", "127.0.0.1:50051"),
            ("origin", "https://evil.example"),
            ("content-type", "application/x-www-form-urlencoded"),
        ]);
        assert_eq!(
            check(&Method::POST, &h, true).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
    }

    /// A form post carries no Origin on older browsers, so the content type is
    /// what gives it away.
    #[test]
    fn a_form_post_without_an_origin_is_refused() {
        for ct in [
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "text/plain",
        ] {
            let h = headers(&[("host", "127.0.0.1:50051"), ("content-type", ct)]);
            assert_eq!(
                check(&Method::POST, &h, true).unwrap_err().0,
                StatusCode::FORBIDDEN,
                "{ct} should be refused without an Origin"
            );
        }
    }

    /// Uploads are multipart and come from the dashboard, so they must still work.
    #[test]
    fn a_dashboard_upload_still_works() {
        let h = headers(&[
            ("host", "localhost:50051"),
            ("origin", "http://localhost:50051"),
            ("content-type", "multipart/form-data; boundary=abc"),
        ]);
        assert!(check(&Method::POST, &h, true).is_ok());
    }

    /// DNS rebinding: a hostile name resolving to 127.0.0.1 still sends its own
    /// Host header, which is not a loopback authority.
    #[test]
    fn a_rebound_hostname_is_refused() {
        let h = headers(&[
            ("host", "rebind.example"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(
            check(&Method::POST, &h, true).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn loopback_authorities_are_recognised() {
        for good in [
            "127.0.0.1",
            "127.0.0.1:50051",
            "localhost",
            "localhost:80",
            "[::1]:50051",
            "::1",
        ] {
            assert!(is_loopback_authority(good), "{good}");
        }
        for bad in ["evil.example", "10.0.0.5:50051", "0.0.0.0:50051", ""] {
            assert!(!is_loopback_authority(bad), "{bad}");
        }
    }

    /// Behind a reverse proxy the Host is the public name. The loopback rule
    /// does not apply there; a cross-site post is still refused by Origin.
    #[test]
    fn a_proxied_daemon_accepts_its_public_host_but_not_cross_site() {
        let same = headers(&[
            ("host", "demo.example"),
            ("origin", "https://demo.example"),
            ("content-type", "application/json"),
        ]);
        assert!(check(&Method::POST, &same, false).is_ok());
        let api = headers(&[
            ("host", "demo.example"),
            ("content-type", "application/json"),
        ]);
        assert!(check(&Method::POST, &api, false).is_ok());
        let cross = headers(&[
            ("host", "demo.example"),
            ("origin", "https://evil.example"),
            ("content-type", "application/json"),
        ]);
        assert!(check(&Method::POST, &cross, false).is_err());
        let form = headers(&[("host", "demo.example"), ("content-type", "text/plain")]);
        assert!(check(&Method::POST, &form, false).is_err());
    }

    #[test]
    fn deletes_and_puts_are_checked_too() {
        let h = headers(&[("host", "evil.example")]);
        for m in [Method::DELETE, Method::PUT, Method::PATCH] {
            assert!(check(&m, &h, true).is_err(), "{m} should be checked");
        }
    }
}

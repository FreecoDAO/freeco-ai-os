//! Build identity and uptime, for the `system_info` tool and `/api/version`.
//!
//! One definition, one place: `build.rs` sets `GIT_SHA`, `GIT_BRANCH`,
//! `BUILD_DATE` (UTC) and `RUSTC_VERSION` at compile time from
//! `FREECO_GIT_*` env vars or `git`, so a binary always knows where it came
//! from, and an agent asked to debug or deploy can quote the exact build.

use std::time::Instant;

/// Cargo package version (`CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Full commit SHA, or `unknown` when neither git nor `FREECO_GIT_SHA` had one.
pub fn git_sha() -> &'static str {
    env!("GIT_SHA")
}

/// Branch the binary was built from, or `unknown`.
pub fn git_branch() -> &'static str {
    env!("GIT_BRANCH")
}

/// UTC build time, `YYYY-MM-DDTHH:MM:SSZ` (or the `FREECO_BUILD_DATE` value).
pub fn build_date() -> &'static str {
    env!("BUILD_DATE")
}

/// `rustc --version` output at build time.
pub fn rustc_version() -> &'static str {
    env!("RUSTC_VERSION")
}

/// Seconds since the first call in this process; 0 until then.
pub fn uptime_seconds() -> u64 {
    start().elapsed().as_secs()
}

/// Process start time, anchored on first use (the runtime is loaded as the
/// kernel boots, so this is within milliseconds of the daemon's start).
fn start() -> &'static Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now)
}

/// Anchor the uptime clock. Called at boot so a daemon that sits idle before
/// its first request does not report that idle time as uptime of a "new"
/// process; harmless to call more than once.
pub fn mark_start() {
    let _ = start();
}

/// Everything about this build, as JSON. The `system_info` tool returns this;
/// `/api/version` returns the same fields plus the package name.
pub fn info_json() -> serde_json::Value {
    serde_json::json!({
        "version": version(),
        "git_sha": git_sha(),
        "git_branch": git_branch(),
        "build_date": build_date(),
        "rust_version": rustc_version(),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "uptime_seconds": uptime_seconds(),
    })
}

/// Human-readable rendering for the tool result, plus the JSON.
pub fn info_text() -> String {
    format!(
        "FreEco.ai {}\ncommit: {} (branch {})\nbuilt: {} UTC\nrustc: {}\nos/arch: {}/{}\nuptime: {}s",
        version(),
        git_sha(),
        git_branch(),
        build_date(),
        rustc_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        uptime_seconds(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_values_exist_and_are_not_empty() {
        // build.rs always emits these, even with no git and no env: the whole
        // point of this module is that /api/version stops saying "dev".
        for v in [
            version(),
            git_sha(),
            git_branch(),
            build_date(),
            rustc_version(),
        ] {
            assert!(!v.is_empty());
        }
    }

    #[test]
    fn info_json_has_the_version_fields_and_uptime() {
        let v = info_json();
        assert_eq!(v["version"], version());
        assert_eq!(v["git_sha"], git_sha());
        assert_eq!(v["git_branch"], git_branch());
        assert_eq!(v["build_date"], build_date());
        assert_eq!(v["rust_version"], rustc_version());
        assert_eq!(v["platform"], std::env::consts::OS);
        assert_eq!(v["arch"], std::env::consts::ARCH);
        assert!(v["uptime_seconds"].is_number());
        // No build identity value is a secret; assert none leaked in.
        assert!(!v.to_string().contains("REDACTED"));
    }

    #[test]
    fn uptime_is_monotonic_and_counts() {
        mark_start();
        let a = uptime_seconds();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let b = uptime_seconds();
        assert!(b >= a + 1, "uptime did not advance: {a} -> {b}");
        // mark_start is idempotent.
        mark_start();
        assert!(uptime_seconds() >= b);
    }

    #[test]
    fn info_text_carries_the_same_values() {
        let t = info_text();
        assert!(t.contains(version()), "{t}");
        assert!(t.contains(git_sha()), "{t}");
        assert!(t.contains(git_branch()), "{t}");
        assert!(t.contains(build_date()), "{t}");
        assert!(t.contains(std::env::consts::OS), "{t}");
    }
}

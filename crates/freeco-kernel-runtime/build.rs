//! Build identity: the commit, branch and UTC build time a binary was made
//! from, for `/api/version` and the `system_info` tool.
//!
//! No dependencies beyond the standard library, and no requirement that git
//! exists: values come from `FREECO_GIT_SHA` / `FREECO_GIT_BRANCH` /
//! `FREECO_BUILD_DATE` when set (the Docker build passes them, since the image
//! has no `.git`), else from `git`, else `unknown`. The build date falls back
//! to `SOURCE_DATE_EPOCH` when set, for reproducible builds.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Re-run when the build identity inputs change, not on every file edit.
    for var in [
        "FREECO_GIT_SHA",
        "FREECO_GIT_BRANCH",
        "FREECO_BUILD_DATE",
        "SOURCE_DATE_EPOCH",
        "RUSTC",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");

    let sha = env_or_git("FREECO_GIT_SHA", &["rev-parse", "HEAD"]);
    let branch = env_or_git("FREECO_GIT_BRANCH", &["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = branch.filter(|b| b != "HEAD"); // detached HEAD is no branch

    emit("GIT_SHA", &sha.unwrap_or_else(|| "unknown".into()));
    emit("GIT_BRANCH", &branch.unwrap_or_else(|| "unknown".into()));
    emit("BUILD_DATE", &build_date());
    emit("RUSTC_VERSION", &rustc_version());
}

fn emit(key: &str, value: &str) {
    // A branch name can contain anything; keep the env value single-line.
    let clean = value.trim().replace(['\n', '\r'], "");
    println!("cargo:rustc-env={key}={clean}");
}

/// The explicit env var if set and non-empty, else `git` (if it runs and has
/// something to say).
fn env_or_git(var: &str, args: &[&str]) -> Option<String> {
    if let Ok(v) = std::env::var(var) {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ`: the explicit value, else `SOURCE_DATE_EPOCH`,
/// else now.
fn build_date() -> String {
    if let Ok(v) = std::env::var("FREECO_BUILD_DATE") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs() as i64)
        })
        .unwrap_or(0);
    format_unix_epoch_utc(epoch)
}

/// Convert a Unix epoch second to `YYYY-MM-DDTHH:MM:SSZ` without chrono:
/// build scripts should stay dependency-free.
fn format_unix_epoch_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    // Truncating division, as the algorithm requires (the z-146096 offset is
    // what makes truncation equal floor for negative z).
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn rustc_version() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_the_epoch() {
        assert_eq!(format_unix_epoch_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_dates_and_times() {
        // 2026-11-19T12:34:56Z
        assert_eq!(format_unix_epoch_utc(1_795_091_696), "2026-11-19T12:34:56Z");
        // 2000-02-29 (leap year divisible by 400)
        assert_eq!(format_unix_epoch_utc(951_782_400), "2000-02-29T00:00:00Z");
        // 1900-03-01 (not a leap year)
        assert_eq!(
            format_unix_epoch_utc(-2_203_891_200),
            "1900-03-01T00:00:00Z"
        );
        // 1999-12-31T23:59:59Z
        assert_eq!(format_unix_epoch_utc(946_684_799), "1999-12-31T23:59:59Z");
        // One second before the epoch (div_euclid, not truncation).
        assert_eq!(format_unix_epoch_utc(-1), "1969-12-31T23:59:59Z");
    }
}

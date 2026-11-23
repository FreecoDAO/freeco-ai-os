//! Back up the data held by service containers.
//!
//! The `.ofbak` archive covers `~/.freeco-ai` -- config, agents, memory -- and
//! nothing inside the Podman VM. The CRM, voice and accounting services keep
//! their data in container volumes, so that data had no backup at all, and when
//! the CRM's volumes were lost its data was simply gone.
//!
//! Two rules decide how each thing is saved:
//!
//! - A database is dumped by its own tool (`pg_dumpall`, `mariadb-dump`), never
//!   copied as files. Copying a running database's files can capture it
//!   mid-write, and a backup that restores to a corrupt database is worse than
//!   none because it is trusted.
//! - Everything else a running service stores in a named volume is exported
//!   with `podman volume export`. Caches (Redis) and anonymous volumes are
//!   skipped: they hold nothing that cannot be rebuilt.
//!
//! Credentials never leave the container. Each dump runs `sh -c` inside it and
//! reads the password from the container's own environment, so no secret ever
//! appears in a host process list or in this code.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// What one run saved, and what it could not.
#[derive(Debug, Default, serde::Serialize)]
pub struct ServiceBackupReport {
    pub directory: String,
    pub saved: Vec<SavedItem>,
    pub failed: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SavedItem {
    pub name: String,
    pub kind: &'static str,
    pub bytes: u64,
}

const STEP_TIMEOUT: Duration = Duration::from_secs(900);

/// How many runs are kept. One run of the accounting service alone reached
/// 5 GB, and seven of them filled the system disk.
pub const RETENTION_RUNS: usize = 3;

/// Older runs are also removed while all runs together exceed this. The
/// newest run is always kept, whatever its size.
pub const MAX_TOTAL_BYTES: u64 = 12 * 1024 * 1024 * 1024;

async fn run(args: &[&str]) -> Result<std::process::Output, String> {
    let out = tokio::time::timeout(
        STEP_TIMEOUT,
        tokio::process::Command::new("podman")
            .kill_on_drop(true)
            .args(args)
            .output(),
    )
    .await
    .map_err(|_| format!("podman {} timed out", args.join(" ")))?
    .map_err(|e| format!("podman {}: {e}", args.join(" ")))?;
    Ok(out)
}

/// A volume named by a 64-character hash was created implicitly and holds
/// nothing a person chose to keep.
fn is_anonymous(volume: &str) -> bool {
    volume.len() == 64 && volume.chars().all(|c| c.is_ascii_hexdigit())
}

/// Which dump, if any, applies to a container, judged from its image.
fn database_kind(image: &str) -> Option<&'static str> {
    let i = image.to_lowercase();
    if i.contains("postgres") || i.contains("pgvector") {
        Some("postgres")
    } else if i.contains("mariadb") || i.contains("mysql") {
        Some("mariadb")
    } else {
        None
    }
}

fn is_cache(image: &str) -> bool {
    let i = image.to_lowercase();
    i.contains("redis") || i.contains("memcached") || i.contains("valkey")
}

/// Save every running service's data into `<backups_dir>/services/<timestamp>/`,
/// keeping the newest `retention` runs.
pub async fn export_service_data(
    backups_dir: &Path,
    retention: usize,
) -> Result<ServiceBackupReport, String> {
    let root = backups_dir.join("services");
    // Ask the runtime first. The run directory used to be created before
    // this, so every day the runtime was down left an empty directory behind
    // -- and pruning counted those as backups and deleted real ones.
    let ps = run(&["ps", "--format", "{{.Names}}"]).await?;
    if !ps.status.success() {
        return Err("the container runtime is not answering".to_string());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dest = root.join(&stamp);
    std::fs::create_dir_all(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut report = ServiceBackupReport {
        directory: dest.display().to_string(),
        ..Default::default()
    };
    let containers: Vec<String> = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // Volumes that belong to a database are covered by its dump.
    let mut database_volumes: Vec<String> = Vec::new();
    let mut file_volumes: Vec<String> = Vec::new();

    for c in &containers {
        // One container that cannot be inspected -- it exited between `ps`
        // and here -- is reported, not allowed to abort everyone else's backup.
        let inspect = match run(&[
            "inspect",
            c,
            "--format",
            "{{.ImageName}}|{{range .Mounts}}{{if eq .Type \"volume\"}}{{.Name}},{{end}}{{end}}",
        ])
        .await
        {
            Ok(out) if out.status.success() => out,
            Ok(out) => {
                report.failed.push(format!(
                    "{c}: inspect failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
                continue;
            }
            Err(e) => {
                report.failed.push(format!("{c}: inspect failed: {e}"));
                continue;
            }
        };
        let text = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
        let (image, vols) = text.split_once('|').unwrap_or((text.as_str(), ""));
        let volumes: Vec<String> = vols
            .split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && !is_anonymous(v))
            .collect();

        match database_kind(image) {
            Some(kind) => {
                database_volumes.extend(volumes);
                match dump_database(c, kind, &dest).await {
                    Ok(item) => report.saved.push(item),
                    Err(e) => report.failed.push(format!("{c}: {e}")),
                }
            }
            None if is_cache(image) => {}
            None => file_volumes.extend(volumes),
        }
    }

    file_volumes.sort();
    file_volumes.dedup();
    for v in file_volumes {
        if database_volumes.contains(&v) {
            continue;
        }
        let out = dest.join(format!("{v}.tar"));
        let out_str = out.display().to_string();
        match run(&["volume", "export", &v, "--output", &out_str]).await {
            Ok(o) if o.status.success() => report.saved.push(SavedItem {
                name: v,
                kind: "volume",
                bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
            }),
            Ok(o) => {
                // A failed export -- the disk filling up is the usual cause --
                // leaves a truncated tar behind. Kept, it looked like a backup
                // and counted as one when older, complete runs were pruned.
                let _ = std::fs::remove_file(&out);
                report.failed.push(format!(
                    "{v}: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                report.failed.push(format!("{v}: {e}"))
            }
        }
    }

    prune(&root, retention, MAX_TOTAL_BYTES);
    Ok(report)
}

async fn dump_database(
    container: &str,
    kind: &'static str,
    dest: &Path,
) -> Result<SavedItem, String> {
    // Runs inside the container, reading credentials from its own environment.
    let script = match kind {
        "postgres" => r#"pg_dumpall -U "${POSTGRES_USER:-postgres}""#,
        _ => {
            r#"(command -v mariadb-dump >/dev/null && mariadb-dump --all-databases --single-transaction -uroot -p"$MYSQL_ROOT_PASSWORD") || mysqldump --all-databases --single-transaction -uroot -p"$MYSQL_ROOT_PASSWORD""#
        }
    };
    let out = run(&["exec", container, "sh", "-c", script]).await?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(format!(
            "{kind} dump failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let path: PathBuf = dest.join(format!("{container}.sql"));
    std::fs::write(&path, &out.stdout).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(SavedItem {
        name: container.to_string(),
        kind,
        bytes: out.stdout.len() as u64,
    })
}

/// Keep the newest `retention` runs, and remove older ones while all runs
/// together exceed `max_bytes`; the newest run is always kept. Names are
/// timestamps, so lexical order is chronological.
fn prune(root: &Path, retention: usize, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    // Only runs that saved something count. An empty run directory is junk
    // from a failed run: removed, and never allowed to push out a real one.
    let mut runs: Vec<(PathBuf, u64)> = Vec::new();
    for p in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let empty = std::fs::read_dir(&p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(&p);
        } else {
            let size = run_size(&p);
            runs.push((p, size));
        }
    }
    runs.sort();
    let mut total: u64 = runs.iter().map(|(_, size)| size).sum();
    let mut kept = runs.len();
    for (old, size) in runs {
        if kept <= 1 || (kept <= retention.max(1) && total <= max_bytes) {
            break;
        }
        let _ = std::fs::remove_dir_all(&old);
        total = total.saturating_sub(size);
        kept -= 1;
    }
}

/// Bytes in a run directory's files. Runs hold files only.
fn run_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases_are_recognised_by_image() {
        assert_eq!(
            database_kind("docker.io/library/postgres:16"),
            Some("postgres")
        );
        assert_eq!(
            database_kind("docker.io/pgvector/pgvector:pg17"),
            Some("postgres")
        );
        assert_eq!(
            database_kind("docker.io/library/mariadb:11"),
            Some("mariadb")
        );
        assert_eq!(database_kind("docker.io/twentycrm/twenty:latest"), None);
    }

    #[test]
    fn caches_and_anonymous_volumes_are_skipped() {
        assert!(is_cache("docker.io/library/redis:7"));
        assert!(!is_cache("docker.io/minio/minio:latest"));
        assert!(is_anonymous(
            "a8c1daacb26b190182c213f3cceb2cb6f5661fe857ce8d3147a93da8e2243785"
        ));
        assert!(!is_anonymous("freeco-voice-minio-data"));
    }

    #[test]
    fn prune_keeps_only_the_newest_runs() {
        let tmp = std::env::temp_dir().join(format!("svcbk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for stamp in ["20260101T000000Z", "20260102T000000Z", "20260103T000000Z"] {
            std::fs::create_dir_all(tmp.join(stamp)).unwrap();
            std::fs::write(tmp.join(stamp).join("db.sql"), "dump").unwrap();
        }
        prune(&tmp, 2, u64::MAX);
        let mut left: Vec<String> = std::fs::read_dir(&tmp)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["20260102T000000Z", "20260103T000000Z"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Empty run directories, left by failed runs, must never push out a real
    /// backup: they are removed and not counted.
    #[test]
    fn empty_runs_do_not_displace_real_backups() {
        let tmp = std::env::temp_dir().join(format!("svcbk-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("20260101T000000Z")).unwrap();
        std::fs::write(tmp.join("20260101T000000Z").join("db.sql"), "dump").unwrap();
        for stamp in ["20260102T000000Z", "20260103T000000Z", "20260104T000000Z"] {
            std::fs::create_dir_all(tmp.join(stamp)).unwrap();
        }
        prune(&tmp, 2, u64::MAX);
        let left: Vec<String> = std::fs::read_dir(&tmp)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left, vec!["20260101T000000Z"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Runs within the count but over the size cap are removed oldest first,
    /// and the newest run survives even when it alone is over the cap.
    #[test]
    fn prune_enforces_the_size_cap_but_keeps_the_newest_run() {
        let tmp = std::env::temp_dir().join(format!("svcbk-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for stamp in ["20260101T000000Z", "20260102T000000Z", "20260103T000000Z"] {
            std::fs::create_dir_all(tmp.join(stamp)).unwrap();
            std::fs::write(tmp.join(stamp).join("data.tar"), vec![0u8; 100]).unwrap();
        }
        let left = |tmp: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(tmp)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        prune(&tmp, 3, 250);
        assert_eq!(left(&tmp), vec!["20260102T000000Z", "20260103T000000Z"]);
        prune(&tmp, 3, 10);
        assert_eq!(left(&tmp), vec!["20260103T000000Z"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

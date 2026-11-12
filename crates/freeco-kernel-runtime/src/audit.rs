//! Merkle hash chain audit trail for security-critical actions.
//!
//! Every auditable event is appended to an append-only log where each entry
//! contains the SHA-256 hash of its own contents concatenated with the hash of
//! the previous entry, forming a tamper-evident chain (similar to a blockchain).
//!
//! When a database connection is provided (`with_db`), entries are persisted to
//! the `audit_entries` table (schema V8) so the trail survives daemon restarts.
//! The database is then the record: it decides where each new entry goes, and
//! verification reads it. Memory only holds the most recent entries, for the
//! dashboard and the live feeds.

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The `prev_hash` of the very first entry.
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// How many recent entries are kept in memory.
///
/// Every entry used to stay in memory for the life of the daemon, and every
/// stored entry was loaded into memory at boot. The database already holds
/// them all; the dashboard and the live feeds read at most a few hundred.
const MEMORY_WINDOW: usize = 20_000;

/// Most breaks listed in one report. A count of all of them is kept.
const MAX_LISTED_BREAKS: usize = 100;

/// Categories of auditable actions within the agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    ToolInvoke,
    CapabilityCheck,
    AgentSpawn,
    AgentKill,
    AgentMessage,
    MemoryAccess,
    FileAccess,
    NetworkAccess,
    ShellExec,
    AuthAttempt,
    WireConnect,
    ConfigChange,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Read an action back from its stored name.
fn parse_action(s: &str) -> AuditAction {
    match s {
        "ToolInvoke" => AuditAction::ToolInvoke,
        "CapabilityCheck" => AuditAction::CapabilityCheck,
        "AgentSpawn" => AuditAction::AgentSpawn,
        "AgentKill" => AuditAction::AgentKill,
        "AgentMessage" => AuditAction::AgentMessage,
        "MemoryAccess" => AuditAction::MemoryAccess,
        "FileAccess" => AuditAction::FileAccess,
        "NetworkAccess" => AuditAction::NetworkAccess,
        "ShellExec" => AuditAction::ShellExec,
        "AuthAttempt" => AuditAction::AuthAttempt,
        "WireConnect" => AuditAction::WireConnect,
        "ConfigChange" => AuditAction::ConfigChange,
        _ => AuditAction::ToolInvoke, // fallback
    }
}

/// A single entry in the Merkle hash chain audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonically increasing sequence number (0-indexed).
    pub seq: u64,
    /// ISO-8601 timestamp of when this entry was recorded.
    pub timestamp: String,
    /// The agent that triggered (or is the subject of) this action.
    pub agent_id: String,
    /// The category of action being audited.
    pub action: AuditAction,
    /// Free-form detail about the action (e.g. tool name, file path).
    pub detail: String,
    /// The outcome of the action (e.g. "ok", "denied", an error message).
    pub outcome: String,
    /// SHA-256 hash of the previous entry (or all-zeros for the genesis).
    pub prev_hash: String,
    /// SHA-256 hash of this entry's content concatenated with `prev_hash`.
    pub hash: String,
}

/// Computes the SHA-256 hash for a single audit entry from its fields.
fn compute_entry_hash(
    seq: u64,
    timestamp: &str,
    agent_id: &str,
    action: &AuditAction,
    detail: &str,
    outcome: &str,
    prev_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seq.to_string().as_bytes());
    hasher.update(timestamp.as_bytes());
    hasher.update(agent_id.as_bytes());
    hasher.update(action.to_string().as_bytes());
    hasher.update(detail.as_bytes());
    hasher.update(outcome.as_bytes());
    hasher.update(prev_hash.as_bytes());
    hex::encode(hasher.finalize())
}

/// One place where the chain does not hold.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChainBreak {
    /// The entry at which the break is seen.
    pub seq: u64,
    pub timestamp: String,
    /// `fork` (does not chain from the entry before it), `missing` (entries
    /// before it are absent) or `altered` (its content does not match its
    /// hash).
    pub kind: String,
}

/// The result of checking the whole chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainReport {
    pub entries: usize,
    /// The first breaks, in chain order (at most [`MAX_LISTED_BREAKS`]).
    pub breaks: Vec<ChainBreak>,
    /// How many breaks there are in all.
    pub break_count: usize,
    /// First entry of the unbroken run that reaches the end.
    pub intact_since_seq: Option<u64>,
    pub intact_since: Option<String>,
}

impl ChainReport {
    /// Whether the chain holds from end to end.
    pub fn is_intact(&self) -> bool {
        self.break_count == 0
    }
}

/// Checks a chain one entry at a time, so that verifying never needs the
/// whole record in memory at once.
#[derive(Default)]
struct ChainCheck {
    entries: usize,
    breaks: Vec<ChainBreak>,
    break_count: usize,
    /// Sequence number and hash of the entry before.
    prev: Option<(u64, String)>,
    /// Start of the unbroken run that ends at the latest entry.
    run_start: Option<(u64, String)>,
}

impl ChainCheck {
    fn push(&mut self, e: &AuditEntry) {
        let link = match &self.prev {
            // A chain that starts at entry 0 starts from the genesis hash. A
            // window that starts later is taken as it is: what it chains from
            // is not in it.
            None if e.seq == 0 && e.prev_hash != GENESIS => Some("fork"),
            None => None,
            Some((seq, _)) if e.seq != seq + 1 => Some("missing"),
            Some((_, hash)) if &e.prev_hash != hash => Some("fork"),
            Some(_) => None,
        };
        let recomputed = compute_entry_hash(
            e.seq,
            &e.timestamp,
            &e.agent_id,
            &e.action,
            &e.detail,
            &e.outcome,
            &e.prev_hash,
        );
        let kind = if recomputed != e.hash {
            Some("altered")
        } else {
            link
        };
        match kind {
            Some(k) => {
                self.break_count += 1;
                if self.breaks.len() < MAX_LISTED_BREAKS {
                    self.breaks.push(ChainBreak {
                        seq: e.seq,
                        timestamp: e.timestamp.clone(),
                        kind: k.to_string(),
                    });
                }
                // An altered entry is not part of any intact run; the run can
                // start with the entry after it. After a fork or a gap, the
                // run starts here.
                self.run_start = if k == "altered" {
                    None
                } else {
                    Some((e.seq, e.timestamp.clone()))
                };
            }
            None => {
                if self.run_start.is_none() {
                    self.run_start = Some((e.seq, e.timestamp.clone()));
                }
            }
        }
        self.prev = Some((e.seq, e.hash.clone()));
        self.entries += 1;
    }

    fn report(self) -> ChainReport {
        ChainReport {
            entries: self.entries,
            breaks: self.breaks,
            break_count: self.break_count,
            intact_since_seq: self.run_start.as_ref().map(|(s, _)| *s),
            intact_since: self.run_start.map(|(_, t)| t),
        }
    }
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    let action: String = row.get(3)?;
    Ok(AuditEntry {
        // Stored as i64 (see the INSERT).
        seq: row.get::<_, i64>(0)? as u64,
        timestamp: row.get(1)?,
        agent_id: row.get(2)?,
        action: parse_action(&action),
        detail: row.get(4)?,
        outcome: row.get(5)?,
        prev_hash: row.get(6)?,
        hash: row.get(7)?,
    })
}

/// Hand every stored entry to `f`, in chain order. A row that cannot be read
/// is skipped, and so shows up as a gap when the chain is checked.
fn for_each_stored(conn: &Connection, mut f: impl FnMut(AuditEntry)) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT seq, timestamp, agent_id, action, detail, outcome, prev_hash, hash \
         FROM audit_entries ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([], read_entry)?;
    for entry in rows.flatten() {
        f(entry);
    }
    Ok(())
}

/// Append one entry chained from the database's last row, in one
/// transaction.
fn append_in_db(
    conn: &Connection,
    timestamp: &str,
    agent_id: &str,
    action: &AuditAction,
    detail: &str,
    outcome: &str,
) -> rusqlite::Result<AuditEntry> {
    // A savepoint nests inside a transaction another part of this process may
    // hold on the shared connection. A write that loses a race with another
    // process fails here (busy, or the primary key on `seq`) and is retried
    // by the caller.
    conn.execute_batch("SAVEPOINT audit_append")?;
    let result = (|| {
        let last: Option<(i64, String)> = conn
            .query_row(
                "SELECT seq, hash FROM audit_entries ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let (seq, prev_hash) = match last {
            Some((s, h)) => (s as u64 + 1, h),
            None => (0, GENESIS.to_string()),
        };
        let hash = compute_entry_hash(
            seq, timestamp, agent_id, action, detail, outcome, &prev_hash,
        );
        conn.execute(
            "INSERT INTO audit_entries (seq, timestamp, agent_id, action, detail, outcome, prev_hash, hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                seq as i64,
                timestamp,
                agent_id,
                action.to_string(),
                detail,
                outcome,
                &prev_hash,
                &hash,
            ],
        )?;
        Ok(AuditEntry {
            seq,
            timestamp: timestamp.to_string(),
            agent_id: agent_id.to_string(),
            action: action.clone(),
            detail: detail.to_string(),
            outcome: outcome.to_string(),
            prev_hash,
            hash,
        })
    })();
    match result {
        Ok(e) => {
            conn.execute_batch("RELEASE audit_append")?;
            Ok(e)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO audit_append; RELEASE audit_append");
            Err(e)
        }
    }
}

/// Drop the oldest in-memory entries once there are clearly too many.
fn trim_window(entries: &mut Vec<AuditEntry>) {
    if entries.len() > MEMORY_WINDOW + MEMORY_WINDOW / 10 {
        let excess = entries.len() - MEMORY_WINDOW;
        entries.drain(..excess);
    }
}

/// An append-only, tamper-evident audit log using a Merkle hash chain.
///
/// Thread-safe — all access is serialised through internal mutexes.
/// Optionally backed by SQLite for persistence across daemon restarts.
pub struct AuditLog {
    /// The most recent entries (at most about [`MEMORY_WINDOW`]).
    entries: Mutex<Vec<AuditEntry>>,
    tip: Mutex<String>,
    /// Entries in the whole record, including those no longer in memory.
    total: AtomicU64,
    /// Optional database connection for persistent storage.
    db: Option<Arc<Mutex<Connection>>>,
}

impl AuditLog {
    /// Creates a new empty audit log (in-memory only, no persistence).
    ///
    /// The initial tip hash is 64 zero characters (the "genesis" sentinel).
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            tip: Mutex::new(GENESIS.to_string()),
            total: AtomicU64::new(0),
            db: None,
        }
    }

    /// Creates an audit log backed by a database connection.
    ///
    /// On construction, reads every stored entry once, checks the chain and
    /// keeps the most recent ones in memory. New entries are written to the
    /// database first, chained from its last row.
    pub fn with_db(conn: Arc<Mutex<Connection>>) -> Self {
        let mut entries = Vec::new();
        let mut tip = GENESIS.to_string();
        let mut check = ChainCheck::default();

        if let Ok(db) = conn.lock() {
            let read = for_each_stored(&db, |entry| {
                check.push(&entry);
                tip = entry.hash.clone();
                entries.push(entry);
                trim_window(&mut entries);
            });
            if let Err(e) = read {
                tracing::error!("Audit trail could not be read from the database: {e}");
            }
        }
        if entries.len() > MEMORY_WINDOW {
            let excess = entries.len() - MEMORY_WINDOW;
            entries.drain(..excess);
        }

        let report = check.report();
        let count = report.entries;
        if count > 0 {
            if report.is_intact() {
                tracing::info!("Audit trail loaded: {count} entries, chain integrity OK");
            } else {
                tracing::error!(
                    breaks = ?report.breaks,
                    intact_since = ?report.intact_since,
                    "Audit trail loaded: {count} entries, {} break(s) in the chain",
                    report.break_count
                );
            }
        }

        Self {
            entries: Mutex::new(entries),
            tip: Mutex::new(tip),
            total: AtomicU64::new(count as u64),
            db: Some(conn),
        }
    }

    /// Records a new auditable event and returns the SHA-256 hash of the entry.
    ///
    /// The entry is atomically appended to the chain with the current tip as
    /// its `prev_hash`, and the tip is advanced to the new hash.
    /// If a database connection is available, the entry is also persisted.
    pub fn record(
        &self,
        agent_id: impl Into<String>,
        action: AuditAction,
        detail: impl Into<String>,
        outcome: impl Into<String>,
    ) -> String {
        self.record_entry(agent_id, action, detail, outcome).hash
    }

    /// Like [`record`](Self::record), returning the whole entry, so a caller
    /// can name its sequence number without reading it back.
    pub fn record_entry(
        &self,
        agent_id: impl Into<String>,
        action: AuditAction,
        detail: impl Into<String>,
        outcome: impl Into<String>,
    ) -> AuditEntry {
        let agent_id = agent_id.into();
        let detail = detail.into();
        let outcome = outcome.into();
        let timestamp = Utc::now().to_rfc3339();

        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut tip = self.tip.lock().unwrap_or_else(|e| e.into_inner());

        // With a database, the database decides where the entry goes: its
        // last row gives the sequence number and the hash to chain from,
        // read and written in one transaction.
        //
        // The chain used to continue from this process's own copy in memory,
        // and the write's result was ignored. Two processes on one database
        // (the daemon and a CLI command) then each extended their own copy --
        // a fork the chain can never pass again -- and a write lost to a busy
        // database left a hole, while the entry still showed in the
        // dashboard until the next restart.
        let mut written = None;
        if let Some(ref db) = self.db {
            for attempt in 0..3u32 {
                let result = match db.lock() {
                    Ok(conn) => {
                        append_in_db(&conn, &timestamp, &agent_id, &action, &detail, &outcome)
                    }
                    Err(_) => break,
                };
                match result {
                    Ok(entry) => {
                        written = Some(entry);
                        break;
                    }
                    Err(e) if attempt == 2 => {
                        tracing::error!(
                            action = %action,
                            detail = %detail,
                            error = %e,
                            "Audit entry could not be written to the database after 3 attempts; \
                             it is kept in memory only"
                        );
                    }
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(
                            50 * (attempt as u64 + 1),
                        ));
                    }
                }
            }
        }

        let entry = written.unwrap_or_else(|| {
            let seq = entries.last().map(|e| e.seq + 1).unwrap_or(0);
            let prev_hash = tip.clone();
            let hash = compute_entry_hash(
                seq, &timestamp, &agent_id, &action, &detail, &outcome, &prev_hash,
            );
            AuditEntry {
                seq,
                timestamp,
                agent_id,
                action,
                detail,
                outcome,
                prev_hash,
                hash,
            }
        });

        *tip = entry.hash.clone();
        entries.push(entry.clone());
        trim_window(&mut entries);
        self.total.fetch_add(1, Ordering::Relaxed);
        entry
    }

    /// Every break in the chain, and where the unbroken run to the end
    /// starts. Read from the database when there is one, since that -- not
    /// this process's memory -- is the record.
    ///
    /// A plain "broken" said nothing useful: one bad link months ago made
    /// every check since read BROKEN, with no way to tell whether anything
    /// recent was touched. Each break is listed with its date, and the run
    /// after the last one is reported as intact.
    pub fn verify_report(&self) -> ChainReport {
        if let Some(ref db) = self.db {
            if let Ok(conn) = db.lock() {
                let mut check = ChainCheck::default();
                match for_each_stored(&conn, |e| check.push(&e)) {
                    Ok(()) => return check.report(),
                    Err(e) => {
                        tracing::warn!("Audit trail could not be read for checking: {e}");
                    }
                }
            }
        }
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut check = ChainCheck::default();
        for e in entries.iter() {
            check.push(e);
        }
        check.report()
    }

    /// Walks the entire chain and recomputes every hash to detect tampering.
    ///
    /// Returns `Ok(())` if the chain is intact, or `Err(msg)` describing
    /// the first inconsistency found.
    pub fn verify_integrity(&self) -> Result<(), String> {
        let report = self.verify_report();
        match report.breaks.first() {
            None => Ok(()),
            Some(b) => Err(format!(
                "{} at seq {} ({} break(s) in all)",
                b.kind, b.seq, report.break_count
            )),
        }
    }

    /// Returns the current tip hash (the hash of the most recent entry,
    /// or the genesis sentinel if the log is empty).
    pub fn tip_hash(&self) -> String {
        self.tip.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Returns the number of entries in the log.
    pub fn len(&self) -> usize {
        self.total.load(Ordering::Relaxed) as usize
    }

    /// Returns whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns up to the most recent `n` entries (cloned).
    pub fn recent(&self, n: usize) -> Vec<AuditEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let start = entries.len().saturating_sub(n);
        entries[start..].to_vec()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_chain_integrity() {
        let log = AuditLog::new();
        log.record(
            "agent-1",
            AuditAction::ToolInvoke,
            "read_file /etc/passwd",
            "ok",
        );
        log.record("agent-1", AuditAction::ShellExec, "ls -la", "ok");
        log.record("agent-2", AuditAction::AgentSpawn, "spawning helper", "ok");
        log.record(
            "agent-1",
            AuditAction::NetworkAccess,
            "https://example.com",
            "denied",
        );

        assert_eq!(log.len(), 4);
        assert!(log.verify_integrity().is_ok());

        // Verify the chain links are correct
        let entries = log.recent(4);
        assert_eq!(entries[0].prev_hash, "0".repeat(64));
        assert_eq!(entries[1].prev_hash, entries[0].hash);
        assert_eq!(entries[2].prev_hash, entries[1].hash);
        assert_eq!(entries[3].prev_hash, entries[2].hash);
    }

    #[test]
    fn test_audit_tamper_detection() {
        let log = AuditLog::new();
        log.record("agent-1", AuditAction::ToolInvoke, "read_file /tmp/a", "ok");
        log.record("agent-1", AuditAction::ShellExec, "rm -rf /", "denied");
        log.record("agent-1", AuditAction::MemoryAccess, "read key foo", "ok");

        // Tamper with an entry
        {
            let mut entries = log.entries.lock().unwrap();
            entries[1].detail = "echo hello".to_string(); // change the detail
        }

        let result = log.verify_integrity();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("altered at seq 1"));
    }

    fn db_with_audit_table() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE audit_entries (seq INTEGER PRIMARY KEY, timestamp TEXT NOT NULL, \
             agent_id TEXT NOT NULL, action TEXT NOT NULL, detail TEXT NOT NULL, \
             outcome TEXT NOT NULL, prev_hash TEXT NOT NULL, hash TEXT NOT NULL)",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    /// Re-hash `entries[i]` and re-link the ones after it, as a writer that
    /// believed its own chain would have.
    fn relink(entries: &mut [AuditEntry], from: usize) {
        for i in from..entries.len() {
            if i > from {
                entries[i].prev_hash = entries[i - 1].hash.clone();
            }
            let e = &entries[i];
            entries[i].hash = compute_entry_hash(
                e.seq,
                &e.timestamp,
                &e.agent_id,
                &e.action,
                &e.detail,
                &e.outcome,
                &e.prev_hash,
            );
        }
    }

    /// Two logs on one database -- the daemon and a CLI command -- used to
    /// fork the chain. Chained from the database's last row, they extend
    /// one chain.
    #[test]
    fn two_writers_on_one_database_keep_one_chain() {
        let db = db_with_audit_table();
        let daemon = AuditLog::with_db(db.clone());
        let cli = AuditLog::with_db(db.clone());
        daemon.record("a", AuditAction::ToolInvoke, "one", "ok");
        cli.record("b", AuditAction::ConfigChange, "two", "ok");
        daemon.record("a", AuditAction::ToolInvoke, "three", "ok");
        let report = daemon.verify_report();
        assert_eq!(report.entries, 3);
        assert!(report.breaks.is_empty(), "{:?}", report.breaks);
        assert_eq!(report.intact_since_seq, Some(0));

        // And a restart reads back one chain of three.
        let reopened = AuditLog::with_db(db);
        assert_eq!(reopened.len(), 3);
        assert!(reopened.verify_integrity().is_ok());
    }

    /// A fork and a lost entry are each reported with where they are, and
    /// the run after the last one counts as intact.
    #[test]
    fn breaks_are_listed_and_the_intact_run_is_named() {
        let log = AuditLog::new();
        for i in 0..6 {
            log.record("a", AuditAction::ToolInvoke, format!("e{i}"), "ok");
        }
        let mut entries = log.entries.lock().unwrap().clone();
        // seq 2 chains from something else: a fork.
        entries[2].prev_hash = "f".repeat(64);
        relink(&mut entries, 2);
        // seq 4 is lost: 5 follows 3.
        entries.remove(4);

        let mut check = ChainCheck::default();
        for e in &entries {
            check.push(e);
        }
        let report = check.report();
        let kinds: Vec<(u64, &str)> = report
            .breaks
            .iter()
            .map(|b| (b.seq, b.kind.as_str()))
            .collect();
        assert_eq!(kinds, vec![(2, "fork"), (5, "missing")]);
        assert_eq!(report.break_count, 2);
        assert_eq!(report.intact_since_seq, Some(5));
    }

    /// An altered entry is never the start of the intact run.
    #[test]
    fn intact_run_starts_after_an_altered_entry() {
        let log = AuditLog::new();
        for i in 0..4 {
            log.record("a", AuditAction::ToolInvoke, format!("e{i}"), "ok");
        }
        log.entries.lock().unwrap()[3].outcome = "denied".to_string();
        let report = log.verify_report();
        assert_eq!(report.break_count, 1);
        assert_eq!(report.breaks[0].kind, "altered");
        assert_eq!(report.intact_since_seq, None);

        log.record("a", AuditAction::ToolInvoke, "e4", "ok");
        assert_eq!(log.verify_report().intact_since_seq, Some(4));
    }

    /// A tampered row in the database is found even though memory still
    /// holds the original: the database is what gets checked.
    #[test]
    fn verification_reads_the_database() {
        let db = db_with_audit_table();
        let log = AuditLog::with_db(db.clone());
        log.record("a", AuditAction::ShellExec, "ls", "ok");
        log.record("a", AuditAction::ShellExec, "pwd", "ok");
        db.lock()
            .unwrap()
            .execute(
                "UPDATE audit_entries SET detail = 'rm -rf /' WHERE seq = 1",
                [],
            )
            .unwrap();
        let report = log.verify_report();
        assert_eq!(report.break_count, 1);
        assert_eq!(report.breaks[0].seq, 1);
        assert_eq!(report.breaks[0].kind, "altered");
    }

    /// When the database refuses the write, the entry is still recorded in
    /// memory rather than lost without a trace.
    #[test]
    fn a_failed_database_write_is_kept_in_memory() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let log = AuditLog::with_db(db); // no audit_entries table
        log.record("a", AuditAction::AgentSpawn, "spawn", "ok");
        log.record("a", AuditAction::AgentKill, "kill", "ok");
        assert_eq!(log.len(), 2);
        let entries = log.recent(10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].prev_hash, entries[0].hash);
        assert!(log.verify_integrity().is_ok());
    }

    /// Memory keeps only the recent entries; the count, the tip and
    /// verification still cover the whole record.
    #[test]
    fn memory_keeps_a_window_and_the_database_keeps_everything() {
        let db = db_with_audit_table();
        let log = AuditLog::with_db(db.clone());
        let n = MEMORY_WINDOW + MEMORY_WINDOW / 10 + 5;
        {
            // Fill the table directly (fast), then load it.
            let conn = db.lock().unwrap();
            let mut prev = GENESIS.to_string();
            conn.execute_batch("BEGIN").unwrap();
            for seq in 0..n as u64 {
                let hash =
                    compute_entry_hash(seq, "t", "a", &AuditAction::ToolInvoke, "d", "ok", &prev);
                conn.execute(
                    "INSERT INTO audit_entries VALUES (?1, 't', 'a', 'ToolInvoke', 'd', 'ok', ?2, ?3)",
                    rusqlite::params![seq as i64, &prev, &hash],
                )
                .unwrap();
                prev = hash;
            }
            conn.execute_batch("COMMIT").unwrap();
        }
        drop(log);

        let log = AuditLog::with_db(db);
        assert_eq!(log.len(), n);
        assert_eq!(log.entries.lock().unwrap().len(), MEMORY_WINDOW);
        assert_eq!(log.recent(1)[0].seq, n as u64 - 1);
        let report = log.verify_report();
        assert_eq!(report.entries, n);
        assert!(report.is_intact());

        let next = log.record_entry("a", AuditAction::ToolInvoke, "next", "ok");
        assert_eq!(next.seq, n as u64);
        assert_eq!(log.len(), n + 1);
    }

    #[test]
    fn test_audit_tip_changes() {
        let log = AuditLog::new();
        let genesis_tip = log.tip_hash();
        assert_eq!(genesis_tip, "0".repeat(64));

        let h1 = log.record("a", AuditAction::AgentSpawn, "spawn", "ok");
        assert_eq!(log.tip_hash(), h1);
        assert_ne!(log.tip_hash(), genesis_tip);

        let h2 = log.record("b", AuditAction::AgentKill, "kill", "ok");
        assert_eq!(log.tip_hash(), h2);
        assert_ne!(h2, h1);
    }

    #[test]
    fn test_audit_persists_to_db() {
        let db = db_with_audit_table();

        // Record entries with DB
        let log = AuditLog::with_db(Arc::clone(&db));
        log.record("agent-1", AuditAction::AgentSpawn, "spawn test", "ok");
        log.record("agent-1", AuditAction::ShellExec, "ls", "ok");
        assert_eq!(log.len(), 2);

        // Verify entries in database
        let db_conn = db.lock().unwrap();
        let count: i64 = db_conn
            .query_row("SELECT COUNT(*) FROM audit_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        drop(db_conn);

        // Simulate restart: create new AuditLog from same DB
        let log2 = AuditLog::with_db(Arc::clone(&db));
        assert_eq!(log2.len(), 2);
        assert!(log2.verify_integrity().is_ok());

        // Chain continues correctly after restart
        log2.record("agent-2", AuditAction::ToolInvoke, "file_read", "ok");
        assert_eq!(log2.len(), 3);
        assert!(log2.verify_integrity().is_ok());

        // Verify tip is correct
        let entries = log2.recent(3);
        assert_eq!(entries[2].prev_hash, entries[1].hash);
    }
}

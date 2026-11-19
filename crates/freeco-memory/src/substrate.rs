//! MemorySubstrate: unified implementation of the `Memory` trait.
//!
//! Composes the structured store, semantic store, knowledge store,
//! session store, and consolidation engine behind a single async API.

use crate::consolidation::ConsolidationEngine;
use crate::knowledge::KnowledgeStore;
use crate::migration::run_migrations;
use crate::org::OrgStore;
use crate::semantic::SemanticStore;
use crate::session::{Session, SessionStore};
use crate::structured::StructuredStore;
use crate::tenancy::TenancyStore;
use crate::usage::UsageStore;

use async_trait::async_trait;
use freeco_types::agent::{AgentEntry, AgentId, SessionId};
use freeco_types::config::MemoryConfig;
use freeco_types::error::{FreecoError, FreecoResult};
use freeco_types::memory::{
    ConsolidationReport, Entity, ExportFormat, GraphMatch, GraphPattern, ImportReport, Memory,
    MemoryFilter, MemoryFragment, MemoryId, MemorySource, Relation,
};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// The unified memory substrate. Implements the `Memory` trait by delegating
/// to specialized stores backed by a shared SQLite connection.
pub struct MemorySubstrate {
    conn: Arc<Mutex<Connection>>,
    structured: StructuredStore,
    semantic: SemanticStore,
    knowledge: KnowledgeStore,
    sessions: SessionStore,
    org: OrgStore,
    tenancy: TenancyStore,
    consolidation: ConsolidationEngine,
    usage: UsageStore,
}

impl MemorySubstrate {
    /// Companies, projects and teams. Exposed so the API can present the
    /// structure conversations belong to; the substrate owns the connection.
    pub fn org(&self) -> &OrgStore {
        &self.org
    }

    /// Open or create a memory substrate at the given database path.
    ///
    /// When `memory_config.backend == "http"` and `http_url`/`http_token_env` are set,
    /// the semantic store routes `remember`/`recall` to the memory-api gateway.
    /// All other stores (KV, knowledge graph, sessions) remain local SQLite.
    pub fn open(
        db_path: &Path,
        decay_rate: f32,
        memory_config: &MemoryConfig,
    ) -> FreecoResult<Self> {
        let conn = Connection::open(db_path).map_err(|e| FreecoError::Memory(e.to_string()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        run_migrations(&conn).map_err(|e| FreecoError::Memory(e.to_string()))?;
        let shared = Arc::new(Mutex::new(conn));

        let semantic = Self::create_semantic_store(Arc::clone(&shared), memory_config);

        Ok(Self {
            conn: Arc::clone(&shared),
            structured: StructuredStore::new(Arc::clone(&shared)),
            semantic,
            knowledge: KnowledgeStore::new(Arc::clone(&shared)),
            sessions: SessionStore::new(Arc::clone(&shared)),
            org: OrgStore::new(Arc::clone(&shared)),
            tenancy: TenancyStore::new(Arc::clone(&shared)),
            usage: UsageStore::new(Arc::clone(&shared)),
            consolidation: ConsolidationEngine::new(shared, decay_rate),
        })
    }

    /// Create the semantic store, optionally with HTTP backend.
    fn create_semantic_store(
        conn: Arc<Mutex<Connection>>,
        memory_config: &MemoryConfig,
    ) -> SemanticStore {
        #[cfg(feature = "http-memory")]
        if memory_config.backend == "http" {
            if let (Some(url), Some(token_env)) =
                (&memory_config.http_url, &memory_config.http_token_env)
            {
                match crate::http_client::MemoryApiClient::new(url, token_env) {
                    Ok(client) => {
                        // Best-effort health check on startup
                        match client.health_check() {
                            Ok(()) => info!(url = %url, "HTTP memory backend connected"),
                            Err(e) => {
                                warn!(url = %url, error = %e, "HTTP memory backend health check failed, will retry on use")
                            }
                        }
                        return SemanticStore::new_with_http(conn, client);
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to create HTTP memory client, falling back to SQLite");
                    }
                }
            } else {
                warn!("backend=http but http_url/http_token_env not set, falling back to SQLite");
            }
        }

        #[cfg(not(feature = "http-memory"))]
        let _ = memory_config;

        SemanticStore::new(conn)
    }

    /// Create an in-memory substrate (for testing). Always uses SQLite backend.
    pub fn open_in_memory(decay_rate: f32) -> FreecoResult<Self> {
        let conn = Connection::open_in_memory().map_err(|e| FreecoError::Memory(e.to_string()))?;
        run_migrations(&conn).map_err(|e| FreecoError::Memory(e.to_string()))?;
        let shared = Arc::new(Mutex::new(conn));

        Ok(Self {
            conn: Arc::clone(&shared),
            structured: StructuredStore::new(Arc::clone(&shared)),
            semantic: SemanticStore::new(Arc::clone(&shared)),
            knowledge: KnowledgeStore::new(Arc::clone(&shared)),
            sessions: SessionStore::new(Arc::clone(&shared)),
            org: OrgStore::new(Arc::clone(&shared)),
            tenancy: TenancyStore::new(Arc::clone(&shared)),
            usage: UsageStore::new(Arc::clone(&shared)),
            consolidation: ConsolidationEngine::new(shared, decay_rate),
        })
    }

    /// Get a reference to the usage store.
    pub fn usage(&self) -> &UsageStore {
        &self.usage
    }

    /// Get the shared database connection (for constructing stores from outside).
    pub fn usage_conn(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Save an agent entry to persistent storage.
    pub fn save_agent(&self, entry: &AgentEntry) -> FreecoResult<()> {
        self.structured.save_agent(entry)
    }

    /// Load an agent entry from persistent storage.
    pub fn load_agent(&self, agent_id: AgentId) -> FreecoResult<Option<AgentEntry>> {
        self.structured.load_agent(agent_id)
    }

    /// Remove an agent from persistent storage and cascade-delete sessions.
    pub fn remove_agent(&self, agent_id: AgentId) -> FreecoResult<()> {
        // Delete associated sessions first
        let _ = self.sessions.delete_agent_sessions(agent_id);
        self.structured.remove_agent(agent_id)
    }

    /// Remove an agent from persistent storage, leaving its conversations.
    ///
    /// For an agent replaced under the same id (a hand reactivated at boot):
    /// what it talked about belongs to the replacement.
    pub fn remove_agent_keeping_sessions(&self, agent_id: AgentId) -> FreecoResult<()> {
        self.structured.remove_agent(agent_id)
    }

    /// Load all agent entries from persistent storage.
    pub fn load_all_agents(&self) -> FreecoResult<Vec<AgentEntry>> {
        self.structured.load_all_agents()
    }

    /// List all saved agents.
    pub fn list_agents(&self) -> FreecoResult<Vec<(String, String, String)>> {
        self.structured.list_agents()
    }

    /// Synchronous get from the structured store (for kernel handle use).
    pub fn structured_get(
        &self,
        agent_id: AgentId,
        key: &str,
    ) -> FreecoResult<Option<serde_json::Value>> {
        self.structured.get(agent_id, key)
    }

    /// List all KV pairs for an agent.
    pub fn list_kv(&self, agent_id: AgentId) -> FreecoResult<Vec<(String, serde_json::Value)>> {
        self.structured.list_kv(agent_id)
    }

    /// Delete a KV entry for an agent.
    pub fn structured_delete(&self, agent_id: AgentId, key: &str) -> FreecoResult<()> {
        self.structured.delete(agent_id, key)
    }

    /// Synchronous set in the structured store (for kernel handle use).
    pub fn structured_set(
        &self,
        agent_id: AgentId,
        key: &str,
        value: serde_json::Value,
    ) -> FreecoResult<()> {
        self.structured.set(agent_id, key, value)
    }

    /// Get a session by ID.
    pub fn get_session(&self, session_id: SessionId) -> FreecoResult<Option<Session>> {
        self.sessions.get_session(session_id)
    }

    /// Load an agent's canonical conversation as a session.
    pub fn get_canonical_as_session(&self, agent_id: AgentId) -> FreecoResult<Option<Session>> {
        self.sessions.get_canonical_as_session(agent_id)
    }

    /// Save a session.
    pub fn save_session(&self, session: &Session) -> FreecoResult<()> {
        self.sessions.save_session(session)
    }

    /// Save a session asynchronously — runs the SQLite write in a blocking
    /// thread so the tokio runtime stays responsive.
    pub async fn save_session_async(&self, session: &Session) -> FreecoResult<()> {
        let sessions = self.sessions.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || sessions.save_session(&session))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    /// Create a new empty session for an agent.
    pub fn create_session(&self, agent_id: AgentId) -> FreecoResult<Session> {
        self.sessions.create_session(agent_id)
    }

    /// The underlying database handle.
    ///
    /// Exposed so the data gateway can keep its token table in the same file
    /// as everything else: a mapping that lives in a second database is a
    /// second thing to back up, and losing it makes every masked conversation
    /// permanently unreadable.
    pub fn connection(&self) -> std::sync::Arc<std::sync::Mutex<rusqlite::Connection>> {
        self.conn.clone()
    }

    /// List all sessions with metadata.
    pub fn list_sessions(&self) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.list_sessions()
    }

    /// Every conversation not in the trash, without its messages, newest
    /// first.
    pub fn list_conversation_summaries(
        &self,
    ) -> FreecoResult<Vec<crate::session::ConversationSummary>> {
        self.sessions.list_conversation_summaries()
    }

    /// List agent conversations (the canonical store) with their names.
    pub fn list_canonical_sessions(&self) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.list_canonical_sessions()
    }

    /// Tenants and users: who owns a row, and who may sign in.
    pub fn tenancy(&self) -> &TenancyStore {
        &self.tenancy
    }

    /// Conversations whose owning agent no longer exists.
    pub fn list_orphaned_sessions(&self) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.list_orphaned_sessions()
    }

    /// Hand orphaned conversations to an existing agent so they are listed again.
    pub fn adopt_orphaned_sessions(&self, agent_id: &str) -> FreecoResult<usize> {
        self.sessions.adopt_orphaned_sessions(agent_id)
    }

    /// Delete a session by ID.
    pub fn delete_session(&self, session_id: SessionId) -> FreecoResult<()> {
        self.sessions.delete_session(session_id)
    }

    /// Delete all sessions belonging to an agent.
    pub fn delete_agent_sessions(&self, agent_id: AgentId) -> FreecoResult<()> {
        self.sessions.delete_agent_sessions(agent_id)
    }

    /// Delete the canonical (cross-channel) session for an agent.
    pub fn delete_canonical_session(&self, agent_id: AgentId) -> FreecoResult<()> {
        self.sessions.delete_canonical_session(agent_id)
    }

    /// Set or clear a session label.
    pub fn set_session_label(
        &self,
        session_id: SessionId,
        label: Option<&str>,
    ) -> FreecoResult<()> {
        self.sessions.set_session_label(session_id, label)
    }

    /// Find a session by label for a given agent.
    pub fn find_session_by_label(
        &self,
        agent_id: AgentId,
        label: &str,
    ) -> FreecoResult<Option<Session>> {
        self.sessions.find_session_by_label(agent_id, label)
    }

    /// List all sessions for a specific agent.
    pub fn list_agent_sessions(&self, agent_id: AgentId) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.list_agent_sessions(agent_id)
    }

    /// An agent's most recent conversations with label and last request.
    pub fn recent_session_topics(
        &self,
        agent_id: AgentId,
        limit: usize,
    ) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.recent_session_topics(agent_id, limit)
    }

    /// Create a new session with an optional label.
    pub fn create_session_with_label(
        &self,
        agent_id: AgentId,
        label: Option<&str>,
    ) -> FreecoResult<Session> {
        self.sessions.create_session_with_label(agent_id, label)
    }

    /// Load canonical session context for cross-channel memory.
    ///
    /// Returns the compacted summary (if any) and recent messages from the
    /// agent's persistent canonical session.
    pub fn canonical_context(
        &self,
        agent_id: AgentId,
        window_size: Option<usize>,
    ) -> FreecoResult<(Option<String>, Vec<freeco_types::message::Message>)> {
        self.sessions.canonical_context(agent_id, window_size)
    }

    /// Total messages held in the agent's canonical session, whatever window
    /// `canonical_context` was asked for.
    pub fn canonical_len(&self, agent_id: AgentId) -> FreecoResult<usize> {
        self.sessions.canonical_len(agent_id)
    }

    /// The agent's whole conversation, for archiving and forking.
    pub fn load_canonical(
        &self,
        agent_id: AgentId,
    ) -> FreecoResult<crate::session::CanonicalSession> {
        self.sessions.load_canonical(agent_id)
    }

    /// Name the agent's conversation; `None` clears the name.
    pub fn set_canonical_label(&self, agent_id: AgentId, label: Option<&str>) -> FreecoResult<()> {
        self.sessions.set_canonical_label(agent_id, label)
    }

    /// Conversations that already discuss a theme, best match first.
    pub fn search_canonical_sessions(
        &self,
        query: &str,
        limit: usize,
    ) -> FreecoResult<Vec<serde_json::Value>> {
        self.sessions.search_canonical_sessions(query, limit)
    }

    /// Store an LLM-generated summary, replacing older messages with the kept subset.
    ///
    /// Used by the compactor to replace text-truncation compaction with an
    /// LLM-generated summary of older conversation history.
    pub fn store_llm_summary(
        &self,
        agent_id: AgentId,
        summary: &str,
        kept_messages: Vec<freeco_types::message::Message>,
    ) -> FreecoResult<()> {
        self.sessions
            .store_llm_summary(agent_id, summary, kept_messages)
    }

    /// Write a human-readable JSONL mirror of a session to disk.
    ///
    /// Best-effort — errors are returned but should be logged,
    /// never affecting the primary SQLite store.
    pub fn write_jsonl_mirror(
        &self,
        session: &Session,
        sessions_dir: &Path,
    ) -> Result<(), std::io::Error> {
        self.sessions.write_jsonl_mirror(session, sessions_dir)
    }

    /// Append messages to the agent's canonical session for cross-channel persistence.
    pub fn append_canonical(
        &self,
        agent_id: AgentId,
        messages: &[freeco_types::message::Message],
        compaction_threshold: Option<usize>,
    ) -> FreecoResult<()> {
        self.sessions
            .append_canonical(agent_id, messages, compaction_threshold)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Paired devices persistence
    // -----------------------------------------------------------------

    /// Load all paired devices from the database.
    pub fn load_paired_devices(&self) -> FreecoResult<Vec<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT device_id, display_name, platform, paired_at, last_seen, push_token FROM paired_devices"
        ).map_err(|e| FreecoError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(serde_json::json!({
                    "device_id": row.get::<_, String>(0)?,
                    "display_name": row.get::<_, String>(1)?,
                    "platform": row.get::<_, String>(2)?,
                    "paired_at": row.get::<_, String>(3)?,
                    "last_seen": row.get::<_, String>(4)?,
                    "push_token": row.get::<_, Option<String>>(5)?,
                }))
            })
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(row.map_err(|e| FreecoError::Memory(e.to_string()))?);
        }
        Ok(devices)
    }

    /// Save a paired device to the database (insert or replace).
    pub fn save_paired_device(
        &self,
        device_id: &str,
        display_name: &str,
        platform: &str,
        paired_at: &str,
        last_seen: &str,
        push_token: Option<&str>,
    ) -> FreecoResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO paired_devices (device_id, display_name, platform, paired_at, last_seen, push_token) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![device_id, display_name, platform, paired_at, last_seen, push_token],
        ).map_err(|e| FreecoError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Remove a paired device from the database.
    pub fn remove_paired_device(&self, device_id: &str) -> FreecoResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        conn.execute(
            "DELETE FROM paired_devices WHERE device_id = ?1",
            rusqlite::params![device_id],
        )
        .map_err(|e| FreecoError::Memory(e.to_string()))?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Embedding-aware memory operations
    // -----------------------------------------------------------------

    /// Store a memory with an embedding vector.
    pub fn remember_with_embedding(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> FreecoResult<MemoryId> {
        self.semantic
            .remember_with_embedding(agent_id, content, source, scope, metadata, embedding)
    }

    /// Recall memories using vector similarity when a query embedding is provided.
    pub fn recall_with_embedding(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> FreecoResult<Vec<MemoryFragment>> {
        self.semantic
            .recall_with_embedding(query, limit, filter, query_embedding)
    }

    /// Update the embedding for an existing memory.
    pub fn update_embedding(&self, id: MemoryId, embedding: &[f32]) -> FreecoResult<()> {
        self.semantic.update_embedding(id, embedding)
    }

    /// Async wrapper for `recall_with_embedding` — runs in a blocking thread.
    pub async fn recall_with_embedding_async(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
        query_embedding: Option<&[f32]>,
    ) -> FreecoResult<Vec<MemoryFragment>> {
        let store = self.semantic.clone();
        let query = query.to_string();
        let embedding_owned = query_embedding.map(|e| e.to_vec());
        tokio::task::spawn_blocking(move || {
            store.recall_with_embedding(&query, limit, filter, embedding_owned.as_deref())
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    /// Async wrapper for `remember_with_embedding` — runs in a blocking thread.
    pub async fn remember_with_embedding_async(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> FreecoResult<MemoryId> {
        let store = self.semantic.clone();
        let content = content.to_string();
        let scope = scope.to_string();
        let embedding_owned = embedding.map(|e| e.to_vec());
        tokio::task::spawn_blocking(move || {
            store.remember_with_embedding(
                agent_id,
                &content,
                source,
                &scope,
                metadata,
                embedding_owned.as_deref(),
            )
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    // -----------------------------------------------------------------
    // Task queue operations
    // -----------------------------------------------------------------

    /// Post a new task to the shared queue. Returns the task ID.
    pub async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> FreecoResult<String> {
        self.task_post_on_board(title, description, assigned_to, created_by, None, &[])
            .await
    }

    /// Post a task, optionally onto a team's board and waiting on other
    /// tasks. Returns the task ID.
    ///
    /// Every task named in `blocked_by` must exist: a dependency on a task
    /// that is not there would keep this one waiting forever, with nothing
    /// on the board to say why.
    pub async fn task_post_on_board(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
        team_id: Option<&str>,
        blocked_by: &[String],
    ) -> FreecoResult<String> {
        let conn = Arc::clone(&self.conn);
        let title = title.to_string();
        let description = description.to_string();
        let assigned_to = assigned_to.unwrap_or("").to_string();
        let created_by = created_by.unwrap_or("").to_string();
        let team_id = team_id.unwrap_or("").to_string();
        let mut blocked_by: Vec<String> = blocked_by
            .iter()
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .collect();
        blocked_by.dedup();

        tokio::task::spawn_blocking(move || {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let db = conn.lock().map_err(|e| FreecoError::Internal(e.to_string()))?;
            for dep in &blocked_by {
                let found: i64 = db
                    .query_row(
                        "SELECT COUNT(*) FROM task_queue WHERE id = ?1",
                        [dep],
                        |r| r.get(0),
                    )
                    .map_err(|e| FreecoError::Memory(e.to_string()))?;
                if found == 0 {
                    return Err(FreecoError::InvalidInput(format!(
                        "No task with id {dep} to wait on"
                    )));
                }
            }
            let deps = serde_json::to_string(&blocked_by)
                .map_err(|e| FreecoError::Internal(e.to_string()))?;
            db.execute(
                "INSERT INTO task_queue (id, agent_id, task_type, payload, status, priority, created_at, title, description, assigned_to, created_by, team_id, blocked_by)
                 VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![id, &created_by, &title, b"", now, title, description, assigned_to, created_by, team_id, deps],
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
            Ok(id)
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    /// Claim the next task this agent may work on. Returns task JSON or None.
    ///
    /// A task is claimable when it is pending, assigned to this agent or to
    /// nobody, every task it is blocked by is completed, and -- when it sits on
    /// a team's board -- the agent is on that team. The check and the claim
    /// run under one lock on the database, so two agents asking at once can
    /// never both get the same task.
    pub async fn task_claim(&self, agent_id: &str) -> FreecoResult<Option<serde_json::Value>> {
        let conn = Arc::clone(&self.conn);
        let agent_id = agent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let db = conn
                .lock()
                .map_err(|e| FreecoError::Internal(e.to_string()))?;
            let teams: Vec<String> = {
                let mut stmt = db
                    .prepare("SELECT team_id FROM team_members WHERE agent_id = ?1")
                    .map_err(|e| FreecoError::Memory(e.to_string()))?;
                let rows = stmt
                    .query_map([&agent_id], |r| r.get::<_, String>(0))
                    .map_err(|e| FreecoError::Memory(e.to_string()))?;
                rows.filter_map(Result::ok).collect()
            };

            let mut stmt = db
                .prepare(
                    "SELECT id, title, description, assigned_to, created_by, created_at,
                            COALESCE(team_id, ''), COALESCE(blocked_by, '[]')
                     FROM task_queue
                     WHERE status = 'pending' AND (assigned_to = ?1 OR assigned_to = '')
                     ORDER BY priority DESC, created_at ASC",
                )
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            let candidates: Vec<ClaimCandidate> = stmt
                .query_map(rusqlite::params![agent_id], |row| {
                    Ok(ClaimCandidate {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        assigned: row.get(3)?,
                        created_by: row.get(4)?,
                        created_at: row.get(5)?,
                        team_id: row.get(6)?,
                        blocked_by: row.get(7)?,
                    })
                })
                .map_err(|e| FreecoError::Memory(e.to_string()))?
                .collect::<Result<_, _>>()
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            drop(stmt);

            for c in candidates {
                if !c.team_id.is_empty() && !teams.contains(&c.team_id) {
                    continue;
                }
                if !open_blockers(&db, &c.blocked_by)?.is_empty() {
                    continue;
                }
                db.execute(
                    "UPDATE task_queue SET status = 'in_progress', assigned_to = ?2 WHERE id = ?1",
                    rusqlite::params![c.id, agent_id],
                )
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
                return Ok(Some(serde_json::json!({
                    "id": c.id,
                    "title": c.title,
                    "description": c.description,
                    "status": "in_progress",
                    "assigned_to": if c.assigned.is_empty() { &agent_id } else { &c.assigned },
                    "created_by": c.created_by,
                    "created_at": c.created_at,
                    "team_id": c.team_id,
                })));
            }
            Ok(None)
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    /// Mark a task as completed with a result string.
    pub async fn task_complete(&self, task_id: &str, result: &str) -> FreecoResult<()> {
        self.task_complete_by(task_id, result, None)
            .await
            .map(|_| ())
    }

    /// Complete a task, checking who is completing it, and report which
    /// waiting tasks this unblocked.
    ///
    /// With `by` set, only the agent that holds the task or the one that
    /// posted it may complete it: otherwise any agent could close a
    /// teammate's work, and the board would say "done" about work nobody did.
    pub async fn task_complete_by(
        &self,
        task_id: &str,
        result: &str,
        by: Option<&str>,
    ) -> FreecoResult<Vec<String>> {
        let conn = Arc::clone(&self.conn);
        let task_id = task_id.to_string();
        let result = result.to_string();
        let by = by.map(str::to_string);

        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let db = conn.lock().map_err(|e| FreecoError::Internal(e.to_string()))?;
            let row = db.query_row(
                "SELECT status, COALESCE(assigned_to, ''), COALESCE(created_by, '')
                 FROM task_queue WHERE id = ?1",
                [&task_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            );
            let (status, holder, poster) = match row {
                Ok(r) => r,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    return Err(FreecoError::Internal(format!("Task not found: {task_id}")))
                }
                Err(e) => return Err(FreecoError::Memory(e.to_string())),
            };
            if status == "completed" {
                return Err(FreecoError::InvalidInput(format!(
                    "Task {task_id} is already completed"
                )));
            }
            if let Some(by) = by.as_deref() {
                if by != holder && by != poster {
                    return Err(FreecoError::InvalidInput(format!(
                        "Task {task_id} is not yours to complete: only the agent working on it \
                         or the one that posted it can"
                    )));
                }
            }
            db.execute(
                "UPDATE task_queue SET status = 'completed', result = ?2, completed_at = ?3 WHERE id = ?1",
                rusqlite::params![task_id, result, now],
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;

            // The waiting tasks this one held up that now have nothing left
            // to wait for.
            let pattern = format!("%\"{task_id}\"%");
            let mut stmt = db
                .prepare(
                    "SELECT id, COALESCE(blocked_by, '[]') FROM task_queue
                     WHERE status = 'pending' AND blocked_by LIKE ?1",
                )
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            let waiting: Vec<(String, String)> = stmt
                .query_map([&pattern], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| FreecoError::Memory(e.to_string()))?
                .collect::<Result<_, _>>()
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            drop(stmt);
            let mut ready = Vec::new();
            for (id, deps) in waiting {
                if open_blockers(&db, &deps)?.is_empty() {
                    ready.push(id);
                }
            }
            Ok(ready)
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    /// List tasks, optionally filtered by status.
    pub async fn task_list(&self, status: Option<&str>) -> FreecoResult<Vec<serde_json::Value>> {
        self.task_board(status, None).await
    }

    /// List tasks, optionally filtered by status and by team, newest first.
    ///
    /// Each task says what it waits on (`blocked_by`) and whether it is still
    /// waiting (`blocked`: pending with at least one of those not completed).
    pub async fn task_board(
        &self,
        status: Option<&str>,
        team_id: Option<&str>,
    ) -> FreecoResult<Vec<serde_json::Value>> {
        let conn = Arc::clone(&self.conn);
        let status = status.map(|s| s.to_string());
        let team_id = team_id.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || {
            let db = conn.lock().map_err(|e| FreecoError::Internal(e.to_string()))?;
            let mut sql = String::from(
                "SELECT id, title, description, status, assigned_to, created_by, created_at, completed_at, result,
                        COALESCE(team_id, ''), COALESCE(blocked_by, '[]')
                 FROM task_queue WHERE 1 = 1",
            );
            let mut params: Vec<String> = Vec::new();
            if let Some(s) = &status {
                params.push(s.clone());
                sql.push_str(&format!(" AND status = ?{}", params.len()));
            }
            if let Some(t) = &team_id {
                params.push(t.clone());
                sql.push_str(&format!(" AND team_id = ?{}", params.len()));
            }
            sql.push_str(" ORDER BY created_at DESC");

            let mut stmt = db
                .prepare(&sql)
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            let rows: Vec<(serde_json::Value, String)> = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    Ok((
                        serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "title": row.get::<_, String>(1).unwrap_or_default(),
                            "description": row.get::<_, String>(2).unwrap_or_default(),
                            "status": row.get::<_, String>(3)?,
                            "assigned_to": row.get::<_, String>(4).unwrap_or_default(),
                            "created_by": row.get::<_, String>(5).unwrap_or_default(),
                            "created_at": row.get::<_, String>(6).unwrap_or_default(),
                            "completed_at": row.get::<_, Option<String>>(7).unwrap_or(None),
                            "result": row.get::<_, Option<String>>(8).unwrap_or(None),
                            "team_id": row.get::<_, String>(9).unwrap_or_default(),
                        }),
                        row.get::<_, String>(10).unwrap_or_else(|_| "[]".into()),
                    ))
                })
                .map_err(|e| FreecoError::Memory(e.to_string()))?
                .collect::<Result<_, _>>()
                .map_err(|e| FreecoError::Memory(e.to_string()))?;
            drop(stmt);

            let mut tasks = Vec::with_capacity(rows.len());
            for (mut task, deps) in rows {
                let list: Vec<String> = serde_json::from_str(&deps).unwrap_or_default();
                let blocked = task["status"] == "pending" && !open_blockers(&db, &deps)?.is_empty();
                if let Some(obj) = task.as_object_mut() {
                    obj.insert("blocked_by".into(), serde_json::json!(list));
                    obj.insert("blocked".into(), serde_json::json!(blocked));
                }
                tasks.push(task);
            }
            Ok(tasks)
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }
}

/// A pending task `task_claim` may hand out, as read from the board.
struct ClaimCandidate {
    id: String,
    title: String,
    description: String,
    assigned: String,
    created_by: String,
    created_at: String,
    team_id: String,
    blocked_by: String,
}

/// The tasks in a `blocked_by` JSON list that are not completed yet.
///
/// A dependency that no longer exists counts as open: a deleted prerequisite
/// is not the same as a finished one, and treating it as done would start
/// work whose input never arrived.
fn open_blockers(db: &Connection, deps_json: &str) -> FreecoResult<Vec<String>> {
    let deps: Vec<String> = serde_json::from_str(deps_json).unwrap_or_default();
    let mut open = Vec::new();
    for dep in deps {
        let status: Option<String> = db
            .query_row("SELECT status FROM task_queue WHERE id = ?1", [&dep], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(FreecoError::Memory(e.to_string())),
            })?;
        if status.as_deref() != Some("completed") {
            open.push(dep);
        }
    }
    Ok(open)
}

#[async_trait]
impl Memory for MemorySubstrate {
    async fn get(&self, agent_id: AgentId, key: &str) -> FreecoResult<Option<serde_json::Value>> {
        let store = self.structured.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || store.get(agent_id, &key))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn set(
        &self,
        agent_id: AgentId,
        key: &str,
        value: serde_json::Value,
    ) -> FreecoResult<()> {
        let store = self.structured.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || store.set(agent_id, &key, value))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn delete(&self, agent_id: AgentId, key: &str) -> FreecoResult<()> {
        let store = self.structured.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || store.delete(agent_id, &key))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn remember(
        &self,
        agent_id: AgentId,
        content: &str,
        source: MemorySource,
        scope: &str,
        metadata: HashMap<String, serde_json::Value>,
    ) -> FreecoResult<MemoryId> {
        let store = self.semantic.clone();
        let content = content.to_string();
        let scope = scope.to_string();
        tokio::task::spawn_blocking(move || {
            store.remember(agent_id, &content, source, &scope, metadata)
        })
        .await
        .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn recall(
        &self,
        query: &str,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> FreecoResult<Vec<MemoryFragment>> {
        let store = self.semantic.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || store.recall(&query, limit, filter))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn forget(&self, id: MemoryId) -> FreecoResult<()> {
        let store = self.semantic.clone();
        tokio::task::spawn_blocking(move || store.forget(id))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn add_entity(&self, entity: Entity) -> FreecoResult<String> {
        let store = self.knowledge.clone();
        tokio::task::spawn_blocking(move || store.add_entity(entity))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn add_relation(&self, relation: Relation) -> FreecoResult<String> {
        let store = self.knowledge.clone();
        tokio::task::spawn_blocking(move || store.add_relation(relation))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn query_graph(&self, pattern: GraphPattern) -> FreecoResult<Vec<GraphMatch>> {
        let store = self.knowledge.clone();
        tokio::task::spawn_blocking(move || store.query_graph(pattern))
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn consolidate(&self) -> FreecoResult<ConsolidationReport> {
        let engine = self.consolidation.clone();
        tokio::task::spawn_blocking(move || engine.consolidate())
            .await
            .map_err(|e| FreecoError::Internal(e.to_string()))?
    }

    async fn export(&self, format: ExportFormat) -> FreecoResult<Vec<u8>> {
        let _ = format;
        Ok(Vec::new())
    }

    async fn import(&self, _data: &[u8], _format: ExportFormat) -> FreecoResult<ImportReport> {
        Ok(ImportReport {
            entities_imported: 0,
            relations_imported: 0,
            memories_imported: 0,
            errors: vec!["Import not yet implemented in Phase 1".to_string()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_substrate_kv() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let agent_id = AgentId::new();
        substrate
            .set(agent_id, "key", serde_json::json!("value"))
            .await
            .unwrap();
        let val = substrate.get(agent_id, "key").await.unwrap();
        assert_eq!(val, Some(serde_json::json!("value")));
    }

    #[tokio::test]
    async fn test_substrate_remember_recall() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let agent_id = AgentId::new();
        substrate
            .remember(
                agent_id,
                "Rust is a great language",
                MemorySource::Conversation,
                "episodic",
                HashMap::new(),
            )
            .await
            .unwrap();
        let results = substrate.recall("Rust", 10, None).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_task_post_and_list() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let id = substrate
            .task_post(
                "Review code",
                "Check the auth module for issues",
                Some("auditor"),
                Some("orchestrator"),
            )
            .await
            .unwrap();
        assert!(!id.is_empty());

        let tasks = substrate.task_list(Some("pending")).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["title"], "Review code");
        assert_eq!(tasks[0]["assigned_to"], "auditor");
        assert_eq!(tasks[0]["status"], "pending");
    }

    #[tokio::test]
    async fn test_task_claim_and_complete() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let task_id = substrate
            .task_post(
                "Audit endpoint",
                "Security audit the /api/login endpoint",
                Some("auditor"),
                None,
            )
            .await
            .unwrap();

        // Claim the task
        let claimed = substrate.task_claim("auditor").await.unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed["id"], task_id);
        assert_eq!(claimed["status"], "in_progress");

        // Complete the task
        substrate
            .task_complete(&task_id, "No vulnerabilities found")
            .await
            .unwrap();

        // Verify it shows as completed
        let tasks = substrate.task_list(Some("completed")).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["result"], "No vulnerabilities found");
    }

    #[tokio::test]
    async fn test_task_claim_empty() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let claimed = substrate.task_claim("nobody").await.unwrap();
        assert!(claimed.is_none());
    }

    /// The rules the Teams board shows: a task waits until what it is
    /// blocked by is done, a team's task goes only to the team, and only the
    /// holder or the poster closes it.
    #[tokio::test]
    async fn team_board_respects_blockers_membership_and_holder() {
        let substrate = MemorySubstrate::open_in_memory(0.1).unwrap();
        let team = substrate.org().create_team("Concierge", None).unwrap();
        substrate
            .org()
            .add_team_member(&team.id, "booker", None)
            .unwrap();

        let research = substrate
            .task_post_on_board("Find hotels", "", None, Some("lead"), Some(&team.id), &[])
            .await
            .unwrap();
        let book = substrate
            .task_post_on_board(
                "Book the best one",
                "",
                None,
                Some("lead"),
                Some(&team.id),
                std::slice::from_ref(&research),
            )
            .await
            .unwrap();
        assert!(substrate
            .task_post_on_board("Bad", "", None, None, None, &["no-such-task".into()])
            .await
            .is_err());

        let board = substrate.task_board(None, Some(&team.id)).await.unwrap();
        assert_eq!(board.len(), 2);
        let waiting = board.iter().find(|t| t["id"] == book.as_str()).unwrap();
        assert_eq!(waiting["blocked"], true);
        assert_eq!(waiting["blocked_by"], serde_json::json!([research]));

        // Not on the team: nothing to claim.
        assert!(substrate.task_claim("outsider").await.unwrap().is_none());
        // On the team: the open task, never the blocked one.
        let first = substrate.task_claim("booker").await.unwrap().unwrap();
        assert_eq!(first["id"], research.as_str());
        assert!(substrate.task_claim("booker").await.unwrap().is_none());

        // Someone else cannot close it; the holder can, and that frees the next.
        assert!(substrate
            .task_complete_by(&research, "three found", Some("outsider"))
            .await
            .is_err());
        let ready = substrate
            .task_complete_by(&research, "three found", Some("booker"))
            .await
            .unwrap();
        assert_eq!(ready, vec![book.clone()]);
        assert!(substrate
            .task_complete_by(&research, "again", Some("booker"))
            .await
            .is_err());
        let next = substrate.task_claim("booker").await.unwrap().unwrap();
        assert_eq!(next["id"], book.as_str());
        assert_eq!(next["team_id"], team.id.as_str());
    }
}

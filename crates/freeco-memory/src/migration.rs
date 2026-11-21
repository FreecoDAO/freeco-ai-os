//! SQLite schema creation and migration.
//!
//! Creates all tables needed by the memory substrate on first boot.

use rusqlite::Connection;

/// Current schema version.
// Must be bumped whenever a migration is added, or the new migration
// never runs on an existing database.
const SCHEMA_VERSION: u32 = 16;

/// Run all migrations to bring the database up to date.
pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    let current_version = get_schema_version(conn);

    if current_version < 1 {
        migrate_v1(conn)?;
    }

    if current_version < 2 {
        migrate_v2(conn)?;
    }

    if current_version < 3 {
        migrate_v3(conn)?;
    }

    if current_version < 4 {
        migrate_v4(conn)?;
    }

    if current_version < 5 {
        migrate_v5(conn)?;
    }

    if current_version < 6 {
        migrate_v6(conn)?;
    }

    if current_version < 7 {
        migrate_v7(conn)?;
    }

    if current_version < 8 {
        migrate_v8(conn)?;
    }

    // v9 and v10 get their own gates. They were originally bundled under
    // `< 8` while SCHEMA_VERSION stayed at 8, which meant any database that
    // had already reached version 8 skipped them forever and silently. That
    // is how `session_archive` came to be missing on live installs: the
    // safety net meant to hold trimmed messages was never created, so
    // compaction had nothing to fall back on. Both migrations guard their own
    // work (IF NOT EXISTS / column_exists), so re-running them is harmless.
    if current_version < 9 {
        migrate_v9(conn)?;
    }

    if current_version < 10 {
        migrate_v10(conn)?;
    }

    if current_version < 11 {
        migrate_v11(conn)?;
    }

    if current_version < 12 {
        migrate_v12(conn)?;
    }

    if current_version < 13 {
        migrate_v13(conn)?;
    }

    if current_version < 14 {
        migrate_v14(conn)?;
    }

    if current_version < 15 {
        migrate_v15(conn)?;
    }

    if current_version < 16 {
        migrate_v16(conn)?;
    }

    set_schema_version(conn, SCHEMA_VERSION)?;
    Ok(())
}

/// Get the current schema version from the database.
fn get_schema_version(conn: &Connection) -> u32 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0)
}

/// Check if a column exists in a table (SQLite has no ADD COLUMN IF NOT EXISTS).
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = rows.filter_map(|r| r.ok()).collect();
    names.iter().any(|n| n == column)
}

/// Set the schema version in the database.
fn set_schema_version(conn: &Connection, version: u32) -> Result<(), rusqlite::Error> {
    conn.pragma_update(None, "user_version", version)
}

/// Version 1: Create all core tables.
fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        -- Agent registry
        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            manifest BLOB NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Session history
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            messages BLOB NOT NULL,
            context_window_tokens INTEGER DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Event log
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            source_agent TEXT NOT NULL,
            target TEXT NOT NULL,
            payload BLOB NOT NULL,
            timestamp TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
        CREATE INDEX IF NOT EXISTS idx_events_source ON events(source_agent);

        -- Key-value store (per-agent)
        CREATE TABLE IF NOT EXISTS kv_store (
            agent_id TEXT NOT NULL,
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (agent_id, key)
        );

        -- Task queue
        CREATE TABLE IF NOT EXISTS task_queue (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            task_type TEXT NOT NULL,
            payload BLOB NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            priority INTEGER NOT NULL DEFAULT 0,
            scheduled_at TEXT,
            created_at TEXT NOT NULL,
            completed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_task_status_priority ON task_queue(status, priority DESC);

        -- Semantic memories
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            content TEXT NOT NULL,
            source TEXT NOT NULL,
            scope TEXT NOT NULL DEFAULT 'episodic',
            confidence REAL NOT NULL DEFAULT 1.0,
            metadata TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            accessed_at TEXT NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
        CREATE INDEX IF NOT EXISTS idx_memories_scope ON memories(scope);

        -- Knowledge graph entities
        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            entity_type TEXT NOT NULL,
            name TEXT NOT NULL,
            properties TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- Knowledge graph relations
        CREATE TABLE IF NOT EXISTS relations (
            id TEXT PRIMARY KEY,
            source_entity TEXT NOT NULL,
            relation_type TEXT NOT NULL,
            target_entity TEXT NOT NULL,
            properties TEXT NOT NULL DEFAULT '{}',
            confidence REAL NOT NULL DEFAULT 1.0,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_entity);
        CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_entity);
        CREATE INDEX IF NOT EXISTS idx_relations_type ON relations(relation_type);

        -- Migration tracking
        CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            description TEXT
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (1, datetime('now'), 'Initial schema');
        ",
    )?;
    Ok(())
}

/// Version 2: Add collaboration columns to task_queue for agent task delegation.
fn migrate_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    // SQLite requires one ALTER TABLE per statement; check before adding
    let cols = [
        ("title", "TEXT DEFAULT ''"),
        ("description", "TEXT DEFAULT ''"),
        ("assigned_to", "TEXT DEFAULT ''"),
        ("created_by", "TEXT DEFAULT ''"),
        ("result", "TEXT DEFAULT ''"),
    ];
    for (name, typedef) in &cols {
        if !column_exists(conn, "task_queue", name) {
            conn.execute(
                &format!("ALTER TABLE task_queue ADD COLUMN {} {}", name, typedef),
                [],
            )?;
        }
    }

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (2, datetime('now'), 'Add collaboration columns to task_queue')",
        [],
    )?;

    Ok(())
}

/// Version 3: Add embedding column to memories table for vector search.
fn migrate_v3(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "memories", "embedding") {
        conn.execute(
            "ALTER TABLE memories ADD COLUMN embedding BLOB DEFAULT NULL",
            [],
        )?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (3, datetime('now'), 'Add embedding column to memories')",
        [],
    )?;
    Ok(())
}

/// Version 4: Add usage_events table for cost tracking and metering.
fn migrate_v4(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS usage_events (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0.0,
            tool_calls INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_usage_agent_time ON usage_events(agent_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage_events(timestamp);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (4, datetime('now'), 'Add usage_events table for cost tracking');
        ",
    )?;
    Ok(())
}

/// Version 5: Add canonical_sessions table for cross-channel persistent memory.
fn migrate_v5(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS canonical_sessions (
            agent_id TEXT PRIMARY KEY,
            messages BLOB NOT NULL,
            compaction_cursor INTEGER NOT NULL DEFAULT 0,
            compacted_summary TEXT,
            updated_at TEXT NOT NULL
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (5, datetime('now'), 'Add canonical_sessions for cross-channel memory');
        ",
    )?;
    Ok(())
}

/// Version 6: Add label column to sessions table.
fn migrate_v6(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Check if column already exists before ALTER (SQLite has no ADD COLUMN IF NOT EXISTS)
    if !column_exists(conn, "sessions", "label") {
        conn.execute("ALTER TABLE sessions ADD COLUMN label TEXT", [])?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (6, datetime('now'), 'Add label column to sessions for human-readable labels')",
        [],
    )?;
    Ok(())
}

/// Version 7: Add paired_devices table for device pairing persistence.
fn migrate_v7(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS paired_devices (
            device_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            platform TEXT NOT NULL,
            paired_at TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            push_token TEXT
        );

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (7, datetime('now'), 'Add paired_devices table for device pairing');
        ",
    )?;
    Ok(())
}

/// Version 8: Add audit_entries table for persistent Merkle audit trail.
fn migrate_v8(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS audit_entries (
            seq INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            action TEXT NOT NULL,
            detail TEXT NOT NULL,
            outcome TEXT NOT NULL,
            prev_hash TEXT NOT NULL,
            hash TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_audit_agent ON audit_entries(agent_id);
        CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_entries(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_entries(action);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (8, datetime('now'), 'Add audit_entries table for persistent Merkle audit trail');
        ",
    )?;
    Ok(())
}

/// Version 9: Add label column to canonical_sessions.
///
/// Agent conversations live in canonical_sessions, not in `sessions` -- the
/// latter is only written during compaction. Labelling only `sessions` meant
/// the auto-generated names never appeared for the conversations users
/// actually have, which is the whole point of naming them.
fn migrate_v9(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "canonical_sessions", "label") {
        conn.execute("ALTER TABLE canonical_sessions ADD COLUMN label TEXT", [])?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) VALUES (9, datetime('now'), 'Add label column to canonical_sessions so agent conversations get names')",
        [],
    )?;
    Ok(())
}

/// Version 10: Archive compacted messages instead of destroying them.
///
/// Compaction used to `split_off` older messages and keep only a lossy
/// summary -- each message cut to 200 characters, the whole summary then
/// clipped to its last 4000. The originals were gone for good, so a user
/// watching their older chats disappear was seeing exactly what was
/// happening. Managing the model's context window is necessary; deleting the
/// user's conversation is not the same thing and must never be a side effect
/// of it. Trimmed messages now land here first and can be read back in full.
fn migrate_v10(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS session_archive (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            message BLOB NOT NULL,
            archived_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_session_archive_agent
            ON session_archive(agent_id, seq);

        INSERT OR IGNORE INTO migrations (version, applied_at, description)
        VALUES (10, datetime('now'), 'Archive compacted messages so chat history is never destroyed');
        ",
    )?;
    Ok(())
}

/// Organisational structure: companies own projects, projects have teams, and
/// conversations belong to one of them.
///
/// Without this a conversation is an orphan. Close the window and its context
/// is gone, the next chat starts from nothing, and there is no way to ask
/// "what has happened on this project?" -- which is the question a business
/// actually needs answered. Attaching sessions to a project is what turns a
/// pile of chats into an organisation's memory.
///
/// `archived` and `trashed` are separate on purpose. Archiving is "done, keep
/// it"; trashing is "wrong, hide it". Collapsing them into one flag means the
/// only way to clear finished work off a list is to make it look deleted.
fn migrate_v11(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS companies (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT,
            archived    INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            company_id  TEXT,
            name        TEXT NOT NULL,
            description TEXT,
            status      TEXT NOT NULL DEFAULT 'active',
            archived    INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            FOREIGN KEY (company_id) REFERENCES companies(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS teams (
            id          TEXT PRIMARY KEY,
            project_id  TEXT,
            name        TEXT NOT NULL,
            description TEXT,
            archived    INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE SET NULL
        );

        -- Which agents belong to which team. An agent can serve several teams,
        -- which is why this is a join table rather than a column on agents.
        CREATE TABLE IF NOT EXISTS team_members (
            team_id    TEXT NOT NULL,
            agent_id   TEXT NOT NULL,
            role       TEXT,
            created_at TEXT NOT NULL,
            PRIMARY KEY (team_id, agent_id),
            FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_projects_company ON projects(company_id);
        CREATE INDEX IF NOT EXISTS idx_teams_project    ON teams(project_id);
        CREATE INDEX IF NOT EXISTS idx_team_members     ON team_members(agent_id);
        ",
    )?;

    // Sessions gain their place in the structure. Added one at a time and
    // guarded, so a partially-migrated database can be re-run safely.
    for (table, column, ddl) in [
        (
            "sessions",
            "project_id",
            "ALTER TABLE sessions ADD COLUMN project_id TEXT",
        ),
        (
            "sessions",
            "team_id",
            "ALTER TABLE sessions ADD COLUMN team_id TEXT",
        ),
        (
            "sessions",
            "kind",
            "ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat'",
        ),
        (
            "sessions",
            "archived",
            "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "sessions",
            "trashed",
            "ALTER TABLE sessions ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !column_exists(conn, table, column) {
            conn.execute(ddl, [])?;
        }
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
         CREATE INDEX IF NOT EXISTS idx_sessions_team    ON sessions(team_id);",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) \
         VALUES (11, datetime('now'), 'Companies, projects, teams; sessions scoped and archivable')",
        [],
    )?;
    Ok(())
}

/// One inbox across every channel.
///
/// Forty-four channel integrations exist - email, Signal, WhatsApp, Telegram,
/// Slack, Matrix, XMPP and the rest - and every message they carried was
/// routed to an agent and then dropped. Nothing was stored, so there was no
/// way to ask "what did this customer say last month?", no way to search
/// across channels, and no record of what an agent said on the company's
/// behalf. The plumbing was complete and the memory was missing.
///
/// `direction` distinguishes what arrived from what was sent, because an
/// audit that cannot tell the two apart cannot answer the question anyone
/// actually asks after an incident.
///
/// `visibility` exists because this table holds both a support queue and
/// someone's private messages. Without it the only options are showing
/// everything to everyone or showing nothing to anyone.
fn migrate_v12(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS inbox_messages (
            id             TEXT PRIMARY KEY,
            channel        TEXT NOT NULL,
            direction      TEXT NOT NULL DEFAULT 'in',
            thread_id      TEXT,
            correspondent  TEXT NOT NULL,
            display_name   TEXT,
            agent_id       TEXT,
            project_id     TEXT,
            body           TEXT NOT NULL,
            visibility     TEXT NOT NULL DEFAULT 'team',
            is_group       INTEGER NOT NULL DEFAULT 0,
            read           INTEGER NOT NULL DEFAULT 0,
            sent_at        TEXT NOT NULL,
            stored_at      TEXT NOT NULL
        );

        -- The inbox view: newest first, which is the only ordering anyone
        -- opens a mailbox expecting.
        CREATE INDEX IF NOT EXISTS idx_inbox_sent      ON inbox_messages(sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_inbox_thread    ON inbox_messages(thread_id, sent_at);
        CREATE INDEX IF NOT EXISTS idx_inbox_person    ON inbox_messages(correspondent, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_inbox_channel   ON inbox_messages(channel, sent_at DESC);
        CREATE INDEX IF NOT EXISTS idx_inbox_project   ON inbox_messages(project_id);

        -- Address book. Separate from inbox_messages so a contact survives
        -- having no messages, and so import and export move one table rather
        -- than reconstructing people from traffic.
        CREATE TABLE IF NOT EXISTS contacts (
            id            TEXT PRIMARY KEY,
            display_name  TEXT NOT NULL,
            organisation  TEXT,
            notes         TEXT,
            -- global, team or private: who may see this entry at all.
            visibility    TEXT NOT NULL DEFAULT 'team',
            owner_user    TEXT,
            archived      INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        );

        -- One person, many handles. A contact reachable on email, Signal and
        -- Telegram is one person, and a schema that cannot say so produces
        -- three half-empty records and a search that misses two of them.
        CREATE TABLE IF NOT EXISTS contact_handles (
            contact_id  TEXT NOT NULL,
            channel     TEXT NOT NULL,
            handle      TEXT NOT NULL,
            is_primary  INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (channel, handle),
            FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_handles_contact ON contact_handles(contact_id);
        CREATE INDEX IF NOT EXISTS idx_contacts_name   ON contacts(display_name);
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) \
         VALUES (12, datetime('now'), 'Unified inbox and address book across all channels')",
        [],
    )?;
    Ok(())
}

/// Calendar, documents and tasks: the three parts of the workspace that had
/// no storage at all.
///
/// Mail, contacts, companies, projects and teams already existed. These three
/// complete the set, and they are deliberately shaped to hang off the same
/// project and team rows rather than forming a parallel world — a task that
/// cannot say which project it belongs to, or a document nobody can attach to
/// a company, is a silo with a nicer name.
///
/// Everything here uses text timestamps in RFC3339, matching every other
/// table in this database. Storing epoch integers would be smaller and would
/// mean two date formats in one file.
fn migrate_v13(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        -- ── Calendar ────────────────────────────────────────────────────
        --
        -- `all_day` is stored rather than inferred from the times, because
        -- 'all day on the 4th' and 'midnight to midnight' are different
        -- claims: one moves with the reader's timezone and the other does
        -- not, and only the author knows which was meant.
        CREATE TABLE IF NOT EXISTS calendar_events (
            id           TEXT PRIMARY KEY,
            title        TEXT NOT NULL,
            description  TEXT,
            location     TEXT,
            starts_at    TEXT NOT NULL,
            ends_at      TEXT NOT NULL,
            all_day      INTEGER NOT NULL DEFAULT 0,
            project_id   TEXT,
            company_id   TEXT,
            agent_id     TEXT,
            -- Where this came from: an agent, a channel, or a person typing.
            source       TEXT NOT NULL DEFAULT 'manual',
            visibility   TEXT NOT NULL DEFAULT 'team',
            status       TEXT NOT NULL DEFAULT 'confirmed',
            archived     INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );

        -- A calendar is read as a window of time, almost never any other way.
        CREATE INDEX IF NOT EXISTS idx_cal_window  ON calendar_events(starts_at, ends_at);
        CREATE INDEX IF NOT EXISTS idx_cal_project ON calendar_events(project_id, starts_at);

        -- Who is expected to attend. Contacts are the address book, so an
        -- attendee points at one rather than repeating a name and an email
        -- that then drift apart.
        CREATE TABLE IF NOT EXISTS calendar_attendees (
            event_id   TEXT NOT NULL,
            contact_id TEXT,
            agent_id   TEXT,
            response   TEXT NOT NULL DEFAULT 'needs_action',
            PRIMARY KEY (event_id, contact_id, agent_id),
            FOREIGN KEY (event_id) REFERENCES calendar_events(id) ON DELETE CASCADE
        );

        -- ── Documents ───────────────────────────────────────────────────
        --
        -- The bytes are not stored here. A database row per file would put
        -- multi-megabyte blobs in the same file as the conversation history
        -- and make every backup carry them; `path` points at the workspace
        -- instead. `sha256` is what lets a later check say whether the file
        -- on disk is still the one that was filed.
        CREATE TABLE IF NOT EXISTS documents (
            id           TEXT PRIMARY KEY,
            title        TEXT NOT NULL,
            path         TEXT NOT NULL,
            mime_type    TEXT,
            size_bytes   INTEGER NOT NULL DEFAULT 0,
            sha256       TEXT,
            project_id   TEXT,
            company_id   TEXT,
            contact_id   TEXT,
            uploaded_by  TEXT,
            summary      TEXT,
            visibility   TEXT NOT NULL DEFAULT 'team',
            archived     INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_docs_project ON documents(project_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_docs_sha     ON documents(sha256);

        -- ── Tasks ───────────────────────────────────────────────────────
        --
        -- Distinct from `task_queue`, which is the agent work queue: that one
        -- holds a msgpack payload and a status a scheduler owns. This one is
        -- what a person or an agent puts on a board and another one picks up.
        --
        -- `assignee_agent` and `assignee_contact` are separate columns
        -- because an agent and a person are not interchangeable: one can be
        -- delegated to automatically, the other has to be asked.
        CREATE TABLE IF NOT EXISTS tasks (
            id               TEXT PRIMARY KEY,
            title            TEXT NOT NULL,
            description      TEXT,
            status           TEXT NOT NULL DEFAULT 'todo',
            priority         INTEGER NOT NULL DEFAULT 2,
            project_id       TEXT,
            team_id          TEXT,
            assignee_agent   TEXT,
            assignee_contact TEXT,
            created_by       TEXT,
            due_at           TEXT,
            -- Ordering within a board column, so a drag that reorders two
            -- cards does not have to rewrite every row.
            board_rank       REAL NOT NULL DEFAULT 0,
            archived         INTEGER NOT NULL DEFAULT 0,
            created_at       TEXT NOT NULL,
            updated_at       TEXT NOT NULL,
            completed_at     TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_tasks_board   ON tasks(project_id, status, board_rank);
        CREATE INDEX IF NOT EXISTS idx_tasks_due     ON tasks(due_at);
        CREATE INDEX IF NOT EXISTS idx_tasks_agent   ON tasks(assignee_agent, status);

        -- A task blocked by another. Kept as a table rather than a column so
        -- a task can wait on several things, which is the normal case.
        CREATE TABLE IF NOT EXISTS task_dependencies (
            task_id     TEXT NOT NULL,
            depends_on  TEXT NOT NULL,
            PRIMARY KEY (task_id, depends_on),
            FOREIGN KEY (task_id)    REFERENCES tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (depends_on) REFERENCES tasks(id) ON DELETE CASCADE
        );
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description)          VALUES (13, datetime('now'), 'Calendar, documents and tasks')",
        [],
    )?;

    Ok(())
}

/// Tenants and users: the row-level answer to "who owns this?".
///
/// Nothing in this database could say that before. There were auth endpoints
/// and a dashboard password, but no table recorded a person, and no row
/// recorded an owner - so every query returned everything to everyone. That is
/// survivable for one person on one laptop and is the whole problem for a
/// hosted service.
///
/// Two separate things, deliberately:
///
/// - a **tenant** is an isolation boundary and owns data. In the cloud it is
///   one customer; locally it is the single install.
/// - a **user** is a person who signs in. Several users can belong to one
///   tenant - a family, a team - which is what the existing role work
///   (owner/admin/user/kid/viewer) is about.
///
/// Both are keyed by a generated id, never by name. Display names change, and
/// history keyed to a mutable string orphans the moment someone renames
/// themselves. That failure has already happened in this project once, to
/// conversation history keyed by an agent id that changed underneath it, and
/// it looked exactly like deletion.
fn migrate_v14(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS tenants (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            -- 'local' for an install on someone's own machine, 'cloud' for a
            -- hosted one. The same schema serves both, which is what makes
            -- syncing between them a reconciliation rather than a translation.
            kind         TEXT NOT NULL DEFAULT 'local',
            status       TEXT NOT NULL DEFAULT 'active',
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS users (
            id            TEXT PRIMARY KEY,
            tenant_id     TEXT NOT NULL,
            -- Display name. Mutable on purpose, and never used as a key.
            name          TEXT NOT NULL,
            email         TEXT,
            role          TEXT NOT NULL DEFAULT 'user',
            password_hash TEXT,
            api_key_hash  TEXT,
            -- Set when the account came from Google, GitHub or similar. Null
            -- for a local password account.
            provider      TEXT,
            provider_sub  TEXT,
            enabled       INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL,
            FOREIGN KEY (tenant_id) REFERENCES tenants(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_users_tenant ON users(tenant_id);
        -- One identity per provider account, so a second sign-in with the same
        -- Google account finds the existing user instead of creating a twin.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_users_provider
            ON users(provider, provider_sub)
            WHERE provider IS NOT NULL;
        ",
    )?;

    // The install that is running now becomes tenant 1, and every existing row
    // is assigned to it. Backfilling with a default is what keeps this
    // migration non-destructive: nothing is orphaned, and a single-user
    // install carries on behaving exactly as it did.
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO tenants (id, name, kind, status, created_at, updated_at) \
         VALUES (?1, 'This install', 'local', 'active', ?2, ?2)",
        rusqlite::params![LOCAL_TENANT_ID, now],
    )?;

    // `ALTER TABLE ... ADD COLUMN` fails if the column is already there, which
    // is the normal case on a database that has run a newer build. Each is
    // attempted independently and a failure ignored, so one already-migrated
    // table cannot abort the rest.
    for statement in [
        r#"ALTER TABLE agents ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE sessions ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE canonical_sessions ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE session_archive ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE companies ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE projects ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE teams ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE team_members ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE inbox_messages ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE contacts ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE contact_handles ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE calendar_events ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE documents ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE tasks ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE audit_entries ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
        r#"ALTER TABLE usage_events ADD COLUMN tenant_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001'"#,
    ] {
        let _ = conn.execute(statement, []);
    }

    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_agents_tenant ON agents(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_tenant ON sessions(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_canonical_sessions_tenant ON canonical_sessions(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_session_archive_tenant ON session_archive(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_companies_tenant ON companies(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_projects_tenant ON projects(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_teams_tenant ON teams(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_team_members_tenant ON team_members(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_inbox_messages_tenant ON inbox_messages(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_contacts_tenant ON contacts(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_contact_handles_tenant ON contact_handles(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_calendar_events_tenant ON calendar_events(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_documents_tenant ON documents(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_tasks_tenant ON tasks(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_audit_entries_tenant ON audit_entries(tenant_id);
        CREATE INDEX IF NOT EXISTS idx_usage_events_tenant ON usage_events(tenant_id);
        ",
    )?;

    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) \
         VALUES (14, datetime('now'), 'Tenants, users, and tenant_id on owned rows')",
        [],
    )?;

    Ok(())
}

/// Version 15: team task boards.
///
/// A task can sit on one team's board, and can wait on other tasks: it is
/// not offered to anyone until every task it is blocked by is completed.
/// Both columns default to "no team" and "waits on nothing", so every task
/// written before this keeps behaving as it did.
fn migrate_v15(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (name, typedef) in [
        ("team_id", "TEXT DEFAULT ''"),
        ("blocked_by", "TEXT DEFAULT '[]'"),
    ] {
        if !column_exists(conn, "task_queue", name) {
            conn.execute(
                &format!("ALTER TABLE task_queue ADD COLUMN {name} {typedef}"),
                [],
            )?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_task_team_status ON task_queue(team_id, status);",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) \
         VALUES (15, datetime('now'), 'Team task boards: team_id and blocked_by on task_queue')",
        [],
    )?;
    Ok(())
}

/// Version 16: companies own teams, people sit on teams, and a board card
/// remembers when it last changed.
///
/// - `teams.company_id`: a team can belong to a company directly. A team
///   inside a project still belongs to that project's company; the direct
///   column is for a team that has no project, which before this belonged to
///   no company at all.
/// - `team_people`: signed-in people (`[[users]]` accounts) on a team, with a
///   team role of `lead`, `member` or `viewer`. Kept apart from
///   `team_members`, which holds agents: an agent on a team is handed work by
///   team runs and claims tasks, a person is not, and mixing the two in one
///   table would put people into every agent query.
/// - `task_queue.updated_at`: set whenever a card is moved, reassigned or
///   edited, so the board can say when anything last happened to it.
///
/// Every step is guarded, so re-running it is harmless.
fn migrate_v16(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (table, column, ddl) in [
        (
            "teams",
            "company_id",
            "ALTER TABLE teams ADD COLUMN company_id TEXT",
        ),
        (
            "task_queue",
            "updated_at",
            "ALTER TABLE task_queue ADD COLUMN updated_at TEXT DEFAULT ''",
        ),
    ] {
        if !column_exists(conn, table, column) {
            conn.execute(ddl, [])?;
        }
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS team_people (
            team_id    TEXT NOT NULL,
            username   TEXT NOT NULL,
            role       TEXT NOT NULL DEFAULT 'member',
            created_at TEXT NOT NULL,
            tenant_id  TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001',
            PRIMARY KEY (team_id, username),
            FOREIGN KEY (team_id) REFERENCES teams(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_team_people_user ON team_people(username);
        CREATE INDEX IF NOT EXISTS idx_teams_company ON teams(company_id);
        ",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at, description) \
         VALUES (16, datetime('now'), 'Teams in companies, people on teams, board card updated_at')",
        [],
    )?;
    Ok(())
}

/// The tenant every pre-existing row belongs to.
///
/// A local install has exactly one, and it keeps the same id across upgrades
/// so that data written before tenancy existed stays reachable afterwards.
pub const LOCAL_TENANT_ID: &str = "00000000-0000-0000-0000-000000000001";

#[cfg(test)]
mod tests {

    /// A database already at an older version must still receive every later
    /// migration. This is the bug that left `session_archive` missing on live
    /// installs: v9 and v10 were gated on `< 8` while SCHEMA_VERSION stayed
    /// at 8, so any database that had reached 8 skipped them permanently and
    /// without complaint. The archive that should have held trimmed messages
    /// was therefore never created.
    #[test]
    fn every_migration_runs_on_an_existing_database() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Simulate an install stuck at the old version, then upgrade again.
        conn.pragma_update(None, "user_version", 8).unwrap();
        run_migrations(&conn).unwrap();

        for table in ["session_archive", "sessions", "canonical_sessions"] {
            let found: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table} must exist after migrating from v8");
        }

        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION, "version must reach the latest");
    }

    /// v16 adds what team boards with people need, and an install that was
    /// already at v15 gets it too.
    #[test]
    fn v16_adds_team_company_people_and_card_timestamps() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.pragma_update(None, "user_version", 15).unwrap();
        run_migrations(&conn).unwrap();
        assert!(column_exists(&conn, "teams", "company_id"));
        assert!(column_exists(&conn, "task_queue", "updated_at"));
        assert!(column_exists(&conn, "team_people", "role"));
    }

    /// Running migrations twice must not fail. Without this, the fix above
    /// would turn a silent skip into a startup crash.
    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
    }

    use super::*;

    #[test]
    fn test_migration_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"agents".to_string()));
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"kv_store".to_string()));
        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"entities".to_string()));
        assert!(tables.contains(&"relations".to_string()));
    }

    #[test]
    fn test_migration_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // Should not error
    }
}

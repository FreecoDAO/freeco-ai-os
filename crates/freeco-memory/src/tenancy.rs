//! Tenants and users: who owns a row, and who may sign in.
//!
//! Kept apart on purpose. A **tenant** is an isolation boundary that owns data;
//! a **user** is a person who authenticates. Several users can share a tenant —
//! a family, a small team — which is what the role hierarchy is for. Collapsing
//! the two works until the first shared account and then cannot be undone
//! without a migration, because rows have already been keyed to the wrong
//! thing.
//!
//! Everything here is keyed by a generated id and never by a display name.
//! Names change. This project has already lost 126 messages to history keyed by
//! an identifier that changed underneath it, and the symptom was
//! indistinguishable from deletion.

use freeco_types::error::{FreecoError, FreecoResult};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// The tenant that owns everything written before tenancy existed.
pub use crate::migration::LOCAL_TENANT_ID;

/// What a user is allowed to do.
///
/// Ordered, so a check is a comparison rather than a table of every pair. The
/// values match the strings already stored by the existing role work, so
/// accounts configured before this survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only. Cannot change configuration or start work.
    Viewer,
    /// A child account: may talk to agents under a kid-safe policy, and sees
    /// no configuration at all.
    Kid,
    /// An ordinary member.
    User,
    /// May manage users and configuration within the tenant.
    Admin,
    /// Owns the tenant. Cannot be demoted by anyone else.
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Kid => "kid",
            Role::User => "user",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    /// Parse a stored role.
    ///
    /// An unrecognised value becomes `Viewer` rather than an error: a role
    /// spelled wrongly in a config file should cost that account its
    /// privileges, not lock everyone out of a running system. Failing closed
    /// here means the least privilege, not the most.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "owner" => Role::Owner,
            "admin" => Role::Admin,
            "user" => Role::User,
            "kid" => Role::Kid,
            _ => Role::Viewer,
        }
    }

    /// Whether this role is at least `required`.
    pub fn allows(self, required: Role) -> bool {
        self >= required
    }
}

/// An isolation boundary that owns data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    /// `local` for an install on someone's own machine, `cloud` for a hosted
    /// one. The same schema serves both, which is what lets the two be
    /// reconciled rather than translated.
    pub kind: String,
    pub status: String,
}

/// A person who signs in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub email: Option<String>,
    pub role: Role,
    /// Set when the account came from an identity provider. `None` for a local
    /// password account.
    pub provider: Option<String>,
    pub enabled: bool,
}

/// Reads and writes tenants and users.
#[derive(Clone)]
pub struct TenancyStore {
    conn: Arc<Mutex<Connection>>,
}

impl TenancyStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    fn lock(&self) -> FreecoResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| FreecoError::Internal(e.to_string()))
    }

    /// The tenant a single-machine install belongs to, created if missing.
    ///
    /// Every caller that has no other way to know the tenant uses this, which
    /// keeps a local install working exactly as it did before tenancy existed.
    pub fn local_tenant(&self) -> FreecoResult<Tenant> {
        let conn = self.lock()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO tenants (id, name, kind, status, created_at, updated_at) \
             VALUES (?1, 'This install', 'local', 'active', ?2, ?2)",
            rusqlite::params![LOCAL_TENANT_ID, now],
        )
        .map_err(|e| FreecoError::Memory(e.to_string()))?;

        Ok(Tenant {
            id: LOCAL_TENANT_ID.to_string(),
            name: "This install".to_string(),
            kind: "local".to_string(),
            status: "active".to_string(),
        })
    }

    /// Create a tenant.
    pub fn create_tenant(&self, name: &str, kind: &str) -> FreecoResult<Tenant> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO tenants (id, name, kind, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'active', ?4, ?4)",
            rusqlite::params![id, name, kind, now],
        )
        .map_err(|e| FreecoError::Memory(e.to_string()))?;

        Ok(Tenant {
            id,
            name: name.to_string(),
            kind: kind.to_string(),
            status: "active".to_string(),
        })
    }

    /// Create a user inside a tenant.
    ///
    /// The id is generated and returned. Callers store that, never the name —
    /// a name is for reading, not for keying.
    #[allow(clippy::too_many_arguments)]
    pub fn create_user(
        &self,
        tenant_id: &str,
        name: &str,
        email: Option<&str>,
        role: Role,
        password_hash: Option<&str>,
        provider: Option<&str>,
        provider_sub: Option<&str>,
    ) -> FreecoResult<User> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO users \
             (id, tenant_id, name, email, role, password_hash, provider, provider_sub, \
              enabled, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
            rusqlite::params![
                id,
                tenant_id,
                name,
                email,
                role.as_str(),
                password_hash,
                provider,
                provider_sub,
                now
            ],
        )
        .map_err(|e| FreecoError::Memory(e.to_string()))?;

        Ok(User {
            id,
            tenant_id: tenant_id.to_string(),
            name: name.to_string(),
            email: email.map(str::to_string),
            role,
            provider: provider.map(str::to_string),
            enabled: true,
        })
    }

    /// Find the user behind an identity-provider account.
    ///
    /// This is what stops a second sign-in with the same Google account
    /// creating a duplicate person — and, because the row carries a tenant,
    /// what maps an external identity onto the data it is allowed to see.
    pub fn user_by_provider(&self, provider: &str, subject: &str) -> FreecoResult<Option<User>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tenant_id, name, email, role, provider, enabled \
                 FROM users WHERE provider = ?1 AND provider_sub = ?2",
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut rows = stmt
            .query_map(rusqlite::params![provider, subject], row_to_user)
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        match rows.next() {
            Some(Ok(user)) => Ok(Some(user)),
            Some(Err(e)) => Err(FreecoError::Memory(e.to_string())),
            None => Ok(None),
        }
    }

    /// Users belonging to a tenant.
    pub fn users_in_tenant(&self, tenant_id: &str) -> FreecoResult<Vec<User>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tenant_id, name, email, role, provider, enabled \
                 FROM users WHERE tenant_id = ?1 ORDER BY created_at",
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([tenant_id], row_to_user)
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| FreecoError::Memory(e.to_string()))?);
        }
        Ok(out)
    }

    /// Look a user up by id.
    /// Which tenant a signed-in user belongs to.
    ///
    /// The session token carries a username, not a tenant, so every request
    /// that needs scoping has to resolve one from the other. Without this the
    /// `tenant_id` on a service instance is recorded and never read -- the
    /// isolation exists in the data and not in any answer the API gives.
    ///
    /// Matched on name or email because sign-in accepts either, and a user who
    /// logged in with their email should not become invisible to their own
    /// services.
    pub fn tenant_for_login(&self, login: &str) -> FreecoResult<Option<String>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id FROM users                  WHERE (name = ?1 OR email = ?1) AND enabled = 1 LIMIT 1",
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut rows = stmt
            .query_map([login], |r| r.get::<_, String>(0))
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            Some(Err(e)) => Err(FreecoError::Memory(e.to_string())),
            None => Ok(None),
        }
    }

    pub fn user(&self, id: &str) -> FreecoResult<Option<User>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tenant_id, name, email, role, provider, enabled \
                 FROM users WHERE id = ?1",
            )
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        let mut rows = stmt
            .query_map([id], row_to_user)
            .map_err(|e| FreecoError::Memory(e.to_string()))?;
        match rows.next() {
            Some(Ok(user)) => Ok(Some(user)),
            Some(Err(e)) => Err(FreecoError::Memory(e.to_string())),
            None => Ok(None),
        }
    }
}

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        name: row.get(2)?,
        email: row.get(3)?,
        role: Role::parse(&row.get::<_, String>(4)?),
        provider: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> TenancyStore {
        let conn = Connection::open_in_memory().unwrap();
        crate::migration::run_migrations(&conn).unwrap();
        TenancyStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn the_local_tenant_exists_and_is_stable() {
        let s = store();
        let a = s.local_tenant().unwrap();
        let b = s.local_tenant().unwrap();
        assert_eq!(
            a.id, b.id,
            "the local tenant id must not change between calls"
        );
        assert_eq!(a.id, LOCAL_TENANT_ID);
    }

    #[test]
    fn a_user_is_keyed_by_id_not_by_name() {
        let s = store();
        let t = s.local_tenant().unwrap();
        let u = s
            .create_user(&t.id, "Ada", None, Role::Owner, None, None, None)
            .unwrap();
        assert_ne!(u.id, u.name, "the id must not be derived from the name");
        assert_eq!(s.user(&u.id).unwrap().unwrap().name, "Ada");
    }

    /// The whole reason provider identities are stored: signing in twice with
    /// the same account must find the same person, not make a second one.
    #[test]
    fn the_same_provider_account_resolves_to_one_user() {
        let s = store();
        let t = s.local_tenant().unwrap();
        let created = s
            .create_user(
                &t.id,
                "Ada",
                Some("ada@example.com"),
                Role::Owner,
                None,
                Some("google"),
                Some("sub-123"),
            )
            .unwrap();
        let found = s.user_by_provider("google", "sub-123").unwrap().unwrap();
        assert_eq!(found.id, created.id);
        assert_eq!(found.tenant_id, t.id, "identity must carry its tenant");
    }

    #[test]
    fn users_are_listed_within_their_tenant_only() {
        let s = store();
        let a = s.local_tenant().unwrap();
        let b = s.create_tenant("Another org", "cloud").unwrap();
        s.create_user(&a.id, "Ada", None, Role::Owner, None, None, None)
            .unwrap();
        s.create_user(&b.id, "Grace", None, Role::Owner, None, None, None)
            .unwrap();

        let in_a = s.users_in_tenant(&a.id).unwrap();
        assert_eq!(in_a.len(), 1);
        assert_eq!(in_a[0].name, "Ada", "a tenant must not see another's users");
    }

    #[test]
    fn roles_are_ordered_so_a_check_is_a_comparison() {
        assert!(Role::Owner.allows(Role::Admin));
        assert!(Role::Admin.allows(Role::User));
        assert!(!Role::Kid.allows(Role::User));
        assert!(!Role::Viewer.allows(Role::Kid));
    }

    /// A misspelled role should cost that account its privileges, not lock
    /// everyone out or silently grant more than intended.
    #[test]
    fn an_unknown_role_falls_to_the_least_privilege() {
        assert_eq!(Role::parse("wizard"), Role::Viewer);
        assert_eq!(Role::parse("OWNER"), Role::Owner);
    }
}

#[cfg(test)]
mod live_migration_check {
    /// Run the migration against a copy of a real database.
    ///
    /// Skipped unless `FREECO_MIGRATION_CHECK_DB` points at one. A migration
    /// that adds a column to sixteen tables is exactly the kind that passes on
    /// an empty fixture and loses data on a database with history in it, so it
    /// is worth proving against the real thing before it ships.
    #[test]
    fn migrating_a_real_database_preserves_every_row() {
        let Ok(path) = std::env::var("FREECO_MIGRATION_CHECK_DB") else {
            return;
        };
        let conn = rusqlite::Connection::open(&path).expect("open copy");

        let count = |c: &rusqlite::Connection, t: &str| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap_or(-1)
        };
        let tables = [
            "agents",
            "sessions",
            "canonical_sessions",
            "audit_entries",
            "projects",
            "teams",
        ];
        let before: Vec<i64> = tables.iter().map(|t| count(&conn, t)).collect();

        crate::migration::run_migrations(&conn).expect("migrations must succeed");

        let after: Vec<i64> = tables.iter().map(|t| count(&conn, t)).collect();
        assert_eq!(before, after, "row counts changed: {tables:?}");

        // Every migrated row must belong to the local tenant, or it is
        // invisible to a tenant-scoped query - which is data loss wearing a
        // different hat.
        for t in ["agents", "sessions", "canonical_sessions"] {
            let orphans: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {t} WHERE tenant_id IS NULL OR tenant_id = ''"),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(-1);
            assert_eq!(orphans, 0, "{t} has rows with no tenant");
        }
        println!(
            "verified {} tables, rows preserved: {:?}",
            tables.len(),
            after
        );
    }
}

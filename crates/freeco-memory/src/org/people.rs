//! Companies as the owners of teams, and people on teams.
//!
//! A company is the organisation unit: it owns projects, and through them or
//! directly it owns teams. (A *tenant* is something else: the isolation
//! boundary of one install or one hosted customer. One tenant holds any number
//! of companies.)
//!
//! A team has two kinds of member. Agents sit in `team_members` and are what a
//! team run hands work to. People, the signed-in `[[users]]` accounts, sit in
//! `team_people` with a [`TeamRole`] that decides what they may do on that
//! team. The two are kept apart because they are not interchangeable: an agent
//! claims tasks by itself, a person has to be asked.

use super::{lock_err, mem_err, now, Company, OrgStore};
use freeco_types::error::{FreecoError, FreecoResult};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A person's role on one team, least to most privileged.
///
/// - `Viewer`: sees the team, its members and its board; changes nothing.
/// - `Member`: may also put tasks on the board, move and assign them.
/// - `Lead`: may also add and remove members and delete tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Viewer,
    Member,
    Lead,
}

impl TeamRole {
    pub const ALL: [TeamRole; 3] = [TeamRole::Viewer, TeamRole::Member, TeamRole::Lead];

    pub fn as_str(self) -> &'static str {
        match self {
            TeamRole::Viewer => "viewer",
            TeamRole::Member => "member",
            TeamRole::Lead => "lead",
        }
    }

    /// Strict parse, case-insensitive: `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        TeamRole::ALL
            .into_iter()
            .find(|r| r.as_str().eq_ignore_ascii_case(s))
    }

    /// A stored value. Unknown text reads as `Viewer`: a role spelled wrongly
    /// costs the person their rights on the team, never grants more.
    fn from_stored(s: &str) -> Self {
        Self::parse(s).unwrap_or(TeamRole::Viewer)
    }
}

/// A person on a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPerson {
    pub team_id: String,
    /// The `[[users]]` account name, as the session carries it.
    pub username: String,
    pub role: TeamRole,
    pub created_at: String,
}

fn company_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Company> {
    Ok(Company {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        archived: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

impl OrgStore {
    // ---- Companies -----------------------------------------------------

    /// One company by id, archived or not.
    pub fn get_company(&self, company_id: &str) -> FreecoResult<Option<Company>> {
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.query_row(
            "SELECT id, name, description, archived, created_at, updated_at
             FROM companies WHERE id = ?1",
            [company_id],
            company_row,
        )
        .optional()
        .map_err(mem_err)
    }

    /// Rename a company, rewrite its description, archive or restore it.
    /// `None` leaves a field as it is; `None` back means no such company.
    pub fn update_company(
        &self,
        company_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        archived: Option<bool>,
    ) -> FreecoResult<Option<Company>> {
        {
            let conn = self.conn.lock().map_err(lock_err)?;
            conn.execute(
                "UPDATE companies SET name = COALESCE(?1, name),
                        description = COALESCE(?2, description),
                        archived = COALESCE(?3, archived), updated_at = ?4
                 WHERE id = ?5",
                rusqlite::params![
                    name,
                    description,
                    archived.map(|a| a as i64),
                    now(),
                    company_id
                ],
            )
            .map_err(mem_err)?;
        }
        self.get_company(company_id)
    }

    /// Delete a company. Its projects and teams stay, with no company: the
    /// work inside a company is not the company, and deleting one must never
    /// take a team's board or a project's history with it. `false` when there
    /// was no such company.
    pub fn delete_company(&self, company_id: &str) -> FreecoResult<bool> {
        let mut conn = self.conn.lock().map_err(lock_err)?;
        let tx = conn.transaction().map_err(mem_err)?;
        tx.execute(
            "UPDATE projects SET company_id = NULL WHERE company_id = ?1",
            [company_id],
        )
        .map_err(mem_err)?;
        tx.execute(
            "UPDATE teams SET company_id = NULL WHERE company_id = ?1",
            [company_id],
        )
        .map_err(mem_err)?;
        let n = tx
            .execute("DELETE FROM companies WHERE id = ?1", [company_id])
            .map_err(mem_err)?;
        tx.commit().map_err(mem_err)?;
        Ok(n > 0)
    }

    // ---- Teams in companies --------------------------------------------

    /// Put a team in a company, or take it out of one with `None`.
    /// `false` when there is no such team; an error when the company does not
    /// exist, so a typo cannot file a team under nothing.
    pub fn set_team_company(&self, team_id: &str, company_id: Option<&str>) -> FreecoResult<bool> {
        if let Some(c) = company_id {
            if self.get_company(c)?.is_none() {
                return Err(FreecoError::InvalidInput(format!("No company with id {c}")));
            }
        }
        let conn = self.conn.lock().map_err(lock_err)?;
        let n = conn
            .execute(
                "UPDATE teams SET company_id = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![company_id, now(), team_id],
            )
            .map_err(mem_err)?;
        Ok(n > 0)
    }

    /// Rename a team, rewrite its description, archive or restore it.
    /// `false` when there is no such team.
    pub fn update_team(
        &self,
        team_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        archived: Option<bool>,
    ) -> FreecoResult<bool> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let n = conn
            .execute(
                "UPDATE teams SET name = COALESCE(?1, name),
                        description = COALESCE(?2, description),
                        archived = COALESCE(?3, archived), updated_at = ?4
                 WHERE id = ?5",
                rusqlite::params![
                    name,
                    description,
                    archived.map(|a| a as i64),
                    now(),
                    team_id
                ],
            )
            .map_err(mem_err)?;
        Ok(n > 0)
    }

    /// The company each team belongs to: its own `company_id`, else its
    /// project's company. Teams in no company are left out of the map.
    pub fn team_companies(&self) -> FreecoResult<HashMap<String, String>> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT t.id, COALESCE(t.company_id, p.company_id)
                 FROM teams t LEFT JOIN projects p ON p.id = t.project_id",
            )
            .map_err(mem_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(mem_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (team, company) = row.map_err(mem_err)?;
            if let Some(c) = company {
                out.insert(team, c);
            }
        }
        Ok(out)
    }

    // ---- People on teams -----------------------------------------------

    /// Put a person on a team with a role, or change the role of one already
    /// on it.
    pub fn add_team_person(
        &self,
        team_id: &str,
        username: &str,
        role: TeamRole,
    ) -> FreecoResult<()> {
        let username = username.trim();
        if username.is_empty() {
            return Err(FreecoError::InvalidInput("A member needs a name".into()));
        }
        let conn = self.conn.lock().map_err(lock_err)?;
        conn.execute(
            "INSERT INTO team_people (team_id, username, role, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (team_id, username) DO UPDATE SET role = excluded.role",
            rusqlite::params![team_id, username, role.as_str(), now()],
        )
        .map_err(mem_err)?;
        Ok(())
    }

    /// Take a person off a team. `false` when they were not on it.
    pub fn remove_team_person(&self, team_id: &str, username: &str) -> FreecoResult<bool> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let n = conn
            .execute(
                "DELETE FROM team_people WHERE team_id = ?1 AND username = ?2",
                rusqlite::params![team_id, username.trim()],
            )
            .map_err(mem_err)?;
        Ok(n > 0)
    }

    /// The people on a team, in the order they joined.
    pub fn team_people(&self, team_id: &str) -> FreecoResult<Vec<TeamPerson>> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT team_id, username, role, created_at FROM team_people
                 WHERE team_id = ?1 ORDER BY created_at, username",
            )
            .map_err(mem_err)?;
        let rows = stmt
            .query_map([team_id], |r| {
                Ok(TeamPerson {
                    team_id: r.get(0)?,
                    username: r.get(1)?,
                    role: TeamRole::from_stored(&r.get::<_, String>(2)?),
                    created_at: r.get(3)?,
                })
            })
            .map_err(mem_err)?;
        rows.collect::<Result<_, _>>().map_err(mem_err)
    }

    /// A person's role on one team, or `None` when they are not on it.
    pub fn team_role_of(&self, team_id: &str, username: &str) -> FreecoResult<Option<TeamRole>> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let role: Option<String> = conn
            .query_row(
                "SELECT role FROM team_people WHERE team_id = ?1 AND username = ?2",
                rusqlite::params![team_id, username],
                |r| r.get(0),
            )
            .optional()
            .map_err(mem_err)?;
        Ok(role.as_deref().map(TeamRole::from_stored))
    }

    /// Every team a person is on, with their role there.
    pub fn teams_for_person(&self, username: &str) -> FreecoResult<Vec<(String, TeamRole)>> {
        let conn = self.conn.lock().map_err(lock_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT team_id, role FROM team_people WHERE username = ?1
                 ORDER BY created_at, team_id",
            )
            .map_err(mem_err)?;
        let rows = stmt
            .query_map([username], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    TeamRole::from_stored(&r.get::<_, String>(1)?),
                ))
            })
            .map_err(mem_err)?;
        rows.collect::<Result<_, _>>().map_err(mem_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn store() -> OrgStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        OrgStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn team_roles_parse_strictly_and_are_ordered() {
        assert_eq!(TeamRole::parse(" LEAD "), Some(TeamRole::Lead));
        assert_eq!(TeamRole::parse("boss"), None);
        assert!(TeamRole::Lead > TeamRole::Member && TeamRole::Member > TeamRole::Viewer);
        assert_eq!(TeamRole::from_stored("garbage"), TeamRole::Viewer);
        for r in TeamRole::ALL {
            assert_eq!(TeamRole::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn a_company_can_be_read_renamed_archived_and_deleted() {
        let s = store();
        let c = s.create_company("FreEco AG", None).unwrap();
        assert_eq!(s.get_company(&c.id).unwrap().unwrap().name, "FreEco AG");

        let renamed = s
            .update_company(&c.id, Some("FreEco GmbH"), Some("Zurich"), None)
            .unwrap()
            .unwrap();
        assert_eq!(renamed.name, "FreEco GmbH");
        assert_eq!(renamed.description.as_deref(), Some("Zurich"));
        assert!(!renamed.archived);

        let archived = s
            .update_company(&c.id, None, None, Some(true))
            .unwrap()
            .unwrap();
        assert!(archived.archived);
        assert_eq!(archived.name, "FreEco GmbH", "an absent name is kept");
        assert!(s.list_companies(false).unwrap().is_empty());
        assert_eq!(s.list_companies(true).unwrap().len(), 1);

        assert!(s
            .update_company("missing", Some("x"), None, None)
            .unwrap()
            .is_none());
        assert!(s.delete_company(&c.id).unwrap());
        assert!(!s.delete_company(&c.id).unwrap());
        assert!(s.get_company(&c.id).unwrap().is_none());
    }

    /// Deleting a company leaves its projects and teams in place, in no
    /// company, rather than taking boards and history with it.
    #[test]
    fn deleting_a_company_keeps_its_projects_and_teams() {
        let s = store();
        let c = s.create_company("Acme", None).unwrap();
        let p = s.create_project("Launch", Some(&c.id), None).unwrap();
        let in_project = s.create_team("Platform", Some(&p.id)).unwrap();
        let direct = s.create_team("Support", None).unwrap();
        assert!(s.set_team_company(&direct.id, Some(&c.id)).unwrap());

        let map = s.team_companies().unwrap();
        assert_eq!(map.get(&in_project.id), Some(&c.id), "via its project");
        assert_eq!(map.get(&direct.id), Some(&c.id), "directly");

        assert!(s.delete_company(&c.id).unwrap());
        assert!(s.team_companies().unwrap().is_empty());
        assert_eq!(s.list_projects(true).unwrap().len(), 1);
        assert_eq!(s.list_teams(true).unwrap().len(), 2);
    }

    #[test]
    fn a_team_cannot_be_filed_under_a_missing_company() {
        let s = store();
        let t = s.create_team("Ops", None).unwrap();
        assert!(s.set_team_company(&t.id, Some("nope")).is_err());
        assert!(!s.set_team_company("no-team", None).unwrap());
        assert!(s.set_team_company(&t.id, None).unwrap());
    }

    #[test]
    fn a_team_can_be_renamed_and_archived() {
        let s = store();
        let t = s.create_team("Ops", None).unwrap();
        assert!(s
            .update_team(&t.id, Some("Operations"), Some("on call"), None)
            .unwrap());
        let team = s.get_team(&t.id).unwrap().unwrap();
        assert_eq!(team["name"], "Operations");
        assert_eq!(team["description"], "on call");
        assert!(s.update_team(&t.id, None, None, Some(true)).unwrap());
        assert!(s.list_teams(false).unwrap().is_empty());
        assert!(!s.update_team("missing", Some("x"), None, None).unwrap());
    }

    #[test]
    fn people_join_change_role_and_leave_a_team() {
        let s = store();
        let a = s.create_team("Alpha", None).unwrap();
        let b = s.create_team("Beta", None).unwrap();
        s.add_team_person(&a.id, "uma", TeamRole::Lead).unwrap();
        s.add_team_person(&a.id, "kim", TeamRole::Viewer).unwrap();
        s.add_team_person(&b.id, "uma", TeamRole::Member).unwrap();
        assert!(s.add_team_person(&a.id, "  ", TeamRole::Member).is_err());

        assert_eq!(s.team_role_of(&a.id, "uma").unwrap(), Some(TeamRole::Lead));
        assert_eq!(s.team_role_of(&a.id, "nobody").unwrap(), None);
        assert_eq!(
            s.teams_for_person("uma").unwrap(),
            vec![
                (a.id.clone(), TeamRole::Lead),
                (b.id.clone(), TeamRole::Member)
            ]
        );

        // Adding again changes the role, it does not add a second row.
        s.add_team_person(&a.id, "kim", TeamRole::Member).unwrap();
        let people = s.team_people(&a.id).unwrap();
        assert_eq!(people.len(), 2);
        assert_eq!(
            s.team_role_of(&a.id, "kim").unwrap(),
            Some(TeamRole::Member)
        );

        assert!(s.remove_team_person(&a.id, "kim").unwrap());
        assert!(!s.remove_team_person(&a.id, "kim").unwrap());
        assert_eq!(s.team_people(&a.id).unwrap().len(), 1);

        // People are not agents: the agent membership of the team is untouched.
        assert!(s.get_team(&a.id).unwrap().unwrap()["members"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

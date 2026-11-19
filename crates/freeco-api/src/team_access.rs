//! Who may see and change which team.
//!
//! One rule, used by every team route and meant for anything else that scopes
//! by team (which agents and channels a person may see, for example):
//!
//! - An **owner** or **admin** account manages every team and every company.
//! - Anyone else reaches only the teams they are on (`team_people`), with
//!   what their [`TeamRole`] there allows:
//!   - `lead` manages the team: members, the board, deleting cards;
//!   - `member` works on the board: posts, moves and assigns cards;
//!   - `viewer` reads the team, its members and its board.
//! - A `viewer` or `kid` *account* changes nothing anywhere, so on a team it
//!   only ever reads, whatever its team role says.
//! - Someone on no team sees no team.
//!
//! Membership is by account name (`Caller::username`). The local user (login
//! off) and an API-key holder are the owner, so they reach everything.
//!
//! For the chat-chamber work: [`teams_visible_to`] and
//! [`team_agents_visible_to`] are the helpers to build `visible_agents` on;
//! there is no `access.rs` on this branch yet, so they live here.

use crate::middleware::Caller;
use axum::{http::StatusCode, Json};
use freeco_memory::org::{OrgStore, TeamRole};
use freeco_types::auth::UserRole;
use freeco_types::error::FreecoResult;
use serde::Serialize;
use std::collections::HashSet;

/// What a caller may do on one team, least to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamAccess {
    /// See the team, its members and its board.
    Read,
    /// Also post, move and assign cards.
    Work,
    /// Also add and remove members and delete cards.
    Manage,
}

impl TeamAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            TeamAccess::Read => "read",
            TeamAccess::Work => "work",
            TeamAccess::Manage => "manage",
        }
    }
}

/// Owner and admin accounts manage every team.
pub fn manages_all_teams(caller: &Caller) -> bool {
    caller.has(UserRole::Admin)
}

/// The rule itself, with no store: what an account with `account_role` and
/// `team_role` on a team (`None`: not on it) may do there.
pub fn access_for(account_role: UserRole, team_role: Option<TeamRole>) -> Option<TeamAccess> {
    if account_role >= UserRole::Admin {
        return Some(TeamAccess::Manage);
    }
    let team_role = team_role?;
    if account_role < UserRole::User {
        return Some(TeamAccess::Read);
    }
    Some(match team_role {
        TeamRole::Lead => TeamAccess::Manage,
        TeamRole::Member => TeamAccess::Work,
        TeamRole::Viewer => TeamAccess::Read,
    })
}

/// The caller's own role on a team, or `None` when they are not on it.
///
/// This is membership only: an admin who is on no team gets `None` here and
/// still manages the team; use [`caller_team_access`] to decide anything.
pub fn caller_team_role(
    org: &OrgStore,
    caller: &Caller,
    team_id: &str,
) -> FreecoResult<Option<TeamRole>> {
    match caller.username.as_deref() {
        Some(user) => org.team_role_of(team_id, user),
        None => Ok(None),
    }
}

/// What the caller may do on a team, or `None` when they may not see it.
pub fn caller_team_access(
    org: &OrgStore,
    caller: &Caller,
    team_id: &str,
) -> FreecoResult<Option<TeamAccess>> {
    if manages_all_teams(caller) {
        return Ok(Some(TeamAccess::Manage));
    }
    Ok(access_for(
        caller.role,
        caller_team_role(org, caller, team_id)?,
    ))
}

/// The ids of every team the caller may see: all of them for an owner or
/// admin (archived included), otherwise the teams they are on.
pub fn teams_visible_to(org: &OrgStore, caller: &Caller) -> FreecoResult<Vec<String>> {
    if manages_all_teams(caller) {
        return Ok(org
            .list_teams(true)?
            .iter()
            .filter_map(|t| t["id"].as_str().map(String::from))
            .collect());
    }
    match caller.username.as_deref() {
        Some(user) => Ok(org
            .teams_for_person(user)?
            .into_iter()
            .map(|(team, _)| team)
            .collect()),
        None => Ok(Vec::new()),
    }
}

/// The agents the caller reaches through teams: every agent on a team they
/// may see. `None` means every agent (owner and admin).
pub fn team_agents_visible_to(
    org: &OrgStore,
    caller: &Caller,
) -> FreecoResult<Option<HashSet<String>>> {
    if manages_all_teams(caller) {
        return Ok(None);
    }
    let visible: HashSet<String> = teams_visible_to(org, caller)?.into_iter().collect();
    let mut agents = HashSet::new();
    for team in org.list_teams(true)? {
        if !team["id"].as_str().is_some_and(|id| visible.contains(id)) {
            continue;
        }
        for m in team["members"].as_array().into_iter().flatten() {
            if let Some(a) = m["agent_id"].as_str() {
                agents.insert(a.to_string());
            }
        }
    }
    Ok(Some(agents))
}

/// The companies the caller may see: `None` for all (owner and admin),
/// otherwise the companies of the teams they are on.
pub fn companies_visible_to(
    org: &OrgStore,
    caller: &Caller,
) -> FreecoResult<Option<HashSet<String>>> {
    if manages_all_teams(caller) {
        return Ok(None);
    }
    let companies = org.team_companies()?;
    Ok(Some(
        teams_visible_to(org, caller)?
            .iter()
            .filter_map(|t| companies.get(t).cloned())
            .collect(),
    ))
}

/// A refusal in the shape every team route returns.
pub type Denied = (StatusCode, Json<serde_json::Value>);

/// `Ok` with the caller's access when it is at least `min`; otherwise the
/// 403 to send (or a 500 when the store failed).
pub fn require_team_access(
    org: &OrgStore,
    caller: &Caller,
    team_id: &str,
    min: TeamAccess,
) -> Result<TeamAccess, Denied> {
    let deny = |msg: &str| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": msg })),
        )
    };
    match caller_team_access(org, caller, team_id) {
        Ok(Some(access)) if access >= min => Ok(access),
        Ok(Some(_)) => Err(deny(match min {
            TeamAccess::Manage => "Only the team's lead or an admin can do that.",
            _ => "Your role on this team lets you read it, not change it.",
        })),
        Ok(None) => Err(deny("You are not on this team.")),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::AuthMethod;
    use freeco_memory::MemorySubstrate;

    fn session(user: &str, role: UserRole) -> Caller {
        Caller {
            username: Some(user.to_string()),
            role,
            method: AuthMethod::Session,
        }
    }

    /// The whole matrix: account role across, team role down.
    #[test]
    fn the_permission_matrix() {
        use TeamAccess::*;
        let cases: [(UserRole, Option<TeamRole>, Option<TeamAccess>); 13] = [
            // Owner and admin manage every team, on it or not.
            (UserRole::Owner, None, Some(Manage)),
            (UserRole::Admin, None, Some(Manage)),
            (UserRole::Admin, Some(TeamRole::Viewer), Some(Manage)),
            // A user account gets what its team role says.
            (UserRole::User, Some(TeamRole::Lead), Some(Manage)),
            (UserRole::User, Some(TeamRole::Member), Some(Work)),
            (UserRole::User, Some(TeamRole::Viewer), Some(Read)),
            // Outsiders see nothing.
            (UserRole::User, None, None),
            (UserRole::Kid, None, None),
            (UserRole::Viewer, None, None),
            // Viewer and kid accounts only ever read, even as a lead.
            (UserRole::Viewer, Some(TeamRole::Lead), Some(Read)),
            (UserRole::Viewer, Some(TeamRole::Member), Some(Read)),
            (UserRole::Kid, Some(TeamRole::Lead), Some(Read)),
            (UserRole::Kid, Some(TeamRole::Viewer), Some(Read)),
        ];
        for (account, team, want) in cases {
            assert_eq!(access_for(account, team), want, "{account:?} as {team:?}");
        }
    }

    #[test]
    fn visibility_follows_membership() {
        let mem = MemorySubstrate::open_in_memory(0.1).unwrap();
        let org = mem.org();
        let acme = org.create_company("Acme", None).unwrap();
        let globex = org.create_company("Globex", None).unwrap();
        let web = org.create_team("Web", None).unwrap();
        let ops = org.create_team("Ops", None).unwrap();
        org.set_team_company(&web.id, Some(&acme.id)).unwrap();
        org.set_team_company(&ops.id, Some(&globex.id)).unwrap();
        org.add_team_member(&web.id, "agent-web", None).unwrap();
        org.add_team_member(&ops.id, "agent-ops", None).unwrap();
        org.add_team_person(&web.id, "lena", TeamRole::Lead)
            .unwrap();
        org.add_team_person(&web.id, "mo", TeamRole::Member)
            .unwrap();

        let owner = Caller {
            username: None,
            role: UserRole::Owner,
            method: AuthMethod::Local,
        };
        let admin = session("alice", UserRole::Admin);
        let lead = session("lena", UserRole::User);
        let member = session("mo", UserRole::User);
        let outsider = session("otto", UserRole::User);

        let mut all = teams_visible_to(org, &owner).unwrap();
        all.sort();
        let mut both = vec![web.id.clone(), ops.id.clone()];
        both.sort();
        assert_eq!(all, both);
        assert_eq!(teams_visible_to(org, &admin).unwrap().len(), 2);
        assert_eq!(teams_visible_to(org, &lead).unwrap(), vec![web.id.clone()]);
        assert_eq!(
            teams_visible_to(org, &member).unwrap(),
            vec![web.id.clone()]
        );
        assert!(teams_visible_to(org, &outsider).unwrap().is_empty());

        assert_eq!(
            caller_team_access(org, &lead, &web.id).unwrap(),
            Some(TeamAccess::Manage)
        );
        assert_eq!(
            caller_team_access(org, &member, &web.id).unwrap(),
            Some(TeamAccess::Work)
        );
        assert_eq!(caller_team_access(org, &lead, &ops.id).unwrap(), None);
        assert_eq!(caller_team_access(org, &outsider, &web.id).unwrap(), None);
        assert_eq!(
            caller_team_access(org, &admin, &ops.id).unwrap(),
            Some(TeamAccess::Manage)
        );
        assert_eq!(
            caller_team_role(org, &lead, &web.id).unwrap(),
            Some(TeamRole::Lead)
        );
        assert_eq!(caller_team_role(org, &admin, &web.id).unwrap(), None);

        assert_eq!(team_agents_visible_to(org, &admin).unwrap(), None);
        assert_eq!(
            team_agents_visible_to(org, &member).unwrap(),
            Some(HashSet::from(["agent-web".to_string()]))
        );
        assert_eq!(
            team_agents_visible_to(org, &outsider).unwrap(),
            Some(HashSet::new())
        );
        assert_eq!(companies_visible_to(org, &owner).unwrap(), None);
        assert_eq!(
            companies_visible_to(org, &lead).unwrap(),
            Some(HashSet::from([acme.id.clone()]))
        );

        assert!(require_team_access(org, &member, &web.id, TeamAccess::Work).is_ok());
        let (code, _) = require_team_access(org, &member, &web.id, TeamAccess::Manage).unwrap_err();
        assert_eq!(code, StatusCode::FORBIDDEN);
        let (code, _) = require_team_access(org, &outsider, &web.id, TeamAccess::Read).unwrap_err();
        assert_eq!(code, StatusCode::FORBIDDEN);
    }
}

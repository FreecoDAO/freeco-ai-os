//! Who a signed-in person is allowed to be.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The role of a FreEco.ai account, least to most privileged.
///
/// This is the one role model for the whole system: `[[users]]` in
/// `config.toml` names it, the kernel's channel RBAC (`AuthManager`) and the
/// guest boundary compare against it, dashboard sessions carry it in their
/// signed cookie, and the API's auth middleware enforces it on every change.
/// Variants are ordered, so `role >= UserRole::Admin` reads as "admin or more".
///
/// - `Viewer`: may read, may change nothing.
/// - `Kid`: may chat (with a kid-safe agent); no config, spawn, kill or billing.
/// - `User`: may chat with agents.
/// - `Admin`: may also spawn and kill agents, install skills, change channels,
///   workflows, budgets and everything else that is not owner-only.
/// - `Owner`: full access, including managing accounts and the system config.
///
/// The local install with dashboard login off, and any caller holding the API
/// key, act as the owner.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    /// Read-only access — can view agent output but cannot interact.
    #[default]
    Viewer = 0,
    /// Child account — can chat with a kid-safe agent, but no config, no
    /// spawn/kill, no billing. The basis for the Kids edition.
    Kid = 1,
    /// Standard user — can chat with agents.
    User = 2,
    /// Admin — can spawn/kill agents, install skills, view usage.
    Admin = 3,
    /// Owner — full access including user management and config changes.
    Owner = 4,
}

impl UserRole {
    /// Every role, least to most privileged.
    pub const ALL: [UserRole; 5] = [
        UserRole::Viewer,
        UserRole::Kid,
        UserRole::User,
        UserRole::Admin,
        UserRole::Owner,
    ];

    /// The lowercase name used in config, cookies and API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            UserRole::Viewer => "viewer",
            UserRole::Kid => "kid",
            UserRole::User => "user",
            UserRole::Admin => "admin",
            UserRole::Owner => "owner",
        }
    }

    /// Strict parse: `None` for anything that is not exactly a role name
    /// (case-insensitive). Use this for anything a client could have sent.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        UserRole::ALL
            .into_iter()
            .find(|r| r.as_str().eq_ignore_ascii_case(s))
    }

    /// Lenient parse for a `[[users]]` entry's `role`: an unknown value is a
    /// standard `User`, which is what `UserConfig`'s default role has always
    /// meant.
    pub fn from_str_role(s: &str) -> Self {
        Self::parse(s).unwrap_or(UserRole::User)
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered_least_to_most_privileged() {
        let mut sorted = UserRole::ALL;
        sorted.sort();
        assert_eq!(sorted, UserRole::ALL);
        assert!(UserRole::Owner > UserRole::Admin);
        assert!(UserRole::Kid > UserRole::Viewer);
    }

    #[test]
    fn strict_parse_rejects_unknown_names() {
        assert_eq!(UserRole::parse("ADMIN"), Some(UserRole::Admin));
        assert_eq!(UserRole::parse("root"), None);
        assert_eq!(UserRole::parse(""), None);
        for r in UserRole::ALL {
            assert_eq!(UserRole::parse(r.as_str()), Some(r));
            assert_eq!(r.to_string(), r.as_str());
        }
    }

    #[test]
    fn lenient_parse_keeps_the_config_default() {
        assert_eq!(UserRole::from_str_role("owner"), UserRole::Owner);
        assert_eq!(UserRole::from_str_role("unknown"), UserRole::User);
    }

    #[test]
    fn serde_uses_lowercase_names() {
        assert_eq!(
            serde_json::to_string(&UserRole::Admin).unwrap(),
            "\"admin\""
        );
        let r: UserRole = serde_json::from_str("\"kid\"").unwrap();
        assert_eq!(r, UserRole::Kid);
    }
}

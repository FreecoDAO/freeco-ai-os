//! Who may see which agent, conversation and channel: the one place that
//! decides.
//!
//! Every route that lists or opens conversations (the Chamber, its live
//! stream) and the WebSocket chat ask here. Nothing else filters on its own,
//! so changing the policy is changing this file.
//!
//! # Default policy (proposed; the owner may change it)
//!
//! | Caller            | Agents                | Agent chats            | External channels (WhatsApp, Telegram, email, ...) | May write |
//! |-------------------|-----------------------|------------------------|----------------------------------------------------|-----------|
//! | owner, admin      | all                   | all                    | all                                                | yes       |
//! | user              | granted               | of granted agents      | granted channels, of granted agents                | yes       |
//! | kid               | granted               | of granted agents      | none                                               | yes, agent chats only |
//! | viewer            | granted               | of granted agents      | granted channels, of granted agents                | no        |
//!
//! Grants are `agents = [...]` and `channels = [...]` on the caller's
//! `[[users]]` entry (`"*"` means all), plus whatever [`team_grants`] adds.
//! A caller with no grants sees nothing but their own conversations.
//!
//! On top of the table, in this order:
//! 1. **Tenants.** A signed-in caller sees only conversations of their own
//!    tenant (the install's local tenant when they have none). This holds for
//!    every role: an admin of one tenant is a guest to every other. Only the
//!    install's operator -- the local user with login off, or an API-key
//!    holder -- spans tenants.
//! 2. **The owner's own channel conversations.** A conversation whose peer is
//!    bound to an owner or admin account (the owner's own WhatsApp, say) is
//!    not a guest's: it carries the owner's private context, so only owners
//!    and admins see it. This is the guest rule of `freeco_kernel::guest`
//!    (`guest_for`), applied to reading.
//! 3. **Your own conversations.** A channel conversation whose peer is one of
//!    the caller's own `channel_bindings` is always theirs to see (and, from
//!    kid up, to write in), whatever the grants.
//!
//! Something a caller may not see does not exist for them: routes answer 404,
//! never 403, so a hidden conversation's existence does not leak.
//!
//! Agent chats (dashboard conversations) record no author, so everyone
//! granted an agent sees all of that agent's chats. See
//! `docs/chamber-access.md` for the questions this leaves for the owner.

use crate::middleware::{AuthMethod, Caller};
use freeco_types::agent::AgentEntry;
use freeco_types::auth::UserRole;
use freeco_types::config::UserConfig;
use serde::Serialize;

/// Channel names a conversation label `channel:user_id` can start with.
///
/// A label of that form with any other prefix is an agent chat that happens
/// to contain a colon.
pub const KNOWN_CHANNELS: &[&str] = &[
    "telegram",
    "whatsapp",
    "email",
    "discord",
    "slack",
    "signal",
    "matrix",
    "teams",
    "mattermost",
    "webchat",
    "irc",
    "google_chat",
    "twitch",
    "rocketchat",
    "zulip",
    "xmpp",
    "line",
    "viber",
    "messenger",
    "reddit",
    "mastodon",
    "bluesky",
    "feishu",
    "revolt",
    "nextcloud",
    "guilded",
    "keybase",
    "threema",
    "nostr",
    "webex",
    "pumble",
    "flock",
    "twist",
    "mumble",
    "dingtalk",
    "discourse",
    "gitter",
    "ntfy",
    "gotify",
    "webhook",
    "linkedin",
    "wecom",
    "mqtt",
];

/// What a caller has been granted beyond their role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    /// Agent names or ids; `"*"` is every agent.
    pub agents: Vec<String>,
    /// Channel names; `"*"` is every channel.
    pub channels: Vec<String>,
}

impl Grants {
    fn extend(&mut self, other: Grants) {
        self.agents.extend(other.agents);
        self.channels.extend(other.channels);
    }
}

/// HOOK(b-teams): what the caller's team memberships grant.
///
/// Returns nothing for now. The team-membership thread (b-teams) fills this
/// in from its membership helper; [`Access::new`] already adds whatever it
/// returns to the caller's `[[users]]` grants, so nothing else changes.
pub fn team_grants(_caller: &Caller) -> Grants {
    Grants::default()
}

/// What kind of conversation a session is, read from its label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationKind {
    /// A chat with an agent from the dashboard, CLI or API.
    Agent,
    /// One person's conversation on an external channel.
    Channel {
        /// `whatsapp`, `telegram`, `email`, ...
        channel: String,
        /// Their id on that channel: a phone number, chat id, address.
        peer: String,
    },
}

impl ConversationKind {
    /// The channel, for a channel conversation.
    pub fn channel(&self) -> Option<&str> {
        match self {
            ConversationKind::Agent => None,
            ConversationKind::Channel { channel, .. } => Some(channel),
        }
    }

    /// `channel:peer`: the identity `[[users]] channel_bindings` and the
    /// guest rules use. `None` for an agent chat.
    pub fn peer_key(&self) -> Option<String> {
        match self {
            ConversationKind::Agent => None,
            ConversationKind::Channel { channel, peer } => Some(format!("{channel}:{peer}")),
        }
    }
}

/// Read a session label.
///
/// - `channel:user_id` with a [`KNOWN_CHANNELS`] prefix: the channel bridge's
///   per-contact conversations (`conversation_for`).
/// - `WhatsApp +41...` / `WhatsApp group <jid>`: the WhatsApp Web gateway's.
/// - a bare `+<digits>`: a phone number sent as `sender_id` without a channel,
///   which only the WhatsApp gateway does.
/// - anything else, or no label: an agent chat.
pub fn classify(label: Option<&str>) -> ConversationKind {
    let Some(label) = label.map(str::trim).filter(|l| !l.is_empty()) else {
        return ConversationKind::Agent;
    };
    let channel = |channel: &str, peer: &str| ConversationKind::Channel {
        channel: channel.to_string(),
        peer: peer.trim().to_string(),
    };
    if let Some(rest) = label.strip_prefix("WhatsApp ") {
        let peer = rest.strip_prefix("group ").unwrap_or(rest);
        if !peer.trim().is_empty() {
            return channel("whatsapp", peer);
        }
    }
    if let Some((prefix, peer)) = label.split_once(':') {
        if !peer.trim().is_empty() && KNOWN_CHANNELS.contains(&prefix) {
            return channel(prefix, peer);
        }
    }
    if let Some(digits) = label.strip_prefix('+') {
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return channel("whatsapp", label);
        }
    }
    ConversationKind::Agent
}

/// What [`Access`] needs to know about one conversation.
#[derive(Debug, Clone)]
pub struct ConversationRef<'a> {
    /// The agent's id.
    pub agent_id: &'a str,
    /// The agent's name.
    pub agent_name: &'a str,
    /// The tenant that owns the conversation.
    pub tenant_id: &'a str,
    /// What kind it is.
    pub kind: &'a ConversationKind,
    /// The peer is bound to an owner or admin account, so this is not a
    /// guest's conversation (see the module doc, rule 2). Always `false` for
    /// an agent chat.
    pub peer_is_operator: bool,
}

/// One caller's view: their role, tenant and grants, resolved once per
/// request.
#[derive(Debug, Clone)]
pub struct Access {
    /// Who is asking.
    pub caller: Caller,
    /// The tenant they are confined to; `None` for the install's operator,
    /// who spans tenants.
    tenant: Option<String>,
    grants: Grants,
    /// The caller's own channel identities, `channel:id`.
    own_keys: Vec<String>,
}

impl Access {
    /// Build a caller's view from what is already known about them.
    ///
    /// `tenant` is the caller's tenant (ignored for the local user and an
    /// API-key holder, who span tenants; the local tenant for a session that
    /// has none). Grants and channel identities come from the caller's
    /// enabled `[[users]]` entry, plus [`team_grants`].
    pub fn new(caller: Caller, tenant: Option<String>, users: &[UserConfig]) -> Self {
        let tenant = match caller.method {
            AuthMethod::Local | AuthMethod::ApiKey => None,
            AuthMethod::Session => Some(
                tenant.unwrap_or_else(|| freeco_memory::migration::LOCAL_TENANT_ID.to_string()),
            ),
        };
        let account = caller
            .username
            .as_deref()
            .and_then(|name| users.iter().find(|u| u.enabled && u.name == name));
        let mut grants = account
            .map(|u| Grants {
                agents: u.agents.clone(),
                channels: u.channels.clone(),
            })
            .unwrap_or_default();
        grants.extend(team_grants(&caller));
        let own_keys = account
            .map(|u| {
                u.channel_bindings
                    .iter()
                    .map(|(channel, id)| format!("{channel}:{id}"))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            caller,
            tenant,
            grants,
            own_keys,
        }
    }

    /// Resolve a caller's view against the running kernel: their tenant from
    /// the tenancy store, their grants from the config.
    pub fn resolve(kernel: &freeco_kernel::FreecoKernel, caller: &Caller) -> Self {
        let tenant = match (caller.method, caller.username.as_deref()) {
            (AuthMethod::Session, Some(login)) => kernel
                .memory
                .tenancy()
                .tenant_for_login(login)
                .ok()
                .flatten(),
            _ => None,
        };
        Self::new(caller.clone(), tenant, &kernel.config.users)
    }

    fn is_admin(&self) -> bool {
        self.caller.role >= UserRole::Admin
    }

    /// Whether the caller may see this agent at all.
    pub fn can_see_agent(&self, agent_id: &str, agent_name: &str) -> bool {
        self.is_admin()
            || self.grants.agents.iter().any(|g| {
                let g = g.trim();
                g == "*" || g == agent_id || g.eq_ignore_ascii_case(agent_name)
            })
    }

    /// The agents of `agents` the caller may see.
    pub fn visible_agents(&self, agents: Vec<AgentEntry>) -> Vec<AgentEntry> {
        self.retain_visible(agents, |a| (a.id.to_string(), a.name.clone()))
    }

    /// The items the caller may see, each named by its agent's id and name.
    pub fn retain_visible<T>(
        &self,
        items: Vec<T>,
        agent: impl Fn(&T) -> (String, String),
    ) -> Vec<T> {
        items
            .into_iter()
            .filter(|item| {
                let (id, name) = agent(item);
                self.can_see_agent(&id, &name)
            })
            .collect()
    }

    /// Whether the caller may chat with this agent: see it, and be at least
    /// a kid (a viewer only reads).
    pub fn can_chat_with_agent(&self, agent_id: &str, agent_name: &str) -> bool {
        self.caller.role >= UserRole::Kid && self.can_see_agent(agent_id, agent_name)
    }

    /// Whether the caller may see conversations on this external channel.
    pub fn can_see_channel(&self, channel: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        if self.caller.role == UserRole::Kid {
            return false;
        }
        self.grants.channels.iter().any(|g| {
            let g = g.trim();
            g == "*" || g.eq_ignore_ascii_case(channel)
        })
    }

    /// Whether the caller may see this conversation. See the module doc.
    pub fn can_see_conversation(&self, conv: &ConversationRef<'_>) -> bool {
        if let Some(tenant) = &self.tenant {
            if conv.tenant_id != tenant {
                return false;
            }
        }
        if self.is_admin() {
            return true;
        }
        match conv.kind {
            ConversationKind::Agent => self.can_see_agent(conv.agent_id, conv.agent_name),
            ConversationKind::Channel { channel, .. } => {
                if self.is_own(conv.kind) {
                    return true;
                }
                !conv.peer_is_operator
                    && self.can_see_channel(channel)
                    && self.can_see_agent(conv.agent_id, conv.agent_name)
            }
        }
    }

    /// Whether the caller may write in this conversation: see it and be at
    /// least a kid. A kid writes only in agent chats and their own
    /// conversations, which [`can_see_conversation`](Self::can_see_conversation)
    /// already limits them to.
    pub fn can_write_conversation(&self, conv: &ConversationRef<'_>) -> bool {
        self.caller.role >= UserRole::Kid && self.can_see_conversation(conv)
    }

    fn is_own(&self, kind: &ConversationKind) -> bool {
        kind.peer_key()
            .is_some_and(|key| self.own_keys.contains(&key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeco_memory::migration::LOCAL_TENANT_ID;
    use std::collections::HashMap;

    const AGENT_ID: &str = "8f0c2a5e-0000-4000-8000-000000000001";
    const OTHER_TENANT: &str = "tenant-b";

    fn session(name: &str, role: UserRole) -> Caller {
        Caller {
            username: Some(name.to_string()),
            role,
            method: AuthMethod::Session,
        }
    }

    fn account(name: &str, role: &str, agents: &[&str], channels: &[&str]) -> UserConfig {
        UserConfig {
            name: name.to_string(),
            role: role.to_string(),
            agents: agents.iter().map(|s| s.to_string()).collect(),
            channels: channels.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// Every non-admin account is granted the agent `assistant` and the
    /// channels `whatsapp`, `email` and `telegram`, so the matrix shows what
    /// the role alone decides.
    fn users() -> Vec<UserConfig> {
        let all = ["whatsapp", "email", "telegram"];
        let mut bound = account("uma", "user", &["assistant"], &all);
        bound.channel_bindings =
            HashMap::from([("telegram".to_string(), "uma-telegram".to_string())]);
        vec![
            account("alice", "admin", &[], &[]),
            bound,
            account("kim", "kid", &["assistant"], &all),
            account("val", "viewer", &["assistant"], &all),
            account("gus", "user", &["assistant"], &all),
        ]
    }

    fn agent_chat() -> ConversationKind {
        classify(Some("Planning the trip"))
    }

    fn channel(c: &str) -> ConversationKind {
        classify(Some(&format!("{c}:+41790000000")))
    }

    fn conv(kind: &ConversationKind) -> ConversationRef<'_> {
        ConversationRef {
            agent_id: AGENT_ID,
            agent_name: "assistant",
            tenant_id: LOCAL_TENANT_ID,
            kind,
            peer_is_operator: false,
        }
    }

    #[test]
    fn labels_are_read_as_channels_or_agent_chats() {
        assert_eq!(classify(None), ConversationKind::Agent);
        assert_eq!(classify(Some("  ")), ConversationKind::Agent);
        assert_eq!(classify(Some("Fix the login")), ConversationKind::Agent);
        assert_eq!(classify(Some("work:notes")), ConversationKind::Agent);
        assert_eq!(classify(Some("telegram:")), ConversationKind::Agent);
        assert_eq!(
            classify(Some("telegram:42")),
            ConversationKind::Channel {
                channel: "telegram".into(),
                peer: "42".into()
            }
        );
        assert_eq!(
            classify(Some("email:bob@example.com"))
                .peer_key()
                .as_deref(),
            Some("email:bob@example.com")
        );
        assert_eq!(
            classify(Some("WhatsApp +41790000000"))
                .peer_key()
                .as_deref(),
            Some("whatsapp:+41790000000")
        );
        assert_eq!(
            classify(Some("WhatsApp group 1203@g.us")).channel(),
            Some("whatsapp")
        );
        assert_eq!(classify(Some("+41790000000")).channel(), Some("whatsapp"));
        assert_eq!(classify(Some("+41 not a number")), ConversationKind::Agent);
    }

    /// The policy matrix: who sees and who may write in an agent chat and a
    /// WhatsApp, email and Telegram conversation, all granted, same tenant.
    #[test]
    fn policy_matrix_by_role() {
        let users = users();
        let local_owner = Caller {
            username: None,
            role: UserRole::Owner,
            method: AuthMethod::Local,
        };
        // (caller, sees agent chat, sees external channels, may write)
        let rows: Vec<(Caller, bool, bool, bool)> = vec![
            (local_owner, true, true, true),
            (session("admin", UserRole::Owner), true, true, true),
            (session("alice", UserRole::Admin), true, true, true),
            (session("gus", UserRole::User), true, true, true),
            (session("kim", UserRole::Kid), true, false, true),
            (session("val", UserRole::Viewer), true, true, false),
        ];
        let agent = agent_chat();
        let channels = [channel("whatsapp"), channel("email"), channel("telegram")];
        for (caller, sees_chat, sees_channels, writes) in rows {
            let who = format!("{:?}", caller.username);
            let access = Access::new(caller, None, &users);
            assert_eq!(
                access.can_see_conversation(&conv(&agent)),
                sees_chat,
                "{who} chat"
            );
            assert_eq!(
                access.can_write_conversation(&conv(&agent)),
                sees_chat && writes,
                "{who} writes chat"
            );
            for c in &channels {
                let name = c.channel().unwrap();
                assert_eq!(access.can_see_channel(name), sees_channels, "{who} {name}");
                assert_eq!(
                    access.can_see_conversation(&conv(c)),
                    sees_channels,
                    "{who} sees {name}"
                );
                assert_eq!(
                    access.can_write_conversation(&conv(c)),
                    sees_channels && writes,
                    "{who} writes {name}"
                );
            }
        }
    }

    /// A guest from another tenant -- even one with an admin role there --
    /// sees nothing of this tenant; the install's operator sees both.
    #[test]
    fn a_caller_of_another_tenant_sees_nothing_here() {
        let users = users();
        let agent = agent_chat();
        let wa = channel("whatsapp");
        for (name, role) in [("alice", UserRole::Admin), ("gus", UserRole::User)] {
            let guest = Access::new(session(name, role), Some(OTHER_TENANT.into()), &users);
            assert!(!guest.can_see_conversation(&conv(&agent)), "{name}");
            assert!(!guest.can_see_conversation(&conv(&wa)), "{name}");
            let mut theirs = conv(&agent);
            theirs.tenant_id = OTHER_TENANT;
            assert!(
                guest.can_see_conversation(&theirs),
                "{name} sees own tenant"
            );
        }
        // A session with no tenant record is in the local tenant.
        let local = Access::new(session("alice", UserRole::Admin), None, &users);
        let mut other = conv(&agent);
        other.tenant_id = OTHER_TENANT;
        assert!(!local.can_see_conversation(&other));
        // The operator (API key) spans tenants.
        let operator = Access::new(
            Caller {
                username: None,
                role: UserRole::Owner,
                method: AuthMethod::ApiKey,
            },
            Some(LOCAL_TENANT_ID.into()),
            &users,
        );
        assert!(operator.can_see_conversation(&other));
    }

    #[test]
    fn without_grants_a_user_sees_nothing() {
        let users = vec![account("nora", "user", &[], &[])];
        let access = Access::new(session("nora", UserRole::User), None, &users);
        assert!(!access.can_see_agent(AGENT_ID, "assistant"));
        assert!(!access.can_see_conversation(&conv(&agent_chat())));
        assert!(!access.can_see_channel("whatsapp"));
        assert!(!access.can_see_conversation(&conv(&channel("whatsapp"))));
        // A signed-in name with no [[users]] entry has no grants either.
        let stranger = Access::new(session("nobody", UserRole::User), None, &users);
        assert!(!stranger.can_see_agent(AGENT_ID, "assistant"));
    }

    #[test]
    fn grants_need_both_the_channel_and_the_agent() {
        let users = vec![
            account("wa", "user", &["assistant"], &["whatsapp"]),
            account("other", "user", &["coder"], &["*"]),
            account("star", "user", &["*"], &["*"]),
        ];
        let wa = Access::new(session("wa", UserRole::User), None, &users);
        assert!(wa.can_see_conversation(&conv(&channel("whatsapp"))));
        assert!(!wa.can_see_conversation(&conv(&channel("email"))));
        assert!(wa.can_see_agent(AGENT_ID, "Assistant"), "names ignore case");
        let other = Access::new(session("other", UserRole::User), None, &users);
        assert!(!other.can_see_conversation(&conv(&channel("whatsapp"))));
        assert!(!other.can_see_conversation(&conv(&agent_chat())));
        let star = Access::new(session("star", UserRole::User), None, &users);
        assert!(star.can_see_conversation(&conv(&channel("telegram"))));
        let by_id = vec![account("id", "user", &[AGENT_ID], &[])];
        let by_id = Access::new(session("id", UserRole::User), None, &by_id);
        assert!(by_id.can_see_conversation(&conv(&agent_chat())));
    }

    /// The owner's own WhatsApp conversation is not a guest's: a user granted
    /// WhatsApp does not see it; an admin does.
    #[test]
    fn the_owners_own_channel_conversation_stays_with_admins() {
        let users = users();
        let wa = channel("whatsapp");
        let mut owners = conv(&wa);
        owners.peer_is_operator = true;
        let gus = Access::new(session("gus", UserRole::User), None, &users);
        assert!(!gus.can_see_conversation(&owners));
        let alice = Access::new(session("alice", UserRole::Admin), None, &users);
        assert!(alice.can_see_conversation(&owners));
    }

    #[test]
    fn your_own_channel_conversation_is_yours_without_a_grant() {
        let mut uma = account("uma", "user", &[], &[]);
        uma.channel_bindings =
            HashMap::from([("telegram".to_string(), "uma-telegram".to_string())]);
        let users = vec![uma];
        let access = Access::new(session("uma", UserRole::User), None, &users);
        let mine = classify(Some("telegram:uma-telegram"));
        let theirs = classify(Some("telegram:someone-else"));
        assert!(access.can_see_conversation(&conv(&mine)));
        assert!(access.can_write_conversation(&conv(&mine)));
        assert!(!access.can_see_conversation(&conv(&theirs)));
    }

    #[test]
    fn a_disabled_account_has_no_grants() {
        let mut off = account("off", "user", &["*"], &["*"]);
        off.enabled = false;
        let access = Access::new(session("off", UserRole::User), None, &[off]);
        assert!(!access.can_see_agent(AGENT_ID, "assistant"));
    }

    #[test]
    fn visible_agents_and_chat_follow_the_grants() {
        let users = users();
        let agents = vec![
            (AGENT_ID.to_string(), "assistant".to_string()),
            (
                "8f0c2a5e-0000-4000-8000-000000000002".to_string(),
                "coder".to_string(),
            ),
        ];
        let gus = Access::new(session("gus", UserRole::User), None, &users);
        let seen = gus.retain_visible(agents.clone(), |a| a.clone());
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, "assistant");
        let alice = Access::new(session("alice", UserRole::Admin), None, &users);
        assert_eq!(alice.retain_visible(agents, |a| a.clone()).len(), 2);

        let val = Access::new(session("val", UserRole::Viewer), None, &users);
        assert!(val.can_see_agent(AGENT_ID, "assistant"));
        assert!(!val.can_chat_with_agent(AGENT_ID, "assistant"));
        let kim = Access::new(session("kim", UserRole::Kid), None, &users);
        assert!(kim.can_chat_with_agent(AGENT_ID, "assistant"));
        assert!(!kim.can_chat_with_agent("8f0c2a5e-0000-4000-8000-000000000002", "coder"));
    }

    #[test]
    fn the_team_hook_grants_nothing_yet() {
        assert_eq!(
            team_grants(&session("gus", UserRole::User)),
            Grants::default()
        );
    }
}

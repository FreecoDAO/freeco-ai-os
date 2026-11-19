//! Who a WhatsApp agent may talk to: one allowlist, shared by every check.
//!
//! The list comes from `[channels.whatsapp]` in config.toml (`allowed_users`
//! plus `owner_number`). The daemon passes it to the Node gateway as
//! `WHATSAPP_ALLOWED_NUMBERS` / `WHATSAPP_ALLOWED_GROUPS` ([`WhatsAppAllowlist::numbers_env`],
//! [`WhatsAppAllowlist::groups_env`]); the gateway drops everything else before
//! forwarding, and the daemon checks the same list again on receipt and on
//! every outbound send. The gateway's `allowlist.js` implements the same
//! normalisation as this module; the two must stay in step.
//!
//! Safe default: an empty list answers nobody except `owner_number`, if one is
//! set. With neither, every chat is refused, inbound and outbound. There is no
//! wildcard: allowed chats are listed one by one.
//!
//! Canonical forms, which are what the list holds and what is compared:
//! - a phone number: `+` and its digits in international form, `+41791234567`
//!   (from `+41 79 123 45 67`, `0041791234567`, `41791234567@s.whatsapp.net`,
//!   `41791234567:3@s.whatsapp.net` or `…@c.us`);
//! - a group: `<id>@g.us`;
//! - a WhatsApp privacy id: `<digits>@lid` (only matches if listed as such).
//!
//! A number in national form (`079 123 45 67`, one leading zero) has no
//! country code and cannot be normalised safely, so it is rejected, never
//! guessed.

/// Shortest and longest digit count accepted for a phone number (E.164 allows
/// at most 15 digits).
const MIN_DIGITS: usize = 6;
const MAX_DIGITS: usize = 15;

/// A phone number in canonical form (`+<digits>`), or `None` if `raw` is not
/// a usable international number.
///
/// Accepts `+`, spaces, `-`, `.`, `(`, `)` and `/` as decoration, a leading
/// `00` as the international prefix, and a user JID (`…@s.whatsapp.net`,
/// `…@c.us`, with or without a `:device` suffix).
pub fn normalize_number(raw: &str) -> Option<String> {
    let s = raw.trim();
    let local = match s.split_once('@') {
        Some((local, domain)) => {
            let domain = domain.trim().to_ascii_lowercase();
            if domain != "s.whatsapp.net" && domain != "c.us" {
                return None;
            }
            strip_device(local)
        }
        None => s,
    };
    let local = local.trim();
    // `+41…` and `41…` (how WhatsApp itself writes numbers) mean the same.
    let rest = local.strip_prefix('+').unwrap_or(local);
    if rest
        .chars()
        .any(|c| !(c.is_ascii_digit() || matches!(c, ' ' | '-' | '.' | '(' | ')' | '/')))
    {
        return None;
    }
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let digits = match digits.strip_prefix("00") {
        Some(international) => international.to_string(),
        None => digits,
    };
    // One leading zero is a national trunk prefix (or a "+0…" typo): the
    // country code is missing, so the number cannot be compared safely.
    if digits.starts_with('0') {
        return None;
    }
    if digits.len() < MIN_DIGITS || digits.len() > MAX_DIGITS {
        return None;
    }
    Some(format!("+{digits}"))
}

/// The canonical form of any WhatsApp chat id or allowlist entry: a number
/// (`+41…`), a group (`…@g.us`) or a privacy id (`…@lid`). `None` when it is
/// none of these.
pub fn normalize_chat(raw: &str) -> Option<String> {
    let s = raw.trim();
    match s.split_once('@') {
        Some((local, domain)) => match domain.trim().to_ascii_lowercase().as_str() {
            "g.us" => {
                let id: String = local.chars().filter(|c| !c.is_whitespace()).collect();
                let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_digit() || c == '-');
                valid.then(|| format!("{id}@g.us"))
            }
            "lid" => {
                let id = strip_device(local).trim();
                let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_digit());
                valid.then(|| format!("{id}@lid"))
            }
            _ => normalize_number(s),
        },
        None => normalize_number(s),
    }
}

/// `41791234567:3` → `41791234567`: a JID can name one of the account's devices.
fn strip_device(local: &str) -> &str {
    local.split(':').next().unwrap_or(local)
}

/// The chats a WhatsApp agent may receive from and send to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhatsAppAllowlist {
    /// Canonical entries, in config order, without repeats.
    entries: Vec<String>,
    /// The owner's own number, canonical, when the config names one.
    owner: Option<String>,
    /// Config entries that could not be normalised (and so allow nothing).
    rejected: Vec<String>,
}

impl WhatsAppAllowlist {
    /// The list from `[channels.whatsapp] allowed_users` and `owner_number`.
    pub fn from_config(allowed_users: &[String], owner_number: Option<&str>) -> Self {
        let mut list = Self::default();
        if let Some(owner) = owner_number.map(str::trim).filter(|o| !o.is_empty()) {
            match normalize_number(owner) {
                Some(n) => {
                    list.owner = Some(n.clone());
                    list.push(n);
                }
                None => list.rejected.push(owner.to_string()),
            }
        }
        for raw in allowed_users
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
        {
            match normalize_chat(raw) {
                Some(canonical) => list.push(canonical),
                None => list.rejected.push(raw.to_string()),
            }
        }
        list
    }

    fn push(&mut self, canonical: String) {
        if !self.entries.contains(&canonical) {
            self.entries.push(canonical);
        }
    }

    /// Whether the chat `chat_id` (a number, user JID, group JID or `@lid`)
    /// is on the list. Anything that does not normalise is refused.
    pub fn allows(&self, chat_id: &str) -> bool {
        normalize_chat(chat_id).is_some_and(|c| self.entries.contains(&c))
    }

    /// Nothing is allowed at all (no entries and no owner number).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The owner's number, canonical, if configured.
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// Allowed direct chats: numbers (the owner's included) and privacy ids.
    pub fn numbers(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.ends_with("@g.us"))
            .map(String::as_str)
            .collect()
    }

    /// Allowed groups.
    pub fn groups(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.ends_with("@g.us"))
            .map(String::as_str)
            .collect()
    }

    /// Config entries that were not understood and therefore allow nothing.
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    /// The value of the gateway's `WHATSAPP_ALLOWED_NUMBERS`.
    pub fn numbers_env(&self) -> String {
        self.numbers().join(",")
    }

    /// The value of the gateway's `WHATSAPP_ALLOWED_GROUPS`.
    pub fn groups_env(&self) -> String {
        self.groups().join(",")
    }

    /// Why a send to `chat_id` was refused, phrased for the agent and the owner.
    pub fn refusal(&self, chat_id: &str) -> String {
        if self.is_empty() {
            format!(
                "WhatsApp chat '{chat_id}' is not allowed: the WhatsApp allowlist is empty, so no \
                 chat is allowed. Add the number or group to [channels.whatsapp] allowed_users \
                 (or set owner_number) in config.toml."
            )
        } else {
            format!(
                "WhatsApp chat '{chat_id}' is not on the allowlist ([channels.whatsapp] \
                 allowed_users in config.toml), so nothing was sent to it."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_normalise_from_every_written_form() {
        for raw in [
            "+41791234567",
            "+41 79 123 45 67",
            "0041 79 123 45 67",
            "41791234567",
            "+41 (79) 123-45.67",
            "41791234567@s.whatsapp.net",
            "41791234567:12@s.whatsapp.net",
            "41791234567@C.US",
            "  +41791234567  ",
        ] {
            assert_eq!(
                normalize_number(raw).as_deref(),
                Some("+41791234567"),
                "{raw}"
            );
        }
    }

    #[test]
    fn numbers_that_cannot_be_compared_safely_are_rejected() {
        for raw in [
            "",
            "+",
            "079 123 45 67", // national form, no country code
            "+0791234567",
            "000791234567",
            "12345",             // too short
            "+1234567890123456", // too long
            "abc",
            "+41 79 abc",
            "120363000000000000@g.us", // a group, not a number
            "41791234567@example.com",
        ] {
            assert_eq!(normalize_number(raw), None, "{raw}");
        }
    }

    #[test]
    fn chats_normalise_to_number_group_or_privacy_id() {
        assert_eq!(
            normalize_chat("120363000000000000@g.us").as_deref(),
            Some("120363000000000000@g.us")
        );
        assert_eq!(
            normalize_chat(" 41791234567-1600000000@G.US ").as_deref(),
            Some("41791234567-1600000000@g.us")
        );
        assert_eq!(
            normalize_chat("123456789012345@lid").as_deref(),
            Some("123456789012345@lid")
        );
        assert_eq!(
            normalize_chat("123456789012345:4@lid").as_deref(),
            Some("123456789012345@lid")
        );
        assert_eq!(
            normalize_chat("41791234567@s.whatsapp.net").as_deref(),
            Some("+41791234567")
        );
        assert_eq!(normalize_chat("@g.us"), None);
        assert_eq!(normalize_chat("abc@g.us"), None);
        assert_eq!(normalize_chat("status@broadcast"), None);
        assert_eq!(normalize_chat("*"), None);
    }

    #[test]
    fn allowlist_matches_every_form_of_a_listed_chat() {
        let list = WhatsAppAllowlist::from_config(
            &[
                "+41 79 123 45 67".to_string(),
                "120363000000000000@g.us".to_string(),
                "+41 79 123 45 67".to_string(), // repeated
            ],
            None,
        );
        assert!(list.allows("41791234567@s.whatsapp.net"));
        assert!(list.allows("0041791234567"));
        assert!(list.allows("+41791234567"));
        assert!(list.allows("120363000000000000@g.us"));
        assert!(!list.allows("+41790000000"));
        assert!(!list.allows("120363999999999999@g.us"));
        assert!(!list.allows("garbage"));
        assert_eq!(list.numbers_env(), "+41791234567");
        assert_eq!(list.groups_env(), "120363000000000000@g.us");
    }

    #[test]
    fn empty_allowlist_denies_all_except_the_owner() {
        let nobody = WhatsAppAllowlist::from_config(&[], None);
        assert!(nobody.is_empty());
        assert!(!nobody.allows("+41791234567"));
        assert!(!nobody.allows("120363000000000000@g.us"));
        assert_eq!(nobody.numbers_env(), "");
        assert!(nobody.refusal("+41791234567").contains("empty"));

        let owner_only = WhatsAppAllowlist::from_config(&[], Some("+41 79 123 45 67"));
        assert!(!owner_only.is_empty());
        assert_eq!(owner_only.owner(), Some("+41791234567"));
        assert!(owner_only.allows("41791234567@s.whatsapp.net"));
        assert!(!owner_only.allows("+41790000000"));
        assert!(!owner_only.allows("120363000000000000@g.us"));
        assert_eq!(owner_only.numbers_env(), "+41791234567");
    }

    #[test]
    fn entries_that_do_not_normalise_allow_nothing_and_are_reported() {
        let list = WhatsAppAllowlist::from_config(
            &[
                "*".to_string(),
                "079 123 45 67".to_string(),
                "  ".to_string(),
            ],
            Some("not a number"),
        );
        assert!(list.is_empty());
        assert_eq!(list.owner(), None);
        assert_eq!(
            list.rejected(),
            &[
                "not a number".to_string(),
                "*".to_string(),
                "079 123 45 67".to_string()
            ]
        );
        assert!(!list.allows("+41791234567"));
    }
}

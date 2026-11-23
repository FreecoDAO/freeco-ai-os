//! Compile-time embedded Hand definitions.

use crate::{parse_hand_toml, HandDefinition, HandError};

/// Returns all bundled hand definitions as (id, HAND.toml content, SKILL.md content).
pub fn bundled_hands() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "clip",
            include_str!("../bundled/clip/HAND.toml"),
            include_str!("../bundled/clip/SKILL.md"),
        ),
        (
            "lead",
            include_str!("../bundled/lead/HAND.toml"),
            include_str!("../bundled/lead/SKILL.md"),
        ),
        (
            "collector",
            include_str!("../bundled/collector/HAND.toml"),
            include_str!("../bundled/collector/SKILL.md"),
        ),
        (
            "predictor",
            include_str!("../bundled/predictor/HAND.toml"),
            include_str!("../bundled/predictor/SKILL.md"),
        ),
        (
            "researcher",
            include_str!("../bundled/researcher/HAND.toml"),
            include_str!("../bundled/researcher/SKILL.md"),
        ),
        (
            "twitter",
            include_str!("../bundled/twitter/HAND.toml"),
            include_str!("../bundled/twitter/SKILL.md"),
        ),
        (
            "browser",
            include_str!("../bundled/browser/HAND.toml"),
            include_str!("../bundled/browser/SKILL.md"),
        ),
        (
            "trader",
            include_str!("../bundled/trader/HAND.toml"),
            include_str!("../bundled/trader/SKILL.md"),
        ),
        (
            "infisical-sync",
            include_str!("../bundled/infisical-sync/HAND.toml"),
            include_str!("../bundled/infisical-sync/SKILL.md"),
        ),
        (
            "omniroute",
            include_str!("../bundled/omniroute/HAND.toml"),
            include_str!("../bundled/omniroute/SKILL.md"),
        ),
        (
            "speaches",
            include_str!("../bundled/speaches/HAND.toml"),
            include_str!("../bundled/speaches/SKILL.md"),
        ),
        // Service hands that are stacks rather than single containers: an
        // application plus its database, brought up with compose.
        (
            "crm",
            include_str!("../bundled/crm/HAND.toml"),
            include_str!("../bundled/crm/SKILL.md"),
        ),
        (
            "accounting",
            include_str!("../bundled/accounting/HAND.toml"),
            include_str!("../bundled/accounting/SKILL.md"),
        ),
        (
            "voice",
            include_str!("../bundled/voice/HAND.toml"),
            include_str!("../bundled/voice/SKILL.md"),
        ),
    ]
}

/// Parse a bundled HAND.toml into a HandDefinition with its skill content attached.
pub fn parse_bundled(
    _id: &str,
    toml_content: &str,
    skill_content: &str,
) -> Result<HandDefinition, HandError> {
    let mut def: HandDefinition =
        parse_hand_toml(toml_content).map_err(|e| HandError::TomlParse(e.to_string()))?;
    if !skill_content.is_empty() {
        def.skill_content = Some(skill_content.to_string());
    }
    Ok(def)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_hands_not_empty() {
        let hands = bundled_hands();
        assert!(!hands.is_empty());
        assert_eq!(hands[0].0, "clip");
    }

    #[test]
    fn bundled_hands_count() {
        // Not a fixed number: that broke on every addition while telling us
        // nothing. What matters is that each entry has an id and content.
        let hands = bundled_hands();
        assert!(hands.len() >= 9, "bundled hand list looks truncated");
        for (id, toml, skill) in hands {
            assert!(!id.is_empty(), "a hand with no id cannot be activated");
            assert!(!toml.is_empty(), "{id} has an empty HAND.toml");
            assert!(!skill.is_empty(), "{id} has an empty SKILL.md");
        }
    }

    #[test]
    fn parse_clip_hand() {
        let hands = bundled_hands();
        let (id, toml_content, skill_content) = hands[0];
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "clip");
        assert_eq!(def.name, "Clip Hand");
        assert_eq!(def.category, crate::HandCategory::Content);
        assert!(def.skill_content.is_some());
        assert!(!def.requires.is_empty());
        assert!(!def.tools.is_empty());
        assert!(!def.agent.system_prompt.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
    }

    #[test]
    fn parse_lead_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "lead")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "lead");
        assert_eq!(def.name, "Lead Hand");
        assert_eq!(def.category, crate::HandCategory::Data);
        assert!(def.skill_content.is_some());
        assert!(def.requires.is_empty());
        assert!(!def.tools.is_empty());
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert!(def.agent.temperature < 0.5);
    }

    #[test]
    fn parse_collector_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "collector")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "collector");
        assert_eq!(def.name, "Collector Hand");
        assert_eq!(def.category, crate::HandCategory::Data);
        assert!(def.skill_content.is_some());
        assert!(def.requires.is_empty());
        assert!(def.tools.contains(&"event_publish".to_string()));
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
    }

    #[test]
    fn parse_predictor_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "predictor")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "predictor");
        assert_eq!(def.name, "Predictor Hand");
        assert_eq!(def.category, crate::HandCategory::Data);
        assert!(def.skill_content.is_some());
        assert!(def.requires.is_empty());
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert!((def.agent.temperature - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_researcher_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "researcher")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "researcher");
        assert_eq!(def.name, "Researcher Hand");
        assert_eq!(def.category, crate::HandCategory::Productivity);
        assert!(def.skill_content.is_some());
        assert!(def.requires.is_empty());
        assert!(def.tools.contains(&"event_publish".to_string()));
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert_eq!(def.agent.max_iterations, Some(25));
    }

    #[test]
    fn parse_twitter_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "twitter")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "twitter");
        assert_eq!(def.name, "Twitter Hand");
        assert_eq!(def.category, crate::HandCategory::Communication);
        assert!(def.skill_content.is_some());
        assert!(!def.requires.is_empty()); // requires TWITTER_BEARER_TOKEN
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert!((def.agent.temperature - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_browser_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "browser")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "browser");
        assert_eq!(def.name, "Browser Hand");
        assert_eq!(def.category, crate::HandCategory::Productivity);
        assert!(def.skill_content.is_some());
        assert!(!def.requires.is_empty()); // requires python3 + chromium
        assert_eq!(def.requires.len(), 2);
        assert!(def.tools.contains(&"browser_navigate".to_string()));
        assert!(def.tools.contains(&"browser_click".to_string()));
        assert!(def.tools.contains(&"browser_type".to_string()));
        assert!(def.tools.contains(&"browser_screenshot".to_string()));
        assert!(def.tools.contains(&"browser_read_page".to_string()));
        assert!(def.tools.contains(&"browser_close".to_string()));
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert!((def.agent.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(def.agent.max_iterations, Some(60));
    }

    #[test]
    fn parse_trader_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "trader")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "trader");
        assert_eq!(def.name, "Trading Hand");
        assert_eq!(def.category, crate::HandCategory::Data);
        assert!(def.skill_content.is_some());
        assert!(def.requires.is_empty()); // no hard requirements
        assert!(!def.tools.is_empty());
        assert!(def.tools.contains(&"event_publish".to_string()));
        assert!(!def.settings.is_empty());
        assert!(!def.dashboard.metrics.is_empty());
        assert!((def.agent.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(def.agent.max_iterations, Some(80));
        for key in [
            "day_toolkit",
            "max_loss_per_trade_usd",
            "max_trades_per_day",
            "flat_by",
            "pdt_guard",
            "sec_user_agent",
        ] {
            assert!(
                def.settings.iter().any(|s| s.key == key),
                "trader hand is missing the day-trader toolkit setting '{key}'"
            );
        }
    }

    /// The trader hand installs FreecoDAO/day-trader-skill at a pinned commit,
    /// and that commit is written in SKILL.md's install commands. Bumping the
    /// pin means re-reviewing the toolkit's code, so the pin must appear in
    /// SKILL.md and the toolkit must never be installed unpinned.
    #[test]
    fn trader_toolkit_is_pinned() {
        const PIN: &str = "3d0f4ef6986f4984b7c6bd46f7ff4f87744c3abd";
        let (_, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "trader")
            .unwrap();
        assert!(toml_content.contains("pinned commit"));
        assert!(
            skill_content.contains(&format!("checkout {PIN}")),
            "SKILL.md must check out the pinned toolkit commit"
        );
        assert!(skill_content.contains(&format!("tar.gz/{PIN}")));
    }

    /// The OmniRoute caretaker works through OmniRoute's MCP tools, not a shell,
    /// and the gateway stays on loopback.
    #[test]
    fn omniroute_hand_uses_mcp_not_shell() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "omniroute")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.mcp_servers, vec!["omniroute".to_string()]);
        assert!(!def.tools.iter().any(|t| t == "shell_exec"));
        assert!(def.tools.iter().any(|t| t == "agent_send"));
        assert!(def
            .tools
            .iter()
            .any(|t| t == "mcp_omniroute_omniroute_get_health"));
        assert!(def
            .tools
            .iter()
            .filter(|t| t.starts_with("mcp_"))
            .all(|t| t.starts_with("mcp_omniroute_omniroute_")));
        let container = def.container.as_ref().expect("omniroute is a service hand");
        assert_eq!(
            container.env.get("OMNIROUTE_HOSTNAME").map(String::as_str),
            Some("127.0.0.1")
        );
        // Owner-specific combo names stay out of the bundled prompt.
        assert!(!def.agent.system_prompt.contains("freeco-cc-"));
        assert!(!def.agent.system_prompt.contains("freeco-agent-"));
    }

    #[test]
    fn all_bundled_hands_parse() {
        for (id, toml_content, skill_content) in bundled_hands() {
            let def = parse_bundled(id, toml_content, skill_content)
                .unwrap_or_else(|e| panic!("Failed to parse hand '{}': {}", id, e));
            assert_eq!(def.id, id);
            assert!(!def.name.is_empty());
            assert!(!def.tools.is_empty());
            assert!(!def.agent.system_prompt.is_empty());
            assert!(def.skill_content.is_some());
        }
    }

    /// Every service hand must declare either an image or a compose stack, and
    /// a port its health check can reach.
    ///
    /// Written after all three stack hands shipped with `compose` placed *after*
    /// `[container.env]`. TOML puts every key after a sub-table header inside
    /// that table, so it parsed as `container.env.compose`, `image` stayed
    /// empty, and activation took the single-container path and failed with
    /// `podman run: reference parameter cannot be empty` -- a message about a
    /// missing image, for a hand that specifies a whole stack. Parsing
    /// successfully is not the same as parsing into what was meant.
    #[test]
    fn service_hands_declare_something_runnable() {
        for (id, toml_content, skill_content) in bundled_hands() {
            let def = parse_bundled(id, toml_content, skill_content).unwrap();
            let Some(c) = &def.container else { continue };
            assert!(
                !c.image.is_empty() || c.is_stack(),
                "service hand '{id}' has neither an image nor a compose stack \
                 -- check that `compose` sits before [container.env]"
            );
            assert!(
                c.image.is_empty() || !c.is_stack(),
                "service hand '{id}' declares both an image and a stack"
            );
            if c.health_path.is_some() {
                assert!(
                    crate::container::service_port(c).is_some(),
                    "service hand '{id}' has a health_path but no reachable port"
                );
            }
        }
    }

    #[test]
    fn all_einstein_hands_have_schedules() {
        let einstein_ids = [
            "lead",
            "collector",
            "predictor",
            "researcher",
            "twitter",
            "trader",
        ];
        for (id, toml_content, skill_content) in bundled_hands() {
            if einstein_ids.contains(&id) {
                let def = parse_bundled(id, toml_content, skill_content).unwrap();
                assert!(
                    def.tools.contains(&"schedule_create".to_string()),
                    "Einstein hand '{}' must have schedule_create tool",
                    id
                );
                assert!(
                    def.tools.contains(&"schedule_list".to_string()),
                    "Einstein hand '{}' must have schedule_list tool",
                    id
                );
                assert!(
                    def.tools.contains(&"schedule_delete".to_string()),
                    "Einstein hand '{}' must have schedule_delete tool",
                    id
                );
            }
        }
    }

    #[test]
    fn all_einstein_hands_have_memory() {
        let einstein_ids = [
            "lead",
            "collector",
            "predictor",
            "researcher",
            "twitter",
            "trader",
        ];
        for (id, toml_content, skill_content) in bundled_hands() {
            if einstein_ids.contains(&id) {
                let def = parse_bundled(id, toml_content, skill_content).unwrap();
                assert!(
                    def.tools.contains(&"memory_store".to_string()),
                    "Einstein hand '{}' must have memory_store tool",
                    id
                );
                assert!(
                    def.tools.contains(&"memory_recall".to_string()),
                    "Einstein hand '{}' must have memory_recall tool",
                    id
                );
            }
        }
    }

    #[test]
    fn parse_infisical_sync_hand() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "infisical-sync")
            .expect("infisical-sync hand must be in bundled_hands()");
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert_eq!(def.id, "infisical-sync");
        assert_eq!(def.name, "Infisical Sync Hand");
        assert_eq!(def.category, crate::HandCategory::Security);
        assert!(def.skill_content.is_some());
        // Required env vars
        assert!(
            !def.requires.is_empty(),
            "infisical-sync must declare env var requirements"
        );
        let req_keys: Vec<&str> = def.requires.iter().map(|r| r.key.as_str()).collect();
        assert!(
            req_keys.contains(&"INFISICAL_URL"),
            "must require INFISICAL_URL"
        );
        assert!(
            req_keys.contains(&"INFISICAL_CLIENT_ID"),
            "must require INFISICAL_CLIENT_ID"
        );
        assert!(
            req_keys.contains(&"INFISICAL_CLIENT_SECRET"),
            "must require INFISICAL_CLIENT_SECRET"
        );
        // Einstein scheduling tools
        assert!(
            def.tools.contains(&"schedule_create".to_string()),
            "must have schedule_create"
        );
        assert!(
            def.tools.contains(&"schedule_list".to_string()),
            "must have schedule_list"
        );
        assert!(
            def.tools.contains(&"schedule_delete".to_string()),
            "must have schedule_delete"
        );
        // Memory tools
        assert!(
            def.tools.contains(&"memory_store".to_string()),
            "must have memory_store"
        );
        assert!(
            def.tools.contains(&"memory_recall".to_string()),
            "must have memory_recall"
        );
        // Knowledge graph tools
        assert!(
            def.tools.contains(&"knowledge_add_entity".to_string()),
            "must have knowledge_add_entity"
        );
        assert!(
            def.tools.contains(&"knowledge_add_relation".to_string()),
            "must have knowledge_add_relation"
        );
        assert!(
            def.tools.contains(&"knowledge_query".to_string()),
            "must have knowledge_query"
        );
        // Event bus
        assert!(
            def.tools.contains(&"event_publish".to_string()),
            "must have event_publish"
        );
        // Infisical-specific tools
        assert!(
            def.tools.contains(&"shell_exec".to_string()),
            "must have shell_exec"
        );
        assert!(
            def.tools.contains(&"vault_set".to_string()),
            "must have vault_set"
        );
        assert!(
            def.tools.contains(&"vault_get".to_string()),
            "must have vault_get"
        );
        assert!(
            def.tools.contains(&"vault_list".to_string()),
            "must have vault_list"
        );
        assert!(
            def.tools.contains(&"vault_delete".to_string()),
            "must have vault_delete"
        );
        // Dashboard
        assert!(
            !def.dashboard.metrics.is_empty(),
            "must have dashboard metrics"
        );
        let metric_keys: Vec<&str> = def
            .dashboard
            .metrics
            .iter()
            .map(|m| m.memory_key.as_str())
            .collect();
        assert!(
            metric_keys.contains(&"infisical_sync_secrets_count"),
            "must have secrets_count metric"
        );
        assert!(
            metric_keys.contains(&"infisical_sync_last_sync"),
            "must have last_sync metric"
        );
        // Agent config
        assert!(
            !def.agent.system_prompt.is_empty(),
            "must have system_prompt"
        );
        assert!(
            def.agent.temperature < 0.2,
            "security hand should use low temperature"
        );
    }

    #[test]
    fn all_einstein_hands_have_knowledge_graph() {
        let einstein_ids = [
            "lead",
            "collector",
            "predictor",
            "researcher",
            "twitter",
            "trader",
        ];
        for (id, toml_content, skill_content) in bundled_hands() {
            if einstein_ids.contains(&id) {
                let def = parse_bundled(id, toml_content, skill_content).unwrap();
                assert!(
                    def.tools.contains(&"knowledge_add_entity".to_string()),
                    "Einstein hand '{}' must have knowledge_add_entity tool",
                    id
                );
                assert!(
                    def.tools.contains(&"knowledge_add_relation".to_string()),
                    "Einstein hand '{}' must have knowledge_add_relation tool",
                    id
                );
                assert!(
                    def.tools.contains(&"knowledge_query".to_string()),
                    "Einstein hand '{}' must have knowledge_query tool",
                    id
                );
            }
        }
    }

    /// The accounting hand reads the books through `accounting_api` (the
    /// prompt says so), and its app container gets the database settings the
    /// image's setup script and the web wizard actually read.
    #[test]
    fn accounting_hand_is_wired_to_its_tool_and_database() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "accounting")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        assert!(def.tools.iter().any(|t| t == "accounting_api"));
        assert!(def.agent.system_prompt.contains("accounting_api"));
        assert!(!def.tools.iter().any(|t| t == "shell_exec"));
        let c = def
            .container
            .as_ref()
            .expect("accounting is a service hand");
        let app = c.services.iter().find(|s| s.name == "app").unwrap();
        assert_eq!(app.env.get("DB_HOST").map(String::as_str), Some("db"));
        assert_eq!(
            app.env.get("DB_NAME").map(String::as_str),
            Some("akaunting")
        );
        assert_eq!(
            app.env.get("DB_DATABASE").map(String::as_str),
            Some("akaunting")
        );
        assert_eq!(app.ports, vec!["8080:80".to_string()]);
    }

    /// Board task 6: the voice api's DATABASE_URL names the asyncpg dialect,
    /// and the voice prompt points dograh's speech at the local Speaches.
    #[test]
    fn voice_hand_uses_asyncpg_and_local_speaches() {
        let (id, toml_content, skill_content) = bundled_hands()
            .into_iter()
            .find(|(id, _, _)| *id == "voice")
            .unwrap();
        let def = parse_bundled(id, toml_content, skill_content).unwrap();
        let c = def.container.as_ref().unwrap();
        let api = c.services.iter().find(|s| s.name == "api").unwrap();
        assert!(api
            .env
            .get("DATABASE_URL")
            .is_some_and(|u| u.starts_with("postgresql+asyncpg://")));
        assert!(def
            .agent
            .system_prompt
            .contains("host.containers.internal:8010/v1"));
        assert!(def
            .skill_content
            .as_deref()
            .is_some_and(|s| s.contains("host.containers.internal:8010/v1")));
    }
}

//! Spend caps must stop an LLM call before the provider is contacted, on both
//! the streaming and non-streaming paths. No API key is needed: the call is
//! refused before any driver runs.

use freeco_kernel::FreecoKernel;
use freeco_memory::usage::UsageRecord;
use freeco_types::agent::{AgentId, AgentManifest};
use freeco_types::config::{BudgetConfig, DefaultModelConfig, KernelConfig};

fn boot(budget: BudgetConfig) -> (std::sync::Arc<FreecoKernel>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config = KernelConfig {
        home_dir: tmp.path().to_path_buf(),
        data_dir: tmp.path().join("data"),
        default_model: DefaultModelConfig {
            provider: "groq".to_string(),
            model: "llama-3.3-70b-versatile".to_string(),
            api_key_env: "GROQ_API_KEY".to_string(),
            base_url: None,
            subprocess_timeout_secs: None,
        },
        budget,
        ..KernelConfig::default()
    };
    let kernel = FreecoKernel::boot_with_config(config).expect("Kernel should boot");
    (std::sync::Arc::new(kernel), tmp)
}

fn spawn(kernel: &FreecoKernel, extra: &str) -> AgentId {
    let manifest: AgentManifest = toml::from_str(&format!(
        r#"
name = "spend-test"
version = "0.1.0"
description = "spend limit test"
author = "test"
module = "builtin:chat"

[model]
provider = "groq"
model = "llama-3.3-70b-versatile"
system_prompt = "test"
{extra}
"#
    ))
    .unwrap();
    kernel.spawn_agent(manifest).expect("Agent should spawn")
}

fn spend(kernel: &FreecoKernel, agent_id: AgentId, cost_usd: f64) {
    kernel
        .metering
        .record(&UsageRecord {
            agent_id,
            model: "llama-3.3-70b-versatile".to_string(),
            input_tokens: 10,
            output_tokens: 10,
            cost_usd,
            tool_calls: 0,
        })
        .unwrap();
}

async fn assert_both_paths_refused(
    kernel: &std::sync::Arc<FreecoKernel>,
    id: AgentId,
    needle: &str,
) {
    let err = kernel.send_message(id, "hi").await.unwrap_err().to_string();
    assert!(err.contains(needle), "non-streaming: {err}");

    let err = match kernel.send_message_streaming(id, "hi", None, None, None, None) {
        Ok(_) => panic!("streaming call should have been refused"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains(needle), "streaming: {err}");
}

#[tokio::test]
async fn global_budget_blocks_both_paths() {
    let (kernel, _tmp) = boot(BudgetConfig {
        max_daily_usd: 1.0,
        ..BudgetConfig::default()
    });
    let id = spawn(&kernel, "");
    // Spent by a different agent: this agent's own quota is untouched, so only
    // the global total can refuse the call.
    spend(&kernel, AgentId::new(), 2.0);
    assert_both_paths_refused(&kernel, id, "Global daily budget exceeded").await;
}

#[tokio::test]
async fn agent_quota_blocks_both_paths() {
    let (kernel, _tmp) = boot(BudgetConfig::default());
    let id = spawn(&kernel, "[resources]\nmax_cost_per_hour_usd = 0.5\n");
    spend(&kernel, id, 1.0);
    assert_both_paths_refused(&kernel, id, "exceeded hourly cost quota").await;
}

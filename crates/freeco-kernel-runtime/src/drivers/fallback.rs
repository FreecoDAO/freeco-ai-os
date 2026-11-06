//! Fallback driver — tries multiple LLM drivers in sequence.
//!
//! If the primary driver fails with a non-retryable error, the fallback driver
//! moves to the next driver in the chain.

use crate::llm_driver::{CompletionRequest, CompletionResponse, LlmDriver, LlmError, StreamEvent};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::warn;

/// A driver that wraps multiple LLM drivers and tries each in order.
///
/// On failure (including rate-limit and overload), moves to the next driver.
/// Only returns an error when ALL drivers in the chain are exhausted.
/// Each driver is paired with the model name it should use.
pub struct FallbackDriver {
    drivers: Vec<(Arc<dyn LlmDriver>, String)>,
}

impl FallbackDriver {
    /// Create a new fallback driver from an ordered chain of (driver, model_name) pairs.
    ///
    /// The first entry is the primary; subsequent are fallbacks.
    pub fn new(drivers: Vec<Arc<dyn LlmDriver>>) -> Self {
        Self {
            drivers: drivers.into_iter().map(|d| (d, String::new())).collect(),
        }
    }

    /// Create a new fallback driver with explicit model names for each driver.
    pub fn with_models(drivers: Vec<(Arc<dyn LlmDriver>, String)>) -> Self {
        Self { drivers }
    }
}

/// The error to report once every link in the chain has failed.
///
/// Three rules, each protecting something a caller relies on:
///
/// - A chain of one reports its own error untouched. Nothing fell back, so
///   there is no trail worth adding and no reason to change what callers see.
/// - A retryable error (`RateLimited`, `Overloaded`) keeps its variant. The
///   agent loop matches on those to back off and try again; wrapping them into
///   `Api` would quietly turn a transient limit into a permanent failure.
/// - Anything else becomes `Api` carrying every link's failure. Reporting only
///   the last one describes the backup and hides why the primary was dropped:
///   a primary out of credit followed by a mistyped backup used to surface as
///   "404 page not found", naming a model the user never chose and saying
///   nothing about the balance that actually stopped the turn.
fn chain_error(last_error: Option<LlmError>, trail: Vec<String>) -> LlmError {
    let Some(last) = last_error else {
        return LlmError::Api {
            status: 0,
            message: "No drivers configured in fallback chain".to_string(),
        };
    };
    if trail.len() <= 1 {
        return last;
    }
    match last {
        e @ (LlmError::RateLimited { .. } | LlmError::Overloaded { .. }) => e,
        other => {
            let status = match &other {
                LlmError::Api { status, .. } => *status,
                _ => 0,
            };
            LlmError::Api {
                status,
                message: format!(
                    "every model in the fallback chain failed — {}",
                    trail.join(" | ")
                ),
            }
        }
    }
}

#[async_trait]
impl LlmDriver for FallbackDriver {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let mut last_error = None;
        // Every link's failure, not just the last one.
        //
        // Reporting only the final error describes the backup and hides the
        // reason the primary was abandoned. A primary out of credit followed
        // by a mistyped backup surfaced as "404 page not found" -- naming the
        // one provider the user never configured, and saying nothing about
        // the balance that actually stopped the turn.
        let mut trail: Vec<String> = Vec::new();

        for (i, (driver, model_name)) in self.drivers.iter().enumerate() {
            let mut req = request.clone();
            if !model_name.is_empty() {
                req.model = model_name.clone();
            }
            let attempted = if model_name.is_empty() {
                request.model.clone()
            } else {
                model_name.clone()
            };
            match driver.complete(req).await {
                Ok(response) => return Ok(response),
                Err(e @ LlmError::RateLimited { .. }) | Err(e @ LlmError::Overloaded { .. }) => {
                    warn!(
                        driver_index = i,
                        model = %model_name,
                        error = %e,
                        "Driver rate-limited/overloaded, trying next fallback"
                    );
                    trail.push(format!("{attempted}: {e}"));
                    last_error = Some(e);
                }
                Err(e) => {
                    warn!(
                        driver_index = i,
                        model = %model_name,
                        error = %e,
                        "Fallback driver failed, trying next"
                    );
                    trail.push(format!("{attempted}: {e}"));
                    last_error = Some(e);
                }
            }
        }

        Err(chain_error(last_error, trail))
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let mut last_error = None;
        let mut trail: Vec<String> = Vec::new();

        for (i, (driver, model_name)) in self.drivers.iter().enumerate() {
            let mut req = request.clone();
            if !model_name.is_empty() {
                req.model = model_name.clone();
            }
            let attempted = if model_name.is_empty() {
                request.model.clone()
            } else {
                model_name.clone()
            };
            // Say when a backup takes over, and why. The switch was silent:
            // with Claude Code signed out, every reply came from the backup
            // while the chat still read as Claude, and the difference in
            // quality looked like Claude being inconsistent.
            if i > 0 {
                let why = trail.last().cloned().unwrap_or_default();
                let _ = tx
                    .send(StreamEvent::PhaseChange {
                        phase: "fallback".to_string(),
                        detail: Some(format!("{attempted} answered instead -- {why}")),
                    })
                    .await;
            }
            match driver.stream(req, tx.clone()).await {
                Ok(response) => return Ok(response),
                Err(e @ LlmError::RateLimited { .. }) | Err(e @ LlmError::Overloaded { .. }) => {
                    warn!(
                        driver_index = i,
                        model = %model_name,
                        error = %e,
                        "Driver rate-limited/overloaded (stream), trying next fallback"
                    );
                    trail.push(format!("{attempted}: {e}"));
                    last_error = Some(e);
                }
                Err(e) => {
                    warn!(
                        driver_index = i,
                        model = %model_name,
                        error = %e,
                        "Fallback driver (stream) failed, trying next"
                    );
                    trail.push(format!("{attempted}: {e}"));
                    last_error = Some(e);
                }
            }
        }

        Err(chain_error(last_error, trail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_driver::CompletionResponse;
    use freeco_types::message::{ContentBlock, StopReason, TokenUsage};

    struct FailDriver;

    #[async_trait]
    impl LlmDriver for FailDriver {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
            Err(LlmError::Api {
                status: 500,
                message: "Internal error".to_string(),
            })
        }
    }

    struct OkDriver;

    #[async_trait]
    impl LlmDriver for OkDriver {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
            Ok(CompletionResponse {
                content: vec![ContentBlock::Text {
                    text: "OK".to_string(),
                    provider_metadata: None,
                }],
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            })
        }
    }

    fn test_request() -> CompletionRequest {
        CompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: 100,
            temperature: 0.0,
            system: None,
            thinking: None,
        }
    }

    #[tokio::test]
    async fn test_fallback_primary_succeeds() {
        let driver = FallbackDriver::new(vec![
            Arc::new(OkDriver) as Arc<dyn LlmDriver>,
            Arc::new(FailDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().text(), "OK");
    }

    #[tokio::test]
    async fn test_fallback_primary_fails_secondary_succeeds() {
        let driver = FallbackDriver::new(vec![
            Arc::new(FailDriver) as Arc<dyn LlmDriver>,
            Arc::new(OkDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fallback_all_fail() {
        let driver = FallbackDriver::new(vec![
            Arc::new(FailDriver) as Arc<dyn LlmDriver>,
            Arc::new(FailDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rate_limit_falls_through() {
        struct RateLimitDriver;

        #[async_trait]
        impl LlmDriver for RateLimitDriver {
            async fn complete(
                &self,
                _req: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::RateLimited {
                    retry_after_ms: 5000,
                })
            }
        }

        let driver = FallbackDriver::new(vec![
            Arc::new(RateLimitDriver) as Arc<dyn LlmDriver>,
            Arc::new(OkDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        // Rate limit should fall through to the OkDriver fallback
        assert!(result.is_ok());
        assert_eq!(result.unwrap().text(), "OK");
    }

    #[tokio::test]
    async fn test_rate_limit_all_fail() {
        struct RateLimitDriver;

        #[async_trait]
        impl LlmDriver for RateLimitDriver {
            async fn complete(
                &self,
                _req: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::RateLimited {
                    retry_after_ms: 5000,
                })
            }
        }

        let driver = FallbackDriver::new(vec![
            Arc::new(RateLimitDriver) as Arc<dyn LlmDriver>,
            Arc::new(RateLimitDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        // All drivers rate-limited — error should bubble up
        assert!(matches!(result, Err(LlmError::RateLimited { .. })));
    }

    /// Regression test for #1003: when the primary driver returns a network /
    /// connection error (e.g. LM Studio shut down → reqwest connection refused),
    /// the FallbackDriver MUST escalate to the next driver in the chain instead
    /// of bubbling the error up to the agent loop (which would then retry the
    /// dead primary forever).
    #[tokio::test]
    async fn test_network_error_falls_through_to_secondary() {
        struct NetworkFailDriver;

        #[async_trait]
        impl LlmDriver for NetworkFailDriver {
            async fn complete(
                &self,
                _req: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                // Simulates `reqwest::Error` from a connection refused — exactly
                // what an offline LM Studio looks like in production.
                Err(LlmError::Http(
                    "error sending request: connection refused (os error 10061)".to_string(),
                ))
            }
        }

        let driver = FallbackDriver::new(vec![
            Arc::new(NetworkFailDriver) as Arc<dyn LlmDriver>,
            Arc::new(OkDriver) as Arc<dyn LlmDriver>,
        ]);
        let result = driver.complete(test_request()).await;
        assert!(
            result.is_ok(),
            "FallbackDriver should escalate network errors to the next driver"
        );
        assert_eq!(result.unwrap().text(), "OK");
    }

    /// Same as above but for streaming. The streaming path is what the agent
    /// loop hits in practice for LM Studio etc., so it must also fall through.
    #[tokio::test]
    async fn test_network_error_falls_through_streaming() {
        struct NetworkFailDriver;

        #[async_trait]
        impl LlmDriver for NetworkFailDriver {
            async fn complete(
                &self,
                _req: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::Http("connection refused".to_string()))
            }

            async fn stream(
                &self,
                _req: CompletionRequest,
                _tx: tokio::sync::mpsc::Sender<StreamEvent>,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::Http("connection refused".to_string()))
            }
        }

        let driver = FallbackDriver::new(vec![
            Arc::new(NetworkFailDriver) as Arc<dyn LlmDriver>,
            Arc::new(OkDriver) as Arc<dyn LlmDriver>,
        ]);
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let result = driver.stream(test_request(), tx).await;
        assert!(
            result.is_ok(),
            "FallbackDriver::stream should also escalate network errors"
        );
    }

    /// A primary out of credit, then a backup whose model id does not exist.
    /// The user must be told about BOTH, and the balance first.
    #[tokio::test]
    async fn exhausted_chain_reports_every_link_not_just_the_last() {
        struct NoBalance;
        #[async_trait]
        impl LlmDriver for NoBalance {
            async fn complete(
                &self,
                _r: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::Api {
                    status: 403,
                    message: "not enough balance".to_string(),
                })
            }
        }
        struct NoSuchModel;
        #[async_trait]
        impl LlmDriver for NoSuchModel {
            async fn complete(
                &self,
                _r: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Err(LlmError::Api {
                    status: 404,
                    message: "404 page not found".to_string(),
                })
            }
        }
        let driver = FallbackDriver::new(vec![
            Arc::new(NoBalance) as Arc<dyn LlmDriver>,
            Arc::new(NoSuchModel) as Arc<dyn LlmDriver>,
        ]);
        let err = driver.complete(test_request()).await.unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("not enough balance"),
            "the primary's reason must survive: {text}"
        );
        assert!(
            text.contains("404 page not found"),
            "the backup's reason must be there too: {text}"
        );
        assert!(
            text.find("not enough balance") < text.find("404 page not found"),
            "links are reported in the order they were tried: {text}"
        );
    }

    /// Nothing fell back, so nothing about the error may change.
    #[tokio::test]
    async fn single_link_chain_returns_its_error_untouched() {
        let driver = FallbackDriver::new(vec![Arc::new(FailDriver) as Arc<dyn LlmDriver>]);
        match driver.complete(test_request()).await {
            Err(LlmError::Api { status, message }) => {
                assert_eq!(status, 500);
                assert_eq!(message, "Internal error");
            }
            other => panic!("expected the driver's own error, got {other:?}"),
        }
    }

    /// The agent loop backs off on RateLimited. Wrapping it into Api would turn
    /// a transient limit into a permanent failure, so the variant must survive
    /// even at the end of a longer chain.
    #[test]
    fn retryable_error_keeps_its_variant_at_the_end_of_a_chain() {
        let err = chain_error(
            Some(LlmError::RateLimited {
                retry_after_ms: 1500,
            }),
            vec!["a: failed".to_string(), "b: rate limited".to_string()],
        );
        assert!(
            matches!(
                err,
                LlmError::RateLimited {
                    retry_after_ms: 1500
                }
            ),
            "got {err:?}"
        );
    }
}

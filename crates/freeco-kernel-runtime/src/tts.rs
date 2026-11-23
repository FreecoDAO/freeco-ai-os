//! Text-to-speech engine — synthesize text to audio.
//!
//! Auto-cascades through available providers based on configured API keys.

use crate::media_understanding::local_speech_error;
use freeco_types::config::TtsConfig;
use freeco_types::media::is_local_url;
use tracing::error;

/// Maximum audio response size (10MB).
const MAX_AUDIO_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Result of TTS synthesis.
#[derive(Debug)]
pub struct TtsResult {
    pub audio_data: Vec<u8>,
    pub format: String,
    pub provider: String,
    pub duration_estimate_ms: u64,
}

/// Text-to-speech engine.
pub struct TtsEngine {
    config: TtsConfig,
    /// Optional override for OpenAI TTS base URL. When set, the engine POSTs
    /// to `<openai_base_url>/v1/audio/speech` instead of the hardcoded
    /// `https://api.openai.com/v1/audio/speech`. Sourced from
    /// `MediaConfig.tts_openai_base_url`. Closes #1051.
    openai_base_url: Option<String>,
    /// Optional override for ElevenLabs TTS base URL. When set, the engine
    /// POSTs to `<elevenlabs_base_url>/v1/text-to-speech/{voice_id}` instead
    /// of the hardcoded `https://api.elevenlabs.io/...`. Sourced from
    /// `MediaConfig.tts_elevenlabs_base_url`. Closes #1051.
    elevenlabs_base_url: Option<String>,
    /// Env var holding the key for a local OpenAI-compatible TTS service,
    /// from `MediaConfig.local_audio_api_key_env`. A custom endpoint must not
    /// be sent the real `OPENAI_API_KEY`.
    local_api_key_env: Option<String>,
    /// `MediaConfig::local_only_effective()`: speak only through a local
    /// endpoint, never auto-detect or fall back to a cloud provider.
    local_only: bool,
}

impl TtsEngine {
    pub fn new(config: TtsConfig) -> Self {
        Self {
            config,
            openai_base_url: None,
            elevenlabs_base_url: None,
            local_api_key_env: None,
            local_only: false,
        }
    }

    /// Keep speech local (`[media].local_only`): only the configured local
    /// endpoint is used, and its failure is returned as an error.
    pub fn with_local_only(mut self, local_only: bool) -> Self {
        self.local_only = local_only;
        self
    }

    /// The provider to use when `local_only` is in effect: the one whose base
    /// URL is set to a local address. OpenAI and ElevenLabs keys in the
    /// environment are never consulted.
    fn local_only_provider(&self) -> Result<&'static str, String> {
        let local = |url: &Option<String>| -> Option<Result<(), String>> {
            let url = url.as_deref().map(str::trim).filter(|u| !u.is_empty())?;
            if is_local_url(url) {
                Some(Ok(()))
            } else {
                Some(Err(format!(
                    "Text-to-speech refused: [media].local_only is on, but {url} is not a \
                     local address, so no text was sent to it. Point it at the local \
                     Speaches service, or set [media].local_only = false to allow it."
                )))
            }
        };
        let not_configured = |setting: &str| {
            format!(
                "Text-to-speech refused: [media].local_only is on, but no local \
                 text-to-speech is configured. Set [media].{setting} to the local \
                 Speaches service (for example http://127.0.0.1:8010). No cloud \
                 service was tried."
            )
        };
        match self.config.provider.as_deref() {
            Some("elevenlabs") => match local(&self.elevenlabs_base_url) {
                Some(Ok(())) => Ok("elevenlabs"),
                Some(Err(e)) => Err(e),
                None => Err(not_configured("tts_elevenlabs_base_url")),
            },
            None | Some("openai") => match local(&self.openai_base_url) {
                Some(Ok(())) => Ok("openai"),
                Some(Err(e)) => Err(e),
                None => Err(not_configured("tts_openai_base_url")),
            },
            Some(other) => Err(format!("Unknown TTS provider: {other}")),
        }
    }

    /// Key for a local TTS service reached through `openai_base_url`.
    pub fn with_local_api_key_env(mut self, env: Option<String>) -> Self {
        self.local_api_key_env = env;
        self
    }

    /// Attach optional base-URL overrides from `MediaConfig`. Use this to
    /// route TTS calls at a local OpenAI-compatible service (e.g.
    /// Lemonade/Kokoro, LM Studio) or an ElevenLabs proxy. Closes #1051.
    pub fn with_base_urls(
        mut self,
        openai_base_url: Option<String>,
        elevenlabs_base_url: Option<String>,
    ) -> Self {
        self.openai_base_url = openai_base_url;
        self.elevenlabs_base_url = elevenlabs_base_url;
        self
    }

    /// Detect which TTS provider is available based on environment variables.
    fn detect_provider() -> Option<&'static str> {
        if std::env::var("OPENAI_API_KEY").is_ok() {
            return Some("openai");
        }
        if std::env::var("ELEVENLABS_API_KEY").is_ok() {
            return Some("elevenlabs");
        }
        None
    }

    /// Synthesize text to audio bytes.
    /// Auto-cascade: configured provider -> OpenAI -> ElevenLabs.
    /// Optional overrides for voice and format (per-request, from tool input).
    pub async fn synthesize(
        &self,
        text: &str,
        voice_override: Option<&str>,
        format_override: Option<&str>,
    ) -> Result<TtsResult, String> {
        if !self.config.enabled {
            return Err("TTS is disabled in configuration".into());
        }

        // Validate text length
        if text.is_empty() {
            return Err("Text cannot be empty".into());
        }
        if text.len() > self.config.max_text_length {
            return Err(format!(
                "Text too long: {} chars (max {})",
                text.len(),
                self.config.max_text_length
            ));
        }

        let provider: &str = if self.local_only {
            self.local_only_provider().inspect_err(|e| {
                error!(error = %e, "text-to-speech refused: [media].local_only is on");
            })?
        } else {
            self.config
                .provider
                .as_deref()
                .or_else(|| Self::detect_provider())
                .ok_or("No TTS provider configured. Set OPENAI_API_KEY or ELEVENLABS_API_KEY")?
        };

        match provider {
            "openai" => {
                self.synthesize_openai(text, voice_override, format_override)
                    .await
            }
            "elevenlabs" => self.synthesize_elevenlabs(text, voice_override).await,
            other => Err(format!("Unknown TTS provider: {other}")),
        }
    }

    /// Synthesize via OpenAI TTS API.
    async fn synthesize_openai(
        &self,
        text: &str,
        voice_override: Option<&str>,
        format_override: Option<&str>,
    ) -> Result<TtsResult, String> {
        // A custom endpoint is a different service and never receives the
        // OpenAI key; it gets its own, or a placeholder a local server accepts.
        let api_key = match (&self.openai_base_url, &self.local_api_key_env) {
            (Some(_), Some(env)) => std::env::var(env).map_err(|_| format!("{env} not set"))?,
            (Some(_), None) => "local".to_string(),
            (None, _) => std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY not set")?,
        };

        // Apply per-request overrides or fall back to config defaults
        let voice = voice_override.unwrap_or(&self.config.openai.voice);
        let format = format_override.unwrap_or(&self.config.openai.format);

        let body = serde_json::json!({
            "model": self.config.openai.model,
            "input": text,
            "voice": voice,
            "response_format": format,
            "speed": self.config.openai.speed,
        });

        // `tts_openai_base_url` (config.media.tts_openai_base_url) overrides
        // the hardcoded provider URL when set, allowing the same OpenAI-compat
        // JSON wire format to be sent to a local TTS service (Lemonade/Kokoro,
        // LM Studio, etc.) instead of the cloud provider. The Authorization
        // header is still built from `OPENAI_API_KEY`; local services typically
        // accept any non-empty bearer token. Closes #1051.
        let url = self
            .openai_base_url
            .as_deref()
            .map(|base| format!("{}/v1/audio/speech", base.trim_end_matches('/')))
            .unwrap_or_else(|| "https://api.openai.com/v1/audio/speech".to_string());

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(self.config.timeout_secs))
            .send()
            .await
            .map_err(|e| {
                if self.local_only {
                    local_speech_error("text-to-speech", &url, &format!("request failed: {e}"))
                } else {
                    format!("OpenAI TTS request failed: {e}")
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let err = response.text().await.unwrap_or_default();
            let truncated = crate::str_utils::safe_truncate_str(&err, 500);
            if self.local_only {
                return Err(local_speech_error(
                    "text-to-speech",
                    &url,
                    &format!("HTTP {status}: {truncated}"),
                ));
            }
            return Err(format!("OpenAI TTS failed (HTTP {status}): {truncated}"));
        }

        // Check content length before downloading
        if let Some(len) = response.content_length() {
            if len as usize > MAX_AUDIO_RESPONSE_BYTES {
                return Err(format!(
                    "Audio response too large: {len} bytes (max {MAX_AUDIO_RESPONSE_BYTES})"
                ));
            }
        }

        let audio_data = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read audio response: {e}"))?;

        if audio_data.len() > MAX_AUDIO_RESPONSE_BYTES {
            return Err(format!(
                "Audio data exceeds {}MB limit",
                MAX_AUDIO_RESPONSE_BYTES / 1024 / 1024
            ));
        }

        // Rough duration estimate: ~150 words/min at ~12 bytes/ms for MP3
        let word_count = text.split_whitespace().count();
        let duration_ms = (word_count as u64 * 400).max(500); // ~400ms per word, min 500ms

        Ok(TtsResult {
            audio_data: audio_data.to_vec(),
            format: format.to_string(),
            provider: "openai".to_string(),
            duration_estimate_ms: duration_ms,
        })
    }

    /// Synthesize via ElevenLabs TTS API.
    async fn synthesize_elevenlabs(
        &self,
        text: &str,
        voice_override: Option<&str>,
    ) -> Result<TtsResult, String> {
        let api_key =
            std::env::var("ELEVENLABS_API_KEY").map_err(|_| "ELEVENLABS_API_KEY not set")?;

        let voice_id = voice_override.unwrap_or(&self.config.elevenlabs.voice_id);
        // `tts_elevenlabs_base_url` (config.media.tts_elevenlabs_base_url)
        // overrides the hardcoded provider URL when set, allowing the same
        // ElevenLabs JSON wire format to be routed through a proxy or
        // self-hosted ElevenLabs-compatible gateway. The `xi-api-key` header
        // still comes from `ELEVENLABS_API_KEY`. Closes #1051.
        let base = self
            .elevenlabs_base_url
            .as_deref()
            .map(|b| b.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.elevenlabs.io".to_string());
        let url = format!("{}/v1/text-to-speech/{}", base, voice_id);

        let body = serde_json::json!({
            "text": text,
            "model_id": self.config.elevenlabs.model_id,
            "voice_settings": {
                "stability": self.config.elevenlabs.stability,
                "similarity_boost": self.config.elevenlabs.similarity_boost,
            }
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("xi-api-key", &api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(self.config.timeout_secs))
            .send()
            .await
            .map_err(|e| {
                if self.local_only {
                    local_speech_error("text-to-speech", &url, &format!("request failed: {e}"))
                } else {
                    format!("ElevenLabs TTS request failed: {e}")
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let err = response.text().await.unwrap_or_default();
            let truncated = crate::str_utils::safe_truncate_str(&err, 500);
            if self.local_only {
                return Err(local_speech_error(
                    "text-to-speech",
                    &url,
                    &format!("HTTP {status}: {truncated}"),
                ));
            }
            return Err(format!(
                "ElevenLabs TTS failed (HTTP {status}): {truncated}"
            ));
        }

        if let Some(len) = response.content_length() {
            if len as usize > MAX_AUDIO_RESPONSE_BYTES {
                return Err(format!(
                    "Audio response too large: {len} bytes (max {MAX_AUDIO_RESPONSE_BYTES})"
                ));
            }
        }

        let audio_data = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read audio response: {e}"))?;

        if audio_data.len() > MAX_AUDIO_RESPONSE_BYTES {
            return Err(format!(
                "Audio data exceeds {}MB limit",
                MAX_AUDIO_RESPONSE_BYTES / 1024 / 1024
            ));
        }

        let word_count = text.split_whitespace().count();
        let duration_ms = (word_count as u64 * 400).max(500);

        Ok(TtsResult {
            audio_data: audio_data.to_vec(),
            format: "mp3".to_string(),
            provider: "elevenlabs".to_string(),
            duration_estimate_ms: duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> TtsConfig {
        TtsConfig::default()
    }

    #[test]
    fn test_engine_creation() {
        let engine = TtsEngine::new(default_config());
        assert!(!engine.config.enabled);
    }

    #[test]
    fn test_config_defaults() {
        let config = TtsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_text_length, 4096);
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.openai.voice, "alloy");
        assert_eq!(config.openai.model, "tts-1");
        assert_eq!(config.openai.format, "mp3");
        assert_eq!(config.openai.speed, 1.0);
        assert_eq!(config.elevenlabs.voice_id, "21m00Tcm4TlvDq8ikWAM");
        assert_eq!(config.elevenlabs.model_id, "eleven_monolingual_v1");
    }

    #[tokio::test]
    async fn test_synthesize_disabled() {
        let engine = TtsEngine::new(default_config());
        let result = engine.synthesize("Hello", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("disabled"));
    }

    #[tokio::test]
    async fn test_synthesize_empty_text() {
        let mut config = default_config();
        config.enabled = true;
        let engine = TtsEngine::new(config);
        let result = engine.synthesize("", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_synthesize_text_too_long() {
        let mut config = default_config();
        config.enabled = true;
        config.max_text_length = 10;
        let engine = TtsEngine::new(config);
        let result = engine
            .synthesize("This text is definitely longer than ten chars", None, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn test_detect_provider_none() {
        // In test env, likely no API keys set
        let _ = TtsEngine::detect_provider(); // Just verify no panic
    }

    #[tokio::test]
    async fn test_synthesize_no_provider() {
        let mut config = default_config();
        config.enabled = true;
        let engine = TtsEngine::new(config);
        // This may or may not error depending on env vars
        let result = engine.synthesize("Hello world", None, None).await;
        // If no API keys are set, should error
        if let Err(err) = result {
            assert!(err.contains("No TTS provider") || err.contains("not set"));
        }
    }

    #[test]
    fn test_max_audio_constant() {
        assert_eq!(MAX_AUDIO_RESPONSE_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn test_with_base_urls_sets_overrides() {
        let engine = TtsEngine::new(default_config()).with_base_urls(
            Some("http://127.0.0.1:8000".to_string()),
            Some("http://127.0.0.1:9000".to_string()),
        );
        assert_eq!(
            engine.openai_base_url.as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(
            engine.elevenlabs_base_url.as_deref(),
            Some("http://127.0.0.1:9000")
        );
    }

    /// Closes #1051: when the OpenAI TTS base URL is overridden, the URL
    /// building logic must append `/v1/audio/speech` and strip any trailing
    /// slash. When unset, the hardcoded provider URL is used.
    #[test]
    fn test_tts_openai_base_url_override_logic() {
        // Helper mirroring the URL construction in `synthesize_openai`.
        fn build(base: Option<&str>) -> String {
            base.map(|b| format!("{}/v1/audio/speech", b.trim_end_matches('/')))
                .unwrap_or_else(|| "https://api.openai.com/v1/audio/speech".to_string())
        }

        // Default: hardcoded URL preserved (backward compatibility).
        assert_eq!(build(None), "https://api.openai.com/v1/audio/speech");

        // Override applied.
        assert_eq!(
            build(Some("http://127.0.0.1:8000")),
            "http://127.0.0.1:8000/v1/audio/speech"
        );

        // Trailing slash on the user-supplied base is stripped.
        assert_eq!(
            build(Some("http://127.0.0.1:8000/")),
            "http://127.0.0.1:8000/v1/audio/speech"
        );
        assert_eq!(
            build(Some("https://tts.example.com/")),
            "https://tts.example.com/v1/audio/speech"
        );
    }

    /// Closes #1051: when the ElevenLabs TTS base URL is overridden, the URL
    /// building logic must append `/v1/text-to-speech/{voice_id}` and strip
    /// any trailing slash. When unset, the hardcoded provider URL is used.
    #[test]
    fn test_tts_elevenlabs_base_url_override_logic() {
        fn build(base: Option<&str>, voice_id: &str) -> String {
            let b = base
                .map(|b| b.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://api.elevenlabs.io".to_string());
            format!("{}/v1/text-to-speech/{}", b, voice_id)
        }

        let voice = "21m00Tcm4TlvDq8ikWAM";

        // Default: hardcoded URL preserved.
        assert_eq!(
            build(None, voice),
            format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}")
        );

        // Override applied.
        assert_eq!(
            build(Some("http://127.0.0.1:9000"), voice),
            format!("http://127.0.0.1:9000/v1/text-to-speech/{voice}")
        );

        // Trailing slash stripped.
        assert_eq!(
            build(Some("http://127.0.0.1:9000/"), voice),
            format!("http://127.0.0.1:9000/v1/text-to-speech/{voice}")
        );
        assert_eq!(
            build(Some("https://eleven.example.com/"), voice),
            format!("https://eleven.example.com/v1/text-to-speech/{voice}")
        );
    }

    /// A local port nothing listens on: bind an ephemeral port, then free it.
    fn closed_local_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    fn enabled_config() -> TtsConfig {
        let mut config = default_config();
        config.enabled = true;
        config
    }

    /// local_only with nothing local configured refuses, whatever cloud keys
    /// the environment holds -- the cloud provider is never auto-detected.
    #[tokio::test]
    async fn local_only_never_picks_a_cloud_provider() {
        let engine = TtsEngine::new(enabled_config()).with_local_only(true);
        let err = engine.synthesize("Hello", None, None).await.unwrap_err();
        assert!(err.contains("local_only"), "{err}");
        assert!(err.contains("tts_openai_base_url"), "{err}");

        // Explicitly asking for ElevenLabs without a local gateway: refused too.
        let mut config = enabled_config();
        config.provider = Some("elevenlabs".into());
        let engine = TtsEngine::new(config).with_local_only(true);
        let err = engine.synthesize("Hello", None, None).await.unwrap_err();
        assert!(err.contains("tts_elevenlabs_base_url"), "{err}");
    }

    #[tokio::test]
    async fn local_only_refuses_a_remote_base_url() {
        let engine = TtsEngine::new(enabled_config())
            .with_base_urls(Some("https://api.openai.com".into()), None)
            .with_local_only(true);
        let err = engine.synthesize("Hello", None, None).await.unwrap_err();
        assert!(err.contains("not a local address"), "{err}");
    }

    #[test]
    fn local_only_provider_accepts_a_local_speaches() {
        let engine = TtsEngine::new(enabled_config())
            .with_base_urls(Some("http://127.0.0.1:8010".into()), None)
            .with_local_only(true);
        assert_eq!(engine.local_only_provider().unwrap(), "openai");
    }

    /// The local endpoint is down: the caller gets an error naming the URL and
    /// saying no cloud service was tried.
    #[tokio::test]
    async fn local_only_surfaces_a_local_failure() {
        let port = closed_local_port();
        let base = format!("http://127.0.0.1:{port}");
        let mut config = enabled_config();
        config.timeout_secs = 5;
        let engine = TtsEngine::new(config)
            .with_base_urls(Some(base.clone()), None)
            .with_local_only(true);
        let err = engine.synthesize("Hello", None, None).await.unwrap_err();
        assert!(
            err.contains(&format!("{base}/v1/audio/speech")),
            "error must name the URL: {err}"
        );
        assert!(err.contains("request failed"), "{err}");
        assert!(err.contains("no cloud service was tried"), "{err}");
    }
}

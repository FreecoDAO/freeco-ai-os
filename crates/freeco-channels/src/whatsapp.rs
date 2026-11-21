//! WhatsApp Cloud API channel adapter.
//!
//! Uses the official WhatsApp Business Cloud API to send and receive messages.
//! Requires a webhook endpoint for incoming messages and the Cloud API for outgoing.

use crate::types::{ChannelAdapter, ChannelContent, ChannelMessage, ChannelType, ChannelUser};
use crate::whatsapp_allowlist::WhatsAppAllowlist;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{error, info};
use zeroize::Zeroizing;

const MAX_MESSAGE_LEN: usize = 4096;

/// WhatsApp Cloud API adapter.
///
/// Supports two modes:
/// - **Cloud API mode**: Uses the official WhatsApp Business Cloud API (requires Meta dev account).
/// - **Web/QR mode**: Routes outgoing messages through a local Baileys-based gateway process.
///
/// Mode is selected automatically: if `gateway_url` is set (from `WHATSAPP_WEB_GATEWAY_URL`),
/// the adapter uses Web mode. Otherwise it falls back to Cloud API mode.
pub struct WhatsAppAdapter {
    /// WhatsApp Business phone number ID (Cloud API mode).
    phone_number_id: String,
    /// SECURITY: Access token is zeroized on drop.
    access_token: Zeroizing<String>,
    /// SECURITY: Verify token is zeroized on drop.
    verify_token: Zeroizing<String>,
    /// Port to listen for webhook callbacks (Cloud API mode).
    webhook_port: u16,
    /// HTTP client.
    client: reqwest::Client,
    /// The chats this adapter may send to (see `whatsapp_allowlist`): empty
    /// means none, except the owner's number when one is configured.
    allowlist: WhatsAppAllowlist,
    /// Optional WhatsApp Web gateway URL for QR/Web mode (e.g. "http://127.0.0.1:3009").
    gateway_url: Option<String>,
    /// Shutdown signal.
    shutdown_tx: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
}

impl WhatsAppAdapter {
    /// Create a new WhatsApp Cloud API adapter.
    pub fn new(
        phone_number_id: String,
        access_token: String,
        verify_token: String,
        webhook_port: u16,
        allowed_users: Vec<String>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            phone_number_id,
            access_token: Zeroizing::new(access_token),
            verify_token: Zeroizing::new(verify_token),
            webhook_port,
            client: reqwest::Client::new(),
            allowlist: WhatsAppAllowlist::from_config(&allowed_users, None),
            gateway_url: None,
            shutdown_tx: Arc::new(shutdown_tx),
            shutdown_rx,
        }
    }

    /// Use the full allowlist: `allowed_users` plus the owner's own number.
    pub fn with_allowlist(mut self, allowlist: WhatsAppAllowlist) -> Self {
        self.allowlist = allowlist;
        self
    }

    /// Create a new WhatsApp adapter with gateway URL for Web/QR mode.
    ///
    /// When `gateway_url` is `Some`, outgoing messages are sent via `POST {gateway_url}/message/send`
    /// instead of the Cloud API. Incoming messages are handled by the gateway itself.
    pub fn with_gateway(mut self, gateway_url: Option<String>) -> Self {
        self.gateway_url = gateway_url.filter(|u| !u.is_empty());
        self
    }

    /// Send a text message via the WhatsApp Cloud API.
    async fn api_send_message(
        &self,
        to: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let url = format!(
            "https://graph.facebook.com/v21.0/{}/messages",
            self.phone_number_id
        );

        // Split long messages
        let chunks = crate::types::split_message(text, MAX_MESSAGE_LEN);
        for chunk in chunks {
            let body = serde_json::json!({
                "messaging_product": "whatsapp",
                "to": to,
                "type": "text",
                "text": { "body": chunk }
            });

            let resp = self
                .client
                .post(&url)
                .bearer_auth(&*self.access_token)
                .json(&body)
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                error!("WhatsApp API error {status}: {body}");
                return Err(format!("WhatsApp API error {status}: {body}").into());
            }
        }

        Ok(())
    }

    /// Mark a message as read.
    #[allow(dead_code)]
    async fn api_mark_read(&self, message_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let url = format!(
            "https://graph.facebook.com/v21.0/{}/messages",
            self.phone_number_id
        );

        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "status": "read",
            "message_id": message_id
        });

        let _ = self
            .client
            .post(&url)
            .bearer_auth(&*self.access_token)
            .json(&body)
            .send()
            .await;

        Ok(())
    }

    /// Send a text message via the WhatsApp Web gateway.
    async fn gateway_send_message(
        &self,
        gateway_url: &str,
        to: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let url = format!("{}/message/send", gateway_url.trim_end_matches('/'));
        let body = serde_json::json!({ "to": to, "text": text });

        let mut request = self.client.post(&url).json(&body);
        if let Ok(token) = std::env::var("WHATSAPP_GATEWAY_TOKEN") {
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
        }
        let resp = request.send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("WhatsApp gateway error {status}: {body}");
            return Err(format!("WhatsApp gateway error {status}: {body}").into());
        }

        Ok(())
    }

    /// Whether a chat (number, user JID or group JID) is on the allowlist.
    fn is_allowed(&self, chat_id: &str) -> bool {
        self.allowlist.allows(chat_id)
    }

    /// Returns true if this adapter is configured for Web/QR gateway mode.
    #[allow(dead_code)]
    pub fn is_gateway_mode(&self) -> bool {
        self.gateway_url.is_some()
    }
}

#[async_trait]
impl ChannelAdapter for WhatsAppAdapter {
    fn name(&self) -> &str {
        "whatsapp"
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::WhatsApp
    }

    async fn start(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = ChannelMessage> + Send>>, Box<dyn std::error::Error>>
    {
        let (_tx, rx) = mpsc::channel::<ChannelMessage>(256);
        let port = self.webhook_port;
        let _verify_token = self.verify_token.clone();
        let _allowlist = self.allowlist.clone();
        let _access_token = self.access_token.clone();
        let _phone_number_id = self.phone_number_id.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();

        info!("Starting WhatsApp webhook listener on port {port}");

        tokio::spawn(async move {
            // Simple webhook polling simulation
            // In production, this would be an axum HTTP server handling webhook POSTs
            // For now, log that the webhook is ready
            info!("WhatsApp webhook ready on port {port} (verify_token configured)");
            info!("Configure your webhook URL: https://your-domain:{port}/webhook");

            // Wait for shutdown
            let _ = shutdown_rx.changed().await;
            info!("WhatsApp adapter stopped");
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn send(
        &self,
        user: &ChannelUser,
        content: ChannelContent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Allowed chats only, in this direction too: an agent's channel_send,
        // a cron delivery or a reply to a chat that is not on the allowlist
        // is refused here, before anything leaves the daemon.
        if !self.is_allowed(&user.platform_id) {
            return Err(self.allowlist.refusal(&user.platform_id).into());
        }

        // Web/QR gateway mode: route all messages through the gateway
        if let Some(ref gw) = self.gateway_url {
            let text = match &content {
                ChannelContent::Text(t) => t.clone(),
                ChannelContent::Image { caption, .. } => caption
                    .clone()
                    .unwrap_or_else(|| "(Image — not supported in Web mode)".to_string()),
                ChannelContent::File { filename, .. } => {
                    format!("(File: {filename} — not supported in Web mode)")
                }
                _ => "(Unsupported content type in Web mode)".to_string(),
            };
            // Split long messages the same way as Cloud API mode
            let chunks = crate::types::split_message(&text, MAX_MESSAGE_LEN);
            for chunk in chunks {
                self.gateway_send_message(gw, &user.platform_id, chunk)
                    .await?;
            }
            return Ok(());
        }

        // Cloud API mode (default)
        match content {
            ChannelContent::Text(text) => {
                self.api_send_message(&user.platform_id, &text).await?;
            }
            ChannelContent::Image { url, caption } => {
                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": user.platform_id,
                    "type": "image",
                    "image": {
                        "link": url,
                        "caption": caption.unwrap_or_default()
                    }
                });
                let api_url = format!(
                    "https://graph.facebook.com/v21.0/{}/messages",
                    self.phone_number_id
                );
                let resp = self
                    .client
                    .post(&api_url)
                    .bearer_auth(&*self.access_token)
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(format!("WhatsApp API error {status}: {body}").into());
                }
            }
            ChannelContent::File { url, filename, .. } => {
                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": user.platform_id,
                    "type": "document",
                    "document": {
                        "link": url,
                        "filename": filename
                    }
                });
                let api_url = format!(
                    "https://graph.facebook.com/v21.0/{}/messages",
                    self.phone_number_id
                );
                let resp = self
                    .client
                    .post(&api_url)
                    .bearer_auth(&*self.access_token)
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(format!("WhatsApp API error {status}: {body}").into());
                }
            }
            ChannelContent::Location { lat, lon } => {
                let body = serde_json::json!({
                    "messaging_product": "whatsapp",
                    "to": user.platform_id,
                    "type": "location",
                    "location": {
                        "latitude": lat,
                        "longitude": lon
                    }
                });
                let api_url = format!(
                    "https://graph.facebook.com/v21.0/{}/messages",
                    self.phone_number_id
                );
                let resp = self
                    .client
                    .post(&api_url)
                    .bearer_auth(&*self.access_token)
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(format!("WhatsApp API error {status}: {body}").into());
                }
            }
            _ => {
                self.api_send_message(&user.platform_id, "(Unsupported content type)")
                    .await?;
            }
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.shutdown_tx.send(true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatsapp_adapter_creation() {
        let adapter = WhatsAppAdapter::new(
            "12345".to_string(),
            "access_token".to_string(),
            "verify_token".to_string(),
            8443,
            vec![],
        );
        assert_eq!(adapter.name(), "whatsapp");
        assert_eq!(adapter.channel_type(), ChannelType::WhatsApp);
    }

    #[test]
    fn test_allowed_users_check() {
        let adapter = WhatsAppAdapter::new(
            "12345".to_string(),
            "token".to_string(),
            "verify".to_string(),
            8443,
            vec!["+1234567890".to_string()],
        );
        assert!(adapter.is_allowed("+1234567890"));
        assert!(adapter.is_allowed("1234567890@s.whatsapp.net"));
        assert!(!adapter.is_allowed("+9999999999"));

        // Empty allowlist: nobody, not everybody.
        let empty = WhatsAppAdapter::new(
            "12345".to_string(),
            "token".to_string(),
            "verify".to_string(),
            8443,
            vec![],
        );
        assert!(!empty.is_allowed("+1234567890"));

        // ...except the owner, when the config names one.
        let owner = empty.with_allowlist(WhatsAppAllowlist::from_config(&[], Some("+1234567890")));
        assert!(owner.is_allowed("+1234567890"));
        assert!(!owner.is_allowed("+9999999999"));
    }

    #[tokio::test]
    async fn test_send_to_a_chat_off_the_allowlist_is_refused_before_any_request() {
        // The gateway URL points nowhere: a refused send must fail on the
        // allowlist, not on the network.
        let adapter = WhatsAppAdapter::new(
            String::new(),
            String::new(),
            String::new(),
            8443,
            vec!["+41791234567".to_string()],
        )
        .with_gateway(Some("http://127.0.0.1:9".to_string()));
        let stranger = ChannelUser {
            platform_id: "+41790000000".to_string(),
            display_name: "x".to_string(),
            freeco_user: None,
        };
        let err = adapter
            .send(&stranger, ChannelContent::Text("hi".to_string()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not on the allowlist"), "{err}");

        let group = ChannelUser {
            platform_id: "120363000000000000@g.us".to_string(),
            display_name: "g".to_string(),
            freeco_user: None,
        };
        let err = adapter
            .send(&group, ChannelContent::Text("hi".to_string()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not on the allowlist"), "{err}");
    }
}

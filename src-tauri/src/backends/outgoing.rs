use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::capabilities::ProviderCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingConfig {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub tls_mode: String,
    pub auth_method: String,
    pub username: String,
}

#[allow(dead_code)] // Keeps explicit outcome states for the outbox state machine.
#[derive(Debug, Error)]
pub enum OutgoingError {
    #[error("network error: {0}")]
    Network(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("authentication failed")]
    Authentication,
    #[error("server rejected message: {0}")]
    Rejected(String),
    #[error("send outcome is unknown")]
    AmbiguousSend,
    #[error("unsupported capability: {0}")]
    Unsupported(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendResult {
    Sent { remote_id: Option<String> },
    Failed,
    OutcomeUnknown,
}

#[allow(dead_code)] // Outgoing backends are selected by application services as providers land.
#[async_trait]
pub trait OutgoingMailBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn test_connection(&self, secret: &str) -> Result<(), OutgoingError>;
    async fn send_mime(
        &self,
        secret: &str,
        mime: Vec<u8>,
        envelope_from: &str,
        recipients: &[String],
    ) -> Result<SendResult, OutgoingError>;
}

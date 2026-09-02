use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::capabilities::ProviderCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingConfig {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub tls_mode: String,
    pub auth_method: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub backend: String,
    pub capabilities: ProviderCapabilities,
    pub greeting: Option<String>,
}

#[derive(Debug, Error)]
pub enum IncomingError {
    #[error("network error: {0}")]
    Network(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("authentication failed")]
    Authentication,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("unsupported protocol: {0}")]
    Unsupported(String),
}

#[allow(dead_code)] // Protocol surface is intentionally richer than the first connected slice.
#[async_trait]
pub trait IncomingMailBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn test_connection(&self, secret: &str) -> Result<ServerCapabilities, IncomingError>;
}

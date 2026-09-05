use serde::Serialize;
use thiserror::Error;

#[allow(dead_code)] // Domain error taxonomy is part of the stable IPC contract.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("secret store error: {0}")]
    SecretStore(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("server rejected operation: {0}")]
    ServerRejected(String),
    #[error("authentication failed")]
    Authentication,
    #[error("unsupported capability: {0}")]
    Capability(String),
    #[error("ambiguous send result")]
    AmbiguousSend,
    #[error("operation cancelled")]
    Cancelled,
    #[error("internal error")]
    Internal(String),
}

impl AppError {
    pub fn not_found(kind: &str) -> Self {
        Self::NotFound(kind.into())
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration(_) => "invalid_configuration",
            Self::NotFound(_) => "not_found",
            Self::Storage(_) => "storage",
            Self::Serialization(_) => "serialization",
            Self::SecretStore(_) => "secret_store",
            Self::Network(_) => "network",
            Self::Protocol(_) => "protocol",
            Self::ServerRejected(_) => "server_rejected",
            Self::Authentication => "authentication",
            Self::Capability(_) => "capability",
            Self::AmbiguousSend => "ambiguous_send",
            Self::Cancelled => "cancelled",
            Self::Internal(_) => "internal",
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Storage(_) | Self::AmbiguousSend
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorDto {
    pub code: String,
    pub message: String,
    pub user_action: Option<String>,
    pub retryable: bool,
    pub account_id: Option<String>,
    pub provider_code: Option<String>,
    pub technical_details: Option<String>,
}

impl From<AppError> for AppErrorDto {
    fn from(error: AppError) -> Self {
        let code = error.code().to_string();
        let message = match &error {
            AppError::Authentication => "认证失败，请检查客户端授权码或重新授权".into(),
            AppError::SecretStore(message) => message.clone(),
            AppError::Network(_) => "网络连接失败，请检查网络和服务器设置后重试".into(),
            AppError::AmbiguousSend => "服务器可能已经接收邮件，是否重试需要你确认".into(),
            _ => error.to_string(),
        };
        Self {
            code,
            message,
            retryable: error.retryable(),
            user_action: None,
            account_id: None,
            provider_code: None,
            technical_details: if cfg!(debug_assertions) {
                Some(error.to_string())
            } else {
                None
            },
        }
    }
}

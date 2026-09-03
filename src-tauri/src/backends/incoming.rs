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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMailbox {
    /// Mailbox name exactly as returned by IMAP. Keep this value for later
    /// SELECT/EXAMINE calls because it may still use modified UTF-7.
    pub remote_id: String,
    /// Human-readable mailbox name decoded from modified UTF-7 when possible.
    pub display_name: String,
    pub delimiter: Option<String>,
    pub attributes: Vec<String>,
    pub special_role: Option<String>,
    pub selectable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub sequence: u32,
    pub uid: u32,
    pub flags: Vec<String>,
    pub internal_date: Option<String>,
    pub size_bytes: Option<u32>,
    /// RFC 822 headers fetched independently from the body so oversized
    /// messages still retain sender, recipients, date and subject metadata.
    pub raw_headers: Option<Vec<u8>>,
    /// Complete RFC 822 message bytes. `None` means the message exceeded the
    /// bounded initial-fetch budget; its metadata remains available.
    pub raw_rfc822: Option<Vec<u8>>,
}

/// One exact message fetched from a selected mailbox. The mailbox identity is
/// returned with the message so callers can reject a stale local UID after an
/// IMAP UIDVALIDITY reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessageFetch {
    pub remote_id: String,
    pub uid_validity: Option<u32>,
    pub message: IncomingMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMailboxSnapshot {
    pub remote_id: String,
    pub uid_validity: Option<u32>,
    pub total_count: u32,
    pub unread_count: u32,
    /// True only when this page was produced by an untruncated `UID SEARCH ALL`.
    /// Incremental and historical pages must remain false even when they are
    /// empty because they do not cover the complete selected mailbox.
    pub coverage_complete: bool,
    pub messages: Vec<IncomingMessage>,
}

/// A metadata-free, complete UID/flag index for reconciling expunges and flag
/// changes without downloading every message header and body again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMailboxIndex {
    pub remote_id: String,
    pub uid_validity: Option<u32>,
    pub total_count: u32,
    pub all_uids: Vec<u32>,
    pub unseen_uids: Vec<u32>,
    pub flagged_uids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteMessageOperation {
    SetFlags {
        mailbox_remote_id: String,
        uid: u32,
        expected_uid_validity: Option<u32>,
        is_read: Option<bool>,
        is_starred: Option<bool>,
    },
    Move {
        source_mailbox_remote_id: String,
        target_mailbox_remote_id: String,
        uid: u32,
        expected_uid_validity: Option<u32>,
    },
    DeletePermanently {
        mailbox_remote_id: String,
        uid: u32,
        expected_uid_validity: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendMessageResult {
    /// Present only when the server returned a valid UIDPLUS APPENDUID pair.
    pub uid_validity: Option<u32>,
    pub uid: Option<u32>,
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
    async fn list_remote_mailboxes(
        &self,
        _secret: &str,
    ) -> Result<Vec<IncomingMailbox>, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement mailbox listing",
            self.backend_name()
        )))
    }
    async fn fetch_remote_messages(
        &self,
        _secret: &str,
        _mailbox: &str,
        _since_uid: Option<u32>,
        _limit: u32,
    ) -> Result<IncomingMailboxSnapshot, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement message fetching",
            self.backend_name()
        )))
    }
    async fn fetch_remote_messages_before(
        &self,
        _secret: &str,
        _mailbox: &str,
        _before_uid: u32,
        _limit: u32,
    ) -> Result<IncomingMailboxSnapshot, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement historical message fetching",
            self.backend_name()
        )))
    }
    async fn fetch_remote_mailbox_index(
        &self,
        _secret: &str,
        _mailbox: &str,
    ) -> Result<IncomingMailboxIndex, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement mailbox index fetching",
            self.backend_name()
        )))
    }
    async fn fetch_remote_message(
        &self,
        _secret: &str,
        _mailbox: &str,
        _uid: u32,
    ) -> Result<Option<IncomingMessageFetch>, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement on-demand message fetching",
            self.backend_name()
        )))
    }
    async fn apply_remote_operation(
        &self,
        _secret: &str,
        _operation: &RemoteMessageOperation,
    ) -> Result<(), IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement remote message mutations",
            self.backend_name()
        )))
    }
    async fn append_message(
        &self,
        _secret: &str,
        _mailbox: &str,
        _raw_rfc822: &[u8],
        _mark_seen: bool,
    ) -> Result<AppendMessageResult, IncomingError> {
        Err(IncomingError::Unsupported(format!(
            "{} does not implement message append",
            self.backend_name()
        )))
    }
}

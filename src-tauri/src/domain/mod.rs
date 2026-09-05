use serde::{Deserialize, Serialize};

pub mod account;
pub mod capabilities;
pub mod message;
pub mod operation;
pub mod sync_cursor;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider_id: String,
    pub email: String,
    pub display_name: String,
    pub enabled: bool,
    pub sync_policy: String,
    pub incoming_configured: bool,
    pub outgoing_configured: bool,
    pub sync_status: String,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub account_id: String,
    pub remote_id: String,
    pub name: String,
    pub display_name: String,
    pub special_role: Option<String>,
    pub unread_count: i64,
    pub total_count: i64,
    pub sync_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub account_id: String,
    pub mailbox_id: String,
    pub thread_id: String,
    pub message_id: Option<String>,
    pub subject: String,
    pub normalized_subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub date: String,
    pub preview: String,
    pub body_text: Option<String>,
    pub body_html_text: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    pub has_attachment: bool,
    pub attachment_count: i64,
    pub attachments: Vec<AttachmentInfo>,
    pub labels: Vec<String>,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftInput {
    pub id: Option<String>,
    pub account_id: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub body_text: String,
    pub in_reply_to: Option<String>,
    pub references: Option<Vec<String>>,
}

/// Binary data selected by the user for one outgoing MIME attachment. The
/// frontend can only obtain these bytes through the native file picker scope;
/// the SMTP worker persists the resulting immutable MIME payload before it
/// starts delivery, so a retry never needs to reopen the user file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttachment {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub id: String,
    pub account_id: String,
    pub subject: String,
    pub recipients: Vec<String>,
    pub state: String,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    /// Delivery and server-side filing are separate facts. `sent` means SMTP
    /// accepted the message; this field says whether a real remote Sent copy
    /// has subsequently been observed.
    pub sent_copy_state: Option<String>,
    pub sent_copy_error_message: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub account_id: String,
    pub state: String,
    pub phase: Option<String>,
    pub processed: Option<i64>,
    pub total: Option<i64>,
    pub message: Option<String>,
    pub retryable: bool,
}

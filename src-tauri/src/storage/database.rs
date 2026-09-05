use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use uuid::Uuid;

use crate::backends::{
    incoming::{IncomingConfig, IncomingMailboxIndex},
    outgoing::OutgoingConfig,
};
use crate::domain::message::normalize_subject;
use crate::domain::sync_cursor::SyncCursor;
use crate::domain::{
    account::CreateAccountInput, Account, Address, AttachmentInfo, DraftInput, Mailbox, Message,
    OutboxItem,
};
use crate::errors::AppError;
use crate::mime::parser::ParsedAttachment;
use crate::providers::registry::ProviderPreset;

pub struct Database {
    connection: Connection,
}

#[derive(Debug, Clone)]
pub struct OutboxDraft {
    pub account_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body_text: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// Immutable SMTP payload owned by one outbox item. This is deliberately kept
/// separate from the editable draft snapshot: retries and Sent reconciliation
/// must refer to exactly the bytes that were first queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedOutboxPayload {
    pub envelope_from: String,
    pub recipients: Vec<String>,
    pub mime: Vec<u8>,
    pub rfc_message_id: String,
    pub sent_copy_state: String,
    pub sent_copy_error_message: Option<String>,
    pub sent_copy_uid_validity: Option<u32>,
    pub sent_copy_uid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedMailboxInput {
    pub remote_id: String,
    pub display_name: String,
    pub delimiter: Option<String>,
    pub special_role: Option<String>,
    pub selectable: bool,
}

#[derive(Debug, Clone)]
pub struct SyncedMessageInput {
    pub uid: u32,
    pub flags: Vec<String>,
    pub received_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub rfc_message_id: Option<String>,
    pub subject: String,
    pub preview: String,
    pub body_text: Option<String>,
    pub body_html_text: Option<String>,
    pub has_attachment: bool,
    pub from: Option<Address>,
    pub to: Vec<Address>,
}

/// Stable local coordinates for fetching one IMAP message body. The instance
/// id and UIDVALIDITY are intentionally retained so a response cannot be
/// written onto a reused UID after a mailbox reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapBodyLocator {
    pub message_id: String,
    pub message_instance_id: String,
    pub account_id: String,
    pub mailbox_id: String,
    pub mailbox_remote_id: String,
    pub uid_validity: Option<u32>,
    pub uid: u32,
    pub message_revision: String,
    pub body_cached: bool,
    pub account_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydratedMessageBody {
    pub preview: String,
    pub body_text: Option<String>,
    pub body_html_text: Option<String>,
    pub has_attachment: bool,
    pub attachments: Vec<ParsedAttachment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingImapOperation {
    pub id: String,
    pub account_id: String,
    pub operation_type: String,
    pub source_mailbox_remote_id: String,
    pub uid: u32,
    pub uid_validity: Option<u32>,
    pub target_mailbox_remote_id: Option<String>,
    pub payload_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImapSyncWindow {
    pub uid_validity: u32,
    pub last_uid: u32,
    pub oldest_uid: Option<u32>,
    pub instance_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImapSnapshotMetadata {
    pub uid_validity: Option<u32>,
    pub total_count: u32,
    pub unread_count: u32,
    pub complete_mailbox: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImapIndexReconcileResult {
    pub removed_instances: usize,
    pub updated_flags: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncApplyResult {
    pub inserted: usize,
    pub updated: usize,
}

type DraftRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

type OutboxDraftRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

impl Database {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let connection = Connection::open(path).map_err(AppError::from)?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
        database.recover_interrupted_outbox()?;
        database.recover_interrupted_sync()?;
        database.recover_interrupted_pending_operations()?;
        Ok(database)
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        let connection = Connection::open_in_memory().map_err(AppError::from)?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
        database.recover_interrupted_outbox()?;
        database.recover_interrupted_sync()?;
        database.recover_interrupted_pending_operations()?;
        Ok(database)
    }

    fn configure(&self) -> Result<(), AppError> {
        self.connection
            .execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
            )
            .map_err(AppError::from)
    }

    fn migrate(&mut self) -> Result<(), AppError> {
        self.connection
            .execute_batch(concat!(
                include_str!("../../migrations/0001_init.sql"),
                "\nCREATE TABLE IF NOT EXISTS attachment_payloads (attachment_id TEXT PRIMARY KEY NOT NULL REFERENCES attachments(id) ON DELETE CASCADE, bytes BLOB NOT NULL);"
            ))
            .map_err(AppError::from)
    }

    fn recover_interrupted_outbox(&mut self) -> Result<usize, AppError> {
        self.connection
            .execute(
                "UPDATE outbox SET state='outcome_unknown',last_error_code='interrupted_during_send',last_error_message='应用在服务器确认发送结果前中断；为避免重复发送，该邮件不会自动重试',updated_at=? WHERE state='sending'",
                [Utc::now().to_rfc3339()],
            )
            .map_err(AppError::from)
    }

    fn recover_interrupted_sync(&mut self) -> Result<usize, AppError> {
        self.connection
            .execute(
                "UPDATE provider_metadata SET value_json='\"idle\"',updated_at=? WHERE key='sync_status' AND trim(value_json,'\"')='syncing'",
                [Utc::now().to_rfc3339()],
            )
            .map_err(AppError::from)
    }

    fn recover_interrupted_pending_operations(&mut self) -> Result<usize, AppError> {
        self.connection
            .execute(
                "UPDATE pending_operations SET state='failed',retry_count=retry_count+1,next_attempt_at=NULL,last_error_code='interrupted_during_operation',updated_at=? WHERE state='sending'",
                [Utc::now().to_rfc3339()],
            )
            .map_err(AppError::from)
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>, AppError> {
        let mut statement = self.connection.prepare("SELECT a.id,a.provider_id,a.email,a.display_name,a.enabled,a.sync_policy,a.incoming_secret_ref IS NOT NULL,a.outgoing_secret_ref IS NOT NULL,COALESCE(sync_status.value_json,'idle'),last_success.value_json FROM accounts a LEFT JOIN provider_metadata sync_status ON sync_status.account_id=a.id AND sync_status.key='sync_status' LEFT JOIN provider_metadata last_success ON last_success.account_id=a.id AND last_success.key='last_sync_success' ORDER BY a.created_at").map_err(AppError::from)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Account {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    email: row.get(2)?,
                    display_name: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? != 0,
                    sync_policy: row.get(5)?,
                    incoming_configured: row.get::<_, bool>(6)?,
                    outgoing_configured: row.get::<_, bool>(7)?,
                    sync_status: row
                        .get::<_, Option<String>>(8)?
                        .unwrap_or_else(|| "idle".into())
                        .trim_matches('"')
                        .to_string(),
                    last_synced_at: row
                        .get::<_, Option<String>>(9)?
                        .map(|value| value.trim_matches('"').to_string()),
                })
            })
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn create_account(
        &mut self,
        input: &CreateAccountInput,
        preset: &ProviderPreset,
        incoming_ref: &str,
        outgoing_ref: &str,
        incoming_enabled: bool,
        outgoing_enabled: bool,
    ) -> Result<Account, AppError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let incoming_id = incoming_enabled.then(|| Uuid::new_v4().to_string());
        let outgoing_id = outgoing_enabled.then(|| Uuid::new_v4().to_string());
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let duplicate_exists = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE provider_id=?1 AND email=?2 COLLATE NOCASE)",
                params![input.provider_id, input.email.trim()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(AppError::from)?;
        if duplicate_exists {
            return Err(AppError::InvalidConfiguration("该邮箱账户已经添加".into()));
        }
        tx.execute("INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,incoming_endpoint_id,default_outgoing_endpoint_id,incoming_secret_ref,outgoing_secret_ref,created_at,updated_at) VALUES (?,?,?,?,1,'automatic',?,?,?,?,?,?)", params![id, input.provider_id, input.email, input.display_name, incoming_id, outgoing_id, if incoming_enabled { Some(incoming_ref) } else { None::<&str> }, if outgoing_enabled { Some(outgoing_ref) } else { None::<&str> }, now, now]).map_err(AppError::from)?;
        if let Some(endpoint_id) = &incoming_id {
            if let Some(endpoint) = &input.incoming {
                tx.execute("INSERT INTO incoming_endpoints (id,account_id,protocol,host,port,tls_mode,auth_method,username) VALUES (?,?,?,?,?,?,?,?)", params![endpoint_id, id, endpoint.protocol, endpoint.host, endpoint.port, endpoint.tls_mode, endpoint.auth_method, endpoint.username]).map_err(AppError::from)?;
            } else if let Some(endpoint) = &preset.incoming {
                tx.execute("INSERT INTO incoming_endpoints (id,account_id,protocol,host,port,tls_mode,auth_method,username) VALUES (?,?,?,?,?,?,?,?)", params![endpoint_id, id, endpoint.protocol, endpoint.host, endpoint.port, endpoint.tls_mode, endpoint.auth_methods.first().cloned().unwrap_or_else(|| "password".into()), endpoint.username.clone().unwrap_or_else(|| input.email.clone())]).map_err(AppError::from)?;
            } else {
                return Err(AppError::InvalidConfiguration(
                    "收件端点缺少服务器配置".into(),
                ));
            }
        }
        if let Some(endpoint_id) = &outgoing_id {
            if let Some(endpoint) = &input.outgoing {
                tx.execute("INSERT INTO outgoing_endpoints (id,account_id,protocol,host,port,tls_mode,auth_method,username) VALUES (?,?,?,?,?,?,?,?)", params![endpoint_id, id, endpoint.protocol, endpoint.host, endpoint.port, endpoint.tls_mode, endpoint.auth_method, endpoint.username]).map_err(AppError::from)?;
            } else if let Some(endpoint) = &preset.outgoing {
                tx.execute("INSERT INTO outgoing_endpoints (id,account_id,protocol,host,port,tls_mode,auth_method,username) VALUES (?,?,?,?,?,?,?,?)", params![endpoint_id, id, endpoint.protocol, endpoint.host, endpoint.port, endpoint.tls_mode, endpoint.auth_methods.first().cloned().unwrap_or_else(|| "password".into()), endpoint.username.clone().unwrap_or_else(|| input.email.clone())]).map_err(AppError::from)?;
            } else {
                return Err(AppError::InvalidConfiguration(
                    "发件端点缺少服务器配置".into(),
                ));
            }
        }
        tx.execute("INSERT INTO identities (id,account_id,display_name,email,is_default) VALUES (?,?,?,?,1)", params![Uuid::new_v4().to_string(), id, input.display_name, input.email]).map_err(AppError::from)?;
        tx.commit().map_err(AppError::from)?;
        Ok(Account {
            id,
            provider_id: input.provider_id.clone(),
            email: input.email.clone(),
            display_name: input.display_name.clone(),
            enabled: true,
            sync_policy: "automatic".into(),
            incoming_configured: incoming_enabled,
            outgoing_configured: outgoing_enabled,
            sync_status: "idle".into(),
            last_synced_at: None,
        })
    }

    pub fn list_mailboxes(&self, account_id: &str) -> Result<Vec<Mailbox>, AppError> {
        let mut statement = self.connection.prepare("SELECT id,account_id,remote_id,name,display_name,special_role,unread_count,total_count,sync_enabled FROM mailboxes WHERE account_id=? AND selectable=1 ORDER BY CASE special_role WHEN 'inbox' THEN 0 WHEN 'starred' THEN 1 WHEN 'drafts' THEN 2 WHEN 'sent' THEN 3 ELSE 4 END,name").map_err(AppError::from)?;
        let rows = statement
            .query_map([account_id], mailbox_from_row)
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Returns the real mailboxes for every enabled account. Synthetic views such as the
    /// unified inbox are expressed as message query scopes rather than fake mailbox rows.
    pub fn list_all_mailboxes(&self) -> Result<Vec<Mailbox>, AppError> {
        let sql = r#"SELECT mb.id,mb.account_id,mb.remote_id,mb.name,mb.display_name,mb.special_role,mb.unread_count,mb.total_count,mb.sync_enabled
                     FROM mailboxes mb
                     JOIN accounts a ON a.id=mb.account_id
                     WHERE a.enabled=1 AND mb.selectable=1
                     ORDER BY a.created_at,
                       CASE mb.special_role WHEN 'inbox' THEN 0 WHEN 'starred' THEN 1 WHEN 'drafts' THEN 2 WHEN 'sent' THEN 3 ELSE 4 END,
                       mb.name"#;
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let rows = statement
            .query_map([], mailbox_from_row)
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Stores a server LIST result without replacing local per-folder sync choices.
    pub fn upsert_remote_mailboxes(
        &mut self,
        account_id: &str,
        mailboxes: &[SyncedMailboxInput],
    ) -> Result<Vec<Mailbox>, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let account_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
                [account_id],
                |row| row.get(0),
            )
            .map_err(AppError::from)?;
        if !account_exists {
            return Err(AppError::not_found("account"));
        }
        for mailbox in mailboxes {
            if mailbox.remote_id.trim().is_empty() {
                return Err(AppError::InvalidConfiguration(
                    "远端文件夹标识不能为空".into(),
                ));
            }
            let mailbox_id = tx
                .query_row(
                    "SELECT id FROM mailboxes WHERE account_id=? AND remote_id=?",
                    params![account_id, mailbox.remote_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            tx.execute(
                r#"INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,delimiter,special_role,selectable,sync_enabled)
                   VALUES (?,?,?,?,?,?,?,?,1)
                   ON CONFLICT(account_id,remote_id) DO UPDATE SET
                     name=excluded.name,
                     display_name=excluded.display_name,
                     delimiter=excluded.delimiter,
                     special_role=excluded.special_role,
                     selectable=excluded.selectable"#,
                params![
                    mailbox_id,
                    account_id,
                    mailbox.remote_id,
                    mailbox.remote_id,
                    mailbox.display_name,
                    mailbox.delimiter,
                    mailbox.special_role,
                    i64::from(mailbox.selectable),
                ],
            )
            .map_err(AppError::from)?;
        }
        let remote_ids = mailboxes
            .iter()
            .map(|mailbox| mailbox.remote_id.as_str())
            .collect::<HashSet<_>>();
        let missing_mailbox_ids = {
            let mut statement = tx
                .prepare("SELECT id,remote_id FROM mailboxes WHERE account_id=?")
                .map_err(AppError::from)?;
            let rows = statement
                .query_map([account_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(AppError::from)?;
            rows.filter_map(|row| match row {
                Ok((_, remote_id)) if remote_ids.contains(remote_id.as_str()) => None,
                Ok((id, _)) => Some(Ok(id)),
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)?
        };
        for mailbox_id in missing_mailbox_ids {
            tx.execute(
                "UPDATE mailboxes SET selectable=0 WHERE id=? AND account_id=?",
                params![mailbox_id, account_id],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)?;
        self.list_mailboxes(account_id)
    }

    /// Applies one bounded IMAP mailbox snapshot atomically. Reapplying the same UID snapshot
    /// updates existing rows rather than duplicating messages. A UIDVALIDITY change invalidates
    /// the old mailbox instances before any new UIDs are written.
    pub fn apply_imap_snapshot(
        &mut self,
        account_id: &str,
        mailbox_remote_id: &str,
        metadata: ImapSnapshotMetadata,
        messages: &[SyncedMessageInput],
    ) -> Result<SyncApplyResult, AppError> {
        let ImapSnapshotMetadata {
            uid_validity,
            total_count,
            unread_count,
            complete_mailbox,
        } = metadata;
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let (mailbox_id, is_sent_mailbox) = tx
            .query_row(
                "SELECT id,COALESCE(lower(special_role)='sent',0) FROM mailboxes WHERE account_id=? AND remote_id=? AND selectable=1",
                params![account_id, mailbox_remote_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("mailbox"))?;

        let mut uid_validity_changed = false;
        if let Some(next_uid_validity) = uid_validity {
            let previous_cursor = tx
                .query_row(
                    "SELECT cursor_json FROM sync_cursors WHERE account_id=? AND mailbox_id=? AND backend_kind='imap'",
                    params![account_id, mailbox_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
                .and_then(|value| serde_json::from_str::<SyncCursor>(&value).ok());
            let cursor_changed = matches!(
                previous_cursor,
                Some(SyncCursor::Imap { uid_validity, .. }) if uid_validity != next_uid_validity
            );
            let instance_validity_changed = {
                let mut statement = tx
                    .prepare(
                        "SELECT DISTINCT uid_validity FROM message_instances WHERE mailbox_id=?",
                    )
                    .map_err(AppError::from)?;
                let rows = statement
                    .query_map([&mailbox_id], |row| row.get::<_, Option<u32>>(0))
                    .map_err(AppError::from)?;
                let mut changed = false;
                for stored in rows {
                    if stored.map_err(AppError::from)? != Some(next_uid_validity) {
                        changed = true;
                        break;
                    }
                }
                changed
            };
            if cursor_changed || instance_validity_changed {
                uid_validity_changed = true;
                tx.execute(
                    "DELETE FROM message_instances WHERE mailbox_id=?",
                    [&mailbox_id],
                )
                .map_err(AppError::from)?;
            }
        }

        let now = Utc::now().to_rfc3339();
        let mut inserted = 0;
        let mut updated = 0;
        let mut touched_threads = Vec::new();
        for message in messages {
            let received_at = canonical_rfc3339(message.received_at.as_deref());
            let remote_locator = message.uid.to_string();
            let existing_instance = tx
                .query_row(
                    "SELECT instance.id,instance.message_id,message.account_id FROM message_instances instance JOIN messages message ON message.id=instance.message_id WHERE instance.mailbox_id=? AND instance.remote_locator=?",
                    params![mailbox_id, remote_locator],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(AppError::from)?;
            if existing_instance
                .as_ref()
                .is_some_and(|(_, _, owner)| owner != account_id)
            {
                return Err(AppError::InvalidConfiguration(
                    "本地邮件实例与邮箱账户归属不一致".into(),
                ));
            }
            let had_instance = existing_instance.is_some();
            let pending_operation_types = if let Some((instance_id, _, _)) = &existing_instance {
                let mut statement = tx
                    .prepare(
                        "SELECT operation_type FROM pending_operations WHERE message_instance_id=? AND state IN ('pending','sending','failed')",
                    )
                    .map_err(AppError::from)?;
                let rows = statement
                    .query_map([instance_id], |row| row.get::<_, String>(0))
                    .map_err(AppError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(AppError::from)?
            } else {
                Vec::new()
            };
            let preserve_local_flags = pending_operation_types
                .iter()
                .any(|operation| operation == "set_flags");
            let preserve_local_deletion = pending_operation_types.iter().any(|operation| {
                matches!(operation.as_str(), "move" | "trash" | "permanent_delete")
            });
            let message_id = if let Some((_, message_id, _)) = existing_instance {
                message_id
            } else if let Some(rfc_message_id) = message
                .rfc_message_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                tx.query_row(
                    "SELECT id FROM messages WHERE account_id=? AND rfc_message_id=? ORDER BY created_at LIMIT 1",
                    params![account_id, rfc_message_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
                .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };
            let normalized_subject = normalize_subject(&message.subject);
            let thread_subject_key = if normalized_subject.is_empty() {
                format!("message:{message_id}")
            } else {
                normalized_subject.clone()
            };
            let thread_id = tx
                .query_row(
                    "SELECT id FROM threads WHERE account_id=? AND normalized_subject=? ORDER BY julianday(last_message_at) DESC,last_message_at DESC LIMIT 1",
                    params![account_id, thread_subject_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            tx.execute(
                r#"INSERT INTO threads (id,account_id,normalized_subject,last_message_at,message_count,unread_count)
                   VALUES (?,?,?,?,0,0)
                   ON CONFLICT(id) DO UPDATE SET last_message_at=CASE
                     WHEN excluded.last_message_at IS NULL THEN threads.last_message_at
                     WHEN threads.last_message_at IS NULL THEN excluded.last_message_at
                     WHEN julianday(excluded.last_message_at) >= julianday(threads.last_message_at) THEN excluded.last_message_at
                     ELSE threads.last_message_at
                   END"#,
                params![
                    thread_id,
                    account_id,
                    thread_subject_key,
                    received_at.as_deref()
                ],
            )
            .map_err(AppError::from)?;

            let message_owner = tx
                .query_row(
                    "SELECT account_id FROM messages WHERE id=?",
                    [&message_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?;
            if message_owner
                .as_ref()
                .is_some_and(|owner| owner != account_id)
            {
                return Err(AppError::InvalidConfiguration(
                    "本地邮件与同步账户归属不一致".into(),
                ));
            }
            if message_owner.is_some() {
                tx.execute(
                    r#"UPDATE messages SET
                         thread_id=?,
                         rfc_message_id=COALESCE(?,rfc_message_id),
                         subject=CASE WHEN ?='' THEN subject ELSE ? END,
                         normalized_subject=CASE WHEN ?='' THEN normalized_subject ELSE ? END,
                         received_at=COALESCE(?,received_at),
                         preview=CASE WHEN ?='' THEN preview ELSE ? END,
                         size_bytes=COALESCE(?,size_bytes),
                         has_attachment=MAX(has_attachment,?),
                         body_text=COALESCE(?,body_text),
                         body_html_text=COALESCE(?,body_html_text),
                         body_cache_state=CASE WHEN ? IS NOT NULL OR ? IS NOT NULL THEN 'full' ELSE body_cache_state END,
                         updated_at=?
                       WHERE id=? AND account_id=?"#,
                    params![
                        thread_id,
                        message.rfc_message_id,
                        message.subject,
                        message.subject,
                        normalized_subject,
                        normalized_subject,
                        received_at.as_deref(),
                        message.preview,
                        message.preview,
                        message.size_bytes,
                        i64::from(message.has_attachment),
                        message.body_text,
                        message.body_html_text,
                        message.body_text,
                        message.body_html_text,
                        now,
                        message_id,
                        account_id,
                    ],
                )
                .map_err(AppError::from)?;
            } else {
                tx.execute(
                    r#"INSERT INTO messages (id,account_id,thread_id,rfc_message_id,subject,normalized_subject,received_at,preview,size_bytes,has_attachment,body_cache_state,body_text,body_html_text,created_at,updated_at)
                       VALUES (?,?,?,?,?,?,?,?,?,?,?, ?,?,?,?)"#,
                    params![
                        message_id,
                        account_id,
                        thread_id,
                        message.rfc_message_id,
                        message.subject,
                        normalized_subject,
                        received_at.as_deref(),
                        message.preview,
                        message.size_bytes,
                        i64::from(message.has_attachment),
                        if message.body_text.is_some() || message.body_html_text.is_some() {
                            "full"
                        } else {
                            "metadata"
                        },
                        message.body_text,
                        message.body_html_text,
                        now,
                        now,
                    ],
                )
                .map_err(AppError::from)?;
            }

            let flags_json = serde_json::to_string(&canonical_imap_flags(&message.flags))
                .map_err(AppError::from)?;
            tx.execute(
                r#"INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,uid_validity,uid,flags_json,is_deleted,last_synced_at)
                   VALUES (?,?,?,?,?,?,?,0,?)
                   ON CONFLICT(mailbox_id,remote_locator) DO UPDATE SET
                     message_id=excluded.message_id,
                     uid_validity=excluded.uid_validity,
                     uid=excluded.uid,
                     flags_json=CASE WHEN ? THEN message_instances.flags_json ELSE excluded.flags_json END,
                     is_deleted=CASE WHEN ? THEN message_instances.is_deleted ELSE 0 END,
                     last_synced_at=excluded.last_synced_at"#,
                params![
                    Uuid::new_v4().to_string(),
                    message_id,
                    mailbox_id,
                    remote_locator,
                    uid_validity,
                    message.uid,
                    flags_json,
                    now,
                    i64::from(preserve_local_flags),
                    i64::from(preserve_local_deletion),
                ],
            )
            .map_err(AppError::from)?;

            if message.from.is_some() || !message.to.is_empty() {
                tx.execute(
                    "DELETE FROM message_addresses WHERE message_id=?",
                    [&message_id],
                )
                .map_err(AppError::from)?;
                if let Some(from) = &message.from {
                    insert_message_address(&tx, &message_id, "from", from, 0)?;
                }
                for (position, address) in message.to.iter().enumerate() {
                    insert_message_address(&tx, &message_id, "to", address, position)?;
                }
            }

            touched_threads.push(thread_id);
            if had_instance {
                updated += 1;
            } else {
                inserted += 1;
            }
        }

        if complete_mailbox {
            let snapshot_uids = messages
                .iter()
                .map(|message| message.uid)
                .collect::<HashSet<_>>();
            let stale_instance_ids = {
                let mut statement = tx
                    .prepare(
                        r#"SELECT instance.id,instance.uid
                           FROM message_instances instance
                           WHERE instance.mailbox_id=?
                             AND NOT EXISTS (
                               SELECT 1 FROM pending_operations operation
                               WHERE operation.message_instance_id=instance.id
                                 AND operation.state IN ('pending','sending','failed')
                             )"#,
                    )
                    .map_err(AppError::from)?;
                let rows = statement
                    .query_map([&mailbox_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<u32>>(1)?))
                    })
                    .map_err(AppError::from)?;
                rows.filter_map(|row| match row {
                    Ok((_, Some(uid))) if snapshot_uids.contains(&uid) => None,
                    Ok((id, _)) => Some(Ok(id)),
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)?
            };
            for instance_id in stale_instance_ids {
                tx.execute("DELETE FROM message_instances WHERE id=?", [instance_id])
                    .map_err(AppError::from)?;
            }
        }

        touched_threads.sort_unstable();
        touched_threads.dedup();
        for thread_id in touched_threads {
            tx.execute(
                r#"UPDATE threads SET
                     message_count=(SELECT count(*) FROM messages WHERE thread_id=?),
                     unread_count=(SELECT count(DISTINCT m.id) FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE m.thread_id=? AND mi.is_deleted=0 AND instr(lower(mi.flags_json),'"\\seen"')=0)
                   WHERE id=?"#,
                params![thread_id, thread_id, thread_id],
            )
            .map_err(AppError::from)?;
        }
        tx.execute(
            "UPDATE mailboxes SET total_count=?,unread_count=? WHERE id=? AND account_id=?",
            params![total_count, unread_count, mailbox_id, account_id],
        )
        .map_err(AppError::from)?;

        if let Some(uid_validity) = uid_validity {
            let previous_last_uid = tx
                .query_row(
                    "SELECT cursor_json FROM sync_cursors WHERE account_id=? AND mailbox_id=? AND backend_kind='imap'",
                    params![account_id, mailbox_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
                .and_then(|value| serde_json::from_str::<SyncCursor>(&value).ok())
                .and_then(|cursor| match cursor {
                    SyncCursor::Imap {
                        uid_validity: previous_uid_validity,
                        last_uid,
                        ..
                    } if previous_uid_validity == uid_validity => Some(last_uid),
                    _ => None,
                })
                .unwrap_or(0);
            let last_uid = messages
                .iter()
                .map(|message| message.uid)
                .max()
                .unwrap_or(0)
                .max(previous_last_uid);
            let cursor_json = serde_json::to_string(&SyncCursor::Imap {
                uid_validity,
                last_uid,
                highest_modseq: None,
            })
            .map_err(AppError::from)?;
            tx.execute(
                r#"INSERT INTO sync_cursors (id,account_id,mailbox_id,backend_kind,cursor_json,updated_at)
                   VALUES (?,?,?,'imap',?,?)
                   ON CONFLICT(account_id,mailbox_id,backend_kind) DO UPDATE SET cursor_json=excluded.cursor_json,updated_at=excluded.updated_at"#,
                params![Uuid::new_v4().to_string(), account_id, mailbox_id, cursor_json, now],
            )
            .map_err(AppError::from)?;
        }
        if uid_validity_changed || complete_mailbox {
            tx.execute(
                "DELETE FROM messages WHERE account_id=? AND NOT EXISTS (SELECT 1 FROM message_instances instance WHERE instance.message_id=messages.id)",
                [account_id],
            )
            .map_err(AppError::from)?;
            tx.execute(
                r#"UPDATE threads SET
                     message_count=(SELECT count(*) FROM messages WHERE thread_id=threads.id),
                     unread_count=(SELECT count(DISTINCT m.id) FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE m.thread_id=threads.id AND mi.is_deleted=0 AND instr(lower(mi.flags_json),'"\\seen"')=0)
                   WHERE account_id=?"#,
                [account_id],
            )
            .map_err(AppError::from)?;
            tx.execute(
                "DELETE FROM threads WHERE account_id=? AND NOT EXISTS (SELECT 1 FROM messages WHERE messages.thread_id=threads.id)",
                [account_id],
            )
                .map_err(AppError::from)?;
        }
        if is_sent_mailbox {
            // Confirmation is based only on a real IMAP instance from the
            // provider's Sent mailbox. Matching by the stable Message-ID
            // connects SMTP delivery to that remote fact without inventing a
            // UID or remote locator.
            tx.execute(
                r#"UPDATE outbox_payloads
                   SET sent_copy_state='confirmed',sent_copy_error_message=NULL,updated_at=?
                   WHERE sent_copy_state='awaiting_server_sync'
                     AND EXISTS (
                       SELECT 1
                       FROM outbox o
                       JOIN messages message
                         ON message.account_id=o.account_id
                        AND message.rfc_message_id=outbox_payloads.rfc_message_id
                       JOIN message_instances instance ON instance.message_id=message.id
                       WHERE o.id=outbox_payloads.outbox_id
                         AND o.account_id=?
                         AND o.state='sent'
                         AND instance.mailbox_id=?
                         AND instance.is_deleted=0
                     )"#,
                params![now, account_id, mailbox_id],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)?;
        Ok(SyncApplyResult { inserted, updated })
    }

    /// Reconciles a complete, metadata-free UID index after bounded message fetching.
    /// The saved cursor and every existing remote instance must agree on UIDVALIDITY before
    /// deletions or flag updates are allowed.
    pub fn reconcile_imap_mailbox_index(
        &mut self,
        account_id: &str,
        index: &IncomingMailboxIndex,
    ) -> Result<ImapIndexReconcileResult, AppError> {
        if index.remote_id.trim().is_empty() {
            return Err(AppError::Protocol("IMAP 文件夹索引缺少远端标识".into()));
        }
        let uid_validity = index
            .uid_validity
            .filter(|value| *value > 0)
            .ok_or_else(|| AppError::Protocol("IMAP 文件夹索引缺少有效的 UIDVALIDITY".into()))?;
        let all_uids = index.all_uids.iter().copied().collect::<HashSet<_>>();
        if all_uids.contains(&0) || all_uids.len() != index.all_uids.len() {
            return Err(AppError::Protocol(
                "IMAP 文件夹索引包含无效或重复的 UID".into(),
            ));
        }
        let indexed_count = u32::try_from(all_uids.len())
            .map_err(|_| AppError::Protocol("IMAP 文件夹索引超过支持范围".into()))?;
        if indexed_count != index.total_count {
            return Err(AppError::Protocol(
                "IMAP 文件夹索引的 UID 数量与总数不一致".into(),
            ));
        }
        let unseen_uids = index.unseen_uids.iter().copied().collect::<HashSet<_>>();
        let flagged_uids = index.flagged_uids.iter().copied().collect::<HashSet<_>>();
        if unseen_uids.contains(&0)
            || flagged_uids.contains(&0)
            || !unseen_uids.is_subset(&all_uids)
            || !flagged_uids.is_subset(&all_uids)
        {
            return Err(AppError::Protocol(
                "IMAP 文件夹索引的标记集合与 UID 集合不一致".into(),
            ));
        }

        let tx = self.connection.transaction().map_err(AppError::from)?;
        let (mailbox_id, cursor_json) = tx
            .query_row(
                r#"SELECT mailbox.id,cursor.cursor_json
                   FROM mailboxes mailbox
                   JOIN sync_cursors cursor ON cursor.mailbox_id=mailbox.id
                     AND cursor.account_id=mailbox.account_id
                     AND cursor.backend_kind='imap'
                   WHERE mailbox.account_id=? AND mailbox.remote_id=? AND mailbox.selectable=1"#,
                params![account_id, index.remote_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::Protocol("IMAP 文件夹尚无可校验的同步游标".into()))?;
        let cursor = serde_json::from_str::<SyncCursor>(&cursor_json).map_err(AppError::from)?;
        let SyncCursor::Imap {
            uid_validity: cursor_uid_validity,
            ..
        } = cursor
        else {
            return Err(AppError::InvalidConfiguration(
                "IMAP 文件夹保存了不兼容的同步游标".into(),
            ));
        };
        if cursor_uid_validity != uid_validity {
            return Err(AppError::Protocol(
                "IMAP UIDVALIDITY 在索引对账期间发生变化".into(),
            ));
        }
        let mixed_identity = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM message_instances WHERE mailbox_id=? AND uid IS NOT NULL AND uid_validity IS NOT ?)",
                params![mailbox_id, uid_validity],
                |row| row.get::<_, bool>(0),
            )
            .map_err(AppError::from)?;
        if mixed_identity {
            return Err(AppError::Protocol(
                "本地 IMAP 实例与文件夹 UIDVALIDITY 不一致".into(),
            ));
        }

        let instances = {
            let mut statement = tx
                .prepare(
                    r#"SELECT instance.id,instance.uid,instance.flags_json,
                              EXISTS(
                                SELECT 1 FROM pending_operations operation
                                WHERE operation.message_instance_id=instance.id
                                  AND operation.state IN ('pending','sending','failed')
                              ),
                              EXISTS(
                                SELECT 1 FROM pending_operations operation
                                WHERE operation.message_instance_id=instance.id
                                  AND operation.operation_type='set_flags'
                                  AND operation.state IN ('pending','sending','failed')
                              )
                       FROM message_instances instance
                       WHERE instance.mailbox_id=? AND instance.uid_validity=? AND instance.uid IS NOT NULL"#,
                )
                .map_err(AppError::from)?;
            let rows = statement
                .query_map(params![mailbox_id, uid_validity], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                })
                .map_err(AppError::from)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)?
        };

        let now = Utc::now().to_rfc3339();
        let mut removed_instances = 0;
        let mut updated_flags = 0;
        for (instance_id, uid, flags_json, has_pending, has_pending_flags) in instances {
            if !all_uids.contains(&uid) {
                if !has_pending {
                    tx.execute("DELETE FROM message_instances WHERE id=?", [&instance_id])
                        .map_err(AppError::from)?;
                    removed_instances += 1;
                }
                continue;
            }
            if has_pending_flags {
                continue;
            }
            let mut flags =
                serde_json::from_str::<Vec<String>>(&flags_json).map_err(AppError::from)?;
            update_flag(&mut flags, "\\Seen", Some(!unseen_uids.contains(&uid)));
            update_flag(&mut flags, "\\Flagged", Some(flagged_uids.contains(&uid)));
            let normalized =
                serde_json::to_string(&canonical_imap_flags(&flags)).map_err(AppError::from)?;
            if normalized != flags_json {
                tx.execute(
                    "UPDATE message_instances SET flags_json=?,last_synced_at=? WHERE id=?",
                    params![normalized, now, instance_id],
                )
                .map_err(AppError::from)?;
                updated_flags += 1;
            }
        }

        let unread_count = u32::try_from(unseen_uids.len())
            .map_err(|_| AppError::Protocol("IMAP 未读索引超过支持范围".into()))?;
        tx.execute(
            "UPDATE mailboxes SET total_count=?,unread_count=? WHERE id=? AND account_id=?",
            params![index.total_count, unread_count, mailbox_id, account_id],
        )
        .map_err(AppError::from)?;
        tx.execute(
            "DELETE FROM messages WHERE account_id=? AND NOT EXISTS (SELECT 1 FROM message_instances instance WHERE instance.message_id=messages.id)",
            [account_id],
        )
        .map_err(AppError::from)?;
        tx.execute(
            r#"UPDATE threads SET
                 message_count=(SELECT count(*) FROM messages WHERE thread_id=threads.id),
                 unread_count=(SELECT count(DISTINCT message.id) FROM messages message JOIN message_instances instance ON instance.message_id=message.id WHERE message.thread_id=threads.id AND instance.is_deleted=0 AND instr(lower(instance.flags_json),'"\\seen"')=0)
               WHERE account_id=?"#,
            [account_id],
        )
        .map_err(AppError::from)?;
        tx.execute(
            "DELETE FROM threads WHERE account_id=? AND NOT EXISTS (SELECT 1 FROM messages WHERE messages.thread_id=threads.id)",
            [account_id],
        )
        .map_err(AppError::from)?;
        tx.commit().map_err(AppError::from)?;
        Ok(ImapIndexReconcileResult {
            removed_instances,
            updated_flags,
        })
    }

    pub fn imap_sync_cursor(
        &self,
        account_id: &str,
        mailbox_remote_id: &str,
    ) -> Result<Option<(u32, u32)>, AppError> {
        let cursor = self
            .connection
            .query_row(
                r#"SELECT cursor.cursor_json
                   FROM sync_cursors cursor
                   JOIN mailboxes mailbox ON mailbox.id=cursor.mailbox_id
                   WHERE cursor.account_id=? AND mailbox.account_id=? AND mailbox.remote_id=? AND cursor.backend_kind='imap'"#,
                params![account_id, account_id, mailbox_remote_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(AppError::from)?;
        cursor
            .map(|value| {
                serde_json::from_str::<SyncCursor>(&value)
                    .map_err(AppError::from)
                    .and_then(|cursor| match cursor {
                        SyncCursor::Imap {
                            uid_validity,
                            last_uid,
                            ..
                        } => Ok((uid_validity, last_uid)),
                        _ => Err(AppError::InvalidConfiguration(
                            "IMAP 文件夹保存了不兼容的同步游标".into(),
                        )),
                    })
            })
            .transpose()
    }

    pub fn imap_sync_window(
        &self,
        account_id: &str,
        mailbox_remote_id: &str,
    ) -> Result<Option<ImapSyncWindow>, AppError> {
        let Some((uid_validity, last_uid)) =
            self.imap_sync_cursor(account_id, mailbox_remote_id)?
        else {
            return Ok(None);
        };
        let (oldest_uid, instance_count) = self
            .connection
            .query_row(
                r#"SELECT MIN(instance.uid),COUNT(*)
                   FROM message_instances instance
                   JOIN mailboxes mailbox ON mailbox.id=instance.mailbox_id
                   WHERE mailbox.account_id=? AND mailbox.remote_id=?
                     AND instance.uid_validity=? AND instance.uid IS NOT NULL
                     AND instance.is_deleted=0"#,
                params![account_id, mailbox_remote_id, uid_validity],
                |row| Ok((row.get::<_, Option<u32>>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(AppError::from)?;
        let instance_count = u32::try_from(instance_count)
            .map_err(|_| AppError::InvalidConfiguration("IMAP 本地实例计数超过支持范围".into()))?;
        Ok(Some(ImapSyncWindow {
            uid_validity,
            last_uid,
            oldest_uid,
            instance_count,
        }))
    }

    pub fn mark_account_sync_started(&mut self, account_id: &str) -> Result<(), AppError> {
        self.set_account_sync_metadata(account_id, "syncing", None, false)
    }

    pub fn mark_account_sync_completed(&mut self, account_id: &str) -> Result<(), AppError> {
        self.set_account_sync_metadata(account_id, "idle", None, true)
    }

    pub fn mark_account_sync_cancelled(&mut self, account_id: &str) -> Result<(), AppError> {
        self.set_account_sync_metadata(account_id, "idle", None, false)
    }

    /// The persisted cache remains usable while the listener reconnects, so distinguish a
    /// transient realtime transport loss from an authentication or data-sync failure.
    pub fn mark_account_sync_offline(&mut self, account_id: &str) -> Result<(), AppError> {
        self.set_account_sync_metadata(account_id, "offline", None, false)
    }

    pub fn mark_account_sync_failed(
        &mut self,
        account_id: &str,
        message: &str,
    ) -> Result<(), AppError> {
        self.set_account_sync_metadata(account_id, "error", Some(message), false)
    }

    fn set_account_sync_metadata(
        &mut self,
        account_id: &str,
        status: &str,
        error_message: Option<&str>,
        successful: bool,
    ) -> Result<(), AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let account_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?)",
                [account_id],
                |row| row.get(0),
            )
            .map_err(AppError::from)?;
        if !account_exists {
            return Err(AppError::not_found("account"));
        }
        let now = Utc::now().to_rfc3339();
        tx.execute(
            r#"INSERT INTO provider_metadata (account_id,key,value_json,updated_at) VALUES (?,'sync_status',?,?)
               ON CONFLICT(account_id,key) DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at"#,
            params![account_id, json!(status).to_string(), now],
        )
        .map_err(AppError::from)?;
        if successful {
            tx.execute(
                r#"INSERT INTO provider_metadata (account_id,key,value_json,updated_at) VALUES (?,'last_sync_success',?,?)
                   ON CONFLICT(account_id,key) DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at"#,
                params![account_id, json!(now.clone()).to_string(), now],
            )
            .map_err(AppError::from)?;
        }
        if let Some(error_message) = error_message {
            tx.execute(
                r#"INSERT INTO provider_metadata (account_id,key,value_json,updated_at) VALUES (?,'sync_error',?,?)
                   ON CONFLICT(account_id,key) DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at"#,
                params![account_id, json!(error_message).to_string(), now],
            )
            .map_err(AppError::from)?;
        } else {
            tx.execute(
                "DELETE FROM provider_metadata WHERE account_id=? AND key='sync_error'",
                [account_id],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)
    }

    pub fn list_messages(
        &self,
        mailbox_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, AppError> {
        self.list_messages_in_scope(None, mailbox_id, None, None, limit)
    }

    /// Queries cached messages across accounts without conflating an account with a mailbox.
    /// When no account is supplied, disabled accounts are excluded from aggregate views.
    pub fn list_messages_in_scope(
        &self,
        account_id: Option<&str>,
        mailbox_id: Option<&str>,
        mailbox_role: Option<&str>,
        is_starred: Option<bool>,
        limit: u32,
    ) -> Result<Vec<Message>, AppError> {
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at,m.created_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(lower(mi.flags_json),'"\\seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(lower(mi.flags_json),'"\\flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown'),COALESCE((SELECT json_group_array(json_object('name',display_name,'email',email)) FROM message_addresses WHERE message_id=m.id AND kind='to' ORDER BY position),'[]'),COALESCE((SELECT json_group_array(mailbox.display_name) FROM message_instances label_instance JOIN mailboxes mailbox ON mailbox.id=label_instance.mailbox_id WHERE label_instance.message_id=m.id AND label_instance.is_deleted=0 AND mailbox.selectable=1),'[]')
                     FROM messages m
                     JOIN accounts account ON account.id=m.account_id
                     JOIN message_instances mi ON mi.id=(
                       SELECT candidate.id
                       FROM message_instances candidate
                       JOIN mailboxes candidate_mailbox ON candidate_mailbox.id=candidate.mailbox_id
                       WHERE candidate.message_id=m.id
                         AND candidate_mailbox.account_id=m.account_id
                         AND candidate_mailbox.selectable=1
                         AND candidate.is_deleted=0
                         AND (?2 IS NULL OR candidate.mailbox_id=?2)
                         AND (?3 IS NULL OR candidate_mailbox.special_role=?3)
                         AND (?4 IS NULL OR (instr(lower(candidate.flags_json),'"\\flagged"') > 0)=?4)
                       ORDER BY candidate.last_synced_at DESC,candidate.id
                       LIMIT 1
                     )
                     WHERE (?1 IS NULL OR m.account_id=?1)
                       AND (?1 IS NOT NULL OR account.enabled=1)
                     ORDER BY julianday(COALESCE(m.received_at,m.sent_at,m.created_at)) DESC,COALESCE(m.received_at,m.sent_at,m.created_at) DESC,m.id
                     LIMIT ?5"#;
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let starred = is_starred.map(i64::from);
        let mapped = statement
            .query_map(
                params![account_id, mailbox_id, mailbox_role, starred, limit],
                message_from_row,
            )
            .map_err(AppError::from)?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)
    }

    pub fn search_messages_in_scope(
        &self,
        account_id: Option<&str>,
        mailbox_id: Option<&str>,
        mailbox_role: Option<&str>,
        is_starred: Option<bool>,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Message>, AppError> {
        let fts_query = build_fts_query(query);
        if fts_query.is_empty() {
            return self.list_messages_in_scope(
                account_id,
                mailbox_id,
                mailbox_role,
                is_starred,
                limit,
            );
        }
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at,m.created_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(lower(mi.flags_json),'"\\seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(lower(mi.flags_json),'"\\flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown'),COALESCE((SELECT json_group_array(json_object('name',display_name,'email',email)) FROM message_addresses WHERE message_id=m.id AND kind='to' ORDER BY position),'[]'),COALESCE((SELECT json_group_array(mailbox.display_name) FROM message_instances label_instance JOIN mailboxes mailbox ON mailbox.id=label_instance.mailbox_id WHERE label_instance.message_id=m.id AND label_instance.is_deleted=0 AND mailbox.selectable=1),'[]')
                     FROM message_fts f
                     JOIN messages m ON m.id=f.message_id
                     JOIN accounts account ON account.id=m.account_id
                     JOIN message_instances mi ON mi.id=(
                       SELECT candidate.id
                       FROM message_instances candidate
                       JOIN mailboxes candidate_mailbox ON candidate_mailbox.id=candidate.mailbox_id
                       WHERE candidate.message_id=m.id
                         AND candidate_mailbox.account_id=m.account_id
                         AND candidate_mailbox.selectable=1
                         AND candidate.is_deleted=0
                         AND (?3 IS NULL OR candidate.mailbox_id=?3)
                         AND (?4 IS NULL OR candidate_mailbox.special_role=?4)
                         AND (?5 IS NULL OR (instr(lower(candidate.flags_json),'"\\flagged"') > 0)=?5)
                       ORDER BY candidate.last_synced_at DESC,candidate.id
                       LIMIT 1
                     )
                     WHERE f.message_fts MATCH ?1
                       AND (?2 IS NULL OR m.account_id=?2)
                       AND (?2 IS NOT NULL OR account.enabled=1)
                     ORDER BY julianday(COALESCE(m.received_at,m.sent_at,m.created_at)) DESC,COALESCE(m.received_at,m.sent_at,m.created_at) DESC,m.id
                     LIMIT ?6"#;
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let starred = is_starred.map(i64::from);
        let mapped = statement
            .query_map(
                params![
                    fts_query,
                    account_id,
                    mailbox_id,
                    mailbox_role,
                    starred,
                    limit
                ],
                message_from_row,
            )
            .map_err(AppError::from)?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)
    }

    pub fn get_message(&self, message_id: &str) -> Result<Message, AppError> {
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at,m.created_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(lower(mi.flags_json),'"\\seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(lower(mi.flags_json),'"\\flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown'),COALESCE((SELECT json_group_array(json_object('name',display_name,'email',email)) FROM message_addresses WHERE message_id=m.id AND kind='to' ORDER BY position),'[]'),COALESCE((SELECT json_group_array(mailbox.display_name) FROM message_instances label_instance JOIN mailboxes mailbox ON mailbox.id=label_instance.mailbox_id WHERE label_instance.message_id=m.id AND label_instance.is_deleted=0 AND mailbox.selectable=1),'[]')
                     FROM messages m
                     JOIN message_instances mi ON mi.id=(
                       SELECT candidate.id
                       FROM message_instances candidate
                       JOIN mailboxes candidate_mailbox ON candidate_mailbox.id=candidate.mailbox_id
                       WHERE candidate.message_id=m.id
                         AND candidate_mailbox.account_id=m.account_id
                         AND candidate_mailbox.selectable=1
                         AND candidate.is_deleted=0
                       ORDER BY candidate.last_synced_at DESC,candidate.id LIMIT 1
                     )
                     WHERE m.id=?"#;
        self.connection
            .query_row(sql, [message_id], message_from_row)
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("message"))
    }

    pub fn get_message_in_mailbox(
        &self,
        message_id: &str,
        mailbox_id: &str,
    ) -> Result<Message, AppError> {
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at,m.created_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(lower(mi.flags_json),'"\\seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(lower(mi.flags_json),'"\\flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown'),COALESCE((SELECT json_group_array(json_object('name',display_name,'email',email)) FROM message_addresses WHERE message_id=m.id AND kind='to' ORDER BY position),'[]'),COALESCE((SELECT json_group_array(mailbox.display_name) FROM message_instances label_instance JOIN mailboxes mailbox ON mailbox.id=label_instance.mailbox_id WHERE label_instance.message_id=m.id AND label_instance.is_deleted=0 AND mailbox.selectable=1),'[]')
                     FROM messages m
                     JOIN message_instances mi ON mi.message_id=m.id AND mi.mailbox_id=?2 AND mi.is_deleted=0
                     JOIN mailboxes selected_mailbox ON selected_mailbox.id=mi.mailbox_id AND selected_mailbox.account_id=m.account_id AND selected_mailbox.selectable=1
                     WHERE m.id=?1
                     ORDER BY mi.last_synced_at DESC,mi.id
                     LIMIT 1"#;
        self.connection
            .query_row(sql, params![message_id, mailbox_id], message_from_row)
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("message instance"))
    }

    /// Resolves the exact active IMAP instance represented by the reader.
    /// Callers must not keep the database mutex locked while performing the
    /// network fetch; `store_hydrated_message_body` revalidates this locator.
    pub fn imap_body_locator(
        &self,
        message_id: &str,
        mailbox_id: &str,
    ) -> Result<ImapBodyLocator, AppError> {
        let row = self
            .connection
            .query_row(
                r#"SELECT m.id,mi.id,m.account_id,mailbox.id,mailbox.remote_id,mi.uid_validity,mi.uid,m.updated_at,
                          CASE WHEN m.body_cache_state='full' AND (m.has_attachment=0 OR EXISTS(SELECT 1 FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=m.id)) THEN 'full' ELSE 'none' END,
                          account.enabled
                   FROM messages m
                   JOIN accounts account ON account.id=m.account_id
                   JOIN message_instances mi ON mi.message_id=m.id
                     AND mi.mailbox_id=?2
                     AND mi.is_deleted=0
                     AND mi.uid IS NOT NULL
                   JOIN mailboxes mailbox ON mailbox.id=mi.mailbox_id AND mailbox.account_id=m.account_id AND mailbox.selectable=1
                   WHERE m.id=?1
                   ORDER BY mi.last_synced_at DESC,mi.id
                   LIMIT 1"#,
                params![message_id, mailbox_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<u32>>(5)?,
                        row.get::<_, u32>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, i64>(9)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("IMAP message instance"))?;
        Ok(ImapBodyLocator {
            message_id: row.0,
            message_instance_id: row.1,
            account_id: row.2,
            mailbox_id: row.3,
            mailbox_remote_id: row.4,
            uid_validity: row.5,
            uid: row.6,
            message_revision: row.7,
            body_cached: row.8 == "full",
            account_enabled: row.9,
        })
    }

    /// Persists a parsed body only if the local mailbox identity is unchanged
    /// since the network request began. This deliberately does not update an
    /// IMAP sync cursor or message-instance timestamp.
    pub fn store_hydrated_message_body(
        &mut self,
        locator: &ImapBodyLocator,
        body: &HydratedMessageBody,
    ) -> Result<Message, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let current = tx
            .query_row(
                r#"SELECT message.body_cache_state,message.updated_at
                   FROM message_instances instance
                   JOIN messages message ON message.id=instance.message_id
                   JOIN mailboxes mailbox ON mailbox.id=instance.mailbox_id
                   JOIN accounts account ON account.id=message.account_id
                   WHERE instance.id=?
                     AND instance.message_id=?
                     AND message.account_id=?
                     AND mailbox.account_id=message.account_id
                     AND mailbox.id=?
                     AND mailbox.remote_id=?
                     AND instance.uid=?
                     AND instance.uid_validity IS ?
                     AND instance.is_deleted=0
                     AND account.enabled=1"#,
                params![
                    locator.message_instance_id,
                    locator.message_id,
                    locator.account_id,
                    locator.mailbox_id,
                    locator.mailbox_remote_id,
                    locator.uid,
                    locator.uid_validity,
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(AppError::from)?;
        match current
            .as_ref()
            .map(|(cache_state, _)| cache_state.as_str())
        {
            None => {
                return Err(AppError::Protocol(
                    "邮件所在文件夹已在正文下载期间发生变化，请同步后重试".into(),
                ));
            }
            Some("full") => {
                tx.commit().map_err(AppError::from)?;
                return self.get_message_in_mailbox(&locator.message_id, &locator.mailbox_id);
            }
            Some(_) => {}
        }
        if current
            .as_ref()
            .is_some_and(|(_, revision)| revision != &locator.message_revision)
        {
            return Err(AppError::Protocol(
                "邮件元数据已在正文下载期间发生变化，请重试".into(),
            ));
        }

        let now = Utc::now().to_rfc3339();
        let updated = tx
            .execute(
                r#"UPDATE messages SET
                     preview=CASE WHEN ?='' THEN preview ELSE ? END,
                     body_text=?,
                     body_html_text=?,
                     has_attachment=MAX(has_attachment,?),
                     body_cache_state='full',
                     parse_warning=NULL,
                     updated_at=?
                   WHERE id=? AND account_id=? AND updated_at=?"#,
                params![
                    body.preview,
                    body.preview,
                    body.body_text,
                    body.body_html_text,
                    i64::from(body.has_attachment),
                    now,
                    locator.message_id,
                    locator.account_id,
                    locator.message_revision,
                ],
            )
            .map_err(AppError::from)?;
        if updated != 1 {
            return Err(AppError::not_found("message"));
        }
        tx.execute(
            "DELETE FROM message_parts WHERE message_id=?",
            [&locator.message_id],
        )
        .map_err(AppError::from)?;
        for (position, attachment) in body.attachments.iter().enumerate() {
            let part_id = Uuid::new_v4().to_string();
            let attachment_id = Uuid::new_v4().to_string();
            let filename = sanitize_attachment_filename(&attachment.filename, position);
            tx.execute(
                "INSERT INTO message_parts (id,message_id,mime_type,size_bytes,body_cache_state) VALUES (?,?,?,?, 'full')",
                params![part_id, locator.message_id, attachment.content_type, attachment.bytes.len() as i64],
            ).map_err(AppError::from)?;
            tx.execute(
                "INSERT INTO attachments (id,message_part_id,filename,sanitized_filename,content_type,size_bytes,download_state) VALUES (?,?,?,?,?,?,'downloaded')",
                params![attachment_id, part_id, attachment.filename, filename, attachment.content_type, attachment.bytes.len() as i64],
            ).map_err(AppError::from)?;
            tx.execute(
                "INSERT INTO attachment_payloads (attachment_id,bytes) VALUES (?,?)",
                params![attachment_id, attachment.bytes],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)?;
        let mut message = self.get_message_in_mailbox(&locator.message_id, &locator.mailbox_id)?;
        message.attachments = self.list_attachments(&locator.message_id)?;
        message.attachment_count = message.attachments.len() as i64;
        Ok(message)
    }

    pub fn list_attachments(&self, message_id: &str) -> Result<Vec<AttachmentInfo>, AppError> {
        let mut statement = self.connection.prepare(
            "SELECT attachment.id,COALESCE(attachment.sanitized_filename,attachment.filename,'attachment'),attachment.content_type,COALESCE(attachment.size_bytes,0) FROM attachments attachment JOIN message_parts part ON part.id=attachment.message_part_id WHERE part.message_id=? ORDER BY part.rowid,attachment.rowid",
        ).map_err(AppError::from)?;
        let attachments = statement
            .query_map([message_id], |row| {
                Ok(AttachmentInfo {
                    id: row.get(0)?,
                    filename: row.get(1)?,
                    content_type: row.get(2)?,
                    size_bytes: row.get(3)?,
                })
            })
            .map_err(AppError::from)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)?;
        Ok(attachments)
    }

    pub fn attachment_payload(
        &self,
        attachment_id: &str,
    ) -> Result<(AttachmentInfo, Vec<u8>), AppError> {
        self.connection.query_row(
            "SELECT attachment.id,COALESCE(attachment.sanitized_filename,attachment.filename,'attachment'),attachment.content_type,COALESCE(attachment.size_bytes,0),payload.bytes FROM attachments attachment JOIN attachment_payloads payload ON payload.attachment_id=attachment.id WHERE attachment.id=?",
            [attachment_id],
            |row| Ok((AttachmentInfo { id: row.get(0)?, filename: row.get(1)?, content_type: row.get(2)?, size_bytes: row.get(3)? }, row.get(4)?)),
        ).optional().map_err(AppError::from)?.ok_or_else(|| AppError::not_found("attachment"))
    }

    /// Claims a bounded batch of durable IMAP mutations for one account. The
    /// source mailbox and UID are resolved in the same transaction that marks
    /// each returned operation as `sending`.
    pub fn claim_pending_imap_operations(
        &mut self,
        account_id: &str,
        limit: u32,
    ) -> Result<Vec<PendingImapOperation>, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let account_exists = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=? AND enabled=1)",
                [account_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(AppError::from)?;
        if !account_exists {
            return Err(AppError::not_found("enabled account"));
        }
        let now = Utc::now().to_rfc3339();
        // Claim only one mutation at a time. If network execution fails or is
        // cancelled, no unvisited sibling is stranded in `sending`.
        let candidates = {
            let mut statement = tx
                .prepare(
                    r#"SELECT operation.id,operation.operation_type,operation.mailbox_id,
                              operation.payload_json,instance.uid_validity,instance.uid,instance.mailbox_id
                       FROM pending_operations operation
                       JOIN message_instances instance ON instance.id=operation.message_instance_id
                       JOIN messages message ON message.id=instance.message_id AND message.account_id=operation.account_id
                       JOIN mailboxes mailbox ON mailbox.id=instance.mailbox_id AND mailbox.account_id=operation.account_id
                       WHERE operation.account_id=?
                         AND operation.state IN ('pending','failed')
                         AND (operation.next_attempt_at IS NULL OR julianday(operation.next_attempt_at)<=julianday(?))
                         AND instance.uid IS NOT NULL
                       ORDER BY operation.created_at,operation.id
                       LIMIT ?"#,
                )
                .map_err(AppError::from)?;
            let rows = statement
                .query_map(params![account_id, now, limit.min(1)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<u32>>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(AppError::from)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)?
        };

        let mut claimed = Vec::with_capacity(candidates.len());
        for (
            operation_id,
            operation_type,
            operation_mailbox_id,
            payload_json,
            uid_validity,
            uid,
            current_mailbox_id,
        ) in candidates
        {
            let payload = match serde_json::from_str::<serde_json::Value>(&payload_json) {
                Ok(payload) => payload,
                Err(_) => {
                    mark_pending_operation_conflicted(&tx, &operation_id, "invalid_payload")?;
                    continue;
                }
            };
            let source_mailbox_id = payload
                .get("from_mailbox_id")
                .and_then(serde_json::Value::as_str)
                .or(operation_mailbox_id.as_deref())
                .unwrap_or(&current_mailbox_id);
            let source_remote_id = tx
                .query_row(
                    "SELECT remote_id FROM mailboxes WHERE id=? AND account_id=?",
                    params![source_mailbox_id, account_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?;
            let Some(source_mailbox_remote_id) = source_remote_id else {
                mark_pending_operation_conflicted(&tx, &operation_id, "missing_source_mailbox")?;
                continue;
            };

            let explicit_target_id = payload
                .get("to_mailbox_id")
                .and_then(serde_json::Value::as_str);
            let target_mailbox_remote_id = if let Some(target_id) = explicit_target_id {
                tx.query_row(
                    "SELECT remote_id FROM mailboxes WHERE id=? AND account_id=?",
                    params![target_id, account_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
            } else if operation_type == "trash" {
                tx.query_row(
                    "SELECT remote_id FROM mailboxes WHERE account_id=? AND special_role='trash' AND selectable=1 ORDER BY name LIMIT 1",
                    [account_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(AppError::from)?
            } else {
                None
            };
            if operation_type == "move" && target_mailbox_remote_id.is_none() {
                mark_pending_operation_conflicted(&tx, &operation_id, "missing_target_mailbox")?;
                continue;
            }

            let changed = tx
                .execute(
                    "UPDATE pending_operations SET state='sending',last_error_code=NULL,updated_at=? WHERE id=? AND state IN ('pending','failed')",
                    params![now, operation_id],
                )
                .map_err(AppError::from)?;
            if changed == 1 {
                claimed.push(PendingImapOperation {
                    id: operation_id,
                    account_id: account_id.to_owned(),
                    operation_type,
                    source_mailbox_remote_id,
                    uid,
                    uid_validity,
                    target_mailbox_remote_id,
                    payload_json: payload,
                });
            }
        }
        tx.commit().map_err(AppError::from)?;
        Ok(claimed)
    }

    pub fn complete_pending_operation(&mut self, operation_id: &str) -> Result<(), AppError> {
        let deleted = self
            .connection
            .execute(
                "DELETE FROM pending_operations WHERE id=? AND state='sending'",
                [operation_id],
            )
            .map_err(AppError::from)?;
        if deleted == 1 {
            Ok(())
        } else {
            Err(AppError::InvalidConfiguration(
                "待同步操作不在可完成状态".into(),
            ))
        }
    }

    pub fn fail_pending_operation(
        &mut self,
        operation_id: &str,
        error_code: &str,
        retryable: bool,
    ) -> Result<(), AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let operation = tx
            .query_row(
                "SELECT operation_type,message_instance_id FROM pending_operations WHERE id=? AND state='sending'",
                [operation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| {
                AppError::InvalidConfiguration("待同步操作不在可失败状态".into())
            })?;
        tx.execute(
            "UPDATE pending_operations SET state=?,retry_count=retry_count+1,next_attempt_at=NULL,last_error_code=?,updated_at=? WHERE id=? AND state='sending'",
            params![
                if retryable { "failed" } else { "conflicted" },
                error_code,
                Utc::now().to_rfc3339(),
                operation_id
            ],
        )
        .map_err(AppError::from)?;
        if !retryable && matches!(operation.0.as_str(), "move" | "trash" | "permanent_delete") {
            if let Some(instance_id) = operation.1 {
                restore_message_instance(&tx, &instance_id)?;
            }
        }
        tx.commit().map_err(AppError::from)
    }

    pub fn mutate_message(
        &mut self,
        message_id: &str,
        mailbox_id: &str,
        is_read: Option<bool>,
        is_starred: Option<bool>,
    ) -> Result<Message, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let instance: Option<(String, String, String)> = tx
            .query_row(
                "SELECT mi.id,m.account_id,mi.flags_json FROM message_instances mi JOIN messages m ON m.id=mi.message_id JOIN mailboxes mailbox ON mailbox.id=mi.mailbox_id AND mailbox.account_id=m.account_id AND mailbox.selectable=1 WHERE mi.message_id=? AND mi.mailbox_id=? AND mi.is_deleted=0",
                params![message_id, mailbox_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(AppError::from)?;
        let (instance_id, account_id, mut flags_json) =
            instance.ok_or_else(|| AppError::not_found("message instance"))?;
        let mut flags: Vec<String> = serde_json::from_str(&flags_json).unwrap_or_default();
        let was_read = flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
        update_flag(&mut flags, "\\Seen", is_read);
        update_flag(&mut flags, "\\Flagged", is_starred);
        let is_now_read = flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
        flags_json = serde_json::to_string(&flags).map_err(AppError::from)?;
        tx.execute(
            "UPDATE message_instances SET flags_json=?,last_synced_at=? WHERE id=?",
            params![flags_json, Utc::now().to_rfc3339(), instance_id],
        )
        .map_err(AppError::from)?;
        if was_read != is_now_read {
            tx.execute(
                "UPDATE mailboxes SET unread_count=MAX(0,unread_count+?) WHERE id=? AND account_id=?",
                params![if is_now_read { -1 } else { 1 }, mailbox_id, account_id],
            )
            .map_err(AppError::from)?;
            tx.execute(
                r#"UPDATE threads SET unread_count=(SELECT count(DISTINCT message.id) FROM messages message JOIN message_instances instance ON instance.message_id=message.id WHERE message.thread_id=threads.id AND instance.is_deleted=0 AND instr(lower(instance.flags_json),'"\\seen"')=0) WHERE id=(SELECT thread_id FROM messages WHERE id=?)"#,
                [message_id],
            )
            .map_err(AppError::from)?;
        }
        tx.execute("INSERT INTO pending_operations (id,account_id,mailbox_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, mailbox_id, instance_id, "set_flags", json!({ "is_read": is_read, "is_starred": is_starred }).to_string(), Utc::now().to_rfc3339(), Utc::now().to_rfc3339()]).map_err(AppError::from)?;
        tx.commit().map_err(AppError::from)?;
        self.get_message_in_mailbox(message_id, mailbox_id)
    }

    /// Applies one flag mutation to every selected message in a single local
    /// transaction. Each successful local change is queued as a durable IMAP
    /// operation, so the selection can be updated offline and is later pushed
    /// to the server by the normal sync pipeline.
    pub fn mutate_messages(
        &mut self,
        message_refs: &[(String, String)],
        is_read: Option<bool>,
        is_starred: Option<bool>,
    ) -> Result<usize, AppError> {
        if is_read.is_none() && is_starred.is_none() {
            return Err(AppError::InvalidConfiguration(
                "批量状态更新至少需要一个字段".into(),
            ));
        }

        let tx = self.connection.transaction().map_err(AppError::from)?;
        let now = Utc::now().to_rfc3339();
        let mut mutated = 0;
        let mut seen = HashSet::new();

        for (message_id, mailbox_id) in message_refs {
            // A selection is normally unique in the UI. De-duplicate anyway
            // so malformed callers cannot queue the same remote mutation more
            // than once in this transaction.
            if !seen.insert((message_id.as_str(), mailbox_id.as_str())) {
                continue;
            }

            let instance: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT mi.id,m.account_id,mi.flags_json FROM message_instances mi JOIN messages m ON m.id=mi.message_id JOIN mailboxes mailbox ON mailbox.id=mi.mailbox_id AND mailbox.account_id=m.account_id AND mailbox.selectable=1 WHERE mi.message_id=? AND mi.mailbox_id=? AND mi.is_deleted=0",
                    params![message_id, mailbox_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(AppError::from)?;
            let (instance_id, account_id, flags_json) =
                instance.ok_or_else(|| AppError::not_found("message instance"))?;
            let mut flags: Vec<String> = serde_json::from_str(&flags_json).unwrap_or_default();
            let was_read = flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
            update_flag(&mut flags, "\\Seen", is_read);
            update_flag(&mut flags, "\\Flagged", is_starred);
            let is_now_read = flags.iter().any(|flag| flag.eq_ignore_ascii_case("\\Seen"));
            let next_flags_json = serde_json::to_string(&flags).map_err(AppError::from)?;

            tx.execute(
                "UPDATE message_instances SET flags_json=?,last_synced_at=? WHERE id=?",
                params![next_flags_json, now, instance_id],
            )
            .map_err(AppError::from)?;
            if was_read != is_now_read {
                tx.execute(
                    "UPDATE mailboxes SET unread_count=MAX(0,unread_count+?) WHERE id=? AND account_id=?",
                    params![if is_now_read { -1 } else { 1 }, mailbox_id, account_id],
                )
                .map_err(AppError::from)?;
                tx.execute(
                    r#"UPDATE threads SET unread_count=(SELECT count(DISTINCT message.id) FROM messages message JOIN message_instances instance ON instance.message_id=message.id WHERE message.thread_id=threads.id AND instance.is_deleted=0 AND instr(lower(instance.flags_json),'"\\seen"')=0) WHERE id=(SELECT thread_id FROM messages WHERE id=?)"#,
                    [message_id],
                )
                .map_err(AppError::from)?;
            }
            tx.execute(
                "INSERT INTO pending_operations (id,account_id,mailbox_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)",
                params![Uuid::new_v4().to_string(), account_id, mailbox_id, instance_id, "set_flags", json!({ "is_read": is_read, "is_starred": is_starred }).to_string(), now, now],
            )
            .map_err(AppError::from)?;
            mutated += 1;
        }

        tx.commit().map_err(AppError::from)?;
        Ok(mutated)
    }

    pub fn move_messages(
        &mut self,
        message_refs: &[(String, String)],
        target_mailbox_id: &str,
    ) -> Result<usize, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let target_account_id: String = tx
            .query_row(
                "SELECT account_id FROM mailboxes WHERE id=? AND selectable=1",
                [target_mailbox_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("target mailbox"))?;
        let now = Utc::now().to_rfc3339();
        let mut moved = 0;
        let mut seen = HashSet::new();
        for (message_id, source_mailbox_id) in message_refs {
            if !seen.insert((message_id, source_mailbox_id)) { continue; }
            let instance: Option<(String, String)> = tx
                .query_row("SELECT mi.id,m.account_id FROM message_instances mi JOIN messages m ON m.id=mi.message_id JOIN mailboxes mailbox ON mailbox.id=mi.mailbox_id AND mailbox.account_id=m.account_id AND mailbox.selectable=1 WHERE mi.message_id=? AND mi.mailbox_id=? AND mi.is_deleted=0", params![message_id, source_mailbox_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(AppError::from)?;
            if let Some((instance_id, account_id)) = instance {
                if account_id != target_account_id {
                    return Err(AppError::InvalidConfiguration(
                        "不能将邮件移动到其他账户的文件夹".into(),
                    ));
                }
                if source_mailbox_id == target_mailbox_id {
                    continue;
                }
                hide_message_instance(&tx, &instance_id, source_mailbox_id, message_id, &now)?;
                tx.execute("INSERT INTO pending_operations (id,account_id,mailbox_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, source_mailbox_id, instance_id, "move", json!({ "from_mailbox_id": source_mailbox_id, "to_mailbox_id": target_mailbox_id, "message_id": message_id }).to_string(), now, now]).map_err(AppError::from)?;
                moved += 1;
            } else { return Err(AppError::not_found("message instance")); }
        }
        tx.commit().map_err(AppError::from)?;
        Ok(moved)
    }

    pub fn delete_messages(
        &mut self,
        message_refs: &[(String, String)],
        permanent: bool,
    ) -> Result<usize, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let now = Utc::now().to_rfc3339();
        let mut deleted = 0;
        let mut seen = HashSet::new();
        for (message_id, mailbox_id) in message_refs {
            if !seen.insert((message_id, mailbox_id)) { continue; }
            let instance: Option<(String, String)> = tx
                .query_row("SELECT mi.id,m.account_id FROM message_instances mi JOIN messages m ON m.id=mi.message_id JOIN mailboxes mailbox ON mailbox.id=mi.mailbox_id AND mailbox.account_id=m.account_id AND mailbox.selectable=1 WHERE mi.message_id=? AND mi.mailbox_id=? AND mi.is_deleted=0", params![message_id, mailbox_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(AppError::from)?;
            if let Some((instance_id, account_id)) = instance {
                if !permanent {
                    let has_trash: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM mailboxes WHERE account_id=? AND special_role='trash' AND selectable=1 AND id<>?)",
                        params![account_id, mailbox_id], |row| row.get(0),
                    ).map_err(AppError::from)?;
                    if !has_trash {
                        return Err(AppError::InvalidConfiguration("没有可用的回收站目标，邮件未被删除".into()));
                    }
                }
                hide_message_instance(&tx, &instance_id, mailbox_id, message_id, &now)?;
                tx.execute("INSERT INTO pending_operations (id,account_id,mailbox_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, mailbox_id, instance_id, if permanent { "permanent_delete" } else { "trash" }, json!({ "message_id": message_id, "mailbox_id": mailbox_id, "permanent": permanent }).to_string(), now, now]).map_err(AppError::from)?;
                deleted += 1;
            } else { return Err(AppError::not_found("message instance")); }
        }
        tx.commit().map_err(AppError::from)?;
        Ok(deleted)
    }

    pub fn save_draft(&mut self, input: &DraftInput) -> Result<String, AppError> {
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let protected: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM drafts WHERE id=?1 AND (account_id<>?2 OR EXISTS(SELECT 1 FROM outbox WHERE draft_id=drafts.id)))",
            params![id, input.account_id], |row| row.get(0),
        ).map_err(AppError::from)?;
        if protected {
            return Err(AppError::InvalidConfiguration("不能覆盖其他账户或发件队列中的草稿".into()));
        }
        let now = Utc::now().to_rfc3339();
        self.connection.execute("INSERT INTO drafts (id,account_id,to_json,cc_json,bcc_json,subject,body_text,in_reply_to,references_json,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET to_json=excluded.to_json,cc_json=excluded.cc_json,bcc_json=excluded.bcc_json,subject=excluded.subject,body_text=excluded.body_text,in_reply_to=excluded.in_reply_to,references_json=excluded.references_json,updated_at=excluded.updated_at", params![id, input.account_id, serde_json::to_string(&split_addresses(&input.to)).map_err(AppError::from)?, serde_json::to_string(&split_addresses(input.cc.as_deref().unwrap_or(""))).map_err(AppError::from)?, serde_json::to_string(&split_addresses(input.bcc.as_deref().unwrap_or(""))).map_err(AppError::from)?, input.subject, input.body_text, input.in_reply_to, serde_json::to_string(&input.references.clone().unwrap_or_default()).map_err(AppError::from)?, now]).map_err(AppError::from)?;
        Ok(id)
    }

    #[cfg(test)]
    pub fn queue_draft(&mut self, input: &DraftInput) -> Result<String, AppError> {
        self.queue_draft_with_payload(input, None)
    }

    pub fn queue_prepared_draft(
        &mut self,
        input: &DraftInput,
        payload: &PreparedOutboxPayload,
    ) -> Result<String, AppError> {
        self.queue_draft_with_payload(input, Some(payload))
    }

    fn queue_draft_with_payload(
        &mut self,
        input: &DraftInput,
        payload: Option<&PreparedOutboxPayload>,
    ) -> Result<String, AppError> {
        // An outbox item owns an immutable snapshot. Reusing the editable draft id would
        // allow a later autosave to change the bytes a queued worker is about to send.
        let draft_id = Uuid::new_v4().to_string();
        let outbox_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let to_json = serde_json::to_string(&split_addresses(&input.to)).map_err(AppError::from)?;
        let cc_json = serde_json::to_string(&split_addresses(input.cc.as_deref().unwrap_or("")))
            .map_err(AppError::from)?;
        let bcc_json = serde_json::to_string(&split_addresses(input.bcc.as_deref().unwrap_or("")))
            .map_err(AppError::from)?;
        let references_json = serde_json::to_string(&input.references.clone().unwrap_or_default())
            .map_err(AppError::from)?;
        let tx = self.connection.transaction().map_err(AppError::from)?;
        tx.execute(
            "INSERT INTO drafts (id,account_id,to_json,cc_json,bcc_json,subject,body_text,in_reply_to,references_json,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![draft_id, input.account_id, to_json, cc_json, bcc_json, input.subject, input.body_text, input.in_reply_to, references_json, now],
        )
        .map_err(AppError::from)?;
        tx.execute(
            "INSERT INTO outbox (id,draft_id,account_id,state,created_at,updated_at) VALUES (?,?,?,'queued',?,?)",
            params![outbox_id, draft_id, input.account_id, now, now],
        )
        .map_err(AppError::from)?;
        if let Some(payload) = payload {
            let recipients_json =
                serde_json::to_string(&payload.recipients).map_err(AppError::from)?;
            tx.execute(
                r#"INSERT INTO outbox_payloads
                   (outbox_id,envelope_from,recipients_json,mime,rfc_message_id,sent_copy_state,sent_copy_error_message,sent_copy_uid_validity,sent_copy_uid,created_at,updated_at)
                   VALUES (?,?,?,?,?,'not_started',NULL,NULL,NULL,?,?)"#,
                params![
                    outbox_id,
                    payload.envelope_from,
                    recipients_json,
                    payload.mime,
                    payload.rfc_message_id,
                    now,
                    now,
                ],
            )
            .map_err(AppError::from)?;
        }
        if let Some(source_draft_id) = input.id.as_deref() {
            // Sending moves the editable draft into an immutable outbox snapshot. Delete
            // the source only when it belongs to this account and no existing queue item
            // still owns it; both changes commit together.
            tx.execute(
                "DELETE FROM drafts
                 WHERE id=? AND account_id=?
                   AND NOT EXISTS (SELECT 1 FROM outbox WHERE draft_id=drafts.id)",
                params![source_draft_id, input.account_id],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)?;
        Ok(outbox_id)
    }

    pub fn list_outbox(&self, account_id: Option<&str>) -> Result<Vec<OutboxItem>, AppError> {
        let sql = "SELECT o.id,o.account_id,COALESCE(d.subject,''),COALESCE(d.to_json,'[]'),o.state,o.last_error_code,o.last_error_message,p.sent_copy_state,p.sent_copy_error_message,o.updated_at FROM outbox o LEFT JOIN drafts d ON d.id=o.draft_id LEFT JOIN outbox_payloads p ON p.outbox_id=o.id WHERE (?1 IS NULL OR o.account_id=?1) ORDER BY o.updated_at DESC";
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let rows = statement
            .query_map([account_id], |row| {
                let recipients_json: String = row.get(3)?;
                Ok(OutboxItem {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    subject: row.get(2)?,
                    recipients: serde_json::from_str(&recipients_json).unwrap_or_default(),
                    state: row.get(4)?,
                    last_error_code: row.get(5)?,
                    last_error_message: row.get(6)?,
                    sent_copy_state: row.get(7)?,
                    sent_copy_error_message: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Returns only work that has never started. `sending`, `sent`, and
    /// `outcome_unknown` must never be auto-dispatched on startup because doing so could
    /// duplicate a message already accepted by the SMTP server.
    pub fn queued_outbox_ids(&self) -> Result<Vec<String>, AppError> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM outbox WHERE state='queued' ORDER BY created_at,id")
            .map_err(AppError::from)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn outbox_draft(&self, outbox_id: &str) -> Result<OutboxDraft, AppError> {
        let row: Option<OutboxDraftRow> = self
            .connection
            .query_row(
                "SELECT o.account_id,COALESCE(d.to_json,'[]'),COALESCE(d.cc_json,'[]'),COALESCE(d.bcc_json,'[]'),COALESCE(d.subject,''),COALESCE(d.body_text,''),d.in_reply_to,COALESCE(d.references_json,'[]') FROM outbox o LEFT JOIN drafts d ON d.id=o.draft_id WHERE o.id=?",
                [outbox_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(AppError::from)?;
        let (account_id, to, cc, bcc, subject, body_text, in_reply_to, references) =
            row.ok_or_else(|| AppError::not_found("outbox item"))?;
        Ok(OutboxDraft {
            account_id,
            to: serde_json::from_str(&to).unwrap_or_default(),
            cc: serde_json::from_str(&cc).unwrap_or_default(),
            bcc: serde_json::from_str(&bcc).unwrap_or_default(),
            subject,
            body_text,
            in_reply_to,
            references: serde_json::from_str(&references).unwrap_or_default(),
        })
    }

    pub fn outbox_payload(
        &self,
        outbox_id: &str,
    ) -> Result<Option<PreparedOutboxPayload>, AppError> {
        let row = self
            .connection
            .query_row(
                r#"SELECT envelope_from,recipients_json,mime,rfc_message_id,
                          sent_copy_state,sent_copy_error_message,
                          sent_copy_uid_validity,sent_copy_uid
                   FROM outbox_payloads WHERE outbox_id=?"#,
                [outbox_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<u32>>(6)?,
                        row.get::<_, Option<u32>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(AppError::from)?;
        row.map(
            |(
                envelope_from,
                recipients_json,
                mime,
                rfc_message_id,
                sent_copy_state,
                sent_copy_error_message,
                sent_copy_uid_validity,
                sent_copy_uid,
            )| {
                Ok(PreparedOutboxPayload {
                    envelope_from,
                    recipients: serde_json::from_str(&recipients_json).map_err(AppError::from)?,
                    mime,
                    rfc_message_id,
                    sent_copy_state,
                    sent_copy_error_message,
                    sent_copy_uid_validity,
                    sent_copy_uid,
                })
            },
        )
        .transpose()
    }

    /// Persists a payload for a legacy queued row that predates
    /// `outbox_payloads`. `INSERT OR IGNORE` means a retry can never replace a
    /// previously chosen Message-ID or MIME body.
    pub fn store_outbox_payload_if_missing(
        &mut self,
        outbox_id: &str,
        payload: &PreparedOutboxPayload,
    ) -> Result<PreparedOutboxPayload, AppError> {
        let recipients_json = serde_json::to_string(&payload.recipients).map_err(AppError::from)?;
        let now = Utc::now().to_rfc3339();
        self.connection
            .execute(
                r#"INSERT OR IGNORE INTO outbox_payloads
                   (outbox_id,envelope_from,recipients_json,mime,rfc_message_id,sent_copy_state,sent_copy_error_message,sent_copy_uid_validity,sent_copy_uid,created_at,updated_at)
                   SELECT id,?,?,?,?,'not_started',NULL,NULL,NULL,?,?
                   FROM outbox WHERE id=? AND state='sending'"#,
                params![
                    payload.envelope_from,
                    recipients_json,
                    payload.mime,
                    payload.rfc_message_id,
                    now,
                    now,
                    outbox_id,
                ],
            )
            .map_err(AppError::from)?;
        self.outbox_payload(outbox_id)?
            .ok_or_else(|| AppError::not_found("outbox payload"))
    }

    /// Records SMTP acceptance and the independent remote-Sent reconciliation
    /// state in one transaction. If either write fails, callers must suppress
    /// the success event; they must never send the SMTP message again.
    pub fn complete_outbox_sent(&mut self, outbox_id: &str) -> Result<String, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let has_incoming: bool = tx
            .query_row(
                r#"SELECT EXISTS(
                     SELECT 1 FROM outbox o
                     JOIN accounts a ON a.id=o.account_id
                     JOIN incoming_endpoints endpoint ON endpoint.account_id=a.id
                     WHERE o.id=? AND a.enabled=1 AND a.incoming_secret_ref IS NOT NULL
                   )"#,
                [outbox_id],
                |row| row.get(0),
            )
            .map_err(AppError::from)?;
        let sent_copy_state = if has_incoming {
            "awaiting_server_sync"
        } else {
            "unavailable"
        };
        let now = Utc::now().to_rfc3339();
        let changed = tx
            .execute(
                "UPDATE outbox SET state='sent',last_error_code=NULL,last_error_message=NULL,updated_at=? WHERE id=? AND state='sending'",
                params![now, outbox_id],
            )
            .map_err(AppError::from)?;
        if changed != 1 {
            let current = tx
                .query_row("SELECT state FROM outbox WHERE id=?", [outbox_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(AppError::from)?;
            return match current {
                Some(current) => Err(AppError::InvalidConfiguration(format!(
                    "发件队列状态不能从 {current} 变为 sent"
                ))),
                None => Err(AppError::not_found("outbox item")),
            };
        }
        let payload_changed = tx
            .execute(
                "UPDATE outbox_payloads SET sent_copy_state=?,sent_copy_error_message=NULL,sent_copy_uid_validity=NULL,sent_copy_uid=NULL,updated_at=? WHERE outbox_id=?",
                params![sent_copy_state, now, outbox_id],
            )
            .map_err(AppError::from)?;
        if payload_changed != 1 {
            return Err(AppError::Internal(
                "outbox payload missing while recording SMTP success".into(),
            ));
        }
        tx.commit().map_err(AppError::from)?;
        Ok(sent_copy_state.into())
    }

    pub fn set_outbox_state(
        &mut self,
        outbox_id: &str,
        state: &str,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), AppError> {
        let allowed_source = match state {
            "queued" => "state IN ('failed','outcome_unknown')",
            "sending" => "state='queued'",
            "sent" | "failed" | "outcome_unknown" => "state='sending'",
            "cancelled" => "state IN ('queued','failed','outcome_unknown')",
            _ => {
                return Err(AppError::InvalidConfiguration(format!(
                    "未知的发件队列状态：{state}"
                )))
            }
        };
        let statement = format!(
            "UPDATE outbox SET state=?,last_error_code=?,last_error_message=?,updated_at=? WHERE id=? AND {allowed_source}"
        );
        let changed = self
            .connection
            .execute(
                &statement,
                params![
                    state,
                    error_code,
                    error_message,
                    Utc::now().to_rfc3339(),
                    outbox_id
                ],
            )
            .map_err(AppError::from)?;
        if changed > 0 {
            return Ok(());
        }

        let current = self
            .connection
            .query_row("SELECT state FROM outbox WHERE id=?", [outbox_id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(AppError::from)?;
        match current {
            Some(current) => Err(AppError::InvalidConfiguration(format!(
                "发件队列状态不能从 {current} 变为 {state}"
            ))),
            None => Err(AppError::not_found("outbox item")),
        }
    }

    /// Atomically claims a queued item for one delivery worker. A `false` result means
    /// another worker or a cancellation already won the race.
    pub fn claim_outbox_for_sending(&mut self, outbox_id: &str) -> Result<bool, AppError> {
        let changed = self
            .connection
            .execute(
                "UPDATE outbox SET state='sending',last_error_code=NULL,last_error_message=NULL,updated_at=? WHERE id=? AND state='queued'",
                params![Utc::now().to_rfc3339(), outbox_id],
            )
            .map_err(AppError::from)?;
        Ok(changed == 1)
    }

    pub fn load_draft(&self, draft_id: &str) -> Result<DraftInput, AppError> {
        let row: Option<DraftRow> =
            self.connection
                .query_row(
                    "SELECT id,account_id,to_json,cc_json,bcc_json,subject,body_text,in_reply_to,references_json FROM drafts WHERE id=?",
                    [draft_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(AppError::from)?;
        let (id, account_id, to, cc, bcc, subject, body_text, in_reply_to, references) =
            row.ok_or_else(|| AppError::not_found("draft"))?;
        let addresses = |json: &str| -> String {
            serde_json::from_str::<Vec<String>>(json)
                .unwrap_or_default()
                .join(", ")
        };
        Ok(DraftInput {
            id: Some(id),
            account_id,
            to: addresses(&to),
            cc: Some(addresses(&cc)).filter(|value| !value.is_empty()),
            bcc: Some(addresses(&bcc)).filter(|value| !value.is_empty()),
            subject,
            body_text,
            in_reply_to,
            references: serde_json::from_str(&references).unwrap_or_default(),
        })
    }

    pub fn delete_draft(&mut self, draft_id: &str) -> Result<(), AppError> {
        let deleted = self
            .connection
            .execute("DELETE FROM drafts WHERE id=? AND NOT EXISTS (SELECT 1 FROM outbox WHERE draft_id=drafts.id)", [draft_id])
            .map_err(AppError::from)?;
        if deleted == 0 {
            Err(AppError::not_found("draft"))
        } else {
            Ok(())
        }
    }

    pub fn set_mailbox_sync_enabled(
        &mut self,
        mailbox_id: &str,
        sync_enabled: bool,
    ) -> Result<(), AppError> {
        let changed = self
            .connection
            .execute(
                "UPDATE mailboxes SET sync_enabled=? WHERE id=?",
                params![sync_enabled, mailbox_id],
            )
            .map_err(AppError::from)?;
        if changed == 0 {
            Err(AppError::not_found("mailbox"))
        } else {
            Ok(())
        }
    }

    pub fn update_account(
        &mut self,
        account_id: &str,
        patch: &serde_json::Value,
    ) -> Result<Account, AppError> {
        let object = patch
            .as_object()
            .ok_or_else(|| AppError::InvalidConfiguration("账户更新必须是 JSON 对象".into()))?;
        let display_name = object
            .get("displayName")
            .and_then(serde_json::Value::as_str);
        let enabled = object.get("enabled").and_then(serde_json::Value::as_bool);
        let sync_policy = object.get("syncPolicy").and_then(serde_json::Value::as_str);
        if display_name.is_none() && enabled.is_none() && sync_policy.is_none() {
            return Err(AppError::InvalidConfiguration(
                "没有可更新的账户字段".into(),
            ));
        }
        if let Some(policy) = sync_policy {
            if !matches!(policy, "automatic" | "manual" | "paused") {
                return Err(AppError::InvalidConfiguration(
                    "syncPolicy 必须是 automatic、manual 或 paused".into(),
                ));
            }
        }
        let changed = self
            .connection
            .execute(
                "UPDATE accounts SET display_name=COALESCE(?1,display_name),enabled=COALESCE(?2,enabled),sync_policy=COALESCE(?3,sync_policy),updated_at=?4 WHERE id=?5",
                params![display_name, enabled.map(i64::from), sync_policy, Utc::now().to_rfc3339(), account_id],
            )
            .map_err(AppError::from)?;
        if changed == 0 {
            return Err(AppError::not_found("account"));
        }
        self.list_accounts()?
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| AppError::not_found("account"))
    }

    pub fn get_settings(&self) -> Result<serde_json::Value, AppError> {
        let mut settings = serde_json::Map::from_iter([
            ("theme".into(), json!("system")),
            ("colorScheme".into(), json!("matcha")),
            ("customThemeSeed".into(), json!("#3F6654")),
            ("androidDynamicColor".into(), json!(false)),
            ("safeReading".into(), json!(true)),
            ("syncPolicy".into(), json!("automatic")),
        ]);
        let mut statement = self
            .connection
            .prepare("SELECT key,value_json FROM app_settings")
            .map_err(AppError::from)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(AppError::from)?;
        for row in rows {
            let (key, value) = row.map_err(AppError::from)?;
            if let Ok(value) = serde_json::from_str(&value) {
                settings.insert(key, value);
            }
        }
        Ok(serde_json::Value::Object(settings))
    }

    pub fn update_settings(
        &mut self,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let object = patch
            .as_object()
            .ok_or_else(|| AppError::InvalidConfiguration("设置更新必须是 JSON 对象".into()))?;
        let allowed = [
            "theme",
            "colorScheme",
            "customThemeSeed",
            "androidDynamicColor",
            "safeReading",
            "syncPolicy",
        ];
        let now = Utc::now().to_rfc3339();
        let tx = self.connection.transaction().map_err(AppError::from)?;
        for (key, value) in object {
            if !allowed.contains(&key.as_str()) {
                continue;
            }
            match key.as_str() {
                "theme"
                    if !matches!(
                        value.as_str(),
                        Some("system") | Some("light") | Some("dark")
                    ) =>
                {
                    return Err(AppError::InvalidConfiguration(
                        "theme 必须是 system、light 或 dark".into(),
                    ));
                }
                "safeReading" if !value.is_boolean() => {
                    return Err(AppError::InvalidConfiguration(
                        "safeReading 必须是布尔值".into(),
                    ));
                }
                "colorScheme"
                    if !matches!(
                        value.as_str(),
                        Some("matcha")
                            | Some("mutsumi")
                            | Some("lavender")
                            | Some("ocean")
                            | Some("sunset")
                            | Some("custom")
                    ) =>
                {
                    return Err(AppError::InvalidConfiguration(
                        "colorScheme 不是受支持的配色方案".into(),
                    ));
                }
                "customThemeSeed" if !value.as_str().is_some_and(is_valid_hex_color) => {
                    return Err(AppError::InvalidConfiguration(
                        "customThemeSeed 必须是 #RRGGBB 颜色".into(),
                    ));
                }
                "androidDynamicColor" if !value.is_boolean() => {
                    return Err(AppError::InvalidConfiguration(
                        "androidDynamicColor 必须是布尔值".into(),
                    ));
                }
                "syncPolicy"
                    if !matches!(
                        value.as_str(),
                        Some("automatic") | Some("manual") | Some("paused")
                    ) =>
                {
                    return Err(AppError::InvalidConfiguration(
                        "syncPolicy 必须是 automatic、manual 或 paused".into(),
                    ));
                }
                _ => {}
            }
            tx.execute(
                "INSERT INTO app_settings(key,value_json,updated_at) VALUES (?,?,?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at",
                params![key, value.to_string(), now],
            )
            .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)?;
        self.get_settings()
    }

    pub fn clear_cache(&mut self) -> Result<usize, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let cleared = tx
            .query_row(
                "SELECT count(*) FROM messages WHERE body_text IS NOT NULL OR body_html_text IS NOT NULL OR body_cache_state='full'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(AppError::from)?;
        // Bump every message revision, including metadata-only rows. This invalidates
        // body fetches that were already in flight when the user cleared the cache.
        tx
            .execute(
                "UPDATE messages SET body_text=NULL,body_html_text=NULL,body_cache_state='metadata',updated_at=?",
                [Utc::now().to_rfc3339()],
            )
            .map_err(AppError::from)?;
        tx.commit().map_err(AppError::from)?;
        usize::try_from(cleared)
            .map_err(|_| AppError::InvalidConfiguration("缓存邮件计数超过支持范围".into()))
    }

    pub fn search_suggestions(&self, query: &str, limit: u32) -> Result<Vec<String>, AppError> {
        let pattern = format!("%{}%", query.trim());
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT subject FROM messages WHERE subject LIKE ? AND subject <> '' ORDER BY updated_at DESC LIMIT ?")
            .map_err(AppError::from)?;
        let rows = statement
            .query_map(params![pattern, limit.min(20)], |row| row.get(0))
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<String>, _>>()
            .map_err(AppError::from)
    }

    pub fn diagnostics(&self) -> Result<serde_json::Value, AppError> {
        let accounts: i64 = self
            .connection
            .query_row("SELECT count(*) FROM accounts", [], |row| row.get(0))
            .map_err(AppError::from)?;
        let messages: i64 = self
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .map_err(AppError::from)?;
        let outbox: i64 = self
            .connection
            .query_row(
                "SELECT count(*) FROM outbox WHERE state IN ('queued','sending','outcome_unknown')",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::from)?;
        Ok(
            json!({ "app": "Mutsumi Mail", "schema": 1, "accounts": accounts, "cachedMessages": messages, "pendingOutbox": outbox, "secrets": "os-keyring" }),
        )
    }

    pub fn account_secret_refs(
        &self,
        account_id: &str,
    ) -> Result<(Option<String>, Option<String>), AppError> {
        self.connection
            .query_row(
                "SELECT incoming_secret_ref,outgoing_secret_ref FROM accounts WHERE id=?",
                [account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("account"))
    }

    pub fn incoming_config(&self, account_id: &str) -> Result<IncomingConfig, AppError> {
        self.connection.query_row("SELECT protocol,host,port,tls_mode,auth_method,username FROM incoming_endpoints WHERE account_id=?", [account_id], |row| Ok(IncomingConfig { protocol: row.get(0)?, host: row.get(1)?, port: row.get::<_, i64>(2)? as u16, tls_mode: row.get(3)?, auth_method: row.get(4)?, username: row.get(5)? })).optional().map_err(AppError::from)?.ok_or_else(|| AppError::not_found("incoming endpoint"))
    }

    pub fn outgoing_config(&self, account_id: &str) -> Result<OutgoingConfig, AppError> {
        self.connection.query_row("SELECT protocol,host,port,tls_mode,auth_method,username FROM outgoing_endpoints WHERE account_id=?", [account_id], |row| Ok(OutgoingConfig { protocol: row.get(0)?, host: row.get(1)?, port: row.get::<_, i64>(2)? as u16, tls_mode: row.get(3)?, auth_method: row.get(4)?, username: row.get(5)? })).optional().map_err(AppError::from)?.ok_or_else(|| AppError::not_found("outgoing endpoint"))
    }

    pub fn outgoing_delivery_details(
        &self,
        account_id: &str,
    ) -> Result<(OutgoingConfig, String, String), AppError> {
        let details = self
            .connection
            .query_row(
                "SELECT oe.protocol,oe.host,oe.port,oe.tls_mode,oe.auth_method,oe.username,a.outgoing_secret_ref,a.email FROM accounts a JOIN outgoing_endpoints oe ON oe.account_id=a.id WHERE a.id=? AND a.enabled=1",
                [account_id],
                |row| {
                    Ok((
                        OutgoingConfig {
                            protocol: row.get(0)?,
                            host: row.get(1)?,
                            port: row.get::<_, i64>(2)? as u16,
                            tls_mode: row.get(3)?,
                            auth_method: row.get(4)?,
                            username: row.get(5)?,
                        },
                        row.get::<_, Option<String>>(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(AppError::from)?;
        if let Some((config, Some(secret_ref), email)) = details {
            return Ok((config, secret_ref, email));
        }

        let account = self
            .connection
            .query_row(
                "SELECT enabled FROM accounts WHERE id=?",
                [account_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(AppError::from)?;
        match account {
            None => Err(AppError::not_found("account")),
            Some(0) => Err(AppError::Capability("该账号已停用，无法发件".into())),
            Some(_) => Err(AppError::Capability("该账号没有可用的发件配置".into())),
        }
    }

    pub fn delete_account(&mut self, account_id: &str) -> Result<(), AppError> {
        let deleted = self
            .connection
            .execute("DELETE FROM accounts WHERE id=?", [account_id])
            .map_err(AppError::from)?;
        if deleted == 0 {
            Err(AppError::not_found("account"))
        } else {
            Ok(())
        }
    }
}

fn insert_message_address(
    tx: &rusqlite::Transaction<'_>,
    message_id: &str,
    kind: &str,
    address: &Address,
    position: usize,
) -> Result<(), AppError> {
    let position = i64::try_from(position)
        .map_err(|_| AppError::InvalidConfiguration("邮件地址数量超过本地索引上限".into()))?;
    tx.execute(
        "INSERT INTO message_addresses (id,message_id,kind,display_name,email,position) VALUES (?,?,?,?,?,?)",
        params![
            Uuid::new_v4().to_string(),
            message_id,
            kind,
            address.name,
            address.email,
            position,
        ],
    )
    .map_err(AppError::from)?;
    Ok(())
}

fn mark_pending_operation_conflicted(
    tx: &rusqlite::Transaction<'_>,
    operation_id: &str,
    error_code: &str,
) -> Result<(), AppError> {
    let operation = tx
        .query_row(
            "SELECT operation_type,message_instance_id FROM pending_operations WHERE id=?",
            [operation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(AppError::from)?;
    tx.execute(
        "UPDATE pending_operations SET state='conflicted',retry_count=retry_count+1,last_error_code=?,updated_at=? WHERE id=?",
        params![error_code, Utc::now().to_rfc3339(), operation_id],
    )
    .map_err(AppError::from)?;
    if let Some((operation_type, Some(instance_id))) = operation {
        if matches!(
            operation_type.as_str(),
            "move" | "trash" | "permanent_delete"
        ) {
            restore_message_instance(&tx, &instance_id)?;
        }
    }
    Ok(())
}

fn canonical_rfc3339(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        DateTime::parse_from_rfc3339(value)
            .map(|timestamp| timestamp.with_timezone(&Utc).to_rfc3339())
            .unwrap_or_else(|_| value.to_owned())
    })
}

fn mailbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mailbox> {
    Ok(Mailbox {
        id: row.get(0)?,
        account_id: row.get(1)?,
        remote_id: row.get(2)?,
        name: row.get(3)?,
        display_name: row.get(4)?,
        special_role: row.get(5)?,
        unread_count: row.get(6)?,
        total_count: row.get(7)?,
        sync_enabled: row.get::<_, i64>(8)? != 0,
    })
}

fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let subject: String = row.get(5)?;
    let mailbox_id: String = row.get(2)?;
    let is_read: bool = row.get::<_, i64>(11)? != 0;
    let is_starred: bool = row.get::<_, i64>(12)? != 0;
    let from_name = row
        .get::<_, Option<String>>(16)?
        .filter(|name| !name.is_empty());
    let to_json: String = row.get(18)?;
    let labels_json: String = row.get(19)?;
    Ok(Message {
        id: row.get(0)?,
        account_id: row.get(1)?,
        mailbox_id: mailbox_id.clone(),
        thread_id: row.get(3)?,
        message_id: row.get(4)?,
        normalized_subject: row.get(6).unwrap_or_else(|_| normalize_subject(&subject)),
        subject,
        date: row.get(7)?,
        preview: row.get(8)?,
        body_text: row.get(9)?,
        body_html_text: row.get(10)?,
        is_read,
        is_starred,
        has_attachment: row.get::<_, i64>(13)? != 0,
        attachment_count: row.get(14)?,
        attachments: Vec::new(),
        labels: serde_json::from_str(&labels_json).unwrap_or_default(),
        size_bytes: row.get(15)?,
        from: Address {
            name: from_name,
            email: row.get(17)?,
        },
        to: serde_json::from_str(&to_json).unwrap_or_default(),
    })
}

fn sanitize_attachment_filename(value: &str, position: usize) -> String {
    let name = value.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        format!("attachment-{}", position + 1)
    } else {
        sanitized
    }
}

fn build_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter_map(|token| {
            let token = token.replace('"', "");
            (!token.is_empty()).then(|| format!("\"{token}\"*"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonical_imap_flags(flags: &[String]) -> Vec<String> {
    let mut normalized = Vec::with_capacity(flags.len());
    for flag in flags {
        let canonical = match flag.to_ascii_lowercase().as_str() {
            "\\answered" => "\\Answered",
            "\\deleted" => "\\Deleted",
            "\\draft" => "\\Draft",
            "\\flagged" => "\\Flagged",
            "\\recent" => "\\Recent",
            "\\seen" => "\\Seen",
            _ => flag,
        };
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(canonical))
        {
            normalized.push(canonical.to_owned());
        }
    }
    normalized
}

fn update_flag(flags: &mut Vec<String>, flag: &str, value: Option<bool>) {
    if let Some(value) = value {
        if value {
            if let Some(existing) = flags
                .iter_mut()
                .find(|item| item.eq_ignore_ascii_case(flag))
            {
                *existing = flag.to_owned();
            } else {
                flags.push(flag.into());
            }
        } else {
            flags.retain(|item| !item.eq_ignore_ascii_case(flag));
        }
    }
}

fn is_valid_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}
fn restore_message_instance(tx: &rusqlite::Transaction<'_>, instance_id: &str) -> Result<(), AppError> {
    tx.execute(
        r#"UPDATE mailboxes SET total_count=total_count+1,
        unread_count=unread_count+(SELECT CASE WHEN EXISTS(SELECT 1 FROM json_each(mi.flags_json) WHERE lower(value)='\seen') THEN 0 ELSE 1 END FROM message_instances mi WHERE mi.id=?)
        WHERE id=(SELECT mailbox_id FROM message_instances WHERE id=? AND is_deleted=1)"#,
        params![instance_id, instance_id],
    ).map_err(AppError::from)?;
    tx.execute("UPDATE message_instances SET is_deleted=0 WHERE id=?", [instance_id]).map_err(AppError::from)?;
    tx.execute(
        r#"UPDATE threads SET unread_count=(SELECT count(DISTINCT m.id) FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE m.thread_id=threads.id AND mi.is_deleted=0 AND NOT EXISTS(SELECT 1 FROM json_each(mi.flags_json) WHERE lower(value)='\seen')) WHERE id=(SELECT m.thread_id FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE mi.id=?)"#,
        [instance_id],
    ).map_err(AppError::from)?;
    Ok(())
}

fn hide_message_instance(
    tx: &rusqlite::Transaction<'_>, instance_id: &str, mailbox_id: &str,
    message_id: &str, now: &str,
) -> Result<(), AppError> {
    tx.execute(
        r#"UPDATE mailboxes SET total_count=MAX(0,total_count-1),
        unread_count=MAX(0,unread_count-(SELECT CASE WHEN EXISTS(SELECT 1 FROM json_each(mi.flags_json) WHERE lower(value)='\seen') THEN 0 ELSE 1 END FROM message_instances mi WHERE mi.id=?)) WHERE id=?"#,
        params![instance_id, mailbox_id],
    ).map_err(AppError::from)?;
    tx.execute("UPDATE message_instances SET is_deleted=1,last_synced_at=? WHERE id=?", params![now, instance_id]).map_err(AppError::from)?;
    tx.execute(
        r#"UPDATE threads SET unread_count=(SELECT count(DISTINCT m.id) FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE m.thread_id=threads.id AND mi.is_deleted=0 AND NOT EXISTS(SELECT 1 FROM json_each(mi.flags_json) WHERE lower(value)='\seen')) WHERE id=(SELECT thread_id FROM messages WHERE id=?)"#,
        [message_id],
    ).map_err(AppError::from)?;
    Ok(())
}

fn split_addresses(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Database, HydratedMessageBody, ImapSnapshotMetadata, ImapSyncWindow, PreparedOutboxPayload,
        SyncedMailboxInput, SyncedMessageInput,
    };
    use crate::backends::incoming::IncomingMailboxIndex;
    use crate::domain::account::CreateAccountInput;
    use crate::domain::{Address, DraftInput};
    use crate::mime::parser::ParsedAttachment;
    use crate::providers::registry::provider_presets;
    use rusqlite::params;
    use serde_json::json;

    fn snapshot_metadata(
        uid_validity: Option<u32>,
        total_count: u32,
        unread_count: u32,
        complete_mailbox: bool,
    ) -> ImapSnapshotMetadata {
        ImapSnapshotMetadata {
            uid_validity,
            total_count,
            unread_count,
            complete_mailbox,
        }
    }

    #[test]
    fn migration_creates_fts_and_account_without_secret_columns() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "test@qq.com".into(),
                    display_name: "Test".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/x/incoming",
                "account/x/outgoing",
                true,
                true,
            )
            .expect("account");
        assert_eq!(database.list_accounts().expect("accounts").len(), 1);
        assert_eq!(account.provider_id, "qq");
        let duplicate_error = database
            .create_account(
                &CreateAccountInput {
                    email: "TEST@qq.com".into(),
                    display_name: "Duplicate".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/duplicate/incoming",
                "account/duplicate/outgoing",
                true,
                true,
            )
            .expect_err("same provider and email must not be added twice");
        assert!(matches!(
            duplicate_error,
            crate::errors::AppError::InvalidConfiguration(_)
        ));
        assert_eq!(database.list_accounts().expect("accounts").len(), 1);
        let fts: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='message_fts'",
                [],
                |row| row.get(0),
            )
            .expect("fts");
        assert_eq!(fts, 1);
        let secret_columns: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('accounts') WHERE name IN ('secret','password','token')",
                [],
                |row| row.get(0),
            )
            .expect("schema");
        assert_eq!(secret_columns, 0);

        let cloudflare = provider_presets()
            .into_iter()
            .find(|item| item.id == "cloudflare-smtp")
            .expect("cloudflare preset");
        let cloudflare_account = database
            .create_account(
                &CreateAccountInput {
                    email: "sender@example.com".into(),
                    display_name: "Sender".into(),
                    provider_id: "cloudflare-smtp".into(),
                    secret: "api token not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &cloudflare,
                "account/cloudflare/incoming",
                "account/cloudflare/outgoing",
                false,
                true,
            )
            .expect("cloudflare account");
        assert_eq!(
            database
                .outgoing_config(&cloudflare_account.id)
                .expect("cloudflare endpoint")
                .username,
            "api_token"
        );
    }

    #[test]
    fn fts_search_and_flag_projection_use_cached_rows() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "fts@qq.com".into(),
                    display_name: "FTS".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/fts/incoming",
                "account/fts/outgoing",
                true,
                true,
            )
            .expect("account");
        database
            .connection
            .execute(
                "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role) VALUES ('mb-fts',?,'INBOX','INBOX','收件箱','inbox')",
                [account.id.as_str()],
            )
            .expect("mailbox");
        database
            .connection
            .execute(
                "INSERT INTO messages (id,account_id,subject,normalized_subject,preview,body_text,received_at,created_at,updated_at) VALUES ('msg-fts',?,'Offline notes','offline notes','Offline first','Offline first body','2026-09-02T00:00:00Z','2026-09-02T00:00:00Z','2026-09-02T00:00:00Z')",
                [account.id.as_str()],
            )
            .expect("message");
        database
            .connection
            .execute(
                "INSERT INTO message_addresses (id,message_id,kind,display_name,email,position) VALUES ('from-fts','msg-fts','from','Alice Sender','alice@example.com',0),('to-fts','msg-fts','to','Bob Recipient','bob@example.com',0)",
                [],
            )
            .expect("addresses");
        database
            .connection
            .execute(
                "INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,uid_validity,uid,flags_json,last_synced_at) VALUES ('inst-fts','msg-fts','mb-fts','1',7,1,'[\"\\\\Seen\"]','2026-09-02T00:00:00Z')",
                [],
            )
            .expect("instance");
        let found = database
            .search_messages_in_scope(None, Some("mb-fts"), None, None, "Offline", 20)
            .expect("search");
        assert_eq!(found.len(), 1);
        assert!(found[0].is_read);
        assert_eq!(found[0].to[0].email, "bob@example.com");
        assert_eq!(found[0].labels, ["收件箱"]);
        assert_eq!(
            database
                .search_messages_in_scope(None, Some("mb-fts"), None, None, "Alice", 20)
                .expect("sender search")
                .len(),
            1
        );
        assert_eq!(
            database
                .list_messages(Some("mb-fts"), 20)
                .expect("list")
                .len(),
            1
        );
        database
            .connection
            .execute(
                "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role) VALUES ('mb-archive',?,'Archive','Archive','归档','archive')",
                [account.id.as_str()],
            )
            .expect("archive mailbox");
        assert_eq!(
            database
                .move_messages(
                    &[(String::from("msg-fts"), String::from("mb-fts"))],
                    "mb-archive",
                )
                .expect("move"),
            1
        );
        assert_eq!(
            database
                .list_messages(Some("mb-fts"), 20)
                .expect("optimistically hidden source")
                .len(),
            0
        );
        assert!(database
            .list_messages(Some("mb-archive"), 20)
            .expect("target waits for a real remote UID")
            .is_empty());
        let claimed = database
            .claim_pending_imap_operations(&account.id, 10)
            .expect("claim move");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].source_mailbox_remote_id, "INBOX");
        assert_eq!(
            claimed[0].target_mailbox_remote_id.as_deref(),
            Some("Archive")
        );
        database
            .fail_pending_operation(&claimed[0].id, "server_rejected", false)
            .expect("conflict move");
        assert_eq!(
            database
                .list_messages(Some("mb-fts"), 20)
                .expect("source restored after permanent failure")
                .len(),
            1
        );
        let refs = vec![(String::from("msg-fts"), String::from("mb-fts"))];
        assert!(database.delete_messages(&refs, false).is_err());
        assert_eq!(database.list_messages(Some("mb-fts"), 20).unwrap().len(), 1);
        database.connection.execute(
            "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role) VALUES ('mb-trash',?,'Trash','Trash','回收站','trash')",
            [account.id.as_str()],
        ).unwrap();
        let invalid_batch = vec![refs[0].clone(), ("missing".into(), "mb-fts".into())];
        assert!(database.delete_messages(&invalid_batch, false).is_err());
        assert_eq!(database.list_messages(Some("mb-fts"), 20).unwrap().len(), 1);
        database.connection.execute("UPDATE mailboxes SET total_count=1,unread_count=0 WHERE id='mb-fts'", []).unwrap();
        assert_eq!(
            database
                .delete_messages(&[(String::from("msg-fts"), String::from("mb-fts"))], false,)
                .expect("trash"),
            1
        );
        assert!(database
            .list_messages(Some("mb-fts"), 20)
            .expect("deleted list")
            .is_empty());
        let remaining: (i64, i64) = database.connection.query_row("SELECT total_count,unread_count FROM mailboxes WHERE id='mb-fts'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(remaining, (0, 0));
        let claimed = database
            .claim_pending_imap_operations(&account.id, 10)
            .expect("claim trash");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].operation_type, "trash");
        database
            .complete_pending_operation(&claimed[0].id)
            .expect("complete trash");
    }

    #[test]
    fn aggregate_queries_keep_accounts_separate_and_deduplicate_messages() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let create = |database: &mut Database, email: &str| {
            database
                .create_account(
                    &CreateAccountInput {
                        email: email.into(),
                        display_name: email.into(),
                        provider_id: "qq".into(),
                        secret: "not persisted".into(),
                        incoming_secret: None,
                        outgoing_secret: None,
                        incoming: None,
                        outgoing: None,
                    },
                    &preset,
                    &format!("account/{email}/incoming"),
                    &format!("account/{email}/outgoing"),
                    true,
                    true,
                )
                .expect("account")
        };
        let first = create(&mut database, "first@qq.com");
        let second = create(&mut database, "second@qq.com");
        let disabled = create(&mut database, "disabled@qq.com");
        database
            .update_account(&disabled.id, &serde_json::json!({ "enabled": false }))
            .expect("disable account");

        for (mailbox_id, account_id, remote_id, role) in [
            ("first-inbox", first.id.as_str(), "INBOX", "inbox"),
            ("first-archive", first.id.as_str(), "Archive", "archive"),
            ("second-inbox", second.id.as_str(), "INBOX", "inbox"),
            ("disabled-inbox", disabled.id.as_str(), "INBOX", "inbox"),
        ] {
            database
                .connection
                .execute(
                    "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role) VALUES (?,?,?,?,?,?)",
                    params![mailbox_id, account_id, remote_id, remote_id, remote_id, role],
                )
                .expect("mailbox");
        }
        for (message_id, account_id, subject, received_at) in [
            (
                "first-message",
                first.id.as_str(),
                "First account",
                "2026-09-03T10:00:00+08:00",
            ),
            (
                "second-message",
                second.id.as_str(),
                "Second account",
                "2026-09-03T03:00:00Z",
            ),
            (
                "disabled-message",
                disabled.id.as_str(),
                "Disabled account",
                "2026-09-03T00:00:00Z",
            ),
        ] {
            database
                .connection
                .execute(
                    "INSERT INTO messages (id,account_id,subject,received_at,created_at,updated_at) VALUES (?,?,?,?,?,?)",
                    params![message_id, account_id, subject, received_at, received_at, received_at],
                )
                .expect("message");
        }
        for (instance_id, message_id, mailbox_id, flags) in [
            (
                "first-inbox-instance",
                "first-message",
                "first-inbox",
                "[\"\\\\flagged\"]",
            ),
            (
                "first-archive-instance",
                "first-message",
                "first-archive",
                "[]",
            ),
            (
                "second-inbox-instance",
                "second-message",
                "second-inbox",
                "[]",
            ),
            (
                "disabled-inbox-instance",
                "disabled-message",
                "disabled-inbox",
                "[]",
            ),
        ] {
            database
                .connection
                .execute(
                    "INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,flags_json,last_synced_at) VALUES (?,?,?,?,?,?)",
                    params![instance_id, message_id, mailbox_id, instance_id, flags, "2026-09-03T00:00:00Z"],
                )
                .expect("message instance");
        }

        let mailboxes = database.list_all_mailboxes().expect("all mailboxes");
        assert_eq!(mailboxes.len(), 3);
        assert!(mailboxes
            .iter()
            .all(|mailbox| mailbox.account_id != disabled.id));

        let unified_inbox = database
            .list_messages_in_scope(None, None, Some("inbox"), None, 20)
            .expect("unified inbox");
        assert_eq!(
            unified_inbox
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["second-message", "first-message"]
        );
        let first_account_inbox = database
            .list_messages_in_scope(Some(&first.id), None, Some("inbox"), None, 20)
            .expect("account inbox");
        assert_eq!(first_account_inbox.len(), 1);
        assert_eq!(first_account_inbox[0].account_id, first.id);
        assert!(database
            .list_messages_in_scope(Some(&first.id), Some("second-inbox"), None, None, 20,)
            .expect("mismatched account and mailbox")
            .is_empty());
        let search_results = database
            .search_messages_in_scope(None, None, Some("inbox"), None, "account", 20)
            .expect("unified search");
        assert_eq!(search_results.len(), 2);
        assert!(search_results
            .iter()
            .all(|message| message.account_id != disabled.id));
        let starred = database
            .list_messages_in_scope(None, None, None, Some(true), 20)
            .expect("unified starred");
        assert_eq!(starred.len(), 1);
        assert_eq!(starred[0].id, "first-message");

        let move_error = database
            .move_messages(
                &[(String::from("first-message"), String::from("first-inbox"))],
                "second-inbox",
            )
            .expect_err("cross-account move must fail");
        assert!(move_error.to_string().contains("不能将邮件移动到其他账户"));
        assert_eq!(
            database
                .list_messages(Some("first-inbox"), 20)
                .expect("original mailbox")
                .len(),
            1
        );
    }

    #[test]
    fn imap_snapshot_is_atomic_idempotent_and_resets_reused_uids() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "sync@qq.com".into(),
                    display_name: "Sync".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/sync/incoming",
                "account/sync/outgoing",
                true,
                true,
            )
            .expect("account");
        let discovered = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "收件箱".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("discover mailbox");
        assert_eq!(discovered.len(), 1);
        let mailbox_id = discovered[0].id.clone();
        database
            .set_mailbox_sync_enabled(&mailbox_id, false)
            .expect("disable folder sync");
        let rediscovered = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("rediscover mailbox");
        assert_eq!(rediscovered[0].id, mailbox_id);
        assert!(!rediscovered[0].sync_enabled);
        let with_archive = database
            .upsert_remote_mailboxes(
                &account.id,
                &[
                    SyncedMailboxInput {
                        remote_id: "INBOX".into(),
                        display_name: "Inbox".into(),
                        delimiter: Some("/".into()),
                        special_role: Some("inbox".into()),
                        selectable: true,
                    },
                    SyncedMailboxInput {
                        remote_id: "Archive".into(),
                        display_name: "Archive".into(),
                        delimiter: Some("/".into()),
                        special_role: Some("archive".into()),
                        selectable: true,
                    },
                ],
            )
            .expect("discover archive");
        assert_eq!(with_archive.len(), 2);
        let reconciled = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("reconcile removed archive");
        assert_eq!(reconciled.len(), 1);
        let archived_selectable = database
            .connection
            .query_row(
                "SELECT selectable FROM mailboxes WHERE account_id=? AND remote_id='Archive'",
                [&account.id],
                |row| row.get::<_, bool>(0),
            )
            .expect("archived mailbox state");
        assert!(!archived_selectable);
        database
            .set_mailbox_sync_enabled(&mailbox_id, true)
            .expect("enable folder sync");

        let first_snapshot = vec![
            SyncedMessageInput {
                uid: 1,
                flags: vec![],
                received_at: Some("2026-09-01T08:00:00+08:00".into()),
                size_bytes: Some(128),
                rfc_message_id: Some("<one@example.com>".into()),
                subject: "One".into(),
                preview: "First body".into(),
                body_text: Some("First body".into()),
                body_html_text: None,
                has_attachment: false,
                from: Some(Address {
                    name: Some("Sender".into()),
                    email: "sender@example.com".into(),
                }),
                to: vec![Address {
                    name: None,
                    email: "sync@qq.com".into(),
                }],
            },
            SyncedMessageInput {
                uid: 2,
                flags: vec!["\\seen".into()],
                received_at: Some("2026-09-02T00:00:00Z".into()),
                size_bytes: Some(256),
                rfc_message_id: Some("<two@example.com>".into()),
                subject: "Two".into(),
                preview: "Second body".into(),
                body_text: Some("Second body".into()),
                body_html_text: None,
                has_attachment: false,
                from: None,
                to: vec![],
            },
        ];
        let applied = database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(42), 10, 5, true),
                &first_snapshot,
            )
            .expect("apply snapshot");
        assert_eq!(applied.inserted, 2);
        assert_eq!(applied.updated, 0);
        assert_eq!(
            database
                .imap_sync_cursor(&account.id, "INBOX")
                .expect("cursor"),
            Some((42, 2))
        );
        let window = database
            .imap_sync_window(&account.id, "INBOX")
            .expect("sync window")
            .expect("sync window state");
        assert_eq!(window.uid_validity, 42);
        assert_eq!(window.last_uid, 2);
        assert_eq!(window.oldest_uid, Some(1));
        assert_eq!(window.instance_count, 2);
        let inbox = database.list_mailboxes(&account.id).expect("mailboxes");
        assert_eq!(inbox[0].total_count, 10);
        assert_eq!(inbox[0].unread_count, 5);
        let normalized = database
            .list_messages(Some(&mailbox_id), 20)
            .expect("messages")
            .into_iter()
            .find(|message| message.subject == "One")
            .expect("first message");
        assert_eq!(normalized.date, "2026-09-01T00:00:00+00:00");
        let seen = database
            .list_messages(Some(&mailbox_id), 20)
            .expect("messages")
            .into_iter()
            .find(|message| message.subject == "Two")
            .expect("second message");
        assert!(seen.is_read);
        database
            .mutate_message(&seen.id, &mailbox_id, Some(false), None)
            .expect("optimistic unread mutation");

        let reapplied = database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(42), 10, 4, true),
                &first_snapshot,
            )
            .expect("reapply snapshot");
        assert_eq!(reapplied.inserted, 0);
        assert_eq!(reapplied.updated, 2);
        let after_stale_snapshot = database
            .list_messages(Some(&mailbox_id), 20)
            .expect("messages");
        assert_eq!(after_stale_snapshot.len(), 2);
        assert!(
            !after_stale_snapshot
                .iter()
                .find(|message| message.id == seen.id)
                .expect("mutated message")
                .is_read
        );

        let reconciled = database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(42), 1, 0, true),
                &first_snapshot[1..],
            )
            .expect("reconcile complete mailbox");
        assert_eq!(reconciled.inserted, 0);
        assert_eq!(reconciled.updated, 1);
        let reconciled_messages = database
            .list_messages(Some(&mailbox_id), 20)
            .expect("messages after complete reconcile");
        assert_eq!(reconciled_messages.len(), 1);
        assert_eq!(reconciled_messages[0].subject, "Two");

        let replacement = [SyncedMessageInput {
            uid: 1,
            flags: vec![],
            received_at: Some("2026-09-03T00:00:00Z".into()),
            size_bytes: Some(64),
            rfc_message_id: Some("<replacement@example.com>".into()),
            subject: "Replacement after UIDVALIDITY".into(),
            preview: "Replacement".into(),
            body_text: Some("Replacement".into()),
            body_html_text: None,
            has_attachment: false,
            from: None,
            to: vec![],
        }];
        let reset = database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(43), 1, 1, true),
                &replacement,
            )
            .expect("reset snapshot");
        assert_eq!(reset.inserted, 1);
        assert_eq!(reset.updated, 0);
        let visible = database
            .list_messages(Some(&mailbox_id), 20)
            .expect("messages after reset");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].subject, "Replacement after UIDVALIDITY");
        let cached_messages: i64 = database
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .expect("cached message count");
        assert_eq!(cached_messages, 1);
        assert_eq!(
            database
                .imap_sync_cursor(&account.id, "INBOX")
                .expect("reset cursor"),
            Some((43, 1))
        );

        assert_eq!(database.clear_cache().expect("clear body cache"), 1);
        let retained = database
            .get_message(&visible[0].id)
            .expect("message metadata remains after cache clear");
        assert!(retained.body_text.is_none());
        assert!(retained.body_html_text.is_none());
        assert_eq!(
            database
                .imap_sync_cursor(&account.id, "INBOX")
                .expect("cursor after cache clear"),
            Some((43, 1))
        );
        assert_eq!(
            database
                .list_messages(Some(&mailbox_id), 20)
                .expect("messages after cache clear")
                .len(),
            1
        );

        database
            .mark_account_sync_completed(&account.id)
            .expect("complete sync");
        let completed = database.list_accounts().expect("accounts").remove(0);
        assert_eq!(completed.sync_status, "idle");
        let last_success = completed.last_synced_at.expect("last successful sync");
        database
            .mark_account_sync_failed(&account.id, "network unavailable")
            .expect("failed sync");
        let failed = database.list_accounts().expect("accounts").remove(0);
        assert_eq!(failed.sync_status, "error");
        assert_eq!(
            failed.last_synced_at.as_deref(),
            Some(last_success.as_str())
        );
        database
            .mark_account_sync_started(&account.id)
            .expect("restart sync");
        let restarted = database.list_accounts().expect("accounts").remove(0);
        assert_eq!(restarted.sync_status, "syncing");
        assert_eq!(
            restarted.last_synced_at.as_deref(),
            Some(last_success.as_str())
        );
    }

    #[test]
    fn mailbox_index_reconciles_old_flags_and_expunges_without_overwriting_pending_flags() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "index@qq.com".into(),
                    display_name: "Index".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/index/incoming",
                "account/index/outgoing",
                true,
                true,
            )
            .expect("account");
        let mailbox_id = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("mailbox")
            .remove(0)
            .id;
        let messages = (1..=3)
            .map(|uid| SyncedMessageInput {
                uid,
                flags: vec!["\\Seen".into()],
                received_at: Some(format!("2026-09-03T00:0{uid}:00Z")),
                size_bytes: None,
                rfc_message_id: Some(format!("<index-{uid}@example.com>")),
                subject: format!("Index {uid}"),
                preview: String::new(),
                body_text: None,
                body_html_text: None,
                has_attachment: false,
                from: None,
                to: Vec::new(),
            })
            .collect::<Vec<_>>();
        database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(7), 3, 0, true),
                &messages,
            )
            .expect("initial snapshot");
        let first_id = database
            .list_messages(Some(&mailbox_id), 10)
            .expect("messages")
            .into_iter()
            .find(|message| message.subject == "Index 1")
            .expect("first message")
            .id;
        database
            .mutate_message(&first_id, &mailbox_id, None, Some(true))
            .expect("optimistic starred mutation");

        let wrong_identity = database
            .reconcile_imap_mailbox_index(
                &account.id,
                &IncomingMailboxIndex {
                    remote_id: "INBOX".into(),
                    uid_validity: Some(8),
                    total_count: 1,
                    all_uids: vec![1],
                    unseen_uids: Vec::new(),
                    flagged_uids: Vec::new(),
                },
            )
            .expect_err("mismatched UIDVALIDITY must not reconcile");
        assert!(matches!(
            wrong_identity,
            crate::errors::AppError::Protocol(_)
        ));
        assert_eq!(
            database
                .list_messages(Some(&mailbox_id), 10)
                .expect("messages after rejected index")
                .len(),
            3
        );

        let reconciled = database
            .reconcile_imap_mailbox_index(
                &account.id,
                &IncomingMailboxIndex {
                    remote_id: "INBOX".into(),
                    uid_validity: Some(7),
                    total_count: 2,
                    all_uids: vec![1, 2],
                    unseen_uids: vec![2],
                    flagged_uids: vec![2],
                },
            )
            .expect("reconcile mailbox index");
        assert_eq!(reconciled.removed_instances, 1);
        assert_eq!(reconciled.updated_flags, 1);
        let visible = database
            .list_messages(Some(&mailbox_id), 10)
            .expect("reconciled messages");
        assert_eq!(visible.len(), 2);
        let first = visible
            .iter()
            .find(|message| message.subject == "Index 1")
            .expect("pending message");
        assert!(first.is_read);
        assert!(first.is_starred);
        let second = visible
            .iter()
            .find(|message| message.subject == "Index 2")
            .expect("remotely changed message");
        assert!(!second.is_read);
        assert!(second.is_starred);
        let mailbox = database
            .list_mailboxes(&account.id)
            .expect("mailboxes")
            .remove(0);
        assert_eq!(mailbox.total_count, 2);
        assert_eq!(mailbox.unread_count, 1);
    }

    fn assert_instance_uidvalidity_reset(stored_uid_validity: Option<u32>, email: &str) {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: email.into(),
                    display_name: "UID validity".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/uidvalidity/incoming",
                "account/uidvalidity/outgoing",
                true,
                true,
            )
            .expect("account");
        let mailbox = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("mailbox")
            .remove(0);
        database
            .connection
            .execute(
                "INSERT INTO messages (id,account_id,rfc_message_id,subject,body_text,body_cache_state,created_at,updated_at) VALUES ('stale-message',?,'<stale@example.com>','Stale','must not survive','full','now','now')",
                [account.id.as_str()],
            )
            .expect("stale message");
        database
            .connection
            .execute(
                "INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,uid_validity,uid,last_synced_at) VALUES ('stale-instance','stale-message',?,'1',?,1,'now')",
                params![mailbox.id, stored_uid_validity],
            )
            .expect("stale instance");

        let replacement = [SyncedMessageInput {
            uid: 1,
            flags: Vec::new(),
            received_at: Some("2026-09-03T00:00:00Z".into()),
            size_bytes: Some(10),
            rfc_message_id: Some("<replacement@example.com>".into()),
            subject: "Replacement".into(),
            preview: "Replacement".into(),
            body_text: None,
            body_html_text: None,
            has_attachment: false,
            from: None,
            to: Vec::new(),
        }];
        let applied = database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(99), 1, 1, true),
                &replacement,
            )
            .expect("reset from instance identity");
        assert_eq!(applied.inserted, 1);
        assert_eq!(applied.updated, 0);
        let messages = database
            .list_messages(Some(&mailbox.id), 10)
            .expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "Replacement");
        let cached_messages: i64 = database
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .expect("cached message count");
        assert_eq!(cached_messages, 1);
    }

    #[test]
    fn first_known_uidvalidity_invalidates_instances_with_unknown_identity() {
        assert_instance_uidvalidity_reset(None, "uid-none@qq.com");
    }

    #[test]
    fn instance_uidvalidity_mismatch_resets_even_without_a_cursor() {
        assert_instance_uidvalidity_reset(Some(7), "uid-mismatch@qq.com");
    }

    #[test]
    fn imap_sync_window_persists_forward_and_backfill_progress() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "backfill@qq.com".into(),
                    display_name: "Backfill".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/backfill/incoming",
                "account/backfill/outgoing",
                true,
                true,
            )
            .expect("account");
        database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("mailbox");
        let recent = (2..=251)
            .map(|uid| SyncedMessageInput {
                uid,
                flags: Vec::new(),
                received_at: Some(format!("2026-09-03T00:{:02}:00Z", uid % 60)),
                size_bytes: None,
                rfc_message_id: Some(format!("<backfill-{uid}@example.com>")),
                subject: format!("Message {uid}"),
                preview: String::new(),
                body_text: None,
                body_html_text: None,
                has_attachment: false,
                from: None,
                to: Vec::new(),
            })
            .collect::<Vec<_>>();
        database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(12), 251, 251, false),
                &recent,
            )
            .expect("recent window");
        assert_eq!(
            database
                .imap_sync_window(&account.id, "INBOX")
                .expect("window")
                .expect("window state"),
            ImapSyncWindow {
                uid_validity: 12,
                last_uid: 251,
                oldest_uid: Some(2),
                instance_count: 250,
            }
        );

        database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(12), 251, 251, false),
                &[SyncedMessageInput {
                    uid: 1,
                    flags: Vec::new(),
                    received_at: Some("2026-09-02T23:59:00Z".into()),
                    size_bytes: None,
                    rfc_message_id: Some("<backfill-1@example.com>".into()),
                    subject: "Message 1".into(),
                    preview: String::new(),
                    body_text: None,
                    body_html_text: None,
                    has_attachment: false,
                    from: None,
                    to: Vec::new(),
                }],
            )
            .expect("oldest page");
        assert_eq!(
            database
                .imap_sync_window(&account.id, "INBOX")
                .expect("window")
                .expect("window state"),
            ImapSyncWindow {
                uid_validity: 12,
                last_uid: 251,
                oldest_uid: Some(1),
                instance_count: 251,
            }
        );
    }

    #[test]
    fn mutate_message_uses_the_owning_account_and_direct_lookup_has_no_list_limit() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "owner@qq.com".into(),
                    display_name: "Owner".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/owner/incoming",
                "account/owner/outgoing",
                true,
                true,
            )
            .expect("account");
        database
            .connection
            .execute(
                "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role,unread_count,total_count) VALUES ('owner-inbox',?,'INBOX','INBOX','INBOX','inbox',1,1)",
                [account.id.as_str()],
            )
            .expect("mailbox");
        database
            .connection
            .execute(
                "INSERT INTO mailboxes (id,account_id,remote_id,name,display_name,special_role) VALUES ('owner-archive',?,'Archive','Archive','Archive','archive')",
                [account.id.as_str()],
            )
            .expect("archive mailbox");
        database
            .connection
            .execute(
                "INSERT INTO messages (id,account_id,subject,created_at,updated_at) VALUES ('owner-message',?,'Owned','2026-09-03T00:00:00Z','2026-09-03T00:00:00Z')",
                [account.id.as_str()],
            )
            .expect("message");
        database
            .connection
            .execute(
                "INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,last_synced_at) VALUES ('owner-instance','owner-message','owner-inbox','1','2026-09-03T00:00:00Z'),('owner-archive-instance','owner-message','owner-archive','2','2026-09-04T00:00:00Z')",
                [],
            )
            .expect("instance");

        let message = database
            .mutate_message("owner-message", "owner-inbox", Some(true), None)
            .expect("mutate");
        assert!(message.is_read);
        assert_eq!(message.mailbox_id, "owner-inbox");
        assert_eq!(
            database
                .list_mailboxes(&account.id)
                .expect("mailboxes after mark read")
                .into_iter()
                .find(|mailbox| mailbox.id == "owner-inbox")
                .expect("inbox")
                .unread_count,
            0
        );
        database
            .mutate_message("owner-message", "owner-inbox", Some(true), None)
            .expect("repeated mark read");
        assert_eq!(
            database
                .list_mailboxes(&account.id)
                .expect("mailboxes after repeated mutation")
                .into_iter()
                .find(|mailbox| mailbox.id == "owner-inbox")
                .expect("inbox")
                .unread_count,
            0
        );
        let archive_flags: String = database
            .connection
            .query_row(
                "SELECT flags_json FROM message_instances WHERE id='owner-archive-instance'",
                [],
                |row| row.get(0),
            )
            .expect("archive flags");
        assert_eq!(archive_flags, "[]");
        let operation_account: String = database
            .connection
            .query_row(
                "SELECT account_id FROM pending_operations WHERE message_instance_id='owner-instance'",
                [],
                |row| row.get(0),
            )
            .expect("pending operation");
        assert_eq!(operation_account, account.id);
        assert_eq!(
            database
                .get_message("owner-message")
                .expect("direct lookup")
                .id,
            "owner-message"
        );
    }

    #[test]
    fn theme_settings_persist_md3_palette_and_reject_invalid_custom_seed() {
        let mut database = Database::open_in_memory().expect("database");
        let settings = database
            .update_settings(&json!({
                "theme": "system",
                "colorScheme": "custom",
                "customThemeSeed": "#123ABC",
                "androidDynamicColor": true
            }))
            .expect("valid theme settings");
        assert_eq!(settings["colorScheme"], "custom");
        assert_eq!(settings["customThemeSeed"], "#123ABC");
        assert_eq!(settings["androidDynamicColor"], true);

        let settings = database
            .update_settings(&json!({ "colorScheme": "mutsumi" }))
            .expect("Mutsumi theme setting");
        assert_eq!(settings["colorScheme"], "mutsumi");

        assert!(database
            .update_settings(&json!({ "customThemeSeed": "not-a-color" }))
            .is_err());
        assert_eq!(
            database.get_settings().expect("settings after rejection")["customThemeSeed"],
            "#123ABC"
        );
    }

    #[test]
    fn batch_mutation_updates_every_selected_instance_or_rolls_back_as_one_unit() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "batch@qq.com".into(),
                    display_name: "Batch".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/batch/incoming",
                "account/batch/outgoing",
                true,
                true,
            )
            .expect("account");
        let mailbox = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("mailbox")
            .remove(0);
        database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(9), 2, 2, true),
                &[
                    SyncedMessageInput {
                        uid: 1,
                        flags: Vec::new(),
                        received_at: Some("2026-09-03T00:00:00Z".into()),
                        size_bytes: None,
                        rfc_message_id: Some("<batch-one@example.com>".into()),
                        subject: "Batch one".into(),
                        preview: String::new(),
                        body_text: None,
                        body_html_text: None,
                        has_attachment: false,
                        from: None,
                        to: Vec::new(),
                    },
                    SyncedMessageInput {
                        uid: 2,
                        flags: Vec::new(),
                        received_at: Some("2026-09-03T00:01:00Z".into()),
                        size_bytes: None,
                        rfc_message_id: Some("<batch-two@example.com>".into()),
                        subject: "Batch two".into(),
                        preview: String::new(),
                        body_text: None,
                        body_html_text: None,
                        has_attachment: false,
                        from: None,
                        to: Vec::new(),
                    },
                ],
            )
            .expect("snapshot");
        let messages = database
            .list_messages(Some(&mailbox.id), 10)
            .expect("messages");
        let refs = messages
            .iter()
            .map(|message| (message.id.clone(), mailbox.id.clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            database
                .mutate_messages(&refs, Some(true), None)
                .expect("batch mark read"),
            2
        );
        assert!(database
            .list_messages(Some(&mailbox.id), 10)
            .expect("marked messages")
            .iter()
            .all(|message| message.is_read));
        assert_eq!(
            database
                .list_mailboxes(&account.id)
                .expect("mailboxes after batch mark read")
                .remove(0)
                .unread_count,
            0
        );
        let pending_count: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM pending_operations WHERE operation_type='set_flags'",
                [],
                |row| row.get(0),
            )
            .expect("pending operations");
        assert_eq!(pending_count, 2);

        let missing = vec![
            refs[0].clone(),
            (String::from("missing-message"), mailbox.id.clone()),
        ];
        assert!(database
            .mutate_messages(&missing, Some(false), None)
            .is_err());
        assert!(database
            .list_messages(Some(&mailbox.id), 10)
            .expect("rolled back messages")
            .iter()
            .all(|message| message.is_read));
        assert_eq!(
            database
                .list_mailboxes(&account.id)
                .expect("mailboxes after rollback")
                .remove(0)
                .unread_count,
            0
        );
        let pending_count_after_error: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM pending_operations WHERE operation_type='set_flags'",
                [],
                |row| row.get(0),
            )
            .expect("pending operations after rollback");
        assert_eq!(pending_count_after_error, 2);
    }

    #[test]
    fn prepared_outbox_payload_is_immutable_across_retry_and_smtp_completion() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "stable@qq.com".into(),
                    display_name: "Stable".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/stable/incoming",
                "account/stable/outgoing",
                true,
                true,
            )
            .expect("account");
        let input = DraftInput {
            id: None,
            account_id: account.id.clone(),
            to: "recipient@example.com".into(),
            cc: None,
            bcc: None,
            subject: "Stable payload".into(),
            body_text: "Body".into(),
            in_reply_to: None,
            references: None,
        };
        let original = PreparedOutboxPayload {
            envelope_from: "stable@qq.com".into(),
            recipients: vec!["recipient@example.com".into()],
            mime: b"Message-ID: <stable@qq.com>\r\n\r\nBody\r\n".to_vec(),
            rfc_message_id: "<stable@qq.com>".into(),
            sent_copy_state: "not_started".into(),
            sent_copy_error_message: None,
            sent_copy_uid_validity: None,
            sent_copy_uid: None,
        };
        let outbox_id = database
            .queue_prepared_draft(&input, &original)
            .expect("queue prepared draft");
        assert_eq!(
            database.outbox_payload(&outbox_id).expect("payload"),
            Some(original.clone())
        );
        assert!(database
            .claim_outbox_for_sending(&outbox_id)
            .expect("claim"));

        let replacement = PreparedOutboxPayload {
            mime: b"different bytes".to_vec(),
            rfc_message_id: "<different@qq.com>".into(),
            ..original.clone()
        };
        let retained = database
            .store_outbox_payload_if_missing(&outbox_id, &replacement)
            .expect("retain first payload");
        assert_eq!(retained.mime, original.mime);
        assert_eq!(retained.rfc_message_id, original.rfc_message_id);

        assert_eq!(
            database
                .complete_outbox_sent(&outbox_id)
                .expect("SMTP accepted"),
            "awaiting_server_sync"
        );
        let item = database
            .list_outbox(Some(&account.id))
            .expect("outbox")
            .remove(0);
        assert_eq!(item.state, "sent");
        assert_eq!(
            item.sent_copy_state.as_deref(),
            Some("awaiting_server_sync")
        );
        assert!(database
            .set_outbox_state(&outbox_id, "queued", None, None)
            .is_err());
    }

    #[test]
    fn queueing_saved_draft_removes_editable_source_and_keeps_immutable_snapshot() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "draft@qq.com".into(),
                    display_name: "Draft".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/draft/incoming",
                "account/draft/outgoing",
                true,
                true,
            )
            .expect("account");
        let mut input = DraftInput {
            id: None,
            account_id: account.id,
            to: "recipient@example.com".into(),
            cc: None,
            bcc: None,
            subject: "Move draft".into(),
            body_text: "Immutable body".into(),
            in_reply_to: None,
            references: None,
        };
        let editable_id = database.save_draft(&input).expect("save editable draft");
        input.id = Some(editable_id.clone());
        let payload = PreparedOutboxPayload {
            envelope_from: "draft@qq.com".into(),
            recipients: vec!["recipient@example.com".into()],
            mime: b"Message-ID: <draft@qq.com>\r\n\r\nImmutable body\r\n".to_vec(),
            rfc_message_id: "<draft@qq.com>".into(),
            sent_copy_state: "not_started".into(),
            sent_copy_error_message: None,
            sent_copy_uid_validity: None,
            sent_copy_uid: None,
        };

        let outbox_id = database
            .queue_prepared_draft(&input, &payload)
            .expect("queue saved draft");

        let editable_count: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM drafts WHERE id=?",
                [&editable_id],
                |row| row.get(0),
            )
            .expect("editable draft count");
        assert_eq!(editable_count, 0);
        let immutable_id: String = database
            .connection
            .query_row(
                "SELECT draft_id FROM outbox WHERE id=?",
                [&outbox_id],
                |row| row.get(0),
            )
            .expect("immutable draft id");
        assert_ne!(immutable_id, editable_id);
        input.id = Some(immutable_id.clone());
        assert!(database.save_draft(&input).is_err(), "queue snapshot must be immutable");
        assert!(database.delete_draft(&immutable_id).is_err(), "queue snapshot must survive draft deletion");
        let snapshot = database.outbox_draft(&outbox_id).expect("outbox draft");
        assert_eq!(snapshot.subject, "Move draft");
        assert_eq!(snapshot.body_text, "Immutable body");
        assert_eq!(
            database.outbox_payload(&outbox_id).expect("payload"),
            Some(payload)
        );
    }

    #[test]
    fn smtp_success_rolls_back_when_sent_copy_state_cannot_be_persisted() {
        let mut database = Database::open_in_memory().expect("migration");
        database
            .connection
            .execute(
                "INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,created_at,updated_at) VALUES ('missing-payload-account','generic','sender@example.com','Sender',1,'automatic','now','now')",
                [],
            )
            .expect("account");
        database
            .connection
            .execute(
                "INSERT INTO outbox (id,account_id,state,created_at,updated_at) VALUES ('missing-payload','missing-payload-account','sending','now','now')",
                [],
            )
            .expect("sending outbox item");

        assert!(database.complete_outbox_sent("missing-payload").is_err());
        let state: String = database
            .connection
            .query_row(
                "SELECT state FROM outbox WHERE id='missing-payload'",
                [],
                |row| row.get(0),
            )
            .expect("state after rollback");
        assert_eq!(state, "sending");
    }

    #[test]
    fn real_sent_snapshot_confirms_only_the_matching_message_id() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "reconcile@qq.com".into(),
                    display_name: "Reconcile".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/reconcile/incoming",
                "account/reconcile/outgoing",
                true,
                true,
            )
            .expect("account");
        database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "Sent".into(),
                    display_name: "已发送".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("sent".into()),
                    selectable: true,
                }],
            )
            .expect("sent mailbox");

        let mut outbox_ids = Vec::new();
        for message_id in ["<observed@qq.com>", "<not-observed@qq.com>"] {
            let input = DraftInput {
                id: None,
                account_id: account.id.clone(),
                to: "recipient@example.com".into(),
                cc: None,
                bcc: None,
                subject: "Same subject".into(),
                body_text: "Body".into(),
                in_reply_to: None,
                references: None,
            };
            let payload = PreparedOutboxPayload {
                envelope_from: account.email.clone(),
                recipients: vec!["recipient@example.com".into()],
                mime: format!("Message-ID: {message_id}\r\n\r\nBody\r\n").into_bytes(),
                rfc_message_id: message_id.into(),
                sent_copy_state: "not_started".into(),
                sent_copy_error_message: None,
                sent_copy_uid_validity: None,
                sent_copy_uid: None,
            };
            let outbox_id = database
                .queue_prepared_draft(&input, &payload)
                .expect("queue");
            assert!(database
                .claim_outbox_for_sending(&outbox_id)
                .expect("claim"));
            database.complete_outbox_sent(&outbox_id).expect("sent");
            outbox_ids.push(outbox_id);
        }

        database
            .apply_imap_snapshot(
                &account.id,
                "Sent",
                snapshot_metadata(Some(91), 1, 0, true),
                &[SyncedMessageInput {
                    uid: 44,
                    flags: vec!["\\Seen".into()],
                    received_at: Some("2026-09-03T00:00:00Z".into()),
                    size_bytes: Some(42),
                    rfc_message_id: Some("<observed@qq.com>".into()),
                    subject: "Same subject".into(),
                    preview: "Body".into(),
                    body_text: Some("Body".into()),
                    body_html_text: None,
                    has_attachment: false,
                    from: Some(Address {
                        name: None,
                        email: account.email.clone(),
                    }),
                    to: vec![Address {
                        name: None,
                        email: "recipient@example.com".into(),
                    }],
                }],
            )
            .expect("real sent snapshot");

        assert_eq!(
            database
                .outbox_payload(&outbox_ids[0])
                .expect("observed payload")
                .expect("observed payload")
                .sent_copy_state,
            "confirmed"
        );
        assert_eq!(
            database
                .outbox_payload(&outbox_ids[1])
                .expect("unobserved payload")
                .expect("unobserved payload")
                .sent_copy_state,
            "awaiting_server_sync"
        );
    }

    #[test]
    fn outgoing_only_account_records_sent_copy_as_unavailable() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "cloudflare-smtp")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "sender@cloudflare.email".into(),
                    display_name: "Outbound".into(),
                    provider_id: "cloudflare-smtp".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/outbound/incoming",
                "account/outbound/outgoing",
                false,
                true,
            )
            .expect("account");
        let payload = PreparedOutboxPayload {
            envelope_from: account.email.clone(),
            recipients: vec!["recipient@example.com".into()],
            mime: b"Message-ID: <outbound@cloudflare.email>\r\n\r\nBody\r\n".to_vec(),
            rfc_message_id: "<outbound@cloudflare.email>".into(),
            sent_copy_state: "not_started".into(),
            sent_copy_error_message: None,
            sent_copy_uid_validity: None,
            sent_copy_uid: None,
        };
        let outbox_id = database
            .queue_prepared_draft(
                &DraftInput {
                    id: None,
                    account_id: account.id,
                    to: "recipient@example.com".into(),
                    cc: None,
                    bcc: None,
                    subject: "Outbound".into(),
                    body_text: "Body".into(),
                    in_reply_to: None,
                    references: None,
                },
                &payload,
            )
            .expect("queue");
        assert!(database
            .claim_outbox_for_sending(&outbox_id)
            .expect("claim"));
        assert_eq!(
            database
                .complete_outbox_sent(&outbox_id)
                .expect("SMTP accepted"),
            "unavailable"
        );
    }

    #[test]
    fn outbox_claim_and_state_transitions_prevent_duplicate_delivery() {
        let mut database = Database::open_in_memory().expect("migration");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "sender@qq.com".into(),
                    display_name: "Sender".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/sender/incoming",
                "account/sender/outgoing",
                true,
                true,
            )
            .expect("account");
        let outbox_id = database
            .queue_draft(&DraftInput {
                id: None,
                account_id: account.id,
                to: "recipient@example.com".into(),
                cc: None,
                bcc: None,
                subject: "One delivery".into(),
                body_text: "Body".into(),
                in_reply_to: None,
                references: None,
            })
            .expect("queue");

        assert!(database
            .claim_outbox_for_sending(&outbox_id)
            .expect("first claim"));
        assert!(!database
            .claim_outbox_for_sending(&outbox_id)
            .expect("duplicate claim"));
        assert!(database
            .set_outbox_state(&outbox_id, "sent", None, None)
            .is_ok());
        assert!(database
            .set_outbox_state(&outbox_id, "queued", None, None)
            .is_err());
        assert!(database
            .set_outbox_state(&outbox_id, "cancelled", None, None)
            .is_err());
    }

    #[test]
    fn only_failed_or_unknown_outbox_items_can_be_retried() {
        let mut database = Database::open_in_memory().expect("migration");
        database
            .connection
            .execute(
                "INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,created_at,updated_at) VALUES ('retry-account','generic','retry@example.com','Retry',1,'automatic','now','now')",
                [],
            )
            .expect("account");
        database
            .connection
            .execute(
                "INSERT INTO outbox (id,account_id,state,created_at,updated_at) VALUES ('retry-item','retry-account','sending','now','now')",
                [],
            )
            .expect("outbox");

        assert!(database
            .set_outbox_state(
                "retry-item",
                "failed",
                Some("network"),
                Some("temporary failure")
            )
            .is_ok());
        let failed = database
            .list_outbox(Some("retry-account"))
            .expect("failed list")
            .into_iter()
            .next()
            .expect("failed item");
        assert_eq!(failed.last_error_code.as_deref(), Some("network"));
        assert_eq!(
            failed.last_error_message.as_deref(),
            Some("temporary failure")
        );
        assert!(database
            .set_outbox_state("retry-item", "queued", None, None)
            .is_ok());
        assert!(database
            .set_outbox_state("retry-item", "cancelled", None, None)
            .is_ok());
        assert!(database
            .set_outbox_state("retry-item", "queued", None, None)
            .is_err());

        let item = database
            .list_outbox(Some("retry-account"))
            .expect("list")
            .into_iter()
            .next()
            .expect("item");
        assert_eq!(item.state, "cancelled");
        assert!(item.last_error_code.is_none());
        assert!(item.last_error_message.is_none());
    }

    #[test]
    fn startup_never_auto_retries_an_interrupted_send() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("mail.sqlite3");
        {
            let database = Database::open(&path).expect("open database");
            database
                .connection
                .execute(
                    "INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,created_at,updated_at) VALUES ('crash-account','generic','crash@example.com','Crash',1,'automatic','now','now')",
                    [],
                )
                .expect("account");
            database
                .connection
                .execute(
                    "INSERT INTO outbox (id,account_id,state,created_at,updated_at) VALUES ('crash-item','crash-account','sending','now','now')",
                    [],
                )
                .expect("simulate interrupted sender");
        }

        let database = Database::open(&path).expect("reopen database");
        let item = database
            .list_outbox(Some("crash-account"))
            .expect("outbox")
            .into_iter()
            .next()
            .expect("recovered item");
        assert_eq!(item.state, "outcome_unknown");
        assert_eq!(
            item.last_error_code.as_deref(),
            Some("interrupted_during_send")
        );
        assert!(item
            .last_error_message
            .as_deref()
            .is_some_and(|message| message.contains("不会自动重试")));
    }

    #[test]
    fn startup_requeues_an_interrupted_pending_imap_operation() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("mail.sqlite3");
        let operation_id;
        {
            let mut database = Database::open(&path).expect("open database");
            let preset = provider_presets()
                .into_iter()
                .find(|item| item.id == "qq")
                .expect("preset");
            let account = database
                .create_account(
                    &CreateAccountInput {
                        email: "pending-crash@qq.com".into(),
                        display_name: "Pending crash".into(),
                        provider_id: "qq".into(),
                        secret: "not persisted".into(),
                        incoming_secret: None,
                        outgoing_secret: None,
                        incoming: None,
                        outgoing: None,
                    },
                    &preset,
                    "account/pending-crash/incoming",
                    "account/pending-crash/outgoing",
                    true,
                    true,
                )
                .expect("account");
            let mailbox = database
                .upsert_remote_mailboxes(
                    &account.id,
                    &[SyncedMailboxInput {
                        remote_id: "INBOX".into(),
                        display_name: "Inbox".into(),
                        delimiter: Some("/".into()),
                        special_role: Some("inbox".into()),
                        selectable: true,
                    }],
                )
                .expect("mailbox")
                .remove(0);
            database
                .apply_imap_snapshot(
                    &account.id,
                    "INBOX",
                    snapshot_metadata(Some(8), 1, 1, true),
                    &[SyncedMessageInput {
                        uid: 1,
                        flags: Vec::new(),
                        received_at: Some("2026-09-03T00:00:00Z".into()),
                        size_bytes: None,
                        rfc_message_id: Some("<pending-crash@example.com>".into()),
                        subject: "Pending crash".into(),
                        preview: String::new(),
                        body_text: None,
                        body_html_text: None,
                        has_attachment: false,
                        from: None,
                        to: Vec::new(),
                    }],
                )
                .expect("snapshot");
            let message = database
                .list_messages(Some(&mailbox.id), 1)
                .expect("messages")
                .remove(0);
            database
                .mutate_message(&message.id, &mailbox.id, Some(true), None)
                .expect("mutation");
            operation_id = database
                .claim_pending_imap_operations(&account.id, 1)
                .expect("claim")
                .remove(0)
                .id;
        }

        let database = Database::open(&path).expect("reopen database");
        let recovered: (String, i64, Option<String>) = database
            .connection
            .query_row(
                "SELECT state,retry_count,last_error_code FROM pending_operations WHERE id=?",
                [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("recovered operation");
        assert_eq!(recovered.0, "failed");
        assert_eq!(recovered.1, 1);
        assert_eq!(recovered.2.as_deref(), Some("interrupted_during_operation"));
    }

    #[test]
    fn startup_dispatches_only_never_started_queued_items() {
        let mut database = Database::open_in_memory().expect("database");
        database
            .connection
            .execute(
                "INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,created_at,updated_at) VALUES ('dispatch-account','generic','dispatch@example.com','Dispatch',1,'automatic','now','now')",
                [],
            )
            .expect("account");
        for (id, state) in [
            ("queued-item", "queued"),
            ("sending-item", "sending"),
            ("sent-item", "sent"),
            ("unknown-item", "outcome_unknown"),
            ("failed-item", "failed"),
            ("cancelled-item", "cancelled"),
        ] {
            database
                .connection
                .execute(
                    "INSERT INTO outbox (id,account_id,state,created_at,updated_at) VALUES (?,'dispatch-account',?,'now','now')",
                    params![id, state],
                )
                .expect("outbox item");
        }

        database
            .recover_interrupted_outbox()
            .expect("recover interrupted sender");
        assert_eq!(
            database.queued_outbox_ids().expect("startup queue"),
            vec!["queued-item"]
        );

        let recovered_state: String = database
            .connection
            .query_row(
                "SELECT state FROM outbox WHERE id='sending-item'",
                [],
                |row| row.get(0),
            )
            .expect("recovered state");
        assert_eq!(recovered_state, "outcome_unknown");
    }

    #[test]
    fn startup_clears_stale_syncing_without_advancing_last_success() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("mail.sqlite3");
        {
            let database = Database::open(&path).expect("open database");
            database
                .connection
                .execute(
                    "INSERT INTO accounts (id,provider_id,email,display_name,enabled,sync_policy,created_at,updated_at) VALUES ('sync-crash-account','generic','sync-crash@example.com','Sync crash',1,'automatic','now','now')",
                    [],
                )
                .expect("account");
            database
                .connection
                .execute(
                    "INSERT INTO provider_metadata (account_id,key,value_json,updated_at) VALUES ('sync-crash-account','sync_status',?,'now')",
                    [serde_json::json!("syncing").to_string()],
                )
                .expect("sync status");
            database
                .connection
                .execute(
                    "INSERT INTO provider_metadata (account_id,key,value_json,updated_at) VALUES ('sync-crash-account','last_sync_success',?,'now')",
                    [serde_json::json!("2026-09-01T00:00:00Z").to_string()],
                )
                .expect("last success");
        }

        let database = Database::open(&path).expect("reopen database");
        let account = database
            .list_accounts()
            .expect("accounts")
            .into_iter()
            .next()
            .expect("account");
        assert_eq!(account.sync_status, "idle");
        assert_eq!(
            account.last_synced_at.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
    }

    #[test]
    fn body_hydration_revalidates_uid_identity_without_advancing_cursor() {
        let mut database = Database::open_in_memory().expect("database");
        let preset = provider_presets()
            .into_iter()
            .find(|item| item.id == "qq")
            .expect("QQ preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "body@qq.com".into(),
                    display_name: "Body".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/body/incoming",
                "account/body/outgoing",
                true,
                true,
            )
            .expect("account");
        let mailbox_id = database
            .upsert_remote_mailboxes(
                &account.id,
                &[SyncedMailboxInput {
                    remote_id: "INBOX".into(),
                    display_name: "Inbox".into(),
                    delimiter: Some("/".into()),
                    special_role: Some("inbox".into()),
                    selectable: true,
                }],
            )
            .expect("mailbox")
            .remove(0)
            .id;
        database
            .apply_imap_snapshot(
                &account.id,
                "INBOX",
                snapshot_metadata(Some(777), 1, 1, true),
                &[SyncedMessageInput {
                    uid: 19,
                    flags: Vec::new(),
                    received_at: Some("2026-09-03T00:00:00Z".into()),
                    size_bytes: Some(512),
                    rfc_message_id: Some("<body@example.com>".into()),
                    subject: "Needs body".into(),
                    preview: String::new(),
                    body_text: None,
                    body_html_text: None,
                    has_attachment: false,
                    from: None,
                    to: Vec::new(),
                }],
            )
            .expect("metadata snapshot");
        let message_id = database
            .list_messages(None, 10)
            .expect("messages")
            .remove(0)
            .id;
        let locator = database
            .imap_body_locator(&message_id, &mailbox_id)
            .expect("body locator");
        assert_eq!(locator.mailbox_remote_id, "INBOX");
        assert_eq!(locator.uid_validity, Some(777));
        assert_eq!(locator.uid, 19);
        assert!(!locator.body_cached);
        let cursor_before = database
            .imap_sync_cursor(&account.id, "INBOX")
            .expect("cursor before hydration");

        let hydrated = database
            .store_hydrated_message_body(
                &locator,
                &HydratedMessageBody {
                    preview: "Hydrated preview".into(),
                    body_text: Some("Hydrated body".into()),
                    body_html_text: None,
                    has_attachment: true,
                    attachments: vec![ParsedAttachment {
                        filename: "notes.txt".into(),
                        content_type: "text/plain".into(),
                        bytes: b"download me".to_vec(),
                    }],
                },
            )
            .expect("hydrate body");
        assert_eq!(hydrated.body_text.as_deref(), Some("Hydrated body"));
        assert_eq!(hydrated.preview, "Hydrated preview");
        assert!(hydrated.has_attachment);
        assert_eq!(hydrated.attachments.len(), 1);
        let (attachment, bytes) = database
            .attachment_payload(&hydrated.attachments[0].id)
            .expect("cached attachment payload");
        assert_eq!(attachment.filename, "notes.txt");
        assert_eq!(bytes, b"download me");
        assert_eq!(
            database
                .imap_sync_cursor(&account.id, "INBOX")
                .expect("cursor after hydration"),
            cursor_before
        );
        assert!(
            database
                .imap_body_locator(&message_id, &mailbox_id)
                .expect("cached locator")
                .body_cached
        );

        database.clear_cache().expect("clear hydrated body");
        let in_flight_before_clear = database
            .imap_body_locator(&message_id, &mailbox_id)
            .expect("metadata locator before cache clear");
        database
            .clear_cache()
            .expect("invalidate in-flight body fetches");
        let clear_race = database
            .store_hydrated_message_body(
                &in_flight_before_clear,
                &HydratedMessageBody {
                    preview: "Stale after clear".into(),
                    body_text: Some("must not persist after clear".into()),
                    body_html_text: None,
                    has_attachment: false,
                    attachments: Vec::new(),
                },
            )
            .expect_err("cache clear must reject an in-flight body response");
        assert!(matches!(clear_race, crate::errors::AppError::Protocol(_)));

        let identity_locator = database
            .imap_body_locator(&message_id, &mailbox_id)
            .expect("locator before identity change");
        database
            .connection
            .execute(
                "UPDATE message_instances SET uid_validity=778 WHERE id=?",
                [&identity_locator.message_instance_id],
            )
            .expect("simulate UIDVALIDITY reset");
        let error = database
            .store_hydrated_message_body(
                &identity_locator,
                &HydratedMessageBody {
                    preview: "Wrong message".into(),
                    body_text: Some("must not persist".into()),
                    body_html_text: None,
                    has_attachment: false,
                    attachments: Vec::new(),
                },
            )
            .expect_err("stale locator must be rejected");
        assert!(matches!(error, crate::errors::AppError::Protocol(_)));
        assert!(database
            .get_message(&message_id)
            .expect("unchanged message")
            .body_text
            .is_none());
    }
}

use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use uuid::Uuid;

use crate::backends::{incoming::IncomingConfig, outgoing::OutgoingConfig};
use crate::domain::message::normalize_subject;
use crate::domain::{
    account::CreateAccountInput, Account, Address, DraftInput, Mailbox, Message, OutboxItem,
};
use crate::errors::AppError;
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
}

type DraftRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

impl Database {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let connection = Connection::open(path).map_err(AppError::from)?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        let connection = Connection::open_in_memory().map_err(AppError::from)?;
        let mut database = Self { connection };
        database.configure()?;
        database.migrate()?;
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
            .execute_batch(include_str!("../../migrations/0001_init.sql"))
            .map_err(AppError::from)
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>, AppError> {
        let mut statement = self.connection.prepare("SELECT a.id,a.provider_id,a.email,a.display_name,a.enabled,a.sync_policy,a.incoming_secret_ref IS NOT NULL,a.outgoing_secret_ref IS NOT NULL,COALESCE(pm.value_json,'idle'),pm.updated_at FROM accounts a LEFT JOIN provider_metadata pm ON pm.account_id=a.id AND pm.key='sync_status' ORDER BY a.created_at").map_err(AppError::from)?;
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
                    last_synced_at: row.get(9)?,
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
        let mut statement = self.connection.prepare("SELECT id,account_id,remote_id,name,display_name,special_role,unread_count,total_count,sync_enabled FROM mailboxes WHERE account_id=? ORDER BY CASE special_role WHEN 'inbox' THEN 0 WHEN 'starred' THEN 1 WHEN 'drafts' THEN 2 WHEN 'sent' THEN 3 ELSE 4 END,name").map_err(AppError::from)?;
        let rows = statement
            .query_map([account_id], |row| {
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
            })
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn list_messages(
        &self,
        mailbox_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, AppError> {
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(mi.flags_json,'"\\Seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(mi.flags_json,'"\\Flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments a JOIN message_parts p ON p.id=a.message_part_id WHERE p.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown') FROM messages m JOIN message_instances mi ON mi.message_id=m.id WHERE (?1 IS NULL OR mi.mailbox_id=?1) AND mi.is_deleted=0 ORDER BY COALESCE(m.received_at,m.sent_at) DESC LIMIT ?2"#;
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let mapped = statement
            .query_map(params![mailbox_id, limit], message_from_row)
            .map_err(AppError::from)?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)
    }

    pub fn search_messages(
        &self,
        mailbox_id: Option<&str>,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Message>, AppError> {
        let fts_query = build_fts_query(query);
        if fts_query.is_empty() {
            return self.list_messages(mailbox_id, limit);
        }
        let sql = r#"SELECT m.id,m.account_id,mi.mailbox_id,COALESCE(m.thread_id,m.id),m.rfc_message_id,m.subject,m.normalized_subject,COALESCE(m.received_at,m.sent_at),m.preview,m.body_text,m.body_html_text,CASE WHEN instr(mi.flags_json,'"\\Seen"') > 0 THEN 1 ELSE 0 END,CASE WHEN instr(mi.flags_json,'"\\Flagged"') > 0 THEN 1 ELSE 0 END,m.has_attachment,(SELECT count(*) FROM attachments a JOIN message_parts p ON p.id=a.message_part_id WHERE p.message_id=m.id),m.size_bytes,COALESCE((SELECT display_name FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),''),COALESCE((SELECT email FROM message_addresses WHERE message_id=m.id AND kind='from' ORDER BY position LIMIT 1),'unknown') FROM message_fts f JOIN messages m ON m.id=f.message_id JOIN message_instances mi ON mi.message_id=m.id WHERE f.message_fts MATCH ?1 AND (?2 IS NULL OR mi.mailbox_id=?2) AND mi.is_deleted=0 ORDER BY COALESCE(m.received_at,m.sent_at) DESC LIMIT ?3"#;
        let mut statement = self.connection.prepare(sql).map_err(AppError::from)?;
        let mapped = statement
            .query_map(params![fts_query, mailbox_id, limit], message_from_row)
            .map_err(AppError::from)?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::from)
    }

    pub fn get_message(&self, message_id: &str) -> Result<Message, AppError> {
        self.list_messages(None, 1000)?
            .into_iter()
            .find(|message| message.id == message_id)
            .ok_or_else(|| AppError::not_found("message"))
    }

    pub fn mutate_message(
        &mut self,
        message_id: &str,
        is_read: Option<bool>,
        is_starred: Option<bool>,
    ) -> Result<Message, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let instance: Option<(String, String, String)> = tx
            .query_row(
                "SELECT id,account_id,flags_json FROM message_instances WHERE message_id=? LIMIT 1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(AppError::from)?;
        let (instance_id, account_id, mut flags_json) =
            instance.ok_or_else(|| AppError::not_found("message instance"))?;
        let mut flags: Vec<String> = serde_json::from_str(&flags_json).unwrap_or_default();
        update_flag(&mut flags, "\\Seen", is_read);
        update_flag(&mut flags, "\\Flagged", is_starred);
        flags_json = serde_json::to_string(&flags).map_err(AppError::from)?;
        tx.execute(
            "UPDATE message_instances SET flags_json=?,last_synced_at=? WHERE id=?",
            params![flags_json, Utc::now().to_rfc3339(), instance_id],
        )
        .map_err(AppError::from)?;
        tx.execute("INSERT INTO pending_operations (id,account_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, instance_id, "set_flags", json!({ "is_read": is_read, "is_starred": is_starred }).to_string(), Utc::now().to_rfc3339(), Utc::now().to_rfc3339()]).map_err(AppError::from)?;
        tx.commit().map_err(AppError::from)?;
        self.get_message(message_id)
    }

    pub fn move_messages(
        &mut self,
        message_ids: &[String],
        target_mailbox_id: &str,
    ) -> Result<usize, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let target_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mailboxes WHERE id=?)",
                [target_mailbox_id],
                |row| row.get(0),
            )
            .map_err(AppError::from)?;
        if !target_exists {
            return Err(AppError::not_found("target mailbox"));
        }
        let now = Utc::now().to_rfc3339();
        let mut moved = 0;
        for message_id in message_ids {
            let instance: Option<(String, String, String)> = tx
                .query_row("SELECT mi.id,m.account_id,mi.mailbox_id FROM message_instances mi JOIN messages m ON m.id=mi.message_id WHERE mi.message_id=? AND mi.is_deleted=0 LIMIT 1", [message_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()
                .map_err(AppError::from)?;
            if let Some((instance_id, account_id, source_mailbox_id)) = instance {
                if source_mailbox_id == target_mailbox_id {
                    continue;
                }
                tx.execute(
                    "UPDATE message_instances SET mailbox_id=?,last_synced_at=? WHERE id=?",
                    params![target_mailbox_id, now, instance_id],
                )
                .map_err(AppError::from)?;
                tx.execute("INSERT INTO pending_operations (id,account_id,mailbox_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, target_mailbox_id, instance_id, "move", json!({ "from_mailbox_id": source_mailbox_id, "to_mailbox_id": target_mailbox_id, "message_id": message_id }).to_string(), now, now]).map_err(AppError::from)?;
                moved += 1;
            }
        }
        tx.commit().map_err(AppError::from)?;
        Ok(moved)
    }

    pub fn delete_messages(
        &mut self,
        message_ids: &[String],
        permanent: bool,
    ) -> Result<usize, AppError> {
        let tx = self.connection.transaction().map_err(AppError::from)?;
        let now = Utc::now().to_rfc3339();
        let mut deleted = 0;
        for message_id in message_ids {
            let instance: Option<(String, String)> = tx
                .query_row("SELECT mi.id,m.account_id FROM message_instances mi JOIN messages m ON m.id=mi.message_id WHERE mi.message_id=? AND mi.is_deleted=0 LIMIT 1", [message_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(AppError::from)?;
            if let Some((instance_id, account_id)) = instance {
                tx.execute(
                    "UPDATE message_instances SET is_deleted=1,last_synced_at=? WHERE id=?",
                    params![now, instance_id],
                )
                .map_err(AppError::from)?;
                tx.execute("INSERT INTO pending_operations (id,account_id,message_instance_id,operation_type,payload_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)", params![Uuid::new_v4().to_string(), account_id, instance_id, if permanent { "permanent_delete" } else { "trash" }, json!({ "message_id": message_id, "permanent": permanent }).to_string(), now, now]).map_err(AppError::from)?;
                deleted += 1;
            }
        }
        tx.commit().map_err(AppError::from)?;
        Ok(deleted)
    }

    pub fn save_draft(&mut self, input: &DraftInput) -> Result<String, AppError> {
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = Utc::now().to_rfc3339();
        self.connection.execute("INSERT INTO drafts (id,account_id,to_json,cc_json,bcc_json,subject,body_text,in_reply_to,references_json,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET to_json=excluded.to_json,cc_json=excluded.cc_json,bcc_json=excluded.bcc_json,subject=excluded.subject,body_text=excluded.body_text,in_reply_to=excluded.in_reply_to,references_json=excluded.references_json,updated_at=excluded.updated_at", params![id, input.account_id, serde_json::to_string(&split_addresses(&input.to)).map_err(AppError::from)?, serde_json::to_string(&split_addresses(input.cc.as_deref().unwrap_or(""))).map_err(AppError::from)?, serde_json::to_string(&split_addresses(input.bcc.as_deref().unwrap_or(""))).map_err(AppError::from)?, input.subject, input.body_text, input.in_reply_to, serde_json::to_string(&input.references.clone().unwrap_or_default()).map_err(AppError::from)?, now]).map_err(AppError::from)?;
        Ok(id)
    }

    pub fn queue_draft(&mut self, input: &DraftInput) -> Result<String, AppError> {
        let draft_id = self.save_draft(input)?;
        let outbox_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.connection.execute("INSERT INTO outbox (id,draft_id,account_id,state,created_at,updated_at) VALUES (?,?,?,'queued',?,?)", params![outbox_id, draft_id, input.account_id, now, now]).map_err(AppError::from)?;
        Ok(outbox_id)
    }

    pub fn list_outbox(&self, account_id: Option<&str>) -> Result<Vec<OutboxItem>, AppError> {
        let sql = "SELECT o.id,o.account_id,COALESCE(d.subject,''),COALESCE(d.to_json,'[]'),o.state,o.updated_at FROM outbox o LEFT JOIN drafts d ON d.id=o.draft_id WHERE (?1 IS NULL OR o.account_id=?1) ORDER BY o.updated_at DESC";
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
                    updated_at: row.get(5)?,
                })
            })
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn outbox_draft(&self, outbox_id: &str) -> Result<OutboxDraft, AppError> {
        let row: Option<(String, String, String, String, String, String)> = self
            .connection
            .query_row(
                "SELECT o.account_id,COALESCE(d.to_json,'[]'),COALESCE(d.cc_json,'[]'),COALESCE(d.bcc_json,'[]'),COALESCE(d.subject,''),COALESCE(d.body_text,'') FROM outbox o LEFT JOIN drafts d ON d.id=o.draft_id WHERE o.id=?",
                [outbox_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(AppError::from)?;
        let (account_id, to, cc, bcc, subject, body_text) =
            row.ok_or_else(|| AppError::not_found("outbox item"))?;
        Ok(OutboxDraft {
            account_id,
            to: serde_json::from_str(&to).unwrap_or_default(),
            cc: serde_json::from_str(&cc).unwrap_or_default(),
            bcc: serde_json::from_str(&bcc).unwrap_or_default(),
            subject,
            body_text,
        })
    }

    pub fn account_email(&self, account_id: &str) -> Result<String, AppError> {
        self.connection
            .query_row(
                "SELECT email FROM accounts WHERE id=?",
                [account_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::from)?
            .ok_or_else(|| AppError::not_found("account"))
    }

    pub fn set_outbox_state(
        &mut self,
        outbox_id: &str,
        state: &str,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), AppError> {
        let changed = self
            .connection
            .execute(
                "UPDATE outbox SET state=?,last_error_code=?,last_error_message=?,updated_at=? WHERE id=?",
                params![state, error_code, error_message, Utc::now().to_rfc3339(), outbox_id],
            )
            .map_err(AppError::from)?;
        if changed == 0 {
            Err(AppError::not_found("outbox item"))
        } else {
            Ok(())
        }
    }

    pub fn load_draft(&self, draft_id: &str) -> Result<DraftInput, AppError> {
        let row: Option<DraftRow> =
            self.connection
                .query_row(
                    "SELECT id,account_id,to_json,cc_json,bcc_json,subject,body_text,references_json FROM drafts WHERE id=?",
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
                        ))
                    },
                )
                .optional()
                .map_err(AppError::from)?;
        let (id, account_id, to, cc, bcc, subject, body_text, references) =
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
            in_reply_to: None,
            references: serde_json::from_str(&references).unwrap_or_default(),
        })
    }

    pub fn delete_draft(&mut self, draft_id: &str) -> Result<(), AppError> {
        let deleted = self
            .connection
            .execute("DELETE FROM drafts WHERE id=?", [draft_id])
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
        let allowed = ["theme", "safeReading", "syncPolicy"];
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
        self.connection
            .execute("DELETE FROM messages", [])
            .map_err(AppError::from)
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

fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let subject: String = row.get(5)?;
    let mailbox_id: String = row.get(2)?;
    let is_read: bool = row.get::<_, i64>(11)? != 0;
    let is_starred: bool = row.get::<_, i64>(12)? != 0;
    let from_name = row
        .get::<_, Option<String>>(16)?
        .filter(|name| !name.is_empty());
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
        labels: Vec::new(),
        size_bytes: row.get(15)?,
        from: Address {
            name: from_name,
            email: row.get(17)?,
        },
        to: Vec::new(),
    })
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

fn update_flag(flags: &mut Vec<String>, flag: &str, value: Option<bool>) {
    if let Some(value) = value {
        if value && !flags.iter().any(|item| item == flag) {
            flags.push(flag.into());
        } else if !value {
            flags.retain(|item| item != flag);
        }
    }
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
    use super::Database;
    use crate::domain::account::CreateAccountInput;
    use crate::providers::registry::provider_presets;

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
                "INSERT INTO message_instances (id,message_id,mailbox_id,remote_locator,flags_json,last_synced_at) VALUES ('inst-fts','msg-fts','mb-fts','1','[\"\\\\Seen\"]','2026-09-02T00:00:00Z')",
                [],
            )
            .expect("instance");
        let found = database
            .search_messages(Some("mb-fts"), "Offline", 20)
            .expect("search");
        assert_eq!(found.len(), 1);
        assert!(found[0].is_read);
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
                .move_messages(&[String::from("msg-fts")], "mb-archive")
                .expect("move"),
            1
        );
        assert_eq!(
            database
                .list_messages(Some("mb-archive"), 20)
                .expect("moved list")
                .len(),
            1
        );
        assert_eq!(
            database
                .delete_messages(&[String::from("msg-fts")], false)
                .expect("trash"),
            1
        );
        assert!(database
            .list_messages(Some("mb-archive"), 20)
            .expect("deleted list")
            .is_empty());
    }
}

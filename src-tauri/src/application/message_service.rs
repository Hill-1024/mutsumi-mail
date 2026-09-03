use std::future::Future;

use crate::app_state::AppState;
use crate::backends::imap::{ImapIncomingBackend, MAX_MESSAGE_BYTES};
use crate::backends::incoming::{
    IncomingConfig, IncomingError, IncomingMailBackend, IncomingMessageFetch,
};
use crate::domain::Message;
use crate::errors::AppError;
use crate::mime::parser::parse_rfc822;
use crate::storage::database::{HydratedMessageBody, ImapBodyLocator};

pub fn list_messages(
    state: &AppState,
    mailbox_id: Option<String>,
    limit: u32,
) -> Result<Vec<Message>, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_messages(mailbox_id.as_deref(), limit.min(500))
}

pub fn list_messages_in_scope(
    state: &AppState,
    account_id: Option<String>,
    mailbox_id: Option<String>,
    mailbox_role: Option<String>,
    is_starred: Option<bool>,
    limit: u32,
) -> Result<Vec<Message>, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_messages_in_scope(
            account_id.as_deref(),
            mailbox_id.as_deref(),
            mailbox_role.as_deref(),
            is_starred,
            limit.min(500),
        )
}
pub fn get_message(state: &AppState, message_id: String) -> Result<Message, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .get_message(&message_id)
}

pub async fn fetch_message_body(
    state: &AppState,
    message_id: String,
    mailbox_id: String,
) -> Result<Message, AppError> {
    fetch_message_body_with(
        state,
        message_id,
        mailbox_id,
        |config, secret, mailbox_remote_id, uid| async move {
            ImapIncomingBackend::new(config)
                .fetch_remote_message(&secret, &mailbox_remote_id, uid)
                .await
        },
    )
    .await
}

async fn fetch_message_body_with<F, Fut>(
    state: &AppState,
    message_id: String,
    mailbox_id: String,
    fetch_remote: F,
) -> Result<Message, AppError>
where
    F: FnOnce(IncomingConfig, String, String, u32) -> Fut,
    Fut: Future<Output = Result<Option<IncomingMessageFetch>, IncomingError>>,
{
    // Resolve every local dependency together, then release the SQLite mutex
    // before touching the network.
    let (locator, config, secret_ref) = {
        let database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        let locator = database.imap_body_locator(&message_id, &mailbox_id)?;
        if locator.body_cached {
            return database.get_message_in_mailbox(&message_id, &mailbox_id);
        }
        if !locator.account_enabled {
            return Err(AppError::Capability(
                "该账号已停用，无法下载邮件正文".into(),
            ));
        }
        let config = database.incoming_config(&locator.account_id)?;
        let secret_ref = database
            .account_secret_refs(&locator.account_id)?
            .0
            .ok_or_else(|| AppError::Capability("该账号没有可用的收件凭据".into()))?;
        (locator, config, secret_ref)
    };

    let secret = state
        .secret_store
        .get(&secret_ref)
        .map_err(|error| AppError::SecretStore(error.to_string()))?;
    let fetched = fetch_remote(
        config,
        secret,
        locator.mailbox_remote_id.clone(),
        locator.uid,
    )
    .await
    .map_err(incoming_error_to_app)?
    .ok_or_else(|| AppError::not_found("remote IMAP message"))?;
    validate_fetched_identity(&locator, &fetched)?;

    let size_bytes = fetched.message.size_bytes.map(u64::from);
    let raw = fetched.message.raw_rfc822.ok_or_else(|| {
        if size_bytes.is_some_and(|size| size > MAX_MESSAGE_BYTES) {
            AppError::Capability(format!(
                "邮件超过 {} MiB 的正文下载上限",
                MAX_MESSAGE_BYTES / (1024 * 1024)
            ))
        } else {
            AppError::Protocol("IMAP 服务器未返回请求的邮件正文".into())
        }
    })?;
    let parsed = parse_rfc822(&raw)
        .ok_or_else(|| AppError::Protocol("无法解析 IMAP 服务器返回的 RFC 822 邮件".into()))?;
    let body = HydratedMessageBody {
        preview: parsed.preview,
        body_text: parsed.text,
        body_html_text: parsed.html_text,
        has_attachment: parsed.attachment_count > 0,
    };

    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .store_hydrated_message_body(&locator, &body)
}

fn validate_fetched_identity(
    locator: &ImapBodyLocator,
    fetched: &IncomingMessageFetch,
) -> Result<(), AppError> {
    if fetched.remote_id != locator.mailbox_remote_id || fetched.message.uid != locator.uid {
        return Err(AppError::Protocol(
            "IMAP 服务器返回了非请求邮件的正文".into(),
        ));
    }
    match (locator.uid_validity, fetched.uid_validity) {
        (Some(expected), Some(received)) if expected == received => Ok(()),
        (Some(_), Some(_)) => Err(AppError::Protocol(
            "IMAP 文件夹 UIDVALIDITY 已变化，请先同步后重试".into(),
        )),
        _ => Err(AppError::Protocol(
            "缺少安全校验邮件 UID 所需的 UIDVALIDITY，请先同步后重试".into(),
        )),
    }
}

fn incoming_error_to_app(error: IncomingError) -> AppError {
    match error {
        IncomingError::Network(message) | IncomingError::Tls(message) => AppError::Network(message),
        IncomingError::Authentication => AppError::Authentication,
        IncomingError::Protocol(message) => AppError::Protocol(message),
        IncomingError::Unsupported(message) => AppError::Capability(message),
    }
}
pub fn mutate_message(
    state: &AppState,
    message_id: String,
    mailbox_id: String,
    is_read: Option<bool>,
    is_starred: Option<bool>,
) -> Result<Message, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .mutate_message(&message_id, &mailbox_id, is_read, is_starred)
}

pub fn move_messages(
    state: &AppState,
    message_refs: Vec<(String, String)>,
    mailbox_id: String,
) -> Result<usize, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .move_messages(&message_refs, &mailbox_id)
}

pub fn delete_messages(
    state: &AppState,
    message_refs: Vec<(String, String)>,
    permanent: bool,
) -> Result<usize, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .delete_messages(&message_refs, permanent)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::fetch_message_body_with;
    use crate::app_state::{AppState, SyncCoordinator};
    use crate::auth::secret_store::{SecretStore, SecretStoreError};
    use crate::backends::incoming::{IncomingMessage, IncomingMessageFetch};
    use crate::domain::account::CreateAccountInput;
    use crate::providers::registry::provider_presets;
    use crate::storage::database::{
        Database, ImapSnapshotMetadata, SyncedMailboxInput, SyncedMessageInput,
    };

    struct TestSecretStore;

    impl SecretStore for TestSecretStore {
        fn set(&self, _reference: &str, _secret: &str) -> Result<(), SecretStoreError> {
            Ok(())
        }

        fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
            if reference == "account/body/incoming" {
                Ok("authorization-code".into())
            } else {
                Err(SecretStoreError::NotFound)
            }
        }

        fn delete(&self, _reference: &str) -> Result<(), SecretStoreError> {
            Ok(())
        }
    }

    fn metadata_only_state() -> (AppState, String, String) {
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
                ImapSnapshotMetadata {
                    uid_validity: Some(91),
                    total_count: 1,
                    unread_count: 1,
                    complete_mailbox: true,
                },
                &[SyncedMessageInput {
                    uid: 7,
                    flags: Vec::new(),
                    received_at: Some("2026-09-03T00:00:00Z".into()),
                    size_bytes: Some(256),
                    rfc_message_id: Some("<body-service@example.com>".into()),
                    subject: "Lazy body".into(),
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
            .list_messages(Some(&mailbox_id), 10)
            .expect("messages")
            .remove(0)
            .id;
        (
            AppState {
                database: Mutex::new(database),
                secret_store: Arc::new(TestSecretStore),
                sync: Arc::new(SyncCoordinator::new()),
            },
            message_id,
            mailbox_id,
        )
    }

    #[tokio::test]
    async fn fetches_sanitizes_and_caches_body_without_holding_database_lock() {
        let (state, message_id, mailbox_id) = metadata_only_state();
        let raw = b"From: sender@example.com\r\nSubject: Lazy body\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hello</p><script>doBadThing()</script>".to_vec();
        let fetched = fetch_message_body_with(
            &state,
            message_id.clone(),
            mailbox_id.clone(),
            |config, secret, mailbox, uid| {
                assert_eq!(config.host, "imap.qq.com");
                assert_eq!(secret, "authorization-code");
                assert_eq!(mailbox, "INBOX");
                assert_eq!(uid, 7);
                assert!(state.database.try_lock().is_ok());
                async move {
                    Ok(Some(IncomingMessageFetch {
                        remote_id: mailbox,
                        uid_validity: Some(91),
                        message: IncomingMessage {
                            sequence: 1,
                            uid,
                            flags: Vec::new(),
                            internal_date: None,
                            size_bytes: Some(raw.len() as u32),
                            raw_headers: None,
                            raw_rfc822: Some(raw),
                        },
                    }))
                }
            },
        )
        .await
        .expect("hydrated message");
        assert_eq!(fetched.mailbox_id, mailbox_id);
        assert_eq!(fetched.body_html_text.as_deref(), Some("Hello"));
        assert!(!fetched
            .body_html_text
            .as_deref()
            .unwrap_or_default()
            .contains("doBadThing"));

        let network_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&network_calls);
        let cached = fetch_message_body_with(
            &state,
            message_id,
            mailbox_id,
            move |_config, _secret, _mailbox, _uid| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(crate::backends::incoming::IncomingError::Network(
                    "cached body must not perform network I/O".into(),
                ))
            },
        )
        .await
        .expect("cached message");
        assert_eq!(cached.body_html_text.as_deref(), Some("Hello"));
        assert_eq!(network_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejects_changed_remote_uidvalidity_without_persisting_body() {
        let (state, message_id, mailbox_id) = metadata_only_state();
        let error = fetch_message_body_with(
            &state,
            message_id.clone(),
            mailbox_id,
            |_config, _secret, mailbox, uid| async move {
                Ok(Some(IncomingMessageFetch {
                    remote_id: mailbox,
                    uid_validity: Some(92),
                    message: IncomingMessage {
                        sequence: 1,
                        uid,
                        flags: Vec::new(),
                        internal_date: None,
                        size_bytes: Some(64),
                        raw_headers: None,
                        raw_rfc822: Some(b"Subject: Wrong\r\n\r\nWrong body".to_vec()),
                    },
                }))
            },
        )
        .await
        .expect_err("changed UIDVALIDITY must fail");
        assert!(matches!(error, crate::errors::AppError::Protocol(_)));
        assert!(state
            .database
            .lock()
            .expect("database lock")
            .get_message(&message_id)
            .expect("local metadata")
            .body_text
            .is_none());
    }
}

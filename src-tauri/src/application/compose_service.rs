use crate::app_state::AppState;
use crate::application::sync_service;
use crate::backends::outgoing::{OutgoingError, OutgoingMailBackend, SendResult};
use crate::backends::smtp::SmtpOutgoingBackend;
use crate::domain::{DraftAttachment, DraftInput, OutboxItem};
use crate::errors::AppError;
use crate::storage::database::{OutboxDraft, PreparedOutboxPayload};
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::Message;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

pub fn save_draft(state: &AppState, input: DraftInput) -> Result<serde_json::Value, AppError> {
    let id = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .save_draft(&input)?;
    Ok(serde_json::json!({ "id": id, "savedAt": chrono::Utc::now().to_rfc3339() }))
}
#[allow(dead_code)]
pub fn queue_draft(state: &AppState, input: DraftInput) -> Result<serde_json::Value, AppError> {
    let outbox_id = queue_draft_id(state, input)?;
    Ok(serde_json::json!({ "outboxId": outbox_id, "state": "queued" }))
}

pub fn queue_draft_id(state: &AppState, input: DraftInput) -> Result<String, AppError> {
    queue_draft_with_attachments(state, input, Vec::new())
}

pub fn queue_draft_with_attachments(
    state: &AppState,
    input: DraftInput,
    attachments: Vec<DraftAttachment>,
) -> Result<String, AppError> {
    // Fail synchronously for deterministic errors. The command must not return a queued
    // success for an invalid recipient, broken MIME, disabled account, or missing SMTP secret.
    let prepared = prepare_delivery(state, &draft_from_input(&input), &attachments)?;
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .queue_prepared_draft(&input, &prepared.payload)
}

pub fn list_outbox(
    state: &AppState,
    account_id: Option<String>,
) -> Result<Vec<OutboxItem>, AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_outbox(account_id.as_deref())
}

pub fn spawn_delivery(app: AppHandle, outbox_id: String) {
    tauri::async_runtime::spawn(async move {
        deliver_outbox(app, outbox_id).await;
    });
}

async fn deliver_outbox(app: AppHandle, outbox_id: String) {
    let state = app.state::<AppState>();
    let claimed = match state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))
        .and_then(|mut database| database.claim_outbox_for_sending(&outbox_id))
    {
        Ok(claimed) => claimed,
        Err(error) => {
            tracing::error!(outbox_id = %outbox_id, error = %error, "unable to claim outbox item");
            return;
        }
    };
    if !claimed {
        tracing::debug!(outbox_id = %outbox_id, "outbox item is no longer queued; duplicate delivery skipped");
        return;
    }
    emit_outbox_changed(&app, &outbox_id, "sending");

    let draft = match state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))
        .and_then(|database| database.outbox_draft(&outbox_id))
    {
        Ok(draft) => draft,
        Err(error) => {
            persist_error_and_emit(&app, &state, &outbox_id, &error, "failed");
            return;
        }
    };
    let prepared = match prepare_claimed_delivery(&state, &outbox_id, &draft) {
        Ok(prepared) => prepared,
        Err(error) => {
            persist_error_and_emit(&app, &state, &outbox_id, &error, "failed");
            return;
        }
    };
    let policy = sent_copy_policy(&prepared.config);
    let result = SmtpOutgoingBackend::new(prepared.config)
        .send_mime(
            &prepared.secret,
            prepared.payload.mime,
            &prepared.payload.envelope_from,
            &prepared.payload.recipients,
        )
        .await;
    match result {
        Ok(SendResult::Sent { .. }) => {
            if let Some(sent_copy_state) = persist_sent_and_emit(&app, &state, &outbox_id, policy) {
                if sent_copy_state == "awaiting_server_sync" {
                    match sync_service::start_sync_if_idle(
                        &state,
                        app.clone(),
                        draft.account_id.clone(),
                    ) {
                        Ok(Some(_)) => tracing::debug!(
                            account_id = %draft.account_id,
                            outbox_id = %outbox_id,
                            "started IMAP reconciliation for a confirmed SMTP delivery"
                        ),
                        Ok(None) => tracing::debug!(
                            account_id = %draft.account_id,
                            outbox_id = %outbox_id,
                            "existing IMAP sync will reconcile the confirmed SMTP delivery"
                        ),
                        Err(error) => tracing::warn!(
                            account_id = %draft.account_id,
                            outbox_id = %outbox_id,
                            error = %error,
                            "SMTP delivery succeeded but Sent reconciliation could not start"
                        ),
                    }
                }
            }
        }
        Ok(SendResult::OutcomeUnknown) => {
            let error = AppError::AmbiguousSend;
            persist_error_and_emit(&app, &state, &outbox_id, &error, "outcome_unknown");
        }
        Ok(SendResult::Failed) => {
            let error = AppError::ServerRejected("SMTP 发送失败".into());
            persist_error_and_emit(&app, &state, &outbox_id, &error, "failed");
        }
        Err(error) => {
            let app_error = map_outgoing_error(error);
            let state_name = if matches!(app_error, AppError::AmbiguousSend) {
                "outcome_unknown"
            } else {
                "failed"
            };
            persist_error_and_emit(&app, &state, &outbox_id, &app_error, state_name);
        }
    }
}

struct PreparedDelivery {
    config: crate::backends::outgoing::OutgoingConfig,
    secret: String,
    payload: PreparedOutboxPayload,
}

/// SMTP does not say whether a provider filed a Sent copy. Gmail documents
/// server-managed copies, while QQ exposes a server-side switch; neither is
/// safe for an unconditional client APPEND. Unknown providers are treated the
/// same way until the user explicitly chooses a client-managed policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentCopyPolicy {
    ServerManaged,
    ServerSettingDependent,
    Unknown,
}

fn sent_copy_policy(config: &crate::backends::outgoing::OutgoingConfig) -> SentCopyPolicy {
    let host = config
        .host
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if matches!(host.as_str(), "smtp.gmail.com" | "smtp.googlemail.com") {
        SentCopyPolicy::ServerManaged
    } else if host == "smtp.qq.com" || host == "smtp.exmail.qq.com" {
        SentCopyPolicy::ServerSettingDependent
    } else {
        SentCopyPolicy::Unknown
    }
}

fn prepare_delivery(
    state: &AppState,
    draft: &OutboxDraft,
    attachments: &[DraftAttachment],
) -> Result<PreparedDelivery, AppError> {
    let (config, secret, envelope_from) = outgoing_session(state, draft)?;
    SmtpOutgoingBackend::new(config.clone())
        .validate_configuration(&secret)
        .map_err(map_outgoing_error)?;
    let recipients = draft
        .to
        .iter()
        .chain(draft.cc.iter())
        .chain(draft.bcc.iter())
        .cloned()
        .collect::<Vec<_>>();
    let rfc_message_id = new_message_id(&envelope_from);
    let mime = build_mime(
        draft,
        &envelope_from,
        &recipients,
        &rfc_message_id,
        attachments,
    )?;
    Ok(PreparedDelivery {
        config,
        secret,
        payload: PreparedOutboxPayload {
            envelope_from,
            recipients,
            mime,
            rfc_message_id,
            sent_copy_state: "not_started".into(),
            sent_copy_error_message: None,
            sent_copy_uid_validity: None,
            sent_copy_uid: None,
        },
    })
}

fn prepare_claimed_delivery(
    state: &AppState,
    outbox_id: &str,
    draft: &OutboxDraft,
) -> Result<PreparedDelivery, AppError> {
    // A queued item already owns an immutable MIME payload. Rebuilding the plain-text
    // fallback here is only for rows created before payload persistence existed; the
    // database keeps the original attachment-bearing payload when present.
    let mut prepared = prepare_delivery(state, draft, &[])?;
    let payload = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .store_outbox_payload_if_missing(outbox_id, &prepared.payload)?;
    prepared.payload = payload;
    Ok(prepared)
}

fn new_message_id(envelope_from: &str) -> String {
    let domain = envelope_from
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim())
        .filter(|domain| !domain.is_empty())
        .unwrap_or("mail.invalid");
    format!("<{}@{}>", Uuid::new_v4(), domain)
}

fn draft_from_input(input: &DraftInput) -> OutboxDraft {
    OutboxDraft {
        account_id: input.account_id.clone(),
        to: split_recipient_field(&input.to),
        cc: split_recipient_field(input.cc.as_deref().unwrap_or("")),
        bcc: split_recipient_field(input.bcc.as_deref().unwrap_or("")),
        subject: input.subject.clone(),
        body_text: input.body_text.clone(),
        in_reply_to: input.in_reply_to.clone(),
        references: input.references.clone().unwrap_or_default(),
    }
}

fn split_recipient_field(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|recipient| !recipient.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn outgoing_session(
    state: &AppState,
    draft: &OutboxDraft,
) -> Result<(crate::backends::outgoing::OutgoingConfig, String, String), AppError> {
    let (config, secret_ref, envelope_from) = {
        let database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        database.outgoing_delivery_details(&draft.account_id)?
    };
    let secret = state
        .secret_store
        .get(&secret_ref)
        .map_err(|error| AppError::SecretStore(error.to_string()))?;
    Ok((config, secret, envelope_from))
}

const MAX_ATTACHMENT_COUNT: usize = 20;
const MAX_ATTACHMENT_BYTES: usize = 18 * 1024 * 1024;

fn build_mime(
    draft: &OutboxDraft,
    envelope_from: &str,
    recipients: &[String],
    rfc_message_id: &str,
    attachments: &[DraftAttachment],
) -> Result<Vec<u8>, AppError> {
    validate_attachments(attachments)?;
    let mut builder =
        Message::builder()
            .from(envelope_from.parse().map_err(|error| {
                AppError::InvalidConfiguration(format!("发件人地址无效：{error}"))
            })?)
            .message_id(Some(rfc_message_id.to_owned()))
            .subject(&draft.subject);
    for recipient in &draft.to {
        builder = builder.to(recipient
            .parse()
            .map_err(|error| AppError::InvalidConfiguration(format!("收件人地址无效：{error}")))?);
    }
    for recipient in &draft.cc {
        builder = builder.cc(recipient
            .parse()
            .map_err(|error| AppError::InvalidConfiguration(format!("抄送地址无效：{error}")))?);
    }
    for recipient in &draft.bcc {
        builder =
            builder.bcc(recipient.parse().map_err(|error| {
                AppError::InvalidConfiguration(format!("密送地址无效：{error}"))
            })?);
    }
    if let Some(in_reply_to) = &draft.in_reply_to {
        builder = builder.in_reply_to(in_reply_to.clone());
    }
    if !draft.references.is_empty() {
        builder = builder.references(draft.references.join(" "));
    }
    if recipients.is_empty() {
        return Err(AppError::InvalidConfiguration("至少需要一个收件人".into()));
    }
    let message = if attachments.is_empty() {
        builder
            .header(ContentType::TEXT_PLAIN)
            .body(draft.body_text.clone())
    } else {
        let mut multipart =
            MultiPart::mixed().singlepart(SinglePart::plain(draft.body_text.clone()));
        for attachment in attachments {
            let content_type = ContentType::parse(&attachment.content_type)
                .or_else(|_| ContentType::parse("application/octet-stream"))
                .map_err(|error| {
                    AppError::InvalidConfiguration(format!("附件 MIME 类型无效：{error}"))
                })?;
            multipart = multipart.singlepart(
                Attachment::new(safe_attachment_name(&attachment.name))
                    .body(attachment.bytes.clone(), content_type),
            );
        }
        builder.multipart(multipart)
    };
    message
        .map(|message| {
            let mut formatted = message.formatted();
            if !formatted.ends_with(b"\r\n") {
                formatted.extend_from_slice(b"\r\n");
            }
            formatted
        })
        .map_err(|error| AppError::InvalidConfiguration(format!("无法生成 MIME：{error}")))
}

fn validate_attachments(attachments: &[DraftAttachment]) -> Result<(), AppError> {
    if attachments.len() > MAX_ATTACHMENT_COUNT {
        return Err(AppError::InvalidConfiguration(format!(
            "一次最多添加 {MAX_ATTACHMENT_COUNT} 个附件"
        )));
    }
    let total = attachments.iter().try_fold(0_usize, |total, attachment| {
        total
            .checked_add(attachment.bytes.len())
            .ok_or_else(|| AppError::InvalidConfiguration("附件总大小超出限制".into()))
    })?;
    if total > MAX_ATTACHMENT_BYTES {
        return Err(AppError::InvalidConfiguration(
            "附件总大小最多 18 MiB，以避免 SMTP 编码后超出常见服务商限制".into(),
        ));
    }
    Ok(())
}

fn safe_attachment_name(name: &str) -> String {
    let candidate = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .replace(['\r', '\n', '\0'], "");
    if candidate.is_empty() {
        "attachment".into()
    } else {
        candidate.chars().take(180).collect()
    }
}

fn set_state(
    state: &AppState,
    outbox_id: &str,
    state_name: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .set_outbox_state(outbox_id, state_name, error_code, error_message)
}

fn persist_delivery_error(
    state: &AppState,
    outbox_id: &str,
    error: &AppError,
    state_name: &str,
) -> Result<(), AppError> {
    set_state(
        state,
        outbox_id,
        state_name,
        Some(error.code()),
        Some(&error.to_string()),
    )
}

fn persist_sent_and_emit(
    app: &AppHandle,
    state: &AppState,
    outbox_id: &str,
    policy: SentCopyPolicy,
) -> Option<String> {
    let persisted = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))
        .and_then(|mut database| database.complete_outbox_sent(outbox_id));
    match persisted {
        Ok(sent_copy_state) => {
            let _ = app.emit(
                "outbox-changed",
                serde_json::json!({
                    "outboxId": outbox_id,
                    "state": "sent",
                    "sentCopyState": sent_copy_state,
                }),
            );
            tracing::debug!(
                outbox_id = %outbox_id,
                ?policy,
                sent_copy_state = %sent_copy_state,
                "SMTP delivery persisted; remote Sent copy will be reconciled without an unconditional APPEND"
            );
            Some(sent_copy_state)
        }
        Err(error) => {
            tracing::error!(
                outbox_id = %outbox_id,
                requested_state = "sent",
                error = %error,
                "outbox state was not persisted; success event suppressed"
            );
            None
        }
    }
}

fn persist_error_and_emit(
    app: &AppHandle,
    state: &AppState,
    outbox_id: &str,
    error: &AppError,
    state_name: &str,
) {
    match persist_delivery_error(state, outbox_id, error, state_name) {
        Ok(()) => emit_outbox_changed(app, outbox_id, state_name),
        Err(persist_error) => tracing::error!(
            outbox_id = %outbox_id,
            requested_state = state_name,
            error = %persist_error,
            "outbox error state was not persisted; event suppressed"
        ),
    }
}

fn emit_outbox_changed(app: &AppHandle, outbox_id: &str, state: &str) {
    let _ = app.emit(
        "outbox-changed",
        serde_json::json!({ "outboxId": outbox_id, "state": state }),
    );
}

fn map_outgoing_error(error: OutgoingError) -> AppError {
    match error {
        OutgoingError::Authentication => AppError::Authentication,
        OutgoingError::Unsupported(message) => AppError::Capability(message),
        OutgoingError::AmbiguousSend => AppError::AmbiguousSend,
        OutgoingError::Network(message) | OutgoingError::Tls(message) => AppError::Network(message),
        OutgoingError::Rejected(message) => AppError::ServerRejected(message),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_mime, draft_from_input, new_message_id, sent_copy_policy, SentCopyPolicy};
    use crate::backends::outgoing::OutgoingConfig;
    use crate::domain::{DraftAttachment, DraftInput};
    use crate::errors::AppError;
    use crate::storage::database::OutboxDraft;

    #[test]
    fn builds_mime_with_separate_envelope_recipients() {
        let draft = OutboxDraft {
            account_id: "account".into(),
            to: vec!["to@example.com".into()],
            cc: vec!["cc@example.com".into()],
            bcc: vec!["bcc@example.com".into()],
            subject: "离线草稿".into(),
            body_text: "正文".into(),
            in_reply_to: Some("<parent@example.com>".into()),
            references: vec!["<root@example.com>".into(), "<parent@example.com>".into()],
        };
        let mime = build_mime(
            &draft,
            "from@example.com",
            &[
                "to@example.com".into(),
                "cc@example.com".into(),
                "bcc@example.com".into(),
            ],
            "<stable-message@example.com>",
            &[],
        )
        .expect("mime");
        let text = String::from_utf8_lossy(&mime);
        assert!(text.contains("Subject:"));
        assert!(text.contains("To: to@example.com"));
        assert!(text.contains("In-Reply-To: <parent@example.com>"));
        assert!(text.contains("References: <root@example.com> <parent@example.com>"));
        assert!(text.contains("Message-ID: <stable-message@example.com>"));
        assert!(!text.contains("Bcc:"));
        assert!(text.ends_with("\r\n"));
    }

    #[test]
    fn rejects_missing_or_invalid_recipients_before_queueing() {
        let no_recipients = OutboxDraft {
            account_id: "account".into(),
            to: vec![],
            cc: vec![],
            bcc: vec![],
            subject: "Subject".into(),
            body_text: "Body".into(),
            in_reply_to: None,
            references: Vec::new(),
        };
        assert!(matches!(
            build_mime(
                &no_recipients,
                "from@example.com",
                &[],
                "<empty@example.com>",
                &[]
            ),
            Err(AppError::InvalidConfiguration(_))
        ));

        let invalid = OutboxDraft {
            to: vec!["not-an-email".into()],
            ..no_recipients
        };
        assert!(matches!(
            build_mime(
                &invalid,
                "from@example.com",
                &["not-an-email".into()],
                "<invalid@example.com>",
                &[]
            ),
            Err(AppError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn builds_multipart_mime_for_user_selected_attachments() {
        let draft = OutboxDraft {
            account_id: "account".into(),
            to: vec!["to@example.com".into()],
            cc: vec![],
            bcc: vec![],
            subject: "附件".into(),
            body_text: "请查收".into(),
            in_reply_to: None,
            references: Vec::new(),
        };
        let mime = build_mime(
            &draft,
            "from@example.com",
            &["to@example.com".into()],
            "<attachment@example.com>",
            &[DraftAttachment {
                name: "report.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: b"PDF bytes".to_vec(),
            }],
        )
        .expect("multipart MIME");
        let text = String::from_utf8_lossy(&mime);
        assert!(text.contains("multipart/mixed"));
        assert!(text.contains("report.pdf"));
        assert!(text.contains("application/pdf"));
    }

    #[test]
    fn queue_preflight_uses_the_same_recipient_split_as_storage() {
        let draft = draft_from_input(&DraftInput {
            id: None,
            account_id: "account".into(),
            to: "first@example.com, second@example.com".into(),
            cc: Some("  ".into()),
            bcc: None,
            subject: "Subject".into(),
            body_text: "Body".into(),
            in_reply_to: None,
            references: None,
        });
        assert_eq!(draft.to, vec!["first@example.com", "second@example.com"]);
        assert!(draft.cc.is_empty());
        assert!(draft.bcc.is_empty());
    }

    #[test]
    fn generated_message_id_uses_the_sender_domain() {
        let id = new_message_id("sender@example.com");
        assert!(id.starts_with('<'));
        assert!(id.ends_with("@example.com>"));
    }

    #[test]
    fn sent_copy_policy_never_assumes_client_append_is_safe() {
        let config = |host: &str| OutgoingConfig {
            protocol: "smtp".into(),
            host: host.into(),
            port: 465,
            tls_mode: "implicit".into(),
            auth_method: "password".into(),
            username: "sender@example.com".into(),
        };
        assert_eq!(
            sent_copy_policy(&config("smtp.gmail.com")),
            SentCopyPolicy::ServerManaged
        );
        assert_eq!(
            sent_copy_policy(&config("SMTP.QQ.COM.")),
            SentCopyPolicy::ServerSettingDependent
        );
        assert_eq!(
            sent_copy_policy(&config("smtp.example.com")),
            SentCopyPolicy::Unknown
        );
    }
}

use crate::app_state::AppState;
use crate::backends::outgoing::{OutgoingError, OutgoingMailBackend, SendResult};
use crate::backends::smtp::SmtpOutgoingBackend;
use crate::domain::{DraftInput, OutboxItem};
use crate::errors::AppError;
use crate::storage::database::OutboxDraft;
use lettre::message::header::ContentType;
use lettre::Message;
use tauri::{AppHandle, Emitter, Manager};

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
    state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .queue_draft(&input)
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
    let draft = match state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))
        .and_then(|database| database.outbox_draft(&outbox_id))
    {
        Ok(draft) => draft,
        Err(error) => {
            let _ = persist_delivery_error(&state, &outbox_id, &error, "failed");
            emit_outbox_changed(&app, &outbox_id, "failed");
            return;
        }
    };
    let (config, secret, envelope_from) = match outgoing_session(&state, &draft) {
        Ok(session) => session,
        Err(error) => {
            let _ = persist_delivery_error(&state, &outbox_id, &error, "failed");
            emit_outbox_changed(&app, &outbox_id, "failed");
            return;
        }
    };
    if let Err(error) = set_state(&state, &outbox_id, "sending", None, None) {
        tracing::warn!(outbox_id = %outbox_id, error = %error, "unable to mark outbox item as sending");
        return;
    }
    emit_outbox_changed(&app, &outbox_id, "sending");
    let recipients = draft
        .to
        .iter()
        .chain(draft.cc.iter())
        .chain(draft.bcc.iter())
        .cloned()
        .collect::<Vec<_>>();
    let mime = match build_plain_mime(&draft, &envelope_from, &recipients) {
        Ok(mime) => mime,
        Err(error) => {
            let _ = persist_delivery_error(&state, &outbox_id, &error, "failed");
            emit_outbox_changed(&app, &outbox_id, "failed");
            return;
        }
    };
    let result = SmtpOutgoingBackend::new(config)
        .send_mime(&secret, mime, &envelope_from, &recipients)
        .await;
    match result {
        Ok(SendResult::Sent { .. }) => {
            let _ = set_state(&state, &outbox_id, "sent", None, None);
            emit_outbox_changed(&app, &outbox_id, "sent");
        }
        Ok(SendResult::OutcomeUnknown) => {
            let error = AppError::AmbiguousSend;
            let _ = persist_delivery_error(&state, &outbox_id, &error, "outcome_unknown");
            emit_outbox_changed(&app, &outbox_id, "outcome_unknown");
        }
        Ok(SendResult::Failed) => {
            let error = AppError::ServerRejected("SMTP 发送失败".into());
            let _ = persist_delivery_error(&state, &outbox_id, &error, "failed");
            emit_outbox_changed(&app, &outbox_id, "failed");
        }
        Err(error) => {
            let app_error = map_outgoing_error(error);
            let state_name = if matches!(app_error, AppError::Network(_)) {
                "queued"
            } else {
                "failed"
            };
            let _ = persist_delivery_error(&state, &outbox_id, &app_error, state_name);
            emit_outbox_changed(&app, &outbox_id, state_name);
        }
    }
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
        let refs = database.account_secret_refs(&draft.account_id)?;
        (
            database.outgoing_config(&draft.account_id)?,
            refs.1
                .ok_or_else(|| AppError::Capability("该账号没有发件端点".into()))?,
            database.account_email(&draft.account_id)?,
        )
    };
    let secret = state
        .secret_store
        .get(&secret_ref)
        .map_err(|error| AppError::SecretStore(error.to_string()))?;
    Ok((config, secret, envelope_from))
}

fn build_plain_mime(
    draft: &OutboxDraft,
    envelope_from: &str,
    recipients: &[String],
) -> Result<Vec<u8>, AppError> {
    let mut builder =
        Message::builder()
            .from(envelope_from.parse().map_err(|error| {
                AppError::InvalidConfiguration(format!("发件人地址无效：{error}"))
            })?)
            .subject(&draft.subject)
            .header(ContentType::TEXT_PLAIN);
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
    if recipients.is_empty() {
        return Err(AppError::InvalidConfiguration("至少需要一个收件人".into()));
    }
    builder
        .body(draft.body_text.clone())
        .map(|message| {
            let mut formatted = message.formatted();
            if !formatted.ends_with(b"\r\n") {
                formatted.extend_from_slice(b"\r\n");
            }
            formatted
        })
        .map_err(|error| AppError::InvalidConfiguration(format!("无法生成 MIME：{error}")))
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
    use super::build_plain_mime;
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
        };
        let mime = build_plain_mime(
            &draft,
            "from@example.com",
            &[
                "to@example.com".into(),
                "cc@example.com".into(),
                "bcc@example.com".into(),
            ],
        )
        .expect("mime");
        let text = String::from_utf8_lossy(&mime);
        assert!(text.contains("Subject:"));
        assert!(text.contains("To: to@example.com"));
        assert!(!text.contains("Bcc:"));
        assert!(text.ends_with("\r\n"));
    }
}

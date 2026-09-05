#![allow(clippy::result_large_err)] // Flat AppErrorDto is deliberate at the Tauri IPC boundary.

use tauri::State;

use crate::app_state::AppState;
use crate::application::{account_service, compose_service, message_service, sync_service};
use crate::backends::{
    imap::ImapIncomingBackend, incoming::IncomingMailBackend, outgoing::OutgoingMailBackend,
    smtp::SmtpOutgoingBackend,
};
use crate::domain::{
    account::CreateAccountInput, Account, DraftAttachment, DraftInput, Message, SyncStatus,
};
use crate::errors::AppErrorDto;
use crate::providers::registry::{
    detect_provider as find_provider, provider_presets, ProviderPreset,
};

#[tauri::command]
pub fn get_provider_presets() -> Result<Vec<ProviderPreset>, AppErrorDto> {
    Ok(provider_presets())
}

#[tauri::command]
pub fn detect_provider(email: String) -> Result<Option<ProviderPreset>, AppErrorDto> {
    Ok(find_provider(&email))
}

#[tauri::command]
pub fn list_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<crate::domain::Account>, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .list_accounts()
        .map_err(Into::into)
}

#[tauri::command]
pub async fn create_account(
    state: State<'_, AppState>,
    input: CreateAccountInput,
) -> Result<crate::domain::Account, AppErrorDto> {
    let account = account_service::create_account(&state, input)
        .await
        .map_err(AppErrorDto::from)?;
    // The account was fully verified before persistence, so it is safe to let the backend own
    // its first sync and long-lived IDLE listener as soon as the wizard returns to the mailbox.
    state.realtime.restart_account(&account.id);
    Ok(account)
}

#[tauri::command]
pub async fn test_incoming_connection(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<crate::backends::incoming::ServerCapabilities, AppErrorDto> {
    let (config, secret_ref) = {
        let database = state.database.lock().map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?;
        (
            database.incoming_config(&account_id),
            database.account_secret_refs(&account_id),
        )
    };
    let config = config.map_err(AppErrorDto::from)?;
    let secret_ref = secret_ref.map_err(AppErrorDto::from)?.0.ok_or_else(|| {
        AppErrorDto::from(crate::errors::AppError::SecretStore(
            "missing incoming reference".into(),
        ))
    })?;
    let secret = state.secret_store.get(&secret_ref).map_err(|error| {
        AppErrorDto::from(crate::errors::AppError::SecretStore(error.to_string()))
    })?;
    ImapIncomingBackend::new(config)
        .test_connection(&secret)
        .await
        .map_err(map_incoming_error)
}

#[tauri::command]
pub async fn test_outgoing_connection(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<(), AppErrorDto> {
    let (config, secret_ref) = {
        let database = state.database.lock().map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?;
        (
            database.outgoing_config(&account_id),
            database.account_secret_refs(&account_id),
        )
    };
    let config = config.map_err(AppErrorDto::from)?;
    let secret_ref = secret_ref.map_err(AppErrorDto::from)?.1.ok_or_else(|| {
        AppErrorDto::from(crate::errors::AppError::SecretStore(
            "missing outgoing reference".into(),
        ))
    })?;
    let secret = state.secret_store.get(&secret_ref).map_err(|error| {
        AppErrorDto::from(crate::errors::AppError::SecretStore(error.to_string()))
    })?;
    SmtpOutgoingBackend::new(config)
        .test_connection(&secret)
        .await
        .map_err(map_outgoing_error)
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, account_id: String) -> Result<(), AppErrorDto> {
    remove_account_from_state(&state, &account_id)
}

fn remove_account_from_state(state: &AppState, account_id: &str) -> Result<(), AppErrorDto> {
    state.sync.cancel(account_id);
    let refs = {
        let mut database = state.database.lock().map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?;
        let refs = database
            .account_secret_refs(account_id)
            .map_err(AppErrorDto::from)?;
        database
            .delete_account(account_id)
            .map_err(AppErrorDto::from)?;
        refs
    };
    let mut references = [refs.0, refs.1].into_iter().flatten().collect::<Vec<_>>();
    references.sort_unstable();
    references.dedup();
    for reference in references {
        if let Err(error) = state.secret_store.delete(&reference) {
            tracing::warn!(
                account_id,
                error = %error,
                "account was removed but an orphaned credential could not be deleted"
            );
        }
    }
    state.realtime.wake();
    Ok(())
}

#[tauri::command]
pub fn list_mailboxes(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<Vec<crate::domain::Mailbox>, AppErrorDto> {
    let database = state.database.lock().map_err(|_| {
        AppErrorDto::from(crate::errors::AppError::Internal(
            "database lock poisoned".into(),
        ))
    })?;
    match account_id {
        Some(account_id) => database.list_mailboxes(&account_id),
        None => database.list_all_mailboxes(),
    }
    .map_err(Into::into)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMessagesInput {
    pub account_id: Option<String>,
    pub mailbox_id: Option<String>,
    pub mailbox_role: Option<String>,
    pub is_starred: Option<bool>,
    pub search: Option<String>,
    pub limit: Option<u32>,
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    input: Option<ListMessagesInput>,
) -> Result<Vec<Message>, AppErrorDto> {
    let input = input.unwrap_or(ListMessagesInput {
        account_id: None,
        mailbox_id: None,
        mailbox_role: None,
        is_starred: None,
        search: None,
        limit: None,
    });
    let messages = message_service::list_messages_in_scope(
        &state,
        input.account_id,
        input.mailbox_id,
        input.mailbox_role,
        input.is_starred,
        input.limit.unwrap_or(100),
    )
    .map_err(AppErrorDto::from)?;
    if let Some(query) = input.search.filter(|query| !query.trim().is_empty()) {
        let query = query.to_lowercase();
        Ok(messages
            .into_iter()
            .filter(|message| {
                format!(
                    "{} {} {}",
                    message.subject, message.preview, message.from.email
                )
                .to_lowercase()
                .contains(&query)
            })
            .collect())
    } else {
        Ok(messages)
    }
}

#[tauri::command]
pub fn search_messages(
    state: State<'_, AppState>,
    input: Option<ListMessagesInput>,
) -> Result<Vec<Message>, AppErrorDto> {
    let input = input.unwrap_or(ListMessagesInput {
        account_id: None,
        mailbox_id: None,
        mailbox_role: None,
        is_starred: None,
        search: Some(String::new()),
        limit: None,
    });
    let query = input.search.unwrap_or_default();
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .search_messages_in_scope(
            input.account_id.as_deref(),
            input.mailbox_id.as_deref(),
            input.mailbox_role.as_deref(),
            input.is_starred,
            &query,
            input.limit.unwrap_or(100).min(500),
        )
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_message(state: State<'_, AppState>, message_id: String) -> Result<Message, AppErrorDto> {
    message_service::get_message(&state, message_id).map_err(Into::into)
}

#[tauri::command]
pub async fn fetch_message_body(
    state: State<'_, AppState>,
    message_id: String,
    mailbox_id: String,
) -> Result<Message, AppErrorDto> {
    fetch_message_body_from_state(&state, message_id, mailbox_id).await
}

async fn fetch_message_body_from_state(
    state: &AppState,
    message_id: String,
    mailbox_id: String,
) -> Result<Message, AppErrorDto> {
    message_service::fetch_message_body(state, message_id, mailbox_id)
        .await
        .map_err(Into::into)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMutation {
    pub is_read: Option<bool>,
    pub is_starred: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInstanceRefInput {
    pub message_id: String,
    pub mailbox_id: String,
}

#[tauri::command]
pub fn mutate_message(
    state: State<'_, AppState>,
    message_id: String,
    mailbox_id: String,
    mutation: MessageMutation,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(
        &state,
        message_id,
        mailbox_id,
        mutation.is_read,
        mutation.is_starred,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn mutate_messages(
    state: State<'_, AppState>,
    messages: Vec<MessageInstanceRefInput>,
    mutation: MessageMutation,
) -> Result<serde_json::Value, AppErrorDto> {
    let message_refs = messages
        .into_iter()
        .map(|message| (message.message_id, message.mailbox_id))
        .collect();
    let count = message_service::mutate_messages(
        &state,
        message_refs,
        mutation.is_read,
        mutation.is_starred,
    )
    .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "mutated": count }))
}

#[tauri::command]
pub fn mark_read(
    state: State<'_, AppState>,
    message_id: String,
    mailbox_id: String,
    is_read: bool,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(&state, message_id, mailbox_id, Some(is_read), None)
        .map_err(Into::into)
}

#[tauri::command]
pub fn set_starred(
    state: State<'_, AppState>,
    message_id: String,
    mailbox_id: String,
    is_starred: bool,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(&state, message_id, mailbox_id, None, Some(is_starred))
        .map_err(Into::into)
}

#[tauri::command]
pub fn save_draft(
    state: State<'_, AppState>,
    input: DraftInput,
) -> Result<serde_json::Value, AppErrorDto> {
    compose_service::save_draft(&state, input).map_err(Into::into)
}

#[tauri::command]
pub fn send_draft(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    input: DraftInput,
) -> Result<serde_json::Value, AppErrorDto> {
    let outbox_id = compose_service::queue_draft_id(&state, input).map_err(AppErrorDto::from)?;
    compose_service::spawn_delivery(app, outbox_id.clone());
    Ok(serde_json::json!({ "outboxId": outbox_id, "state": "queued" }))
}

#[tauri::command]
pub fn send_draft_with_attachments(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    input: DraftInput,
    attachments: Vec<DraftAttachment>,
) -> Result<serde_json::Value, AppErrorDto> {
    let outbox_id = compose_service::queue_draft_with_attachments(&state, input, attachments)
        .map_err(AppErrorDto::from)?;
    compose_service::spawn_delivery(app, outbox_id.clone());
    Ok(serde_json::json!({ "outboxId": outbox_id, "state": "queued" }))
}

#[tauri::command]
pub fn list_outbox(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<Vec<crate::domain::OutboxItem>, AppErrorDto> {
    compose_service::list_outbox(&state, account_id).map_err(Into::into)
}

#[tauri::command]
pub fn start_sync(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
) -> Result<SyncStatus, AppErrorDto> {
    let status =
        sync_service::start_sync(&state, app, account_id.clone()).map_err(AppErrorDto::from)?;
    // A direct user refresh is an explicit retry after a terminal listener failure.
    state.realtime.restart_account(&account_id);
    Ok(status)
}

#[tauri::command]
pub fn cancel_sync(state: State<'_, AppState>, account_id: String) -> Result<(), AppErrorDto> {
    state.sync.cancel(&account_id);
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .mark_account_sync_cancelled(&account_id)
        .map_err(AppErrorDto::from)?;
    state.realtime.wake();
    Ok(())
}

#[tauri::command]
pub fn get_sync_status(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<SyncStatus, AppErrorDto> {
    Ok(state.sync.status(&account_id).unwrap_or(SyncStatus {
        account_id,
        state: "idle".into(),
        phase: None,
        processed: None,
        total: None,
        message: None,
        retryable: false,
    }))
}

#[tauri::command]
pub fn sync_all(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<SyncStatus>, AppErrorDto> {
    let accounts = state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .list_accounts()
        .map_err(AppErrorDto::from)?;
    let statuses = accounts
        .into_iter()
        .filter(is_sync_candidate)
        .map(|account| {
            let account_id = account.id;
            let status = match sync_service::start_sync(&state, app.clone(), account_id.clone()) {
                Ok(status) => status,
                Err(error) => state
                    .sync
                    .status(&account_id)
                    .unwrap_or_else(|| SyncStatus {
                        account_id: account_id.clone(),
                        state: "error".into(),
                        phase: Some("folders".into()),
                        processed: None,
                        total: None,
                        message: Some(error.to_string()),
                        retryable: error.retryable(),
                    }),
            };
            state.realtime.restart_account(&account_id);
            status
        })
        .collect();
    Ok(statuses)
}

fn is_sync_candidate(account: &Account) -> bool {
    account.enabled && account.incoming_configured && account.sync_policy != "paused"
}

#[tauri::command]
pub fn update_account(
    state: State<'_, AppState>,
    account_id: String,
    patch: serde_json::Value,
) -> Result<serde_json::Value, AppErrorDto> {
    let account = state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .update_account(&account_id, &patch)
        .map_err(AppErrorDto::from)?;
    state.realtime.restart_account(&account_id);
    Ok(serde_json::to_value(account).unwrap_or_default())
}

#[tauri::command]
pub fn get_account_status(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<crate::domain::Account, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .list_accounts()
        .map_err(AppErrorDto::from)?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| AppErrorDto::from(crate::errors::AppError::NotFound("account".into())))
}

#[tauri::command]
pub fn update_account_credentials(
    state: State<'_, AppState>,
    account_id: String,
    secret: String,
    outgoing_secret: Option<String>,
) -> Result<(), AppErrorDto> {
    crate::application::account_service::update_credentials(
        &state,
        &account_id,
        &secret,
        outgoing_secret.as_deref(),
    )
    .map_err(AppErrorDto::from)
}

#[tauri::command]
pub fn reconnect_account(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
) -> Result<SyncStatus, AppErrorDto> {
    let status =
        sync_service::start_sync(&state, app, account_id.clone()).map_err(AppErrorDto::from)?;
    state.realtime.restart_account(&account_id);
    Ok(status)
}

#[tauri::command]
pub fn refresh_mailboxes(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<crate::domain::Mailbox>, AppErrorDto> {
    list_mailboxes(state, Some(account_id))
}

#[tauri::command]
pub fn set_mailbox_sync_policy(
    state: State<'_, AppState>,
    mailbox_id: String,
    sync_enabled: bool,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .set_mailbox_sync_enabled(&mailbox_id, sync_enabled)
        .map_err(AppErrorDto::from)?;
    state.realtime.wake();
    Ok(serde_json::json!({ "mailboxId": mailbox_id, "syncEnabled": sync_enabled }))
}

#[tauri::command]
pub fn move_messages(
    state: State<'_, AppState>,
    messages: Vec<MessageInstanceRefInput>,
    mailbox_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    let message_refs = messages
        .into_iter()
        .map(|message| (message.message_id, message.mailbox_id))
        .collect();
    let count = message_service::move_messages(&state, message_refs, mailbox_id)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "moved": count }))
}

#[tauri::command]
pub fn delete_messages(
    state: State<'_, AppState>,
    messages: Vec<MessageInstanceRefInput>,
    permanent: bool,
) -> Result<serde_json::Value, AppErrorDto> {
    let message_refs = messages
        .into_iter()
        .map(|message| (message.message_id, message.mailbox_id))
        .collect();
    let count = message_service::delete_messages(&state, message_refs, permanent)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "deleted": count }))
}

#[tauri::command]
pub fn list_thread(
    state: State<'_, AppState>,
    thread_id: String,
) -> Result<Vec<Message>, AppErrorDto> {
    let messages = message_service::list_messages(&state, None, 500).map_err(AppErrorDto::from)?;
    Ok(messages
        .into_iter()
        .filter(|message| message.thread_id == thread_id)
        .collect())
}

#[tauri::command]
pub fn load_draft(
    state: State<'_, AppState>,
    draft_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .load_draft(&draft_id)
        .and_then(|draft| serde_json::to_value(draft).map_err(Into::into))
        .map_err(AppErrorDto::from)
}

#[tauri::command]
pub fn delete_draft(
    state: State<'_, AppState>,
    draft_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .delete_draft(&draft_id)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "draftId": draft_id, "deleted": true }))
}

#[tauri::command]
pub fn retry_outbox_item(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    outbox_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .set_outbox_state(&outbox_id, "queued", None, None)
        .map_err(AppErrorDto::from)?;
    compose_service::spawn_delivery(app, outbox_id.clone());
    Ok(serde_json::json!({ "outboxId": outbox_id, "state": "queued" }))
}

#[tauri::command]
pub fn cancel_outbox_item(
    state: State<'_, AppState>,
    outbox_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .set_outbox_state(&outbox_id, "cancelled", None, None)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "outboxId": outbox_id, "state": "cancelled" }))
}

#[tauri::command]
pub fn download_attachment(
    state: State<'_, AppState>,
    attachment_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    let (attachment, bytes) = state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .attachment_payload(&attachment_id)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "attachment": attachment, "bytes": bytes }))
}

#[tauri::command]
pub fn cancel_attachment_download(
    _state: State<'_, AppState>,
    _attachment_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    Err(unsupported("cancel_attachment_download"))
}

#[tauri::command]
pub fn save_attachment_as(
    _state: State<'_, AppState>,
    _attachment_id: String,
    _path: String,
) -> Result<serde_json::Value, AppErrorDto> {
    Err(unsupported("save_attachment_as"))
}

#[tauri::command]
pub fn open_attachment(
    _state: State<'_, AppState>,
    _attachment_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    Err(unsupported("open_attachment"))
}

#[tauri::command]
pub fn reveal_attachment(
    _state: State<'_, AppState>,
    _attachment_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    Err(unsupported("reveal_attachment"))
}

#[tauri::command]
pub fn get_search_suggestions(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<String>, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .search_suggestions(&query, 20)
        .map_err(AppErrorDto::from)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .get_settings()
        .map_err(AppErrorDto::from)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: serde_json::Value,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .update_settings(&settings)
        .map_err(AppErrorDto::from)
}

#[tauri::command]
pub fn clear_cache(state: State<'_, AppState>) -> Result<serde_json::Value, AppErrorDto> {
    let deleted = state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .clear_cache()
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "deletedMessages": deleted }))
}

#[tauri::command]
pub fn export_diagnostics(state: State<'_, AppState>) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .diagnostics()
        .map_err(AppErrorDto::from)
}

fn unsupported(command: &str) -> AppErrorDto {
    AppErrorDto::from(crate::errors::AppError::Capability(format!(
        "{command} 尚未连接到协议 worker"
    )))
}

fn map_incoming_error(error: crate::backends::incoming::IncomingError) -> AppErrorDto {
    use crate::backends::incoming::IncomingError;
    let app_error = match error {
        IncomingError::Authentication => crate::errors::AppError::Authentication,
        IncomingError::Unsupported(message) => crate::errors::AppError::Capability(message),
        IncomingError::Tls(message) | IncomingError::Network(message) => {
            crate::errors::AppError::Network(message)
        }
        IncomingError::Protocol(message) => crate::errors::AppError::Protocol(message),
    };
    AppErrorDto::from(app_error)
}

fn map_outgoing_error(error: crate::backends::outgoing::OutgoingError) -> AppErrorDto {
    use crate::backends::outgoing::OutgoingError;
    let app_error = match error {
        OutgoingError::Authentication => crate::errors::AppError::Authentication,
        OutgoingError::Unsupported(message) => crate::errors::AppError::Capability(message),
        OutgoingError::AmbiguousSend => crate::errors::AppError::AmbiguousSend,
        OutgoingError::Tls(message) | OutgoingError::Network(message) => {
            crate::errors::AppError::Network(message)
        }
        OutgoingError::Rejected(message) => crate::errors::AppError::ServerRejected(message),
    };
    AppErrorDto::from(app_error)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{fetch_message_body_from_state, is_sync_candidate, remove_account_from_state};
    use crate::app_state::{AppState, RealtimeSyncCoordinator, SyncCoordinator};
    use crate::auth::secret_store::{SecretStore, SecretStoreError};
    use crate::domain::account::CreateAccountInput;
    use crate::domain::Account;
    use crate::providers::registry::provider_presets;
    use crate::storage::database::Database;

    struct FailingDeleteSecretStore;

    impl SecretStore for FailingDeleteSecretStore {
        fn set(&self, _reference: &str, _secret: &str) -> Result<(), SecretStoreError> {
            Ok(())
        }

        fn get(&self, _reference: &str) -> Result<String, SecretStoreError> {
            Err(SecretStoreError::NotFound)
        }

        fn delete(&self, _reference: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::OperationFailed)
        }
    }

    fn account(enabled: bool, incoming_configured: bool, sync_policy: &str) -> Account {
        Account {
            id: "account".into(),
            provider_id: "qq".into(),
            email: "test@qq.com".into(),
            display_name: "Test".into(),
            enabled,
            sync_policy: sync_policy.into(),
            incoming_configured,
            outgoing_configured: true,
            sync_status: "idle".into(),
            last_synced_at: None,
        }
    }

    #[test]
    fn sync_all_only_targets_enabled_incoming_accounts_that_are_not_paused() {
        assert!(is_sync_candidate(&account(true, true, "automatic")));
        assert!(is_sync_candidate(&account(true, true, "manual")));
        assert!(!is_sync_candidate(&account(false, true, "automatic")));
        assert!(!is_sync_candidate(&account(true, false, "automatic")));
        assert!(!is_sync_candidate(&account(true, true, "paused")));
    }

    #[tokio::test]
    async fn fetch_body_command_delegates_to_async_service_and_preserves_error_code() {
        let state = AppState {
            database: Mutex::new(Database::open_in_memory().expect("database")),
            secret_store: Arc::new(FailingDeleteSecretStore),
            sync: Arc::new(SyncCoordinator::new()),
            realtime: Arc::new(RealtimeSyncCoordinator::new()),
        };
        let error = fetch_message_body_from_state(
            &state,
            "missing-message".into(),
            "missing-mailbox".into(),
        )
        .await
        .expect_err("missing message must fail");
        assert_eq!(error.code, "not_found");
    }

    #[test]
    fn removing_account_keeps_database_consistent_when_secret_cleanup_fails() {
        let mut database = Database::open_in_memory().expect("database");
        let preset = provider_presets()
            .into_iter()
            .find(|preset| preset.id == "qq")
            .expect("QQ preset");
        let account = database
            .create_account(
                &CreateAccountInput {
                    email: "remove@qq.com".into(),
                    display_name: "Remove".into(),
                    provider_id: "qq".into(),
                    secret: "not persisted".into(),
                    incoming_secret: None,
                    outgoing_secret: None,
                    incoming: None,
                    outgoing: None,
                },
                &preset,
                "account/remove/incoming",
                "account/remove/outgoing",
                true,
                true,
            )
            .expect("account");
        let state = AppState {
            database: Mutex::new(database),
            secret_store: Arc::new(FailingDeleteSecretStore),
            sync: Arc::new(SyncCoordinator::new()),
            realtime: Arc::new(RealtimeSyncCoordinator::new()),
        };

        remove_account_from_state(&state, &account.id)
            .expect("credential cleanup must not resurrect the account");
        assert!(state
            .database
            .lock()
            .expect("database lock")
            .list_accounts()
            .expect("accounts")
            .is_empty());
    }
}

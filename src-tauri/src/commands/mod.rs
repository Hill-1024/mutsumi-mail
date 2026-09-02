#![allow(clippy::result_large_err)] // Flat AppErrorDto is deliberate at the Tauri IPC boundary.

use tauri::State;

use crate::app_state::AppState;
use crate::application::{account_service, compose_service, message_service, sync_service};
use crate::backends::{
    imap::ImapIncomingBackend, incoming::IncomingMailBackend, outgoing::OutgoingMailBackend,
    smtp::SmtpOutgoingBackend,
};
use crate::domain::{account::CreateAccountInput, DraftInput, Message, SyncStatus};
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
pub fn create_account(
    state: State<'_, AppState>,
    input: CreateAccountInput,
) -> Result<crate::domain::Account, AppErrorDto> {
    account_service::create_account(&state, input).map_err(Into::into)
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
    state.sync.cancel(&account_id);
    let refs = state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .account_secret_refs(&account_id)
        .map_err(AppErrorDto::from)?;
    let mut secret_delete_error = None;
    for reference in [refs.0, refs.1].into_iter().flatten() {
        if let Err(error) = state.secret_store.delete(&reference) {
            secret_delete_error = Some(error.to_string());
        }
    }
    if let Some(error) = secret_delete_error {
        return Err(AppErrorDto::from(crate::errors::AppError::SecretStore(
            format!("无法删除账户凭据：{error}"),
        )));
    }
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .delete_account(&account_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_mailboxes(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<crate::domain::Mailbox>, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .list_mailboxes(&account_id)
        .map_err(Into::into)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMessagesInput {
    pub mailbox_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    input: Option<ListMessagesInput>,
) -> Result<Vec<Message>, AppErrorDto> {
    let input = input.unwrap_or(ListMessagesInput {
        mailbox_id: None,
        search: None,
        limit: None,
    });
    let messages =
        message_service::list_messages(&state, input.mailbox_id, input.limit.unwrap_or(100))
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
        mailbox_id: None,
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
        .search_messages(
            input.mailbox_id.as_deref(),
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
pub fn fetch_message_body(
    state: State<'_, AppState>,
    message_id: String,
) -> Result<Message, AppErrorDto> {
    get_message(state, message_id)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMutation {
    pub is_read: Option<bool>,
    pub is_starred: Option<bool>,
}

#[tauri::command]
pub fn mutate_message(
    state: State<'_, AppState>,
    message_id: String,
    mutation: MessageMutation,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(&state, message_id, mutation.is_read, mutation.is_starred)
        .map_err(Into::into)
}

#[tauri::command]
pub fn mark_read(
    state: State<'_, AppState>,
    message_id: String,
    is_read: bool,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(&state, message_id, Some(is_read), None).map_err(Into::into)
}

#[tauri::command]
pub fn set_starred(
    state: State<'_, AppState>,
    message_id: String,
    is_starred: bool,
) -> Result<Message, AppErrorDto> {
    message_service::mutate_message(&state, message_id, None, Some(is_starred)).map_err(Into::into)
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
    sync_service::start_sync(&state, app, account_id).map_err(Into::into)
}

#[tauri::command]
pub fn cancel_sync(state: State<'_, AppState>, account_id: String) -> Result<(), AppErrorDto> {
    state.sync.cancel(&account_id);
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
    accounts
        .into_iter()
        .map(|account| {
            sync_service::start_sync(&state, app.clone(), account.id).map_err(Into::into)
        })
        .collect()
}

#[tauri::command]
pub fn update_account(
    state: State<'_, AppState>,
    account_id: String,
    patch: serde_json::Value,
) -> Result<serde_json::Value, AppErrorDto> {
    state
        .database
        .lock()
        .map_err(|_| {
            AppErrorDto::from(crate::errors::AppError::Internal(
                "database lock poisoned".into(),
            ))
        })?
        .update_account(&account_id, &patch)
        .map(|account| serde_json::to_value(account).unwrap_or_default())
        .map_err(AppErrorDto::from)
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
pub fn reconnect_account(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
) -> Result<SyncStatus, AppErrorDto> {
    sync_service::start_sync(&state, app, account_id).map_err(Into::into)
}

#[tauri::command]
pub fn refresh_mailboxes(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<crate::domain::Mailbox>, AppErrorDto> {
    list_mailboxes(state, account_id)
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
    Ok(serde_json::json!({ "mailboxId": mailbox_id, "syncEnabled": sync_enabled }))
}

#[tauri::command]
pub fn move_messages(
    state: State<'_, AppState>,
    message_ids: Vec<String>,
    mailbox_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    let count = message_service::move_messages(&state, message_ids, mailbox_id)
        .map_err(AppErrorDto::from)?;
    Ok(serde_json::json!({ "moved": count }))
}

#[tauri::command]
pub fn delete_messages(
    state: State<'_, AppState>,
    message_ids: Vec<String>,
    permanent: bool,
) -> Result<serde_json::Value, AppErrorDto> {
    let count = message_service::delete_messages(&state, message_ids, permanent)
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
    _state: State<'_, AppState>,
    _attachment_id: String,
) -> Result<serde_json::Value, AppErrorDto> {
    Err(unsupported("download_attachment"))
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

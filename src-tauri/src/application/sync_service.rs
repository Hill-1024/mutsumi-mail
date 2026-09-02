use crate::backends::imap::ImapIncomingBackend;
use crate::backends::incoming::{IncomingError, IncomingMailBackend};
use crate::errors::AppError;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;
use crate::domain::SyncStatus;

pub fn start_sync(
    state: &AppState,
    app: AppHandle,
    account_id: String,
) -> Result<SyncStatus, AppError> {
    let token = state.sync.start(&account_id);
    let (config, secret) = match load_incoming_session(state, &account_id) {
        Ok(session) => session,
        Err(error) => {
            state.sync.cancel(&account_id);
            return Err(error);
        }
    };
    state.sync.set_status(SyncStatus {
        account_id: account_id.clone(),
        state: "syncing".into(),
        phase: Some("metadata".into()),
        processed: Some(0),
        total: None,
        message: Some("正在连接收件服务".into()),
        retryable: true,
    });
    let account_for_task = account_id.clone();
    tauri::async_runtime::spawn(async move {
        if token.is_cancelled() {
            return;
        }
        let connecting = SyncStatus {
            account_id: account_for_task.clone(),
            state: "syncing".into(),
            phase: Some("folders".into()),
            processed: Some(0),
            total: None,
            message: Some("正在建立 IMAP TLS 连接并读取 CAPABILITY".into()),
            retryable: true,
        };
        state_sync_status(&app, connecting.clone());
        let _ = app.emit("sync-progress", connecting);
        let result = ImapIncomingBackend::new(config)
            .test_connection(&secret)
            .await;
        if token.is_cancelled() {
            return;
        }
        match result {
            Ok(capabilities) => {
                let status = SyncStatus {
                    account_id: account_for_task,
                    state: "partial".into(),
                    phase: Some("metadata".into()),
                    processed: Some(0),
                    total: None,
                    message: Some(format!(
                        "连接成功（{} 项能力）；增量 FETCH worker 尚未启用",
                        capabilities.capabilities.enabled_count()
                    )),
                    retryable: false,
                };
                state_sync_status(&app, status.clone());
                let _ = app.emit("sync-progress", status);
            }
            Err(error) => {
                let retryable = !matches!(
                    error,
                    IncomingError::Authentication | IncomingError::Unsupported(_)
                );
                let status = SyncStatus {
                    account_id: account_for_task,
                    state: "error".into(),
                    phase: Some("folders".into()),
                    processed: None,
                    total: None,
                    message: Some(error.to_string()),
                    retryable,
                };
                state_sync_status(&app, status.clone());
                let _ = app.emit("sync-progress", status);
            }
        }
    });
    Ok(SyncStatus {
        account_id,
        state: "syncing".into(),
        phase: Some("metadata".into()),
        processed: Some(0),
        total: None,
        message: Some("正在连接收件服务".into()),
        retryable: true,
    })
}

fn state_sync_status(app: &tauri::AppHandle, status: SyncStatus) {
    if let Some(state) = app.try_state::<AppState>() {
        state.sync.set_status(status);
    }
}

fn load_incoming_session(
    state: &AppState,
    account_id: &str,
) -> Result<(crate::backends::incoming::IncomingConfig, String), AppError> {
    let (config, secret_ref) = {
        let database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        (
            database.incoming_config(account_id)?,
            database.account_secret_refs(account_id)?.0,
        )
    };
    let reference = secret_ref.ok_or_else(|| AppError::Capability("该账号没有收件端点".into()))?;
    let secret = state
        .secret_store
        .get(&reference)
        .map_err(|error| AppError::SecretStore(error.to_string()))?;
    Ok((config, secret))
}

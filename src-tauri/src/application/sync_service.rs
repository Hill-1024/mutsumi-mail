use std::collections::HashMap;

use mail_parser::MessageParser;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::app_state::AppState;
use crate::backends::imap::{ImapIncomingBackend, MAX_FETCH_MESSAGES};
use crate::backends::incoming::{
    IncomingError, IncomingMailBackend, IncomingMailboxSnapshot, IncomingMessage,
    RemoteMessageOperation,
};
use crate::domain::{Address, SyncStatus};
use crate::errors::AppError;
use crate::storage::database::{
    ImapSnapshotMetadata, ImapSyncWindow, PendingImapOperation, SyncedMailboxInput,
    SyncedMessageInput,
};

/// History is deliberately bounded per mailbox so one very large archive cannot starve the
/// account's other folders. The persisted oldest UID makes the next sync resume where this one
/// stopped instead of permanently limiting the mailbox to the newest page.
const MAX_BACKFILL_PAGES_PER_MAILBOX: usize = 8;
const MAX_FORWARD_PAGES_PER_MAILBOX: usize = 8;
const MAX_PENDING_OPERATIONS_PER_SYNC: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
struct MailboxFetchPlan {
    remote_id: String,
    expected_uid_validity: Option<u32>,
    since_uid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FetchedSnapshot {
    snapshot: IncomingMailboxSnapshot,
    complete_mailbox: bool,
    history_limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncReport {
    mailbox_count: usize,
    inserted: usize,
    updated: usize,
    applied_operations: usize,
    operation_budget_exhausted: bool,
    forward_limited_mailboxes: usize,
    history_limited_mailboxes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct BackfillReport {
    inserted: usize,
    updated: usize,
    history_remaining: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PendingFlushReport {
    completed: usize,
    budget_exhausted: bool,
}

struct BackfillRequest<'a> {
    app: &'a AppHandle,
    account_id: &'a str,
    secret: &'a str,
    mailbox_remote_id: &'a str,
    expected_uid_validity: Option<u32>,
    remote_total_count: u32,
    token: &'a CancellationToken,
}

pub fn start_sync(
    state: &AppState,
    app: AppHandle,
    account_id: String,
) -> Result<SyncStatus, AppError> {
    let token = state.sync.start(&account_id);
    start_sync_with_token(state, app, account_id, token)
}

/// Starts a background refresh only when this account is idle. This is used for incidental
/// refreshes (for example, reconciling a server-side Sent copy after SMTP succeeds) so they never
/// cancel a user-initiated sync that is already in progress.
pub fn start_sync_if_idle(
    state: &AppState,
    app: AppHandle,
    account_id: String,
) -> Result<Option<SyncStatus>, AppError> {
    let Some(token) = state.sync.try_start(&account_id) else {
        return Ok(None);
    };
    start_sync_with_token(state, app, account_id, token).map(Some)
}

fn start_sync_with_token(
    state: &AppState,
    app: AppHandle,
    account_id: String,
    token: CancellationToken,
) -> Result<SyncStatus, AppError> {
    let (config, secret) = match load_incoming_session(state, &account_id) {
        Ok(session) => session,
        Err(error) => {
            record_start_error(
                state,
                &app,
                &account_id,
                &token,
                error.to_string(),
                error.retryable(),
            );
            return Err(error);
        }
    };
    let initial_status = SyncStatus {
        account_id: account_id.clone(),
        state: "syncing".into(),
        phase: Some("folders".into()),
        processed: Some(0),
        total: None,
        message: Some("正在连接收件服务".into()),
        retryable: true,
    };
    let started = state.sync.with_current(&account_id, &token, || {
        let result = match state.database.lock() {
            Ok(mut database) => database.mark_account_sync_started(&account_id),
            Err(_) => Err(AppError::Internal("database lock poisoned".into())),
        };
        if result.is_ok() {
            state.sync.set_status(initial_status.clone());
            let _ = app.emit("sync-progress", initial_status.clone());
        }
        result
    });
    match started {
        None => return Err(AppError::Cancelled),
        Some(Ok(())) => {}
        Some(Err(error)) => {
            record_start_error(
                state,
                &app,
                &account_id,
                &token,
                error.to_string(),
                error.retryable(),
            );
            return Err(error);
        }
    }

    let account_for_task = account_id.clone();
    tauri::async_runtime::spawn(async move {
        let backend = ImapIncomingBackend::new(config);
        match synchronize_account(&app, &account_for_task, &backend, &secret, &token).await {
            Ok(report) => {
                let completed_account = account_for_task.clone();
                finish_current_sync(&app, &account_for_task, &token, move |state| {
                    match state.database.lock() {
                        Ok(mut database) => {
                            match database.mark_account_sync_completed(&completed_account) {
                                Ok(()) => successful_sync_status(completed_account, report),
                                Err(error) => sync_error_status(completed_account, error),
                            }
                        }
                        Err(_) => sync_error_status(
                            completed_account,
                            AppError::Internal("database lock poisoned".into()),
                        ),
                    }
                });
            }
            Err(AppError::Cancelled) => {}
            Err(error) => {
                let failed_account = account_for_task.clone();
                finish_current_sync(&app, &account_for_task, &token, move |state| {
                    if let Ok(mut database) = state.database.lock() {
                        let _ =
                            database.mark_account_sync_failed(&failed_account, &error.to_string());
                    }
                    sync_error_status(failed_account, error)
                });
            }
        }
    });
    Ok(initial_status)
}

async fn synchronize_account<B: IncomingMailBackend + ?Sized>(
    app: &AppHandle,
    account_id: &str,
    backend: &B,
    secret: &str,
    token: &CancellationToken,
) -> Result<SyncReport, AppError> {
    ensure_not_cancelled(token)?;
    publish_status(
        app,
        token,
        SyncStatus {
            account_id: account_id.to_owned(),
            state: "syncing".into(),
            phase: Some("folders".into()),
            processed: Some(0),
            total: None,
            message: Some("正在读取远端文件夹".into()),
            retryable: true,
        },
    )?;

    // LIST itself performs a complete TLS login. A separate connection probe here would make
    // every refresh authenticate twice while proving less than the operation we actually need.
    let remote_mailboxes = backend
        .list_remote_mailboxes(secret)
        .await
        .map_err(incoming_error_to_app)?;
    ensure_not_cancelled(token)?;

    let synced_mailboxes = remote_mailboxes
        .iter()
        .map(|mailbox| SyncedMailboxInput {
            remote_id: mailbox.remote_id.clone(),
            display_name: mailbox.display_name.clone(),
            delimiter: mailbox.delimiter.clone(),
            special_role: mailbox.special_role.clone(),
            selectable: mailbox.selectable,
        })
        .collect::<Vec<_>>();

    let local_by_remote = with_current_sync_state(app, account_id, token, |state| {
        let mut database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        let mailboxes = database
            .upsert_remote_mailboxes(account_id, &synced_mailboxes)?
            .into_iter()
            .map(|mailbox| (mailbox.remote_id.clone(), mailbox))
            .collect::<HashMap<_, _>>();
        Ok(mailboxes)
    })?;

    // Local mutations are optimistic. Push them before reading message snapshots so a stale
    // server view cannot immediately undo a flag, move, trash, or permanent-delete action.
    let pending_flush = flush_pending_operations(app, account_id, backend, secret, token).await?;

    let plans = with_current_sync_state(app, account_id, token, |state| {
        let database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        remote_mailboxes
            .iter()
            .filter(|remote| {
                remote.selectable
                    && local_by_remote
                        .get(&remote.remote_id)
                        .is_some_and(|local| local.sync_enabled)
            })
            .map(|remote| {
                let cursor = database.imap_sync_cursor(account_id, &remote.remote_id)?;
                Ok(MailboxFetchPlan {
                    remote_id: remote.remote_id.clone(),
                    expected_uid_validity: cursor.map(|(uid_validity, _)| uid_validity),
                    since_uid: cursor
                        .map(|(_, last_uid)| last_uid)
                        .filter(|last_uid| *last_uid > 0),
                })
            })
            .collect::<Result<Vec<_>, AppError>>()
    })?;

    let mut report = SyncReport {
        mailbox_count: plans.len(),
        inserted: 0,
        updated: 0,
        applied_operations: pending_flush.completed,
        operation_budget_exhausted: pending_flush.budget_exhausted,
        forward_limited_mailboxes: 0,
        history_limited_mailboxes: 0,
    };
    for (index, plan) in plans.iter().enumerate() {
        ensure_not_cancelled(token)?;
        publish_status(
            app,
            token,
            SyncStatus {
                account_id: account_id.to_owned(),
                state: "syncing".into(),
                phase: Some("messages".into()),
                processed: Some(count_as_i64(index)),
                total: Some(count_as_i64(plans.len())),
                message: Some(format!("正在同步 {}", plan.remote_id)),
                retryable: true,
            },
        )?;

        let mut next_plan = plan.clone();
        let mut forward_pages = 0;
        let (mut remote_total_count, mut remote_uid_validity, forward_remaining, coverage_complete) = loop {
            let fetched = fetch_snapshot(backend, secret, &next_plan, token).await?;
            forward_pages += 1;
            ensure_not_cancelled(token)?;
            let batch_size = fetched.snapshot.messages.len();
            let batch_last_uid = fetched
                .snapshot
                .messages
                .iter()
                .map(|message| message.uid)
                .max();
            let messages = fetched
                .snapshot
                .messages
                .iter()
                .map(map_incoming_message)
                .collect::<Vec<_>>();
            let applied = with_current_sync_state(app, account_id, token, |state| {
                let mut database = state
                    .database
                    .lock()
                    .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
                database.apply_imap_snapshot(
                    account_id,
                    &fetched.snapshot.remote_id,
                    ImapSnapshotMetadata {
                        uid_validity: fetched.snapshot.uid_validity,
                        total_count: fetched.snapshot.total_count,
                        unread_count: fetched.snapshot.unread_count,
                        complete_mailbox: fetched.complete_mailbox,
                    },
                    &messages,
                )
            })?;
            report.inserted += applied.inserted;
            report.updated += applied.updated;

            let Some(follow_up) =
                next_incremental_plan(&next_plan, &fetched, batch_size, batch_last_uid)
            else {
                break (
                    fetched.snapshot.total_count,
                    fetched.snapshot.uid_validity,
                    false,
                    fetched.complete_mailbox,
                );
            };
            if forward_pages >= MAX_FORWARD_PAGES_PER_MAILBOX {
                break (
                    fetched.snapshot.total_count,
                    fetched.snapshot.uid_validity,
                    true,
                    fetched.complete_mailbox,
                );
            }
            next_plan = follow_up;
        };
        if forward_remaining {
            report.forward_limited_mailboxes += 1;
        }

        if !coverage_complete {
            publish_status(
                app,
                token,
                SyncStatus {
                    account_id: account_id.to_owned(),
                    state: "syncing".into(),
                    phase: Some("reconcile".into()),
                    processed: Some(count_as_i64(index)),
                    total: Some(count_as_i64(plans.len())),
                    message: Some(format!("正在核对 {} 的远端状态", plan.remote_id)),
                    retryable: true,
                },
            )?;
            let mailbox_index = backend
                .fetch_remote_mailbox_index(secret, &plan.remote_id)
                .await
                .map_err(incoming_error_to_app)?;
            ensure_not_cancelled(token)?;
            if mailbox_index.remote_id != plan.remote_id
                || mailbox_index.uid_validity != remote_uid_validity
            {
                return Err(AppError::Protocol(format!(
                    "IMAP mailbox identity changed while reconciling {}",
                    plan.remote_id
                )));
            }
            let reconciled = with_current_sync_state(app, account_id, token, |state| {
                let mut database = state
                    .database
                    .lock()
                    .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
                database.reconcile_imap_mailbox_index(account_id, &mailbox_index)
            })?;
            report.updated += reconciled.updated_flags;
            remote_total_count = mailbox_index.total_count;
            remote_uid_validity = mailbox_index.uid_validity;
        }

        let backfill = if forward_remaining {
            BackfillReport::default()
        } else {
            backfill_mailbox_history(
                backend,
                BackfillRequest {
                    app,
                    account_id,
                    secret,
                    mailbox_remote_id: &plan.remote_id,
                    expected_uid_validity: remote_uid_validity,
                    remote_total_count,
                    token,
                },
            )
            .await?
        };
        report.inserted += backfill.inserted;
        report.updated += backfill.updated;
        if backfill.history_remaining {
            report.history_limited_mailboxes += 1;
        }
    }

    ensure_not_cancelled(token)?;
    Ok(report)
}

async fn flush_pending_operations<B: IncomingMailBackend + ?Sized>(
    app: &AppHandle,
    account_id: &str,
    backend: &B,
    secret: &str,
    token: &CancellationToken,
) -> Result<PendingFlushReport, AppError> {
    let mut completed = 0;
    loop {
        if completed >= MAX_PENDING_OPERATIONS_PER_SYNC {
            return Ok(PendingFlushReport {
                completed,
                budget_exhausted: true,
            });
        }
        ensure_not_cancelled(token)?;
        let operation = with_current_sync_state(app, account_id, token, |state| {
            let mut database = state
                .database
                .lock()
                .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
            Ok(database
                .claim_pending_imap_operations(account_id, 1)?
                .into_iter()
                .next())
        })?;
        let Some(operation) = operation else {
            return Ok(PendingFlushReport {
                completed,
                budget_exhausted: false,
            });
        };

        if token.is_cancelled() {
            fail_pending_operation(app, &operation.id, "cancelled", true)?;
            return Err(AppError::Cancelled);
        }
        publish_status(
            app,
            token,
            SyncStatus {
                account_id: account_id.to_owned(),
                state: "syncing".into(),
                phase: Some("operations".into()),
                processed: Some(count_as_i64(completed)),
                total: None,
                message: Some("正在同步本地邮件操作".into()),
                retryable: true,
            },
        )?;

        let remote_operation = match pending_operation_to_remote(&operation, account_id) {
            Ok(operation) => operation,
            Err(error) => {
                fail_pending_operation(app, &operation.id, error.code(), false)?;
                return Err(error);
            }
        };
        match backend
            .apply_remote_operation(secret, &remote_operation)
            .await
        {
            Ok(()) => {
                // A tagged remote success wins over a concurrent local cancellation. Completing
                // the durable item prevents a safe, already-applied MOVE/DELETE from being
                // retried only because the cancel signal arrived during network I/O.
                complete_pending_operation(app, &operation.id)?;
                completed += 1;
                ensure_not_cancelled(token)?;
            }
            Err(incoming_error) => {
                let error = incoming_error_to_app(incoming_error);
                fail_pending_operation(app, &operation.id, error.code(), error.retryable())?;
                if token.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                return Err(error);
            }
        }
    }
}

fn pending_operation_to_remote(
    operation: &PendingImapOperation,
    expected_account_id: &str,
) -> Result<RemoteMessageOperation, AppError> {
    if operation.account_id != expected_account_id {
        return Err(AppError::InvalidConfiguration(
            "待同步操作不属于当前账户".into(),
        ));
    }
    match operation.operation_type.as_str() {
        "set_flags" => Ok(RemoteMessageOperation::SetFlags {
            mailbox_remote_id: operation.source_mailbox_remote_id.clone(),
            uid: operation.uid,
            expected_uid_validity: operation.uid_validity,
            is_read: optional_boolean(&operation.payload_json, "is_read")?,
            is_starred: optional_boolean(&operation.payload_json, "is_starred")?,
        }),
        "move" | "trash" => {
            let target_mailbox_remote_id = operation
                .target_mailbox_remote_id
                .as_deref()
                .filter(|target| !target.trim().is_empty())
                .ok_or_else(|| AppError::InvalidConfiguration("移动操作缺少目标文件夹".into()))?;
            if target_mailbox_remote_id == operation.source_mailbox_remote_id {
                return Err(AppError::InvalidConfiguration(
                    "源文件夹与目标文件夹相同".into(),
                ));
            }
            Ok(RemoteMessageOperation::Move {
                source_mailbox_remote_id: operation.source_mailbox_remote_id.clone(),
                target_mailbox_remote_id: target_mailbox_remote_id.to_owned(),
                uid: operation.uid,
                expected_uid_validity: operation.uid_validity,
            })
        }
        "permanent_delete" => Ok(RemoteMessageOperation::DeletePermanently {
            mailbox_remote_id: operation.source_mailbox_remote_id.clone(),
            uid: operation.uid,
            expected_uid_validity: operation.uid_validity,
        }),
        unsupported => Err(AppError::InvalidConfiguration(format!(
            "未知的待同步 IMAP 操作：{unsupported}"
        ))),
    }
}

fn optional_boolean(payload: &serde_json::Value, field: &str) -> Result<Option<bool>, AppError> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(AppError::InvalidConfiguration(format!(
            "待同步操作字段 {field} 必须是布尔值"
        ))),
    }
}

fn complete_pending_operation(app: &AppHandle, operation_id: &str) -> Result<(), AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
    database.complete_pending_operation(operation_id)
}

fn fail_pending_operation(
    app: &AppHandle,
    operation_id: &str,
    error_code: &str,
    retryable: bool,
) -> Result<(), AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    let mut database = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
    database.fail_pending_operation(operation_id, error_code, retryable)
}

async fn backfill_mailbox_history<B: IncomingMailBackend + ?Sized>(
    backend: &B,
    request: BackfillRequest<'_>,
) -> Result<BackfillReport, AppError> {
    let BackfillRequest {
        app,
        account_id,
        secret,
        mailbox_remote_id,
        expected_uid_validity,
        mut remote_total_count,
        token,
    } = request;
    let mut report = BackfillReport::default();
    for page_index in 0..MAX_BACKFILL_PAGES_PER_MAILBOX {
        ensure_not_cancelled(token)?;
        let Some(window) = load_imap_sync_window(app, account_id, mailbox_remote_id, token)? else {
            // Without UIDVALIDITY there is no safe identity against which an older UID page can
            // be committed. Keep the limitation visible and try again on a later sync.
            report.history_remaining = remote_total_count > MAX_FETCH_MESSAGES;
            return Ok(report);
        };
        if expected_uid_validity != Some(window.uid_validity) {
            return Err(AppError::Protocol(format!(
                "IMAP UIDVALIDITY changed while backfilling {mailbox_remote_id}"
            )));
        }
        let Some(before_uid) = next_backfill_before_uid(window, remote_total_count) else {
            report.history_remaining = false;
            return Ok(report);
        };

        publish_status(
            app,
            token,
            SyncStatus {
                account_id: account_id.to_owned(),
                state: "syncing".into(),
                phase: Some("history".into()),
                processed: Some(i64::from(window.instance_count)),
                total: Some(i64::from(remote_total_count)),
                message: Some(format!(
                    "正在回填 {} 的较早邮件（第 {} 页）",
                    mailbox_remote_id,
                    page_index + 1
                )),
                retryable: true,
            },
        )?;

        let snapshot = fetch_backfill_snapshot(
            backend,
            secret,
            mailbox_remote_id,
            before_uid,
            window.uid_validity,
            token,
        )
        .await?;
        let batch_size = snapshot.messages.len();
        let mapped = snapshot
            .messages
            .iter()
            .map(map_incoming_message)
            .collect::<Vec<_>>();
        remote_total_count = snapshot.total_count;
        let applied = with_current_sync_state(app, account_id, token, |state| {
            let mut database = state
                .database
                .lock()
                .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
            database.apply_imap_snapshot(
                account_id,
                &snapshot.remote_id,
                ImapSnapshotMetadata {
                    uid_validity: snapshot.uid_validity,
                    total_count: snapshot.total_count,
                    unread_count: snapshot.unread_count,
                    complete_mailbox: false,
                },
                &mapped,
            )
        })?;
        report.inserted += applied.inserted;
        report.updated += applied.updated;

        // SEARCH returns every UID below `before_uid` and the backend keeps the newest bounded
        // suffix. A short page therefore proves that no still-older page exists. Concurrent new
        // arrivals are handled by the forward cursor on the next sync.
        if batch_size < usize::try_from(MAX_FETCH_MESSAGES).unwrap_or(usize::MAX) {
            report.history_remaining = false;
            return Ok(report);
        }
    }

    report.history_remaining = load_imap_sync_window(app, account_id, mailbox_remote_id, token)?
        .and_then(|window| next_backfill_before_uid(window, remote_total_count))
        .is_some();
    Ok(report)
}

fn load_imap_sync_window(
    app: &AppHandle,
    account_id: &str,
    mailbox_remote_id: &str,
    token: &CancellationToken,
) -> Result<Option<ImapSyncWindow>, AppError> {
    with_current_sync_state(app, account_id, token, |state| {
        let database = state
            .database
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
        database.imap_sync_window(account_id, mailbox_remote_id)
    })
}

fn next_backfill_before_uid(window: ImapSyncWindow, remote_total_count: u32) -> Option<u32> {
    (window.instance_count < remote_total_count)
        .then_some(window.oldest_uid)
        .flatten()
        .filter(|oldest_uid| *oldest_uid > 1)
}

async fn fetch_backfill_snapshot<B: IncomingMailBackend + ?Sized>(
    backend: &B,
    secret: &str,
    mailbox_remote_id: &str,
    before_uid: u32,
    expected_uid_validity: u32,
    token: &CancellationToken,
) -> Result<IncomingMailboxSnapshot, AppError> {
    ensure_not_cancelled(token)?;
    let snapshot = backend
        .fetch_remote_messages_before(secret, mailbox_remote_id, before_uid, MAX_FETCH_MESSAGES)
        .await
        .map_err(incoming_error_to_app)?;
    validate_snapshot_mailbox(&snapshot, mailbox_remote_id)?;
    ensure_not_cancelled(token)?;
    if snapshot.uid_validity != Some(expected_uid_validity) {
        return Err(AppError::Protocol(format!(
            "IMAP UIDVALIDITY changed while backfilling {mailbox_remote_id}"
        )));
    }
    if snapshot
        .messages
        .iter()
        .any(|message| message.uid == 0 || message.uid >= before_uid)
    {
        return Err(AppError::Protocol(format!(
            "IMAP history page for {mailbox_remote_id} contained a UID outside the requested range"
        )));
    }
    Ok(snapshot)
}

fn next_incremental_plan(
    current: &MailboxFetchPlan,
    fetched: &FetchedSnapshot,
    batch_size: usize,
    batch_last_uid: Option<u32>,
) -> Option<MailboxFetchPlan> {
    let last_uid = batch_last_uid?;
    let full_batch = batch_size >= usize::try_from(MAX_FETCH_MESSAGES).unwrap_or(usize::MAX);
    (current.since_uid.is_some()
        && !fetched.complete_mailbox
        && !fetched.history_limited
        && full_batch
        && current.since_uid.is_none_or(|previous| last_uid > previous))
    .then(|| MailboxFetchPlan {
        remote_id: current.remote_id.clone(),
        expected_uid_validity: fetched.snapshot.uid_validity,
        since_uid: Some(last_uid),
    })
}

async fn fetch_snapshot<B: IncomingMailBackend + ?Sized>(
    backend: &B,
    secret: &str,
    plan: &MailboxFetchPlan,
    token: &CancellationToken,
) -> Result<FetchedSnapshot, AppError> {
    ensure_not_cancelled(token)?;
    let mut snapshot = backend
        .fetch_remote_messages(secret, &plan.remote_id, plan.since_uid, MAX_FETCH_MESSAGES)
        .await
        .map_err(incoming_error_to_app)?;
    validate_snapshot_mailbox(&snapshot, &plan.remote_id)?;
    require_uid_validity(&snapshot, &plan.remote_id)?;
    ensure_not_cancelled(token)?;

    let cursor_invalidated =
        plan.expected_uid_validity.is_some() && snapshot.uid_validity != plan.expected_uid_validity;
    let small_mailbox_needs_reconciliation =
        plan.since_uid.is_some() && snapshot.total_count <= MAX_FETCH_MESSAGES;
    if cursor_invalidated || small_mailbox_needs_reconciliation {
        // UIDs may be reused after UIDVALIDITY changes. Re-fetch a bounded recent window without
        // the old cursor, then let the database atomically invalidate the stale instances.
        snapshot = backend
            .fetch_remote_messages(secret, &plan.remote_id, None, MAX_FETCH_MESSAGES)
            .await
            .map_err(incoming_error_to_app)?;
        validate_snapshot_mailbox(&snapshot, &plan.remote_id)?;
        require_uid_validity(&snapshot, &plan.remote_id)?;
        ensure_not_cancelled(token)?;
    }
    let complete_mailbox = snapshot.coverage_complete;
    Ok(FetchedSnapshot {
        history_limited: !complete_mailbox && (plan.since_uid.is_none() || cursor_invalidated),
        snapshot,
        complete_mailbox,
    })
}

fn require_uid_validity(
    snapshot: &IncomingMailboxSnapshot,
    mailbox_remote_id: &str,
) -> Result<(), AppError> {
    if snapshot.uid_validity.is_some() {
        Ok(())
    } else {
        Err(AppError::Protocol(format!(
            "IMAP server omitted UIDVALIDITY for {mailbox_remote_id}"
        )))
    }
}

fn validate_snapshot_mailbox(
    snapshot: &IncomingMailboxSnapshot,
    expected_remote_id: &str,
) -> Result<(), AppError> {
    if snapshot.remote_id == expected_remote_id {
        Ok(())
    } else {
        Err(AppError::Protocol(format!(
            "IMAP mailbox response mismatch: expected {expected_remote_id}, received {}",
            snapshot.remote_id
        )))
    }
}

fn map_incoming_message(message: &IncomingMessage) -> SyncedMessageInput {
    let mut result = SyncedMessageInput {
        uid: message.uid,
        flags: message.flags.clone(),
        received_at: message.internal_date.clone(),
        size_bytes: message.size_bytes.map(i64::from),
        rfc_message_id: None,
        subject: String::new(),
        preview: String::new(),
        body_text: None,
        body_html_text: None,
        has_attachment: false,
        from: None,
        to: Vec::new(),
    };
    let has_full_message = message.raw_rfc822.is_some();
    let Some(raw) = message
        .raw_rfc822
        .as_deref()
        .or(message.raw_headers.as_deref())
    else {
        return result;
    };
    let Some(parsed) = MessageParser::default().parse(raw) else {
        return result;
    };

    result.rfc_message_id = parsed.message_id().map(ToOwned::to_owned);
    result.subject = parsed.subject().unwrap_or_default().to_owned();
    if has_full_message {
        result.preview = parsed
            .body_preview(240)
            .map(|value| value.into_owned())
            .unwrap_or_default();
        result.body_text = parsed.body_text(0).map(|value| value.into_owned());
        if parsed.html_body_count() > 0 {
            result.body_html_text = parsed
                .body_html(0)
                .map(|value| crate::mime::sanitizer::html_to_safe_text(value.as_ref()));
        }
        result.has_attachment = parsed.attachment_count() > 0;
    }
    result.received_at = parsed
        .date()
        .map(mail_parser::DateTime::to_rfc3339)
        .or(result.received_at);
    result.from = parsed
        .from()
        .and_then(|addresses| addresses.first())
        .and_then(map_address);
    result.to = parsed
        .to()
        .map(|addresses| addresses.iter().filter_map(map_address).collect())
        .unwrap_or_default();
    result
}

fn map_address(address: &mail_parser::Addr<'_>) -> Option<Address> {
    let email = address.address.as_deref()?.trim();
    if email.is_empty() {
        return None;
    }
    Some(Address {
        name: address
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned),
        email: email.to_owned(),
    })
}

fn ensure_not_cancelled(token: &CancellationToken) -> Result<(), AppError> {
    if token.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
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

fn count_as_i64(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

fn record_start_error(
    state: &AppState,
    app: &AppHandle,
    account_id: &str,
    token: &CancellationToken,
    message: String,
    retryable: bool,
) {
    state.sync.finish_current(account_id, token, || {
        if let Ok(mut database) = state.database.lock() {
            let _ = database.mark_account_sync_failed(account_id, &message);
        }
        let status = SyncStatus {
            account_id: account_id.to_owned(),
            state: "error".into(),
            phase: Some("authentication".into()),
            processed: None,
            total: None,
            message: Some(message),
            retryable,
        };
        state.sync.set_status(status.clone());
        let _ = app.emit("sync-progress", status);
    });
}

fn successful_sync_status(account_id: String, report: SyncReport) -> SyncStatus {
    let mut message = if report.history_limited_mailboxes == 0 {
        format!(
            "同步完成：{} 个文件夹，新增 {} 封，更新 {} 封",
            report.mailbox_count, report.inserted, report.updated
        )
    } else {
        format!(
            "本轮同步结束：{} 个文件夹，新增 {} 封，更新 {} 封；{} 个大型文件夹的较早邮件仍在分批回填（本轮每个文件夹最多 {} 封）",
            report.mailbox_count,
            report.inserted,
            report.updated,
            report.history_limited_mailboxes,
            usize::try_from(MAX_FETCH_MESSAGES).unwrap_or_default()
                * MAX_BACKFILL_PAGES_PER_MAILBOX
        )
    };
    if report.applied_operations > 0 {
        message.push_str(&format!(
            "；已向服务器提交 {} 项本地操作",
            report.applied_operations
        ));
    }
    if report.operation_budget_exhausted {
        message.push_str("；仍有本地操作将在下轮继续提交");
    }
    if report.forward_limited_mailboxes > 0 {
        message.push_str(&format!(
            "；{} 个文件夹的新邮件积压将在下轮继续同步",
            report.forward_limited_mailboxes
        ));
    }
    SyncStatus {
        account_id,
        state: "idle".into(),
        phase: None,
        processed: Some(count_as_i64(report.inserted + report.updated)),
        total: Some(count_as_i64(report.inserted + report.updated)),
        message: Some(message),
        retryable: false,
    }
}

fn sync_error_status(account_id: String, error: AppError) -> SyncStatus {
    SyncStatus {
        account_id,
        state: "error".into(),
        phase: Some("messages".into()),
        processed: None,
        total: None,
        message: Some(error.to_string()),
        retryable: error.retryable(),
    }
}

fn finish_current_sync(
    app: &AppHandle,
    account_id: &str,
    token: &CancellationToken,
    status: impl FnOnce(&AppState) -> SyncStatus,
) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    state.sync.finish_current(account_id, token, || {
        let status = status(&state);
        state.sync.set_status(status.clone());
        let _ = app.emit("sync-progress", status);
    });
}

fn with_current_sync_state<R>(
    app: &AppHandle,
    account_id: &str,
    token: &CancellationToken,
    action: impl FnOnce(&AppState) -> Result<R, AppError>,
) -> Result<R, AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    state
        .sync
        .with_current(account_id, token, || action(&state))
        .ok_or(AppError::Cancelled)?
}

fn publish_status(
    app: &AppHandle,
    token: &CancellationToken,
    status: SyncStatus,
) -> Result<(), AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    let account_id = status.account_id.clone();
    state
        .sync
        .with_current(&account_id, token, || {
            state.sync.set_status(status.clone());
            let _ = app.emit("sync-progress", status);
        })
        .ok_or(AppError::Cancelled)
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::{
        fetch_backfill_snapshot, fetch_snapshot, map_incoming_message, next_backfill_before_uid,
        next_incremental_plan, pending_operation_to_remote, FetchedSnapshot, MailboxFetchPlan,
    };
    use crate::backends::imap::MAX_FETCH_MESSAGES;
    use crate::backends::incoming::{
        IncomingConfig, IncomingError, IncomingMailBackend, IncomingMailbox,
        IncomingMailboxSnapshot, IncomingMessage, RemoteMessageOperation, ServerCapabilities,
    };
    use crate::domain::capabilities::ProviderCapabilities;
    use crate::errors::AppError;
    use crate::storage::database::{ImapSyncWindow, PendingImapOperation};
    use tokio_util::sync::CancellationToken;

    struct FakeBackend {
        snapshots: Mutex<VecDeque<IncomingMailboxSnapshot>>,
        requested_cursors: Mutex<Vec<Option<u32>>>,
        requested_before_uids: Mutex<Vec<u32>>,
    }

    #[async_trait::async_trait]
    impl IncomingMailBackend for FakeBackend {
        fn backend_name(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn test_connection(
            &self,
            _secret: &str,
        ) -> Result<ServerCapabilities, IncomingError> {
            Ok(ServerCapabilities {
                backend: "fake".into(),
                capabilities: ProviderCapabilities::default(),
                greeting: None,
            })
        }

        async fn list_remote_mailboxes(
            &self,
            _secret: &str,
        ) -> Result<Vec<IncomingMailbox>, IncomingError> {
            Ok(Vec::new())
        }

        async fn fetch_remote_messages(
            &self,
            _secret: &str,
            _mailbox: &str,
            since_uid: Option<u32>,
            _limit: u32,
        ) -> Result<IncomingMailboxSnapshot, IncomingError> {
            self.requested_cursors
                .lock()
                .expect("cursor lock")
                .push(since_uid);
            self.snapshots
                .lock()
                .expect("snapshot lock")
                .pop_front()
                .ok_or_else(|| IncomingError::Protocol("missing fake snapshot".into()))
        }

        async fn fetch_remote_messages_before(
            &self,
            _secret: &str,
            _mailbox: &str,
            before_uid: u32,
            _limit: u32,
        ) -> Result<IncomingMailboxSnapshot, IncomingError> {
            self.requested_before_uids
                .lock()
                .expect("before UID lock")
                .push(before_uid);
            self.snapshots
                .lock()
                .expect("snapshot lock")
                .pop_front()
                .ok_or_else(|| IncomingError::Protocol("missing fake snapshot".into()))
        }
    }

    fn snapshot(uid_validity: u32) -> IncomingMailboxSnapshot {
        IncomingMailboxSnapshot {
            remote_id: "INBOX".into(),
            uid_validity: Some(uid_validity),
            total_count: 1_000,
            unread_count: 1,
            coverage_complete: false,
            messages: Vec::new(),
        }
    }

    fn pending_operation(operation_type: &str) -> PendingImapOperation {
        PendingImapOperation {
            id: "operation-1".into(),
            account_id: "account-1".into(),
            operation_type: operation_type.into(),
            source_mailbox_remote_id: "INBOX".into(),
            uid: 42,
            uid_validity: Some(7),
            target_mailbox_remote_id: None,
            payload_json: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn matching_uid_validity_uses_saved_incremental_cursor_once() {
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([snapshot(7)])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let plan = MailboxFetchPlan {
            remote_id: "INBOX".into(),
            expected_uid_validity: Some(7),
            since_uid: Some(41),
        };
        fetch_snapshot(&backend, "secret", &plan, &CancellationToken::new())
            .await
            .expect("snapshot");
        assert_eq!(
            *backend.requested_cursors.lock().expect("cursor lock"),
            vec![Some(41)]
        );
    }

    #[tokio::test]
    async fn changed_uid_validity_discards_the_old_cursor_and_refetches() {
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([snapshot(8), snapshot(8)])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let plan = MailboxFetchPlan {
            remote_id: "INBOX".into(),
            expected_uid_validity: Some(7),
            since_uid: Some(41),
        };
        let fetched = fetch_snapshot(&backend, "secret", &plan, &CancellationToken::new())
            .await
            .expect("snapshot");
        assert_eq!(fetched.snapshot.uid_validity, Some(8));
        assert_eq!(
            *backend.requested_cursors.lock().expect("cursor lock"),
            vec![Some(41), None]
        );
    }

    #[tokio::test]
    async fn missing_uid_validity_is_rejected_before_any_snapshot_is_applied() {
        let mut invalid = snapshot(7);
        invalid.uid_validity = None;
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([invalid])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let error = fetch_snapshot(
            &backend,
            "secret",
            &MailboxFetchPlan {
                remote_id: "INBOX".into(),
                expected_uid_validity: None,
                since_uid: None,
            },
            &CancellationToken::new(),
        )
        .await
        .expect_err("UIDVALIDITY is required for safe UID persistence");
        assert!(matches!(error, AppError::Protocol(_)));
    }

    #[tokio::test]
    async fn small_mailbox_gets_a_complete_reconciliation_snapshot() {
        let mut incremental = snapshot(7);
        incremental.total_count = 3;
        let mut full = incremental.clone();
        full.coverage_complete = true;
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([incremental, full])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let fetched = fetch_snapshot(
            &backend,
            "secret",
            &MailboxFetchPlan {
                remote_id: "INBOX".into(),
                expected_uid_validity: Some(7),
                since_uid: Some(41),
            },
            &CancellationToken::new(),
        )
        .await
        .expect("snapshot");
        assert!(fetched.complete_mailbox);
        assert_eq!(
            *backend.requested_cursors.lock().expect("cursor lock"),
            vec![Some(41), None]
        );
    }

    #[tokio::test]
    async fn exists_count_below_limit_is_not_treated_as_complete_without_search_coverage() {
        let mut raced_snapshot = snapshot(7);
        raced_snapshot.total_count = MAX_FETCH_MESSAGES;
        raced_snapshot.coverage_complete = false;
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([raced_snapshot])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let fetched = fetch_snapshot(
            &backend,
            "secret",
            &MailboxFetchPlan {
                remote_id: "INBOX".into(),
                expected_uid_validity: None,
                since_uid: None,
            },
            &CancellationToken::new(),
        )
        .await
        .expect("bounded page");
        assert!(!fetched.complete_mailbox);
    }

    #[test]
    fn full_incremental_batch_advances_locally_without_skipping_backlog() {
        let current = MailboxFetchPlan {
            remote_id: "INBOX".into(),
            expected_uid_validity: Some(7),
            since_uid: Some(41),
        };
        let fetched = FetchedSnapshot {
            snapshot: snapshot(7),
            complete_mailbox: false,
            history_limited: false,
        };
        let next = next_incremental_plan(
            &current,
            &fetched,
            usize::try_from(MAX_FETCH_MESSAGES).expect("usize limit"),
            Some(291),
        )
        .expect("next batch");
        assert_eq!(next.since_uid, Some(291));
        assert_eq!(next.expected_uid_validity, Some(7));
    }

    #[tokio::test]
    async fn mailbox_with_251_messages_pages_back_to_uid_one() {
        let recent_window = ImapSyncWindow {
            uid_validity: 7,
            last_uid: 251,
            oldest_uid: Some(2),
            instance_count: 250,
        };
        let before_uid =
            next_backfill_before_uid(recent_window, 251).expect("one older message remains");
        assert_eq!(before_uid, 2);

        let mut older_page = snapshot(7);
        older_page.total_count = 251;
        older_page.messages = vec![IncomingMessage {
            sequence: 1,
            uid: 1,
            flags: Vec::new(),
            internal_date: None,
            size_bytes: None,
            raw_headers: None,
            raw_rfc822: None,
        }];
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::from([older_page])),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let fetched = fetch_backfill_snapshot(
            &backend,
            "secret",
            "INBOX",
            before_uid,
            7,
            &CancellationToken::new(),
        )
        .await
        .expect("older page");
        assert_eq!(fetched.messages[0].uid, 1);
        assert_eq!(
            *backend
                .requested_before_uids
                .lock()
                .expect("before UID lock"),
            vec![2]
        );

        let completed_window = ImapSyncWindow {
            oldest_uid: Some(1),
            instance_count: 251,
            ..recent_window
        };
        assert_eq!(next_backfill_before_uid(completed_window, 251), None);
    }

    #[test]
    fn maps_pending_flags_with_exact_mailbox_uid_and_validity() {
        let mut pending = pending_operation("set_flags");
        pending.payload_json = serde_json::json!({
            "is_read": true,
            "is_starred": false,
        });
        assert_eq!(
            pending_operation_to_remote(&pending, "account-1").expect("mapped operation"),
            RemoteMessageOperation::SetFlags {
                mailbox_remote_id: "INBOX".into(),
                uid: 42,
                expected_uid_validity: Some(7),
                is_read: Some(true),
                is_starred: Some(false),
            }
        );
    }

    #[test]
    fn maps_trash_to_a_safe_remote_move() {
        let mut pending = pending_operation("trash");
        pending.target_mailbox_remote_id = Some("Trash".into());
        assert_eq!(
            pending_operation_to_remote(&pending, "account-1").expect("mapped operation"),
            RemoteMessageOperation::Move {
                source_mailbox_remote_id: "INBOX".into(),
                target_mailbox_remote_id: "Trash".into(),
                uid: 42,
                expected_uid_validity: Some(7),
            }
        );
    }

    #[test]
    fn rejects_corrupt_pending_payload_instead_of_guessing() {
        let mut pending = pending_operation("set_flags");
        pending.payload_json = serde_json::json!({ "is_read": "yes" });
        assert!(matches!(
            pending_operation_to_remote(&pending, "account-1"),
            Err(AppError::InvalidConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn cancellation_stops_before_network_io() {
        let backend = FakeBackend {
            snapshots: Mutex::new(VecDeque::new()),
            requested_cursors: Mutex::new(Vec::new()),
            requested_before_uids: Mutex::new(Vec::new()),
        };
        let token = CancellationToken::new();
        token.cancel();
        let error = fetch_snapshot(
            &backend,
            "secret",
            &MailboxFetchPlan {
                remote_id: "INBOX".into(),
                expected_uid_validity: None,
                since_uid: None,
            },
            &token,
        )
        .await
        .expect_err("cancelled");
        assert!(matches!(error, AppError::Cancelled));
        assert!(backend
            .requested_cursors
            .lock()
            .expect("cursor lock")
            .is_empty());
    }

    #[test]
    fn maps_rfc822_metadata_body_and_addresses_without_unsafe_code() {
        let raw = b"From: =?UTF-8?B?5rWL6K+V?= <sender@example.com>\r\nTo: Receiver <receiver@example.com>\r\nDate: Thu, 03 Sep 2026 12:30:00 +0800\r\nSubject: =?UTF-8?B?5L2g5aW9?=\r\nMessage-ID: <message@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody text";
        let mapped = map_incoming_message(&IncomingMessage {
            sequence: 1,
            uid: 9,
            flags: vec!["\\Seen".into()],
            internal_date: None,
            size_bytes: Some(raw.len() as u32),
            raw_rfc822: Some(raw.to_vec()),
            raw_headers: None,
        });
        assert_eq!(mapped.uid, 9);
        assert_eq!(mapped.subject, "你好");
        assert_eq!(mapped.body_text.as_deref(), Some("Body text"));
        assert_eq!(
            mapped.from.as_ref().map(|address| address.email.as_str()),
            Some("sender@example.com")
        );
        assert_eq!(mapped.to[0].email, "receiver@example.com");
        assert_eq!(
            mapped.received_at.as_deref(),
            Some("2026-09-03T12:30:00+08:00")
        );
    }

    #[test]
    fn oversized_metadata_only_message_is_still_persistable() {
        let mapped = map_incoming_message(&IncomingMessage {
            sequence: 4,
            uid: 99,
            flags: Vec::new(),
            internal_date: Some("2026-09-03T01:02:03Z".into()),
            size_bytes: Some(30_000_000),
            raw_rfc822: None,
            raw_headers: Some(
                b"From: sender@example.com\r\nSubject: Large message\r\n\r\n".to_vec(),
            ),
        });
        assert_eq!(mapped.uid, 99);
        assert_eq!(mapped.subject, "Large message");
        assert_eq!(mapped.received_at.as_deref(), Some("2026-09-03T01:02:03Z"));
        assert!(mapped.body_text.is_none());
    }

    #[test]
    fn fake_config_shape_remains_constructible_for_backend_contract_tests() {
        let config = IncomingConfig {
            protocol: "imap".into(),
            host: "localhost".into(),
            port: 993,
            tls_mode: "implicit".into(),
            auth_method: "password".into(),
            username: "user@example.com".into(),
        };
        assert_eq!(config.protocol, "imap");
    }
}

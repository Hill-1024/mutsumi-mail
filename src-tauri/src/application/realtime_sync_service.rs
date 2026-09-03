use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppState, RealtimeSyncCoordinator};
use crate::application::sync_service;
use crate::backends::imap::ImapIncomingBackend;
use crate::backends::incoming::{IncomingConfig, IncomingError};
use crate::domain::{Account, SyncStatus};
use crate::errors::AppError;

// RFC 2177 recommends ending IDLE at least every 29 minutes. This is only a socket renewal; it
// never fetches messages unless the server sent an actual mailbox-change notification.
const IDLE_RENEWAL: Duration = Duration::from_secs(25 * 60);
const RETRY_BASE: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);

// IDLE is broadly supported by modern providers. A legacy server that lacks it cannot deliver
// realtime changes, so this deliberately slow fallback is isolated to that protocol limitation.
#[cfg(any(target_os = "android", target_os = "ios"))]
const IDLE_UNSUPPORTED_REFRESH: Duration = Duration::from_secs(60 * 60);
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const IDLE_UNSUPPORTED_REFRESH: Duration = Duration::from_secs(30 * 60);

struct WorkerSlot {
    cancellation: CancellationToken,
    generation: u64,
}

struct WorkerExit {
    account_id: String,
    generation: u64,
}

enum WorkerOutcome {
    Cancelled,
    Terminal,
}

enum WatchError {
    Cancelled,
    Retryable(AppError),
    Terminal(AppError),
}

impl WatchError {
    fn from_sync(error: AppError) -> Self {
        if error.retryable() {
            Self::Retryable(error)
        } else {
            Self::Terminal(error)
        }
    }

    fn from_idle(error: IncomingError) -> Self {
        match error {
            IncomingError::Authentication => Self::Terminal(AppError::Authentication),
            IncomingError::Unsupported(message) => Self::Terminal(AppError::Capability(message)),
            IncomingError::Network(message) | IncomingError::Tls(message) => {
                Self::Retryable(AppError::Network(message))
            }
            // A server may close an IDLE socket without a BYE response. The connection itself is
            // disposable, so retry this listener instead of permanently disabling realtime mail.
            IncomingError::Protocol(message) => Self::Retryable(AppError::Protocol(message)),
        }
    }
}

pub fn start(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        tracing::error!("realtime sync was started before application state was available");
        return;
    };
    let coordinator = Arc::clone(&state.realtime);
    tauri::async_runtime::spawn(async move {
        run_supervisor(app, coordinator).await;
    });
}

async fn run_supervisor(app: AppHandle, coordinator: Arc<RealtimeSyncCoordinator>) {
    let (worker_exits, mut worker_exit_events) = mpsc::unbounded_channel::<WorkerExit>();
    let mut workers = HashMap::<String, WorkerSlot>::new();
    let mut blocked = HashSet::<String>::new();
    let mut lifecycle_changes = coordinator.subscribe();
    let mut next_generation = 0_u64;

    loop {
        reconcile_workers(
            &app,
            &coordinator,
            &worker_exits,
            &mut workers,
            &mut blocked,
            &mut next_generation,
        );

        tokio::select! {
            changed = lifecycle_changes.changed() => {
                if changed.is_err() {
                    cancel_workers(&mut workers);
                    return;
                }
            }
            Some(exit) = worker_exit_events.recv() => {
                if workers
                    .get(&exit.account_id)
                    .is_some_and(|worker| worker.generation == exit.generation)
                {
                    workers.remove(&exit.account_id);
                    // Authentication and secure-storage failures are terminal until an explicit
                    // reconnect/account change. This is what prevents endless keychain prompts.
                    blocked.insert(exit.account_id);
                }
            }
        }
    }
}

fn reconcile_workers(
    app: &AppHandle,
    coordinator: &RealtimeSyncCoordinator,
    worker_exits: &mpsc::UnboundedSender<WorkerExit>,
    workers: &mut HashMap<String, WorkerSlot>,
    blocked: &mut HashSet<String>,
    next_generation: &mut u64,
) {
    let desired = match automatic_incoming_accounts(app) {
        Ok(accounts) => accounts,
        Err(error) => {
            tracing::warn!(error = %error, "unable to reconcile realtime IMAP workers");
            return;
        }
    };

    for account_id in coordinator.take_restart_requests() {
        blocked.remove(&account_id);
        if let Some(worker) = workers.remove(&account_id) {
            worker.cancellation.cancel();
        }
    }

    // A disabled/paused account loses its terminal marker, so enabling it later starts a fresh
    // listener without needing a full application restart.
    blocked.retain(|account_id| desired.contains(account_id));

    if !coordinator.network_allowed() {
        cancel_workers(workers);
        return;
    }

    let no_longer_desired = workers
        .keys()
        .filter(|account_id| !desired.contains(*account_id))
        .cloned()
        .collect::<Vec<_>>();
    for account_id in no_longer_desired {
        if let Some(worker) = workers.remove(&account_id) {
            worker.cancellation.cancel();
        }
    }

    for account_id in desired {
        if blocked.contains(&account_id)
            || workers.contains_key(&account_id)
            || account_has_active_sync(app, &account_id)
        {
            continue;
        }
        *next_generation = next_generation.wrapping_add(1);
        let generation = *next_generation;
        let cancellation = CancellationToken::new();
        workers.insert(
            account_id.clone(),
            WorkerSlot {
                cancellation: cancellation.clone(),
                generation,
            },
        );
        let app = app.clone();
        let worker_exits = worker_exits.clone();
        tauri::async_runtime::spawn(async move {
            if matches!(
                watch_account(app, account_id.clone(), cancellation.clone()).await,
                WorkerOutcome::Terminal
            ) && !cancellation.is_cancelled()
            {
                let _ = worker_exits.send(WorkerExit {
                    account_id,
                    generation,
                });
            }
        });
    }
}

fn account_has_active_sync(app: &AppHandle, account_id: &str) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|state| state.sync.is_active(account_id))
}

fn automatic_incoming_accounts(app: &AppHandle) -> Result<HashSet<String>, AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    let accounts = state
        .database
        .lock()
        .map_err(|_| AppError::Internal("database lock poisoned".into()))?
        .list_accounts()?;
    Ok(accounts
        .into_iter()
        .filter(is_realtime_candidate)
        .map(|account| account.id)
        .collect())
}

fn is_realtime_candidate(account: &Account) -> bool {
    account.enabled && account.incoming_configured && account.sync_policy == "automatic"
}

fn cancel_workers(workers: &mut HashMap<String, WorkerSlot>) {
    for worker in workers.values() {
        worker.cancellation.cancel();
    }
    workers.clear();
}

async fn watch_account(
    app: AppHandle,
    account_id: String,
    cancellation: CancellationToken,
) -> WorkerOutcome {
    let (config, secret) = match load_realtime_session(&app, &account_id) {
        Ok(session) => session,
        Err(error) => {
            report_realtime_problem(&app, &account_id, &error, false);
            return WorkerOutcome::Terminal;
        }
    };
    let backend = ImapIncomingBackend::new(config.clone());
    let mut needs_sync = true;
    let mut retry_attempt = 0_u32;
    let mut offline_reported = false;

    loop {
        if cancellation.is_cancelled() {
            return WorkerOutcome::Cancelled;
        }

        if needs_sync {
            match synchronize_after_signal(&app, &account_id, &config, &secret, &cancellation).await
            {
                Ok(()) => {
                    needs_sync = false;
                    retry_attempt = 0;
                    offline_reported = false;
                }
                Err(WatchError::Cancelled) => return WorkerOutcome::Cancelled,
                Err(WatchError::Retryable(error)) => {
                    tracing::debug!(account_id, error = %error, "realtime sync will retry");
                    if !wait_for_retry(retry_delay(retry_attempt, &account_id), &cancellation).await
                    {
                        return WorkerOutcome::Cancelled;
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    continue;
                }
                Err(WatchError::Terminal(error)) => {
                    tracing::warn!(account_id, error = %error, "realtime sync stopped for this account");
                    // `sync_service` has already persisted and emitted this failure.
                    return WorkerOutcome::Terminal;
                }
            }
        }

        let mut idle = match backend.open_idle_connection(&secret).await {
            Ok(connection) => connection,
            Err(IncomingError::Unsupported(_)) => {
                return watch_with_fallback(&app, &account_id, &config, &secret, &cancellation)
                    .await;
            }
            Err(error) => match WatchError::from_idle(error) {
                WatchError::Retryable(error) => {
                    if !offline_reported {
                        report_realtime_problem(&app, &account_id, &error, true);
                        offline_reported = true;
                    }
                    if !wait_for_retry(retry_delay(retry_attempt, &account_id), &cancellation).await
                    {
                        return WorkerOutcome::Cancelled;
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    needs_sync = true;
                    continue;
                }
                WatchError::Terminal(error) => {
                    report_realtime_problem(&app, &account_id, &error, false);
                    return WorkerOutcome::Terminal;
                }
                WatchError::Cancelled => return WorkerOutcome::Cancelled,
            },
        };

        let idle_result = tokio::select! {
            _ = cancellation.cancelled() => return WorkerOutcome::Cancelled,
            result = idle.wait_for_change(IDLE_RENEWAL) => result,
        };
        match idle_result {
            Ok(true) => {
                // Drop this selected session before the incremental sync so each account holds at
                // most one IMAP connection during normal steady state.
                drop(idle);
                needs_sync = true;
            }
            Ok(false) => {
                // Successful IDLE renewal proves the long-lived socket is healthy. Re-entering
                // IDLE uses the same authenticated connection and does not perform a sync.
                retry_attempt = 0;
            }
            Err(error) => match WatchError::from_idle(error) {
                WatchError::Retryable(error) => {
                    if !offline_reported {
                        report_realtime_problem(&app, &account_id, &error, true);
                        offline_reported = true;
                    }
                    if !wait_for_retry(retry_delay(retry_attempt, &account_id), &cancellation).await
                    {
                        return WorkerOutcome::Cancelled;
                    }
                    retry_attempt = retry_attempt.saturating_add(1);
                    needs_sync = true;
                }
                WatchError::Terminal(error) => {
                    report_realtime_problem(&app, &account_id, &error, false);
                    return WorkerOutcome::Terminal;
                }
                WatchError::Cancelled => return WorkerOutcome::Cancelled,
            },
        }
    }
}

async fn watch_with_fallback(
    app: &AppHandle,
    account_id: &str,
    config: &IncomingConfig,
    secret: &str,
    cancellation: &CancellationToken,
) -> WorkerOutcome {
    let mut retry_attempt = 0_u32;
    let mut next_sync_delay = IDLE_UNSUPPORTED_REFRESH;
    loop {
        if !wait_for_retry(next_sync_delay, cancellation).await {
            return WorkerOutcome::Cancelled;
        }
        match synchronize_after_signal(app, account_id, config, secret, cancellation).await {
            Ok(()) => {
                retry_attempt = 0;
                next_sync_delay = IDLE_UNSUPPORTED_REFRESH;
            }
            Err(WatchError::Cancelled) => return WorkerOutcome::Cancelled,
            Err(WatchError::Retryable(error)) => {
                tracing::debug!(account_id, error = %error, "legacy IMAP fallback sync will retry");
                next_sync_delay = retry_delay(retry_attempt, account_id);
                retry_attempt = retry_attempt.saturating_add(1);
            }
            Err(WatchError::Terminal(error)) => {
                tracing::warn!(account_id, error = %error, "legacy IMAP fallback stopped for this account");
                return WorkerOutcome::Terminal;
            }
        }
    }
}

async fn synchronize_after_signal(
    app: &AppHandle,
    account_id: &str,
    config: &IncomingConfig,
    secret: &str,
    cancellation: &CancellationToken,
) -> Result<(), WatchError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(WatchError::Cancelled);
        }
        let (sync, started) = {
            let state = app.try_state::<AppState>().ok_or_else(|| {
                WatchError::Terminal(AppError::Internal("application state unavailable".into()))
            })?;
            let sync = Arc::clone(&state.sync);
            let started = sync_service::start_sync_if_idle_with_session(
                &state,
                app.clone(),
                account_id.to_owned(),
                config.clone(),
                secret.to_owned(),
            )
            .map_err(WatchError::from_sync)?;
            (sync, started)
        };

        if !sync.wait_until_idle(account_id, cancellation).await {
            return Err(WatchError::Cancelled);
        }
        if started.is_none() {
            // A manual sync was already using this account. Once it completes, start one bounded
            // incremental pass to make sure an IDLE notification that raced it is not lost.
            continue;
        }

        match sync.status(account_id) {
            Some(status) if status.state == "error" => {
                let message = status
                    .message
                    .unwrap_or_else(|| "background sync failed".into());
                return if status.retryable {
                    Err(WatchError::Retryable(AppError::Network(message)))
                } else {
                    Err(WatchError::Terminal(AppError::Protocol(message)))
                };
            }
            Some(status) if status.state == "offline" => {
                return Err(WatchError::Retryable(AppError::Network(
                    status
                        .message
                        .unwrap_or_else(|| "IMAP listener is offline".into()),
                )));
            }
            _ => return Ok(()),
        }
    }
}

fn load_realtime_session(
    app: &AppHandle,
    account_id: &str,
) -> Result<(IncomingConfig, String), AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Internal("application state unavailable".into()))?;
    sync_service::load_incoming_session(&state, account_id)
}

async fn wait_for_retry(delay: Duration, cancellation: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancellation.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn retry_delay(attempt: u32, account_id: &str) -> Duration {
    let multiplier = 1_u64 << attempt.min(6);
    let base_millis = u64::try_from(RETRY_BASE.as_millis())
        .unwrap_or(u64::MAX)
        .saturating_mul(multiplier)
        .min(u64::try_from(RETRY_MAX.as_millis()).unwrap_or(u64::MAX));
    let jitter_bound = (base_millis / 5).max(1);
    let jitter = account_id
        .bytes()
        .fold(1_469_598_103_934_665_603_u64, |hash, byte| {
            hash.wrapping_mul(1_099_511_628_211)
                .wrapping_add(u64::from(byte))
        })
        % jitter_bound;
    Duration::from_millis(base_millis.saturating_add(jitter))
}

fn report_realtime_problem(app: &AppHandle, account_id: &str, error: &AppError, offline: bool) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.sync.is_active(account_id) {
        return;
    }
    let status = SyncStatus {
        account_id: account_id.to_owned(),
        state: if offline { "offline" } else { "error" }.into(),
        phase: Some("realtime".into()),
        processed: None,
        total: None,
        message: Some(if offline {
            "实时连接已断开，正在低频重连".into()
        } else {
            error.to_string()
        }),
        retryable: offline,
    };
    if let Ok(mut database) = state.database.lock() {
        let result = if offline {
            database.mark_account_sync_offline(account_id)
        } else {
            database.mark_account_sync_failed(account_id, &error.to_string())
        };
        if let Err(error) = result {
            tracing::debug!(account_id, error = %error, "unable to persist realtime listener status");
        }
    }
    state.sync.set_status(status.clone());
    let _ = app.emit("sync-progress", status);
}

#[cfg(test)]
mod tests {
    use super::{is_realtime_candidate, retry_delay};
    use crate::domain::Account;

    fn account(sync_policy: &str) -> Account {
        Account {
            id: "account-a".into(),
            provider_id: "generic".into(),
            email: "person@example.com".into(),
            display_name: "Person".into(),
            enabled: true,
            sync_policy: sync_policy.into(),
            incoming_configured: true,
            outgoing_configured: true,
            sync_status: "idle".into(),
            last_synced_at: None,
        }
    }

    #[test]
    fn only_enabled_automatic_incoming_accounts_keep_a_listener() {
        assert!(is_realtime_candidate(&account("automatic")));
        assert!(!is_realtime_candidate(&account("manual")));
        assert!(!is_realtime_candidate(&account("paused")));
        let mut no_incoming = account("automatic");
        no_incoming.incoming_configured = false;
        assert!(!is_realtime_candidate(&no_incoming));
    }

    #[test]
    fn retry_backoff_is_bounded_and_stable_per_account() {
        let initial = retry_delay(0, "account-a");
        let repeated = retry_delay(0, "account-a");
        let capped = retry_delay(99, "account-a");
        assert_eq!(initial, repeated);
        assert!(capped <= std::time::Duration::from_secs(6 * 60));
        assert!(capped > initial);
    }
}

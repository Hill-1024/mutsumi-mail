mod all_files_access;
mod app_state;
mod application;
mod auth;
mod backends;
mod commands;
mod domain;
mod errors;
mod mime;
mod providers;
mod storage;
mod sync;

use std::error::Error;
use std::fs;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(all_files_access::init());
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, _arguments, _working_directory| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        },
    ));
    let app = match builder
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| Box::new(error) as Box<dyn Error>)?;
            fs::create_dir_all(&data_dir)?;
            let database_path = data_dir.join("mutsumi-mail.sqlite3");
            let state = app_state::AppState::open(&database_path)
                .map_err(|error| Box::new(error) as Box<dyn Error>)?;
            let queued_outbox_ids = state
                .database
                .lock()
                .map_err(|_| errors::AppError::Internal("database lock poisoned".into()))?
                .queued_outbox_ids()?;
            app.manage(state);
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .with_target(false)
                .try_init()
                .ok();
            let app_handle = app.handle().clone();
            application::realtime_sync_service::start(app_handle.clone());
            for outbox_id in queued_outbox_ids {
                application::compose_service::spawn_delivery(app_handle.clone(), outbox_id);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_provider_presets,
            commands::detect_provider,
            commands::list_accounts,
            commands::create_account,
            commands::test_incoming_connection,
            commands::test_outgoing_connection,
            commands::remove_account,
            commands::list_mailboxes,
            commands::list_messages,
            commands::search_messages,
            commands::get_message,
            commands::fetch_message_body,
            commands::mutate_message,
            commands::mutate_messages,
            commands::mark_read,
            commands::set_starred,
            commands::save_draft,
            commands::send_draft,
            commands::send_draft_with_attachments,
            commands::list_outbox,
            commands::start_sync,
            commands::cancel_sync,
            commands::get_sync_status,
            commands::sync_all,
            commands::update_account,
            commands::get_account_status,
            commands::reconnect_account,
            commands::refresh_mailboxes,
            commands::set_mailbox_sync_policy,
            commands::move_messages,
            commands::delete_messages,
            commands::list_thread,
            commands::load_draft,
            commands::delete_draft,
            commands::retry_outbox_item,
            commands::cancel_outbox_item,
            commands::download_attachment,
            commands::cancel_attachment_download,
            commands::save_attachment_as,
            commands::open_attachment,
            commands::reveal_attachment,
            commands::get_search_suggestions,
            commands::get_settings,
            commands::update_settings,
            commands::clear_cache,
            commands::export_diagnostics
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Mutsumi Mail failed to start: {error}");
            return;
        }
    };
    app.run(|app, event| {
        // Android keeps the process hosting this runtime alive with MailSyncService. Do not
        // suspend IMAP IDLE when the Activity loses focus: that includes removing the UI task.
        #[cfg(target_os = "android")]
        if let tauri::RunEvent::Resumed = event {
            if let Some(state) = app.try_state::<app_state::AppState>() {
                state.realtime.resume();
            }
        }
        #[cfg(target_os = "ios")]
        match event {
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Focused(false),
                ..
            } => {
                if let Some(state) = app.try_state::<app_state::AppState>() {
                    state.realtime.suspend();
                }
            }
            tauri::RunEvent::Resumed => {
                if let Some(state) = app.try_state::<app_state::AppState>() {
                    state.realtime.resume();
                }
            }
            _ => {}
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let _ = (app, event);
        }
    });
}

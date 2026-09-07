use crate::{app_state::AppState, errors::AppError};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
use tauri_plugin_autostart::ManagerExt;

pub fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open-mail", "打开邮箱", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "check-mail", "立即收信", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit-mail", "退出 Mutsumi Mail", true, None::<&str>)?;
    let divider = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &divider, &quit])?;
    let mut tray = TrayIconBuilder::with_id("mail-background")
        .tooltip("Mutsumi Mail · 后台收信")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open-mail" => show(app),
            "check-mail" => {
                let state = app.state::<AppState>();
                if let Err(error) = crate::commands::sync_all(state, app.clone()) {
                    tracing::warn!(?error, "tray mail refresh failed");
                }
            }
            "quit-mail" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub fn keep_running(app: &AppHandle) -> bool {
    // Do not hide the last way back into the app if tray registration failed.
    app.tray_by_id("mail-background").is_some() && super::enabled(&app.state::<AppState>())
}

pub fn status(app: &AppHandle, enabled: bool) -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({
        "platform": std::env::consts::OS,
        "backgroundMail": enabled,
        "autostartSupported": true,
        "launchAtLogin": app.autolaunch().is_enabled().map_err(|e| AppError::Internal(e.to_string()))?,
        "serviceRunning": keep_running(app),
    }))
}

pub fn set_launch_at_login(app: &AppHandle, enabled: bool) -> Result<(), AppError> {
    let manager = app.autolaunch();
    (if enabled {
        manager.enable()
    } else {
        manager.disable()
    })
    .map_err(|error| AppError::Internal(error.to_string()))
}

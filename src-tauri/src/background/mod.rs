// IPC errors retain the structured DTO used by the other application commands.
#![allow(clippy::result_large_err)]

#[cfg(target_os = "android")]
pub mod android;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod desktop;

use crate::{
    app_state::AppState,
    errors::{AppError, AppErrorDto},
};
use tauri::{AppHandle, Manager};

pub fn enabled(state: &AppState) -> bool {
    state
        .database
        .lock()
        .ok()
        .and_then(|db| db.get_settings().ok())
        .and_then(|settings| settings["backgroundMail"].as_bool())
        .unwrap_or(true)
}

#[tauri::command]
pub fn get_background_status(app: AppHandle) -> Result<serde_json::Value, AppErrorDto> {
    let state = app.state::<AppState>();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return desktop::status(&app, enabled(&state)).map_err(AppErrorDto::from);
    #[cfg(target_os = "android")]
    return android::status(enabled(&state)).map_err(AppErrorDto::from);
    #[cfg(target_os = "ios")]
    Ok(
        serde_json::json!({"platform":"ios", "backgroundMail":enabled(&state), "autostartSupported":false}),
    )
}

#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> Result<(), AppErrorDto> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return desktop::set_launch_at_login(&app, enabled).map_err(AppErrorDto::from);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = (app, enabled);
        Err(AppErrorDto::from(AppError::Capability(
            "此平台由系统管理后台任务".into(),
        )))
    }
}

#[tauri::command]
pub fn open_background_settings() -> Result<(), AppErrorDto> {
    #[cfg(target_os = "android")]
    return android::open_settings().map_err(AppErrorDto::from);
    #[cfg(not(target_os = "android"))]
    Err(AppErrorDto::from(AppError::Capability(
        "此平台无需电池优化设置".into(),
    )))
}

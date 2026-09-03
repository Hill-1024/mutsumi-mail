//! Android's `MANAGE_EXTERNAL_STORAGE` is a special-access setting rather than
//! a regular runtime permission. This tiny Tauri plugin exposes only its
//! status and the system Settings entry point; it never reads a file itself.

#[cfg(target_os = "android")]
use tauri::{plugin::Builder, Runtime};

#[cfg(target_os = "android")]
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new("all-files-access")
        .setup(|_app, api| {
            api.register_android_plugin("moe.mutsumi.mail", "AllFilesAccessPlugin")?;
            Ok(())
        })
        .build()
}

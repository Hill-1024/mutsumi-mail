//! Android 12+ exposes wallpaper-derived Monet tonal palettes as framework
//! color resources. The web layer uses the accent seed with Material Color
//! Utilities so every MD3 role stays consistent across light and dark modes.

#[cfg(target_os = "android")]
use tauri::{plugin::Builder, Runtime};

#[cfg(target_os = "android")]
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new("dynamic-color")
        .setup(|_app, api| {
            api.register_android_plugin("moe.mutsumi.mail", "DynamicColorPlugin")?;
            Ok(())
        })
        .build()
}

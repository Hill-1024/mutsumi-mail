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

#[cfg(test)]
mod tests {
    use tauri::utils::{
        acl::{capability::Capability, resolved::Resolved, ExecutionContext},
        platform::Target,
    };

    #[test]
    fn palette_command_is_allowed_in_the_android_main_window() {
        // Resolve the actual build output: registering a Kotlin plugin alone does
        // not permit WebView invocations, even when Android supports its API.
        let manifests = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/acl-manifests.json"
        )))
        .unwrap();
        let capabilities: std::collections::BTreeMap<String, Capability> =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/capabilities.json")))
                .unwrap();
        let android = Resolved::resolve(&manifests, capabilities.clone(), Target::Android).unwrap();
        let permissions = android
            .allowed_commands
            .get("plugin:dynamic-color|palette")
            .expect("the Android palette command must be callable from the WebView");
        assert!(permissions.iter().any(|permission| permission
            .windows
            .iter()
            .any(|window| window.matches("main"))));
        assert!(permissions.iter().all(|permission| {
            permission.context == ExecutionContext::Local
                && permission
                    .windows
                    .iter()
                    .all(|window| window.as_str() == "main")
                && permission.webviews.is_empty()
        }));
        for target in [Target::MacOS, Target::Windows, Target::Linux, Target::Ios] {
            let resolved = Resolved::resolve(&manifests, capabilities.clone(), target).unwrap();
            assert!(!resolved
                .allowed_commands
                .contains_key("plugin:dynamic-color|palette"));
        }
    }
}

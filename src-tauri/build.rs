fn main() {
    // Kotlin commands still pass through Tauri's ACL before mobile dispatch.
    let attributes = tauri_build::Attributes::new().plugin(
        "dynamic-color",
        tauri_build::InlinedPlugin::new().commands(&["palette"]),
    );
    tauri_build::try_build(attributes).expect("failed to build Tauri capabilities");
}

package moe.mutsumi.mail

import android.app.Activity
import android.os.Build
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@TauriPlugin
class DynamicColorPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun palette(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            invoke.resolveObject(JSObject().apply { put("available", false) })
            return
        }

        val resourceId = activity.resources.getIdentifier("system_accent1_600", "color", "android")
        if (resourceId == 0) {
            invoke.resolveObject(JSObject().apply { put("available", false) })
            return
        }

        val color = activity.resources.getColor(resourceId, activity.theme)
        invoke.resolveObject(JSObject().apply {
            put("available", true)
            put("seedHex", String.format("#%06X", 0xFFFFFF and color))
        })
    }
}

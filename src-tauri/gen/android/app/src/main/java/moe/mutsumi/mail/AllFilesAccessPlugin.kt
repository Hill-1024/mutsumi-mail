package moe.mutsumi.mail

import android.app.Activity
import android.content.Intent
import android.content.ActivityNotFoundException
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@TauriPlugin
class AllFilesAccessPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun status(invoke: Invoke) {
        invoke.resolveObject(JSObject().apply {
            put(
                "granted",
                Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager(),
            )
        })
    }

    @Command
    fun request(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager()) {
            invoke.resolve()
            return
        }

        activity.runOnUiThread {
            try {
                val appPermission = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
                    data = Uri.parse("package:${activity.packageName}")
                }
                try {
                    activity.startActivity(appPermission)
                } catch (_: ActivityNotFoundException) {
                    // Some Android distributions omit the per-app special-access screen.
                    // Their global all-files-access list is the closest supported fallback.
                    activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                }
                invoke.resolve()
            } catch (error: Exception) {
                // Last-resort fallback still gives the person a usable settings screen on
                // devices that replace both stock special-access activities.
                try {
                    activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    })
                    invoke.resolve()
                } catch (fallbackError: Exception) {
                    invoke.reject(
                        "Unable to open Android file access settings: ${fallbackError.message}",
                    )
                }
            }
        }
    }
}

package moe.mutsumi.mail

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Credentials use LocalSecretStore; no native keyring bootstrap is needed.
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    enterImmersiveMode()
    MailSyncBridge.prepare(this)
  }

  override fun onStart() {
    super.onStart()
    MailSyncBridge.setForeground(true)
  }

  override fun onStop() {
    MailSyncBridge.setForeground(false)
    super.onStop()
  }

  override fun onWindowFocusChanged(hasFocus: Boolean) {
    super.onWindowFocusChanged(hasFocus)
    if (hasFocus) enterImmersiveMode()
  }

  private fun enterImmersiveMode() {
    WindowCompat.setDecorFitsSystemWindows(window, false)
    WindowInsetsControllerCompat(window, window.decorView).apply {
      hide(WindowInsetsCompat.Type.statusBars())
      systemBarsBehavior =
        WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    }
  }
}

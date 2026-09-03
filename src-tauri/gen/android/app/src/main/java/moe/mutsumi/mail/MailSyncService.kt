package moe.mutsumi.mail

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Keeps the process which owns Tauri's Rust IMAP-IDLE runtime alive after the
 * foreground Activity task is removed. This is deliberately in the default
 * application process: a separate process would not share the Rust runtime,
 * encrypted secrets, or database state managed by Tauri.
 */
class MailSyncService : Service() {
  override fun onCreate() {
    super.onCreate()
    createChannel()
    promoteToForeground()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    promoteToForeground()
    // Android may recreate this service after reclaiming its process. The system decides when
    // resources permit; this does not attempt to bypass a user force-stop.
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  // Android 15 stops a dataSync FGS after its six-hour background budget is exhausted.
  // Stop cleanly instead of attempting a forbidden immediate restart.
  override fun onTimeout(startId: Int, fgsType: Int) {
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf(startId)
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val channel = NotificationChannel(
      CHANNEL_ID,
      "后台邮件同步",
      NotificationManager.IMPORTANCE_LOW,
    ).apply {
      description = "在移除应用界面后保持 IMAP 实时收件连接"
      setShowBadge(false)
    }
    getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }

  private fun promoteToForeground() {
    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, CHANNEL_ID)
    } else {
      Notification.Builder(this)
    }
    val notification = builder
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentTitle("Mutsumi Mail")
      .setContentText("后台实时收件正在运行")
      .setCategory(Notification.CATEGORY_SERVICE)
      .setOngoing(true)
      .setContentIntent(openAppIntent())
      .build()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
  }

  private fun openAppIntent(): PendingIntent {
    val intent = Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
    }
    return PendingIntent.getActivity(
      this,
      0,
      intent,
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )
  }

  companion object {
    private const val CHANNEL_ID = "background-mail-sync"
    private const val NOTIFICATION_ID = 4_200

    fun start(context: Context) {
      val intent = Intent(context, MailSyncService::class.java)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        context.startForegroundService(intent)
      } else {
        context.startService(intent)
      }
    }
  }
}

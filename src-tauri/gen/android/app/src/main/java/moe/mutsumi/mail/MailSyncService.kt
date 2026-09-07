package moe.mutsumi.mail

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.lang.ref.WeakReference

/** Same process as the UI; a cold service start also starts the shared Rust engine. */
class MailSyncService : Service() {
  private var live = false
  override fun onCreate() {
    super.onCreate()
    live = true
    instance = WeakReference(this)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      getSystemService(NotificationManager::class.java).createNotificationChannel(
        NotificationChannel(CHANNEL, "后台邮件同步", NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false) },
      )
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (!MailSyncBridge.wanted(this) || MailSyncBridge.quotaExpired || !promote()) {
      stopSelf()
      return START_NOT_STICKY
    }
    MailSyncBridge.prepare(this) { ready ->
      android.os.Handler(mainLooper).post {
        if (!live) return@post
        if (ready && MailSyncBridge.wanted(this) && !MailSyncBridge.quotaExpired) {
          MailSyncBridge.setServiceActive(true)
          if (!promote()) { MailSyncBridge.setServiceActive(false); stopSelf() }
        } else stopSelf()
      }
    }
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onTimeout(startId: Int, fgsType: Int) {
    MailSyncBridge.serviceTimedOut()
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf() // Do not restart a dataSync service after Android's six-hour limit.
  }

  override fun onDestroy() {
    live = false
    instance = null
    MailSyncBridge.setServiceActive(false)
    super.onDestroy()
  }

  private fun promote(): Boolean {
    val text = when {
      !MailSyncBridge.engineReady -> "正在连接收件服务"
      !MailSyncBridge.online -> "后台收件等待网络恢复"
      else -> "后台收件已开启"
    }
    val notification = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_mail_notification).setContentTitle("Mutsumi Mail")
      .setContentText(text).setOngoing(true).setSilent(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setContentIntent(MailSyncBridge.openAppIntent(this)).build()
    return try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
      } else startForeground(NOTIFICATION_ID, notification)
      true
    } catch (error: Exception) {
      Log.w("MailSync", "Foreground execution unavailable; keeping scheduled checks", error)
      MailSyncJobService.schedule(this)
      false
    }
  }

  companion object {
    private const val CHANNEL = "background-mail-sync"
    private const val NOTIFICATION_ID = 4_200
    private var instance: WeakReference<MailSyncService>? = null
    fun refreshNotification() { instance?.get()?.let { if (MailSyncBridge.serviceActive) it.promote() } }
    fun start(context: Context) {
      if (MailSyncBridge.serviceActive || MailSyncBridge.quotaExpired) return
      try {
        val intent = Intent(context, MailSyncService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(intent)
        else context.startService(intent)
      } catch (error: Exception) {
        Log.w("MailSync", "Background service start deferred to the system scheduler", error)
        MailSyncJobService.schedule(context)
      }
    }
  }
}

package moe.mutsumi.mail

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONObject
import java.util.concurrent.Executors

/** Application-context bridge: no Activity, WebView or JavaScript is required by the engine. */
object MailSyncBridge {
  private const val PREFS = "mail-background"
  private const val WANTED = "wanted"
  private val main = Handler(Looper.getMainLooper())
  private val initializer = Executors.newSingleThreadExecutor()
  private lateinit var app: Context
  @Volatile private var initialized = false
  @Volatile var engineReady = false
    private set
  @Volatile var foreground = false
    private set
  @Volatile var serviceActive = false
    private set
  @Volatile var online = false
    private set
  @Volatile var quotaExpired = false
    private set
  private val syncing = mutableSetOf<String>()
  private var wakeLock: PowerManager.WakeLock? = null

  @Synchronized fun attach(context: Context) {
    if (initialized) return
    app = context.applicationContext
    System.loadLibrary("mutsumi_mail_lib")
    val connectivity = app.getSystemService(ConnectivityManager::class.java)
    connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
      override fun onAvailable(network: Network) = refreshNetwork()
      override fun onLost(network: Network) = refreshNetwork()
      override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) = refreshNetwork()
    })
    val idleReceiver = object : BroadcastReceiver() {
      override fun onReceive(context: Context, intent: Intent) = refreshNetwork()
    }
    if (Build.VERSION.SDK_INT >= 33) {
      app.registerReceiver(idleReceiver, IntentFilter(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED), Context.RECEIVER_NOT_EXPORTED)
    } else {
      @Suppress("UnspecifiedRegisterReceiverFlag")
      app.registerReceiver(idleReceiver, IntentFilter(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED))
    }
    initialized = true
    refreshNetwork()
  }

  fun prepare(context: Context, completion: (Boolean) -> Unit = {}) {
    attach(context)
    initializer.execute {
      try {
        if (!engineReady) engineReady = nativeInitialize(app.applicationInfo.dataDir)
        publishState()
        completion(engineReady)
      } catch (error: Exception) {
        Log.e("MailSync", "Unable to initialize background mail", error)
        completion(false)
      }
    }
  }

  fun wanted(context: Context): Boolean = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(WANTED, false)

  fun setForeground(value: Boolean) {
    foreground = value
    if (value) quotaExpired = false // Android resets the dataSync budget on foreground interaction.
    refreshNetwork()
    if (value && initialized && wanted(app)) main.post { MailSyncService.start(app) }
  }

  fun setServiceActive(value: Boolean) {
    serviceActive = value
    if (!value) synchronized(syncing) {
      syncing.clear()
      releaseWakeLock()
    }
    publishState()
  }

  fun serviceTimedOut() {
    quotaExpired = true
    setServiceActive(false)
    MailSyncJobService.schedule(app)
  }

  private fun publishState() {
    // Read one current snapshot on the main thread. An older network callback must not publish
    // stale Activity/service flags after a newer lifecycle event.
    if (Looper.myLooper() != Looper.getMainLooper()) { main.post { publishState() }; return }
    if (engineReady) nativePlatformState(foreground, serviceActive, online)
  }

  private fun refreshNetwork() {
    if (Looper.myLooper() != Looper.getMainLooper()) { main.post { refreshNetwork() }; return }
    if (!::app.isInitialized) return
    val connectivity = app.getSystemService(ConnectivityManager::class.java)
    val capabilities = connectivity.getNetworkCapabilities(connectivity.activeNetwork)
    val power = app.getSystemService(PowerManager::class.java)
    online = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true &&
      (!power.isDeviceIdleMode || power.isIgnoringBatteryOptimizations(app.packageName) || foreground)
    publishState()
    main.post { MailSyncService.refreshNotification() }
  }

  @JvmStatic fun updatePolicy(wanted: Boolean) {
    if (!::app.isInitialized) return
    main.post {
      val preferences = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
      if (preferences.getBoolean(WANTED, false) != wanted) preferences.edit().putBoolean(WANTED, wanted).apply()
      if (wanted) {
        MailSyncJobService.schedule(app)
        if (foreground && !serviceActive && !quotaExpired) MailSyncService.start(app)
      } else {
        app.stopService(Intent(app, MailSyncService::class.java))
        MailSyncJobService.cancel(app)
      }
    }
  }

  @JvmStatic fun syncActivity(accountId: String, active: Boolean) {
    if (!::app.isInitialized) return
    synchronized(syncing) {
      if (active) syncing.add(accountId) else syncing.remove(accountId)
      if (syncing.isEmpty() || !serviceActive) releaseWakeLock()
      else {
        val lock = wakeLock ?: app.getSystemService(PowerManager::class.java)
          .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "MutsumiMail:Transfer").apply { setReferenceCounted(false) }
          .also { wakeLock = it }
        lock.acquire(30_000L) // No permanent lock: each bounded transfer renews at most 30 seconds.
      }
    }
  }

  private fun releaseWakeLock() {
    wakeLock?.let { if (it.isHeld) it.release() }
  }

  @JvmStatic fun notifyNewMail(count: Int) {
    if (!::app.isInitialized || count <= 0) return
    val manager = NotificationManagerCompat.from(app)
    if (!manager.areNotificationsEnabled()) return
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      app.getSystemService(NotificationManager::class.java).createNotificationChannel(
        NotificationChannel("new-mail", "新邮件", NotificationManager.IMPORTANCE_DEFAULT),
      )
    }
    val notification = NotificationCompat.Builder(app, "new-mail")
      .setSmallIcon(R.drawable.ic_mail_notification)
      .setContentTitle("Mutsumi Mail")
      .setContentText("你有 $count 封新邮件")
      .setCategory(NotificationCompat.CATEGORY_EMAIL)
      .setAutoCancel(true)
      .setContentIntent(openAppIntent(app))
      .build()
    try { manager.notify(4_201, notification) }
    catch (error: SecurityException) { Log.i("MailSync", "Notification permission is unavailable") }
  }

  fun openAppIntent(context: Context): PendingIntent = PendingIntent.getActivity(context, 0,
    Intent(context, MainActivity::class.java).apply { flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP },
    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
  )

  @JvmStatic fun statusJson(): String = JSONObject().apply {
    put("platform", "android")
    put("autostartSupported", false)
    put("serviceRunning", serviceActive && engineReady)
    put("engineReady", engineReady)
    put("online", online)
    put("quotaExpired", quotaExpired)
    put("batteryUnrestricted", app.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(app.packageName))
    put("notificationsAllowed", NotificationManagerCompat.from(app).areNotificationsEnabled())
  }.toString()

  @JvmStatic fun openSettings() {
    main.post {
      try {
        app.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      } catch (error: Exception) {
        app.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${app.packageName}"))
          .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      }
    }
  }

  @JvmStatic private external fun nativeInitialize(dataDirectory: String): Boolean
  @JvmStatic private external fun nativePlatformState(foreground: Boolean, service: Boolean, online: Boolean)
  @JvmStatic external fun nativePrepareCheck(): Long
  @JvmStatic external fun nativeCheckOnce(id: Long): Boolean
  @JvmStatic external fun nativeCancelCheck(id: Long)
}

package moe.mutsumi.mail

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.BroadcastReceiver
import android.content.Intent
import android.os.Handler
import android.os.Looper
import java.util.concurrent.Executors

/** Persisted, network-constrained fallback. Android decides the actual execution time. */
class MailSyncJobService : JobService() {
  private val executor = Executors.newSingleThreadExecutor()
  private val main = Handler(Looper.getMainLooper())
  @Volatile private var checkId: Long = 0
  @Volatile private var current: JobParameters? = null

  override fun onStartJob(params: JobParameters): Boolean {
    if (!MailSyncBridge.wanted(this)) return false
    current = params
    MailSyncBridge.prepare(this) { ready ->
      if (current !== params) return@prepare
      val id = if (ready) MailSyncBridge.nativePrepareCheck() else 0
      checkId = id
      if (current !== params) { if (id != 0L) MailSyncBridge.nativeCancelCheck(id); return@prepare }
      executor.execute {
        val success = ready && current === params && MailSyncBridge.nativeCheckOnce(id)
        main.post {
          if (current === params) {
            current = null
            jobFinished(params, !success)
          }
        }
      }
    }
    return true
  }

  override fun onStopJob(params: JobParameters): Boolean {
    current = null
    if (MailSyncBridge.engineReady) MailSyncBridge.nativeCancelCheck(checkId)
    return MailSyncBridge.wanted(this)
  }

  override fun onDestroy() {
    current = null
    if (MailSyncBridge.engineReady) MailSyncBridge.nativeCancelCheck(checkId)
    executor.shutdown()
    super.onDestroy()
  }

  companion object {
    private const val JOB_ID = 4_202
    fun schedule(context: Context) {
      if (!MailSyncBridge.wanted(context)) return
      val scheduler = context.getSystemService(JobScheduler::class.java)
      if (scheduler.getPendingJob(JOB_ID) != null) return
      val job = JobInfo.Builder(JOB_ID, ComponentName(context, MailSyncJobService::class.java))
        .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
        .setPeriodic(15 * 60_000L)
        .setPersisted(true)
        .setBackoffCriteria(30_000L, JobInfo.BACKOFF_POLICY_EXPONENTIAL)
        .build()
      scheduler.schedule(job)
    }
    fun cancel(context: Context) { context.getSystemService(JobScheduler::class.java).cancel(JOB_ID) }
  }
}

class MailSyncBootReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) {
      // Boot receivers cannot start dataSync FGS on Android 15+. Schedule a bounded check.
      MailSyncJobService.schedule(context)
    }
  }
}

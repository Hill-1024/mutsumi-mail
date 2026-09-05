package io.crates.keyring

import android.content.Context

/** Initializes the Android native credential store inside the already-loaded Tauri Rust library. */
class Keyring private constructor() {
  companion object {
    init {
      System.loadLibrary("mutsumi_mail_lib")
    }

    external fun initializeNdkContext(context: Context)
  }
}

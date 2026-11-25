// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Written by uniffi-bindgen-react-native and edited by hand, so `ubrn
// generate` leaves it be (see noOverwrite in ubrn.config.yaml).
package dev.maxhq.core

import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.turbomodule.core.interfaces.CallInvokerHolder

@ReactModule(name = MaxhqCoreModule.NAME)
class MaxhqCoreModule(reactContext: ReactApplicationContext) :
  NativeMaxhqCoreSpec(reactContext) {

  init {
    // The core reads its keys through MaxhqKeys, which needs the app's
    // context before JS can start the core.
    MaxhqKeys.init(reactContext)
  }

  override fun getName(): String {
    return NAME
  }

  // Two native methods implemented in cpp-adapter.cpp, and ultimately
  // maxhq-core.cpp

  external fun nativeInstallRustCrate(runtimePointer: Long, callInvoker: CallInvokerHolder): Boolean
  external fun nativeCleanupRustCrate(runtimePointer: Long): Boolean

  override fun installRustCrate(): Boolean {
    val context = this.reactApplicationContext
    return nativeInstallRustCrate(
      context.javaScriptContextHolder!!.get(),
      context.jsCallInvokerHolder!!
    )
  }

  override fun cleanupRustCrate(): Boolean {
    return nativeCleanupRustCrate(
      this.reactApplicationContext.javaScriptContextHolder!!.get()
    )
  }

  companion object {
    const val NAME = "MaxhqCore"

    init {
      System.loadLibrary("maxhq-core")
    }
  }
}
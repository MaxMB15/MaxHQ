// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.core

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import java.security.ProviderException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The phone's keys, which the Rust core reads and writes over JNI and the
 * push service reads without it. Each is a file under the app's no-backup
 * folder, so neither a backup nor a device-to-device copy takes it, sealed
 * under an AES-256 key the Android Keystore makes and never lets out:
 * StrongBox's where the phone has one, the TEE's otherwise. The key asks
 * for no screen lock, so the app can use it in the background once the
 * phone has been unlocked after it started.
 *
 * The accounts are the ones in crates/app/src/keys.rs, such as "x25519"
 * for the push key.
 */
object MaxhqKeys {
  private const val KEY_STORE = "AndroidKeyStore"
  private const val ALIAS = "maxhq-keys"

  // One lock for both, since a static method and an instance method of an
  // object would each lock something else under @Synchronized.
  private val lock = Any()
  private var context: Context? = null
  private var files: KeyFiles? = null

  /**
   * Sets the store up with the app's context. The core's module calls it
   * as it loads, and the push service can too: only the first call counts.
   * The Keystore key waits for the first read, since making one can take a
   * moment and this runs on the main thread.
   */
  @JvmStatic
  fun init(context: Context) {
    synchronized(lock) { if (this.context == null) this.context = context.applicationContext }
  }

  /** The key kept under [account], or null when there's none. */
  @JvmStatic
  fun load(account: String): ByteArray? = files().load(account)

  /** Keeps [bytes] under [account] in place of what was there. */
  @JvmStatic
  fun save(account: String, bytes: ByteArray) = files().save(account, bytes)

  /** Drops the key kept under [account]. */
  @JvmStatic
  fun forget(account: String) = files().forget(account)

  private fun files(): KeyFiles =
    synchronized(lock) {
      files?.let {
        return it
      }
      val app = context ?: throw IllegalStateException("The app didn't set up its key store.")
      val store = KeyStore.getInstance(KEY_STORE).apply { load(null) }
      val kept = store.getKey(ALIAS, null) as SecretKey?
      val folder = KeyFiles(File(app.noBackupFilesDir, "keys"), kept ?: newKey(app))
      // Files sealed under a key the Keystore no longer has can never open.
      if (kept == null) folder.clear()
      files = folder
      folder
    }

  private fun newKey(context: Context): SecretKey {
    val strongBox =
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.P &&
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
    if (strongBox) {
      // Some phones list StrongBox but can't make this key in it.
      try {
        return generate(strongBox = true)
      } catch (e: ProviderException) {}
    }
    return generate(strongBox = false)
  }

  private fun generate(strongBox: Boolean): SecretKey {
    val spec =
      KeyGenParameterSpec.Builder(
          ALIAS,
          KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .apply {
          if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            setIsStrongBoxBacked(true)
          }
        }
        .build()
    return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEY_STORE).run {
      init(spec)
      generateKey()
    }
  }
}

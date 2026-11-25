// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.core

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The phone's keys as files in [folder], one per account, each sealed with
 * AES-256-GCM under [key]. A file holds the format's version byte, the
 * 12-byte nonce, then the sealed key with its 16-byte tag. The version and
 * the account are bound into the tag, so a file moved to another account's
 * name doesn't open.
 *
 * It uses nothing from Android, so the JVM tests run it with a key of
 * their own; MaxhqKeys hands it the Keystore's.
 */
internal class KeyFiles(private val folder: File, private val key: SecretKey) {
  /** The key kept under [account], or null when there's none. */
  @Synchronized
  fun load(account: String): ByteArray? {
    val file = fileFor(account)
    if (!file.exists()) return null
    return open(account, file.readBytes())
  }

  /**
   * Keeps [bytes] under [account] in place of what was there. The new file
   * is written in full and synced before it takes the old one's name, so
   * a crash leaves one or the other.
   */
  @Synchronized
  fun save(account: String, bytes: ByteArray) {
    val file = fileFor(account)
    val sealed = seal(account, bytes)
    if (!folder.isDirectory && !folder.mkdirs()) {
      throw IOException("Couldn't make the phone's key folder.")
    }
    // A leading dot never names an account, so the spare can't be read as one.
    val spare = File(folder, ".$account.new")
    FileOutputStream(spare).use { out ->
      out.write(sealed)
      out.fd.sync()
    }
    if (!spare.renameTo(file)) {
      spare.delete()
      throw IOException("Couldn't keep the key for $account.")
    }
  }

  /** Drops the key kept under [account]. One that isn't there is gone already. */
  @Synchronized
  fun forget(account: String) {
    val file = fileFor(account)
    if (file.exists() && !file.delete()) {
      throw IOException("Couldn't forget the key for $account.")
    }
  }

  /**
   * Drops every key. A new Keystore key can't open the files the old one
   * sealed, so the phone starts over as a new one.
   */
  @Synchronized
  fun clear() {
    if (folder.exists() && !folder.deleteRecursively()) {
      throw IOException("Couldn't clear the phone's old keys.")
    }
  }

  private fun fileFor(account: String): File {
    require(ACCOUNT.matches(account)) { "\"$account\" isn't a key's name." }
    return File(folder, account)
  }

  private fun seal(account: String, plain: ByteArray): ByteArray {
    // The Keystore picks the nonce itself and won't take one from us.
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, key)
    val nonce = cipher.iv
    if (nonce == null || nonce.size != NONCE_BYTES) {
      throw IOException("The key store gave a nonce of the wrong size.")
    }
    cipher.updateAAD(boundTo(account))
    return byteArrayOf(VERSION) + nonce + cipher.doFinal(plain)
  }

  private fun open(account: String, sealed: ByteArray): ByteArray {
    if (sealed.size < HEADER_BYTES + TAG_BYTES || sealed[0] != VERSION) {
      throw IOException("The key file for $account isn't one MaxHQ wrote.")
    }
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BYTES * 8, sealed, 1, NONCE_BYTES))
    cipher.updateAAD(boundTo(account))
    return cipher.doFinal(sealed, HEADER_BYTES, sealed.size - HEADER_BYTES)
  }

  private fun boundTo(account: String) = byteArrayOf(VERSION) + account.toByteArray(Charsets.UTF_8)

  internal companion object {
    const val VERSION: Byte = 1
    const val NONCE_BYTES = 12
    const val TAG_BYTES = 16
    const val HEADER_BYTES = 1 + NONCE_BYTES
    const val TRANSFORMATION = "AES/GCM/NoPadding"

    // The accounts in crates/app/src/keys.rs: letters, digits and dashes.
    private val ACCOUNT = Regex("[A-Za-z0-9][A-Za-z0-9-]{0,127}")
  }
}

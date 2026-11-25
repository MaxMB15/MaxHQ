// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.push

import com.google.crypto.tink.subtle.Base64
import com.google.crypto.tink.subtle.ChaCha20Poly1305
import com.google.crypto.tink.subtle.Hkdf
import com.google.crypto.tink.subtle.X25519
import java.security.GeneralSecurityException

/**
 * Opens what maxd seals, as the iPhone's extension does: X25519 with a
 * fresh key, HKDF-SHA256 salted with both public keys, then
 * ChaCha20-Poly1305 with no associated data. The sealed text is base64 of
 * the fresh public key, the nonce, the ciphertext and the tag. Tink's
 * subtle primitives do the work, since Android has no X25519 or ChaCha20
 * of its own before API 33.
 */
object PushOpen {
  private val INFO = "maxhq push 1".toByteArray(Charsets.UTF_8)
  private const val KEY_SIZE = 32
  private const val NONCE_SIZE = 12
  private const val TAG_SIZE = 16

  // Tink's decoder lets some stray characters through, and iOS takes only
  // padded standard base64, so both refuse the same text.
  private val BASE64 = Regex("^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$")

  /** The plaintext of [sealed] under the phone's private key, or null when it won't open. */
  fun open(sealed: String, phonePrivate: ByteArray): ByteArray? {
    if (!BASE64.matches(sealed)) return null
    val bytes =
      try {
        Base64.decode(sealed)
      } catch (_: IllegalArgumentException) {
        return null
      }
    if (bytes.size <= KEY_SIZE + NONCE_SIZE + TAG_SIZE) return null
    val freshPublic = bytes.copyOfRange(0, KEY_SIZE)
    var shared: ByteArray? = null
    var key: ByteArray? = null
    return try {
      val phonePublic = X25519.publicFromPrivate(phonePrivate)
      shared = X25519.computeSharedSecret(phonePrivate, freshPublic)
      key = Hkdf.computeHkdf("HMACSHA256", shared, freshPublic + phonePublic, INFO, KEY_SIZE)
      ChaCha20Poly1305(key).decrypt(bytes.copyOfRange(KEY_SIZE, bytes.size), ByteArray(0))
    } catch (_: GeneralSecurityException) {
      null
    } catch (_: IllegalArgumentException) {
      null
    } finally {
      shared?.fill(0)
      key?.fill(0)
    }
  }
}

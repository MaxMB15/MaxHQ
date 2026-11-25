// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.core

import java.io.File
import java.io.IOException
import java.nio.file.Files
import javax.crypto.AEADBadTagException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

// The file format MaxhqKeys keeps the phone's keys in, run on the JVM with
// a software AES key in place of the Keystore's.
class KeyFilesTest {
  private val root: File = Files.createTempDirectory("maxhq-keys").toFile()
  private val folder = File(root, "keys")
  private val key = aesKey()
  private val files = KeyFiles(folder, key)
  private val secret = ByteArray(32) { it.toByte() }

  @After
  fun cleanUp() {
    root.deleteRecursively()
  }

  @Test
  fun aKeyComesBackAsItWasSaved() {
    assertNull(files.load("x25519"))
    files.save("x25519", secret)
    assertArrayEquals(secret, files.load("x25519"))
    assertArrayEquals(secret, KeyFiles(folder, key).load("x25519"))
  }

  @Test
  fun eachKeyIsAFileOfVersionNonceAndSealedBytes() {
    files.save("ssh-ed25519", secret)
    val sealed = File(folder, "ssh-ed25519").readBytes()
    assertEquals(1 + 12 + secret.size + 16, sealed.size)
    assertEquals(KeyFiles.VERSION, sealed[0])
    val body = sealed.copyOfRange(KeyFiles.HEADER_BYTES, KeyFiles.HEADER_BYTES + secret.size)
    assertFalse(body.contentEquals(secret))
    assertEquals(listOf("ssh-ed25519"), folder.list()!!.toList())
  }

  @Test
  fun eachSaveTakesANewNonce() {
    files.save("ed25519", secret)
    val first = File(folder, "ed25519").readBytes().copyOfRange(1, KeyFiles.HEADER_BYTES)
    files.save("ed25519", secret)
    val second = File(folder, "ed25519").readBytes().copyOfRange(1, KeyFiles.HEADER_BYTES)
    assertNotEquals(first.toList(), second.toList())
  }

  @Test
  fun aSaveReplacesTheOldKeyAndLeavesNoSpare() {
    files.save("token-a1", secret)
    val newer = ByteArray(32) { 7 }
    files.save("token-a1", newer)
    assertArrayEquals(newer, files.load("token-a1"))
    assertEquals(listOf("token-a1"), folder.list()!!.toList())
  }

  @Test
  fun aFileMovedToAnotherAccountsNameDoesNotOpen() {
    files.save("x25519", secret)
    File(folder, "x25519").renameTo(File(folder, "ed25519"))
    assertThrows(AEADBadTagException::class.java) { files.load("ed25519") }
  }

  @Test
  fun aChangedOrShortFileDoesNotOpen() {
    files.save("x25519", secret)
    val file = File(folder, "x25519")
    val sealed = file.readBytes()
    sealed[sealed.size - 1] = (sealed[sealed.size - 1] + 1).toByte()
    file.writeBytes(sealed)
    assertThrows(AEADBadTagException::class.java) { files.load("x25519") }
    file.writeBytes(byteArrayOf(KeyFiles.VERSION, 1, 2, 3))
    assertThrows(IOException::class.java) { files.load("x25519") }
    file.writeBytes(byteArrayOf(2) + sealed.copyOfRange(1, sealed.size))
    assertThrows(IOException::class.java) { files.load("x25519") }
  }

  @Test
  fun anotherKeyCannotOpenTheFiles() {
    files.save("x25519", secret)
    assertThrows(AEADBadTagException::class.java) { KeyFiles(folder, aesKey()).load("x25519") }
  }

  @Test
  fun forgettingDropsTheFileAndForgettingTwiceIsFine() {
    files.save("x25519", secret)
    files.forget("x25519")
    assertNull(files.load("x25519"))
    files.forget("x25519")
    assertFalse(File(folder, "x25519").exists())
  }

  @Test
  fun clearingDropsEveryKey() {
    files.save("x25519", secret)
    files.save("ed25519", secret)
    files.clear()
    assertFalse(folder.exists())
    assertNull(files.load("x25519"))
    files.clear()
  }

  @Test
  fun onlyAnAccountNameNamesAFile() {
    for (account in listOf("", ".hidden", "../x25519", "a/b", "x25519.new", "a".repeat(129))) {
      assertThrows(IllegalArgumentException::class.java) { files.save(account, secret) }
      assertThrows(IllegalArgumentException::class.java) { files.load(account) }
    }
    assertTrue(root.listFiles()!!.isEmpty())
  }

  private fun aesKey(): SecretKey = KeyGenerator.getInstance("AES").run {
    init(256)
    generateKey()
  }
}

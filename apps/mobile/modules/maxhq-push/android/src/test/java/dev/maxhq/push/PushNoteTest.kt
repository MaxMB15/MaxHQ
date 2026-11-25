// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.push

import dev.maxhq.push.PushOpenTest.Companion.hex
import dev.maxhq.push.PushOpenTest.Companion.vectors
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

// What a push from maxd shows, from the data FCM hands over.
class PushNoteTest {
  private val vector = vectors().first()
  private val key = hex(vector.getString("phone_private"))
  private val data =
    mapOf(
      "c" to vector.getString("sealed"),
      "e" to "0000-attention",
      "t" to "0000-agent",
      "k" to "needs_input",
      "a" to "permission",
      "n" to "0000-note",
    )

  @Test
  fun anOpenedPushShowsWhatItSays() {
    val note = PushNote.of(data, "fallback", key)
    assertEquals("0000-note", note.id)
    assertEquals("Fix the merge", note.title)
    assertEquals("Run cargo test --workspace?", note.text)
    assertEquals("maxhq on devbox", note.subtitle)
    assertEquals("needs_input", note.channel)
    assertEquals("0000-agent", note.group)
    assertEquals("permission", note.category)
    assertTrue(note.prompt)
    // The app reads the same keys an iPhone's push has, without the seal.
    val body = note.body
    assertFalse(body.has("c"))
    assertEquals("0000-attention", body.getString("e"))
    assertEquals("0000-agent", body.getString("t"))
    assertEquals("needs_input", body.getString("k"))
    assertEquals("permission", body.getString("a"))
    assertEquals("0000-note", body.getString("n"))
    val opened = body.getJSONObject("m")
    assertEquals(plain(JSONObject(vector.getString("plaintext"))), plain(opened))
  }

  @Test
  fun aPushThatWontOpenShowsThePlaceholder() {
    val wrong = key.copyOf().also { it[5] = (it[5].toInt() xor 1).toByte() }
    for (note in
      listOf(
        PushNote.of(data, "fallback", null),
        PushNote.of(data, "fallback", wrong),
        PushNote.of(data + ("c" to "not base64"), "fallback", key),
      )) {
      assertEquals("MaxHQ", note.title)
      assertEquals("An agent needs you", note.text)
      assertNull(note.subtitle)
      assertFalse(note.body.has("m"))
      // What came in the clear still points at the request.
      assertEquals("0000-attention", note.body.getString("e"))
      assertEquals("0000-note", note.id)
    }
  }

  @Test
  fun theKeyIsLeftAsItCame() {
    val copy = key.copyOf()
    PushNote.of(data, "fallback", copy)
    assertEquals(key.toList(), copy.toList())
  }

  @Test
  fun aPushWithoutAnIdTakesTheFallback() {
    assertEquals("fallback", PushNote.of(data - "n", "fallback", key).id)
    assertEquals("fallback", PushNote.of(data + ("n" to ""), "fallback", key).id)
  }

  @Test
  fun otherKindsAreNotPrompts() {
    val note = PushNote.of(data - "a" - "e" + ("k" to "finished"), "fallback", key)
    assertEquals("finished", note.channel)
    assertFalse(note.prompt)
    assertNull(note.category)
    assertFalse(note.body.has("e"))
  }

  @Test
  fun aMessageKeepsOnlyWhatTheIphoneKeeps() {
    val message =
      PushNote.message(
        """{"title":"T","body":"B","agent":"A","device":"D","extra":1,
          "options":[{"id":"allow","label":"Allow","kind":"allow_once"}]}"""
      )!!
    assertEquals(setOf("title", "body", "agent", "device", "options"), message.keys().asSequence().toSet())
    assertEquals(listOf(mapOf("id" to "allow", "label" to "Allow")), plain(message.getJSONArray("options")))
  }

  @Test
  fun aMessageMissingAFieldOrOfTheWrongTypeIsRefused() {
    val whole = """"title":"T","body":"B","agent":"A","device":"D""""
    assertTrue(PushNote.message("{$whole}") != null)
    assertTrue(PushNote.message("""{$whole,"subtitle":null,"attention":null,"options":null}""") != null)
    assertNull(PushNote.message("""{"title":"T","body":"B","agent":"A"}"""))
    assertNull(PushNote.message("""{"title":1,"body":"B","agent":"A","device":"D"}"""))
    assertNull(PushNote.message("""{$whole,"subtitle":2}"""))
    assertNull(PushNote.message("""{$whole,"options":{}}"""))
    assertNull(PushNote.message("""{$whole,"options":[{"id":"x"}]}"""))
    assertNull(PushNote.message("not json"))
  }

  // JSON as plain maps and lists, which compare by value. Android's
  // org.json, which the test compiles against, has no toMap of its own.
  private fun plain(value: Any?): Any? =
    when (value) {
      is JSONObject -> value.keys().asSequence().associateWith { plain(value.get(it)) }
      is JSONArray -> (0 until value.length()).map { plain(value.get(it)) }
      else -> value
    }
}

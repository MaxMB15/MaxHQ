// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.push

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * What a push from maxd shows, worked out from its FCM data. maxd sends
 * every value as a string: the sealed message under "c", the request
 * under "e", the agent under "t", the kind under "k", "permission" under
 * "a" when the push gets its Deny button, and the notification's id under
 * "n". The app reads the same keys as an iPhone's push, with what opened
 * under "m", so one set of listeners serves both.
 */
data class PushNote(
  /** The notification's id, so a reminder takes the place of the first. */
  val id: String,
  val title: String,
  val text: String,
  val subtitle: String?,
  /** What the app reads back when the notification comes or is tapped. */
  val body: JSONObject,
  /** The channel, one per push kind, which the app makes. */
  val channel: String?,
  /** The agent, so each agent's notifications sit together. */
  val group: String?,
  /** The category whose buttons the notification shows. */
  val category: String?,
  /** Whether it waits on you, which gets it sound and a heads-up. */
  val prompt: Boolean,
) {
  companion object {
    /** What shows when the message won't open, as maxd writes for an iPhone. */
    const val TITLE = "MaxHQ"
    const val TEXT = "An agent needs you"

    /** The kind that waits on you. */
    const val PROMPT = "needs_input"

    /** The category with the Deny button, as maxd and the app name it. */
    const val PERMISSION = "permission"

    private val CLEAR = listOf("e", "t", "k", "a", "n")

    /**
     * The note for a push's [data], opened with [key] when there is one.
     * [fallbackId] stands in for a push without an id.
     */
    fun of(data: Map<String, String>, fallbackId: String, key: ByteArray?): PushNote {
      val opened =
        key?.let { phone -> data["c"]?.let { PushOpen.open(it, phone) } }?.let { plain ->
          try {
            message(String(plain, Charsets.UTF_8))
          } finally {
            plain.fill(0)
          }
        }
      val body = JSONObject()
      for (name in CLEAR) data[name]?.let { body.put(name, it) }
      opened?.let { body.put("m", it) }
      val kind = data["k"]?.takeIf { it.isNotEmpty() }
      return PushNote(
        id = data["n"]?.takeIf { it.isNotEmpty() } ?: fallbackId,
        title = opened?.getString("title") ?: TITLE,
        text = opened?.getString("body") ?: TEXT,
        subtitle = opened?.takeIf { it.has("subtitle") }?.getString("subtitle")?.ifEmpty { null },
        body = body,
        channel = kind,
        group = data["t"]?.takeIf { it.isNotEmpty() },
        category = PERMISSION.takeIf { data["a"] == PERMISSION },
        prompt = kind == PROMPT,
      )
    }

    /**
     * The opened message with only the fields the iPhone's extension
     * keeps, or null when one it needs is missing or of the wrong type.
     * Both phones then hand the app the same thing.
     */
    internal fun message(plain: String): JSONObject? {
      try {
        val read = JSONObject(plain)
        val kept = JSONObject()
        for (name in listOf("title", "body", "agent", "device")) {
          kept.put(name, read.get(name) as? String ?: return null)
        }
        for (name in listOf("subtitle", "attention")) {
          if (!read.isNull(name)) kept.put(name, read.get(name) as? String ?: return null)
        }
        if (!read.isNull("options")) {
          val options = read.get("options") as? JSONArray ?: return null
          val choices = JSONArray()
          for (index in 0 until options.length()) {
            val option = options.get(index) as? JSONObject ?: return null
            choices.put(
              JSONObject()
                .put("id", option.get("id") as? String ?: return null)
                .put("label", option.get("label") as? String ?: return null)
            )
          }
          kept.put("options", choices)
        }
        return kept
      } catch (_: JSONException) {
        return null
      }
    }
  }
}

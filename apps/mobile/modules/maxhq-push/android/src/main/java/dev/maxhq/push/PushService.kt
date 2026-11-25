// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

package dev.maxhq.push

import android.content.Context
import android.util.Log
import com.google.firebase.messaging.RemoteMessage
import dev.maxhq.core.MaxhqKeys
import expo.modules.notifications.notifications.enums.NotificationPriority
import expo.modules.notifications.notifications.model.Notification
import expo.modules.notifications.notifications.model.NotificationContent
import expo.modules.notifications.notifications.model.NotificationRequest
import expo.modules.notifications.notifications.triggers.ChannelAwareTrigger
import expo.modules.notifications.service.ExpoFirebaseMessagingService
import expo.modules.notifications.service.NotificationsService
import expo.modules.notifications.service.delegates.FirebaseMessagingDelegate
import java.util.Date
import java.util.UUID

/**
 * Receives the phone's pushes from FCM. It extends expo-notifications'
 * service rather than standing beside it, since Android hands each
 * message to one service only, and expo's keeps the token listeners, the
 * handler the app sets for a push that comes while it's open, the
 * category buttons, and the taps and button presses the app's listeners
 * hear. This one only turns maxd's data message into the notification it
 * stands for: maxd sends no notification block, so that what it says
 * stays sealed to this phone.
 */
class PushService : ExpoFirebaseMessagingService() {
  override val firebaseMessagingDelegate by lazy { Delegate(this) }

  class Delegate(context: Context) : FirebaseMessagingDelegate(context) {
    override fun onMessageReceived(remoteMessage: RemoteMessage) {
      val data = remoteMessage.data
      // A message without a seal isn't maxd's, so it shows as expo would.
      if (data["c"] == null) return super.onMessageReceived(remoteMessage)
      val key = pushKey()
      val note =
        try {
          PushNote.of(data, remoteMessage.messageId ?: UUID.randomUUID().toString(), key)
        } finally {
          key?.fill(0)
        }
      NotificationsService.receive(context, Notification(request(note), Date(remoteMessage.sentTime)))
    }

    /**
     * The push key the core keeps, or null when there's none or it won't
     * read, so the push shows its placeholder. The core may not have run
     * in this process yet, so the store is set up here too.
     */
    private fun pushKey(): ByteArray? =
      try {
        MaxhqKeys.init(context)
        MaxhqKeys.load("x25519")?.takeIf { it.size == 32 }
      } catch (error: Exception) {
        Log.w(TAG, "The push key didn't read.", error)
        null
      }
  }

  companion object {
    private const val TAG = "MaxhqPush"

    /**
     * A prompt sounds and comes up over what's on screen, as on an iPhone,
     * where it's time sensitive. The other kinds come quietly, as an
     * iPhone shows them. Android 8 and later take both from the channel;
     * these count on older systems and while the app is open.
     */
    internal fun request(note: PushNote): NotificationRequest {
      val content =
        NotificationContent.Builder()
          .setTitle(note.title)
          .setText(note.text)
          .setSubtitle(note.subtitle)
          .setBody(note.body)
          .setGroup(note.group)
          .setCategoryId(note.category)
          .setAutoDismiss(true)
      if (note.prompt) {
        content.setPriority(NotificationPriority.HIGH)
      } else {
        content.setPriority(NotificationPriority.DEFAULT).setSound(null).setVibrationPattern(null)
      }
      return NotificationRequest(note.id, content.build(), ChannelAwareTrigger(note.channel))
    }
  }
}

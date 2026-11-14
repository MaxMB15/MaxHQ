// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import Foundation
import Security
import UserNotifications

/// Opens each push with the phone's push key and shows what it says. On
/// any failure the push shows as it came, "An agent needs you". It never
/// reaches the network.
final class NotificationService: UNNotificationServiceExtension {
    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        guard
            let content = request.content.mutableCopy() as? UNMutableNotificationContent,
            let sealed = content.userInfo["c"] as? String,
            let key = Self.pushKey(),
            let message = try? PushOpen.message(sealed, key: key)
        else {
            contentHandler(request.content)
            return
        }
        content.title = message.title
        content.subtitle = message.subtitle ?? ""
        content.body = message.body
        // The app reads the agent, the request and its choices from "m".
        if let opened = try? JSONEncoder().encode(message),
           let json = try? JSONSerialization.jsonObject(with: opened)
        {
            content.userInfo["m"] = json
        }
        contentHandler(content)
    }

    /// The push key the app keeps in the Keychain group the two share.
    private static func pushKey() -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "maxhq.phone-key",
            kSecAttrAccount as String: "x25519",
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let key = item as? Data, key.count == 32
        else {
            return nil
        }
        return key
    }
}

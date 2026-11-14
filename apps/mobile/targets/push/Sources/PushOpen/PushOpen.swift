// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import CryptoKit
import Foundation

/// What a sealed push says, as maxd writes it.
struct PushMessage: Codable, Equatable {
    struct Choice: Codable, Equatable {
        let id: String
        let label: String
    }

    let title: String
    let subtitle: String?
    let body: String
    let agent: String
    let device: String
    let attention: String?
    let options: [Choice]?
}

enum PushOpenError: Error {
    case malformed
}

/// Opens what maxd seals: X25519 with a fresh key, HKDF-SHA256 salted with
/// both public keys, then ChaCha20-Poly1305. The sealed text is base64 of
/// the fresh public key, the nonce, the ciphertext and the tag.
enum PushOpen {
    static let info = Data("maxhq push 1".utf8)

    static func open(_ sealed: String, key: Data) throws -> Data {
        guard let bytes = Data(base64Encoded: sealed), bytes.count > 32 + 12 + 16 else {
            throw PushOpenError.malformed
        }
        let phone = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: key)
        let freshPublic = Data(bytes.prefix(32))
        let fresh = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: freshPublic)
        let shared = try phone.sharedSecretFromKeyAgreement(with: fresh)
        let symmetric = shared.hkdfDerivedSymmetricKey(
            using: SHA256.self,
            salt: freshPublic + phone.publicKey.rawRepresentation,
            sharedInfo: info,
            outputByteCount: 32
        )
        let box = try ChaChaPoly.SealedBox(combined: Data(bytes.dropFirst(32)))
        return try ChaChaPoly.open(box, using: symmetric)
    }

    static func message(_ sealed: String, key: Data) throws -> PushMessage {
        try JSONDecoder().decode(PushMessage.self, from: open(sealed, key: key))
    }
}

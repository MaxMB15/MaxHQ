// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The proof a connection gives `maxd` that it holds a key: an Ed25519
//! signature over the challenge in the hello and what names the
//! connection. The app, the phone and `maxd` all lay out the signed bytes
//! here, so they can't drift apart.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::ids::{ClientId, DeviceId, InstanceId};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use thiserror::Error;

/// What every signed statement starts with. A signature over these bytes
/// can never read as an SSH or DPoP signature, though phones and peers sign
/// those with the same keys.
const PREFIX: &[u8] = b"maxhq-prove-v1\0";

/// The kind of connection that proves itself. The signature names it, so a
/// proof for one kind can't pass for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The app, after its hello.
    App,
    /// A phone, after a `Phone` frame.
    Phone,
    /// Another device's `maxd`, after a `Peer` frame.
    Peer,
    /// A phone that's pairing, after a `Pair` frame.
    Pair,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Phone => "phone",
            Self::Peer => "peer",
            Self::Pair => "pair",
        }
    }
}

/// The challenge a hello carries: 32 random bytes, new for each connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge(pub [u8; 32]);

impl Challenge {
    /// A new challenge, or none when the system gives no random bytes.
    pub fn generate() -> Option<Self> {
        let mut bytes = [0u8; 32];
        SystemRandom::new().fill(&mut bytes).ok()?;
        Some(Self(bytes))
    }

    /// Reads a challenge as `ServerHello::challenge` carries it, or none
    /// when it isn't 32 bytes of unpadded base64url.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// The challenge as `ServerHello::challenge` carries it.
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }
}

/// What a proof signs: which connection, to which run of which device's
/// `maxd`, answering which challenge. A proof made for one connection
/// fails on any other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Statement {
    /// The kind of connection.
    pub kind: Kind,
    /// The device, from the hello.
    pub device: DeviceId,
    /// The run of `maxd`, from the hello.
    pub instance: InstanceId,
    /// The client's ID, from its own hello.
    pub client: ClientId,
    /// The challenge, from the hello.
    pub challenge: Challenge,
}

impl Statement {
    /// The bytes a proof signs: [`PREFIX`], then the kind's name, the
    /// device, the instance, the client and the challenge. IDs go as their
    /// 16 bytes.
    ///
    /// Each part goes as a 4-byte big-endian length and then its bytes, as
    /// SSH lays out strings. A part can then never run into the next, so no
    /// two statements give the same bytes, even if a later version adds a
    /// kind or changes how long a part is.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let parts: [&[u8]; 5] = [
            self.kind.name().as_bytes(),
            self.device.0.as_bytes(),
            self.instance.0.as_bytes(),
            self.client.0.as_bytes(),
            &self.challenge.0,
        ];
        let mut bytes = Vec::with_capacity(
            PREFIX.len() + parts.iter().map(|part| 4 + part.len()).sum::<usize>(),
        );
        bytes.extend_from_slice(PREFIX);
        for part in parts {
            let len = u32::try_from(part.len()).expect("every part is a few bytes");
            bytes.extend_from_slice(&len.to_be_bytes());
            bytes.extend_from_slice(part);
        }
        bytes
    }
}

/// A signed statement, as `ClientMessage::Prove` carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    /// The Ed25519 public key, as base64url of its 32 bytes, unpadded.
    pub public_key: String,
    /// The signature, as base64url of its 64 bytes, unpadded.
    pub signature: String,
}

/// Why a proof failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum Failure {
    /// The key or the signature isn't unpadded base64url of the right
    /// length.
    #[error("the key or the signature isn't Ed25519")]
    Malformed,
    /// The signature isn't the key's over this statement.
    #[error("the signature doesn't match")]
    Mismatch,
}

/// Signs `statement` with the Ed25519 key whose seed is `seed`. None only
/// when ring turns the seed down.
pub fn sign(seed: &[u8; 32], statement: &Statement) -> Option<Proof> {
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).ok()?;
    Some(Proof {
        public_key: URL_SAFE_NO_PAD.encode(pair.public_key()),
        signature: URL_SAFE_NO_PAD.encode(pair.sign(&statement.signed_bytes())),
    })
}

/// Checks that `signature` is `public_key`'s over `statement`, both as
/// [`Proof`] carries them. Gives the key's 32 bytes when it is.
pub fn verify(
    public_key: &str,
    signature: &str,
    statement: &Statement,
) -> Result<[u8; 32], Failure> {
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public_key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Failure::Malformed)?;
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signature)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Failure::Malformed)?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&statement.signed_bytes(), &signature)
        .map_err(|_| Failure::Mismatch)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    const SEED: [u8; 32] = [7; 32];

    fn statement() -> Statement {
        Statement {
            kind: Kind::App,
            device: DeviceId(Uuid::from_u128(0x0199a7c2_0000_7000_8000_000000000001)),
            instance: InstanceId(Uuid::from_u128(0x0199a7c2_0000_7000_8000_000000000002)),
            client: ClientId(Uuid::from_u128(0x0199a7c2_0000_7000_8000_000000000003)),
            challenge: Challenge(std::array::from_fn(|at| at as u8)),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Fixes the signed bytes and the signature, so the app, the phone and
    /// `maxd` keep agreeing on them across releases.
    #[test]
    fn a_golden_statement_signs_to_known_bytes() {
        let statement = statement();
        assert_eq!(
            hex(&statement.signed_bytes()),
            [
                "6d617868712d70726f76652d763100", // maxhq-prove-v1\0
                "00000003",
                "617070", // app
                "00000010",
                "0199a7c2000070008000000000000001", // the device
                "00000010",
                "0199a7c2000070008000000000000002", // the instance
                "00000010",
                "0199a7c2000070008000000000000003", // the client
                "00000020",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            ]
            .concat()
        );
        // OpenSSL's Ed25519 gives the same key and signature for this seed.
        let proof = sign(&SEED, &statement).unwrap();
        assert_eq!(
            proof,
            Proof {
                public_key: "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw".into(),
                signature: [
                    "xBj65UTC0Jdu1J0pemaVvDGMb99A6AZJy1eZA6MBCV2G",
                    "KplvCnSP4brmUsmiBpdLFXlEbLMLebM1ncB3Il1AAQ",
                ]
                .concat(),
            }
        );
        let key = verify(&proof.public_key, &proof.signature, &statement).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.encode(key), proof.public_key);
    }

    #[test]
    fn a_proof_for_another_connection_fails() {
        let statement = statement();
        let proof = sign(&SEED, &statement).unwrap();
        let others = [
            Statement {
                kind: Kind::Phone,
                ..statement
            },
            Statement {
                device: DeviceId::derive("other"),
                ..statement
            },
            Statement {
                instance: InstanceId::generate(),
                ..statement
            },
            Statement {
                client: ClientId::generate(),
                ..statement
            },
            Statement {
                challenge: Challenge([9; 32]),
                ..statement
            },
        ];
        for other in others {
            assert_ne!(other.signed_bytes(), statement.signed_bytes());
            assert_eq!(
                verify(&proof.public_key, &proof.signature, &other),
                Err(Failure::Mismatch)
            );
        }

        // Another key's signature fails too.
        let theirs = sign(&[8; 32], &statement).unwrap();
        assert_eq!(
            verify(&proof.public_key, &theirs.signature, &statement),
            Err(Failure::Mismatch)
        );
    }

    #[test]
    fn a_malformed_key_or_signature_is_refused() {
        let statement = statement();
        let proof = sign(&SEED, &statement).unwrap();
        let short = URL_SAFE_NO_PAD.encode([1u8; 31]);
        let padded = format!("{}=", proof.public_key);
        for (key, signature) in [
            ("", proof.signature.as_str()),
            (short.as_str(), proof.signature.as_str()),
            (padded.as_str(), proof.signature.as_str()),
            ("not base64!", proof.signature.as_str()),
            (proof.public_key.as_str(), ""),
            (proof.public_key.as_str(), short.as_str()),
        ] {
            assert_eq!(
                verify(key, signature, &statement),
                Err(Failure::Malformed),
                "{key:?} {signature:?}"
            );
        }
    }

    #[test]
    fn challenges_read_back_and_differ() {
        let challenge = Challenge::generate().unwrap();
        assert_ne!(challenge, Challenge::generate().unwrap());
        let text = challenge.encode();
        assert_eq!(text.len(), 43);
        assert_eq!(Challenge::parse(&text), Some(challenge));
        assert_eq!(
            Challenge(std::array::from_fn(|at| at as u8)).encode(),
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
        );
        assert_eq!(Challenge::parse(&URL_SAFE_NO_PAD.encode([1u8; 31])), None);
        assert_eq!(Challenge::parse("not base64!"), None);
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who may control a device's `maxd`. A connection proves it holds a key by
//! signing the challenge in [`crate::message::ServerHello::challenge`] and
//! sending [`crate::ClientMessage::Prove`]. The device publishes an
//! [`AppKey`] for each app key it trusts, and an [`Admission`] while it asks
//! whether to let in a program that proved no key it trusts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AdmissionId, AppKeyId, DeviceId};
use crate::time::Timestamp;
use crate::timeline::Question;

/// An app key a device trusts. A connection that proves it holds one may
/// do everything the app does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AppKey {
    /// From [`AppKey::id_for`].
    pub id: AppKeyId,
    /// The device that trusts it.
    pub device: DeviceId,
    /// The Ed25519 public key, as base64url of its 32 bytes, unpadded.
    pub public_key: String,
    /// What the key is called, such as "MaxHQ on studio-mac".
    pub label: String,
    /// When the device started trusting it.
    pub added_at: Timestamp,
    /// How the device came to trust it. One from a newer `maxd` reads as
    /// unknown, so the key still shows.
    #[serde(deserialize_with = "crate::tolerant::or_unknown")]
    #[specta(type = AppKeyOrigin)]
    pub origin: AppKeyOrigin,
    /// When a connection last proved it holds the key, or none since the
    /// device started trusting it.
    pub proved_at: Option<Timestamp>,
}

impl AppKey {
    /// The ID of `public_key` on `device`. The same key on another device
    /// gets another ID.
    pub fn id_for(device: DeviceId, public_key: &str) -> AppKeyId {
        AppKeyId::derive(&format!("{device}:{public_key}"))
    }
}

/// How a device came to trust an app key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppKeyOrigin {
    /// The Mac checked the code signature of the app that offered it.
    Signature,
    /// Setup sent it along with MaxHQ's SSH key.
    Setup,
    /// It was the first key offered while the device trusted none.
    First,
    /// You allowed it from a card on another device.
    Approved,
    /// An app the device already trusted added it, such as when it changed
    /// its key.
    App,
    /// A way from a newer `maxd`.
    Unknown,
}

/// A card that asks whether to let in a program that connected without a
/// key the device trusts, and asked for more than such a connection may
/// do. A proven desktop app answers it with
/// [`crate::ClientMessage::AnswerAdmission`]. A yes lasts until the program
/// disconnects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Admission {
    /// The card.
    pub id: AdmissionId,
    /// The device the program connected to.
    pub device: DeviceId,
    /// What the card asks, with its consent laid out. The consent's facts
    /// describe the connection: what it says it is, how it reached `maxd`,
    /// and on a Mac the program behind it.
    pub question: Question,
    /// When the device raised the card.
    pub raised_at: Timestamp,
    /// When an unanswered card counts as Don't allow.
    pub expires_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::ids::{BootId, IdempotencyKey};
    use crate::message::{ActionOutcome, Capabilities, ServerHello, APP_KEYS_SINCE};
    use crate::state::{Entity, EntityKey};
    use crate::timeline::{Consent, ConsentFact, QuestionOption};
    use crate::{ClientMessage, ProtocolRange};

    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";
    const DEVICE: &str = "0199a7c2-0000-7000-8000-000000000007";
    const CARD: &str = "0199a7c2-0000-7000-8000-0000000000c1";
    const PUBLIC: &str = "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw";

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) -> Value {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(&serde_json::from_value::<T>(json.clone()).unwrap(), value);
        json
    }

    fn device() -> DeviceId {
        DEVICE.parse().unwrap()
    }

    #[test]
    fn proofs_keys_and_answers_read_back_and_need_a_newer_maxd() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let messages = [
            (
                ClientMessage::Prove {
                    key,
                    public_key: PUBLIC.into(),
                    signature: "c2lnbmF0dXJl".into(),
                },
                json!({
                    "type": "prove",
                    "key": KEY,
                    "public_key": PUBLIC,
                    "signature": "c2lnbmF0dXJl",
                }),
            ),
            (
                ClientMessage::EnrollAppKey {
                    key,
                    label: "MaxHQ on studio-mac".into(),
                },
                json!({ "type": "enroll_app_key", "key": KEY, "label": "MaxHQ on studio-mac" }),
            ),
            (
                ClientMessage::RemoveAppKey {
                    key,
                    public_key: PUBLIC.into(),
                },
                json!({ "type": "remove_app_key", "key": KEY, "public_key": PUBLIC }),
            ),
            (
                ClientMessage::AnswerAdmission {
                    key,
                    admission: CARD.parse().unwrap(),
                    allow: false,
                },
                json!({ "type": "answer_admission", "key": KEY, "admission": CARD, "allow": false }),
            ),
        ];
        for (message, wire) in messages {
            assert_eq!(round_trip(&message), wire);
            assert_eq!(message.since(), APP_KEYS_SINCE);
            assert_eq!(APP_KEYS_SINCE, 34);
            assert!(!message.reaches(Some(33)), "an older maxd never gets it");
            assert!(message.reaches(Some(34)));
        }
    }

    #[test]
    fn a_hello_carries_a_challenge_and_one_from_before_reads_without() {
        let mut hello = ServerHello {
            protocol: ProtocolRange::OURS,
            maxd_version: "0.1.0".into(),
            device: device(),
            boot_id: BootId("4f1c".into()),
            instance: "0199a7c2-0000-7000-8000-0000000000d7".parse().unwrap(),
            capabilities: Capabilities::default(),
            build: None,
            target: None,
            challenge: Some("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8".into()),
        };
        let value = round_trip(&hello);
        assert_eq!(
            value["challenge"],
            json!("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8")
        );

        // Without one, it's left out, as an older maxd leaves it out, and
        // reads back as none.
        hello.challenge = None;
        assert!(round_trip(&hello).get("challenge").is_none());
    }

    #[test]
    fn asked_reads_back_and_takes_fields_added_later() {
        assert_eq!(
            round_trip(&ActionOutcome::Asked),
            json!({ "status": "asked" })
        );
        let later: ActionOutcome =
            serde_json::from_value(json!({ "status": "asked", "admission": CARD })).unwrap();
        assert_eq!(later, ActionOutcome::Asked);
    }

    #[test]
    fn app_keys_and_admissions_read_back_under_their_keys() {
        let app_key = AppKey {
            id: AppKey::id_for(device(), PUBLIC),
            device: device(),
            public_key: PUBLIC.into(),
            label: "MaxHQ on studio-mac".into(),
            added_at: Timestamp(1_762_600_000_000),
            origin: AppKeyOrigin::Signature,
            proved_at: Some(Timestamp(1_762_600_000_500)),
        };
        let entity = Entity::AppKey(app_key.clone());
        let value = round_trip(&entity);
        assert_eq!(value["entity"], json!("app_key"));
        assert_eq!(value["origin"], json!("signature"));
        assert_eq!(entity.key(), EntityKey::AppKey(app_key.id));
        assert_eq!(
            round_trip(&entity.key()),
            json!({ "entity": "app_key", "id": app_key.id.to_string() })
        );

        let admission = Entity::Admission(Admission {
            id: CARD.parse().unwrap(),
            device: device(),
            question: Question {
                id: "admission".into(),
                header: None,
                text: "Let this program control devbox?".into(),
                options: vec![
                    QuestionOption {
                        label: "Don't allow".into(),
                        description: None,
                    },
                    QuestionOption {
                        label: "Allow until it disconnects".into(),
                        description: None,
                    },
                ],
                multi: false,
                other: false,
                secret: false,
                consent: Some(Consent {
                    title: "Let this program control devbox?".into(),
                    risk: "A program connected without MaxHQ's app key.".into(),
                    changes: vec![],
                    facts: vec![ConsentFact {
                        label: "Says it is".into(),
                        value: "maxhq-desktop 0.9.0".into(),
                    }],
                    rows: vec![],
                    remember: None,
                    edit: None,
                }),
            },
            raised_at: Timestamp(1_762_600_000_000),
            expires_at: Timestamp(1_762_600_600_000),
        });
        let value = round_trip(&admission);
        assert_eq!(value["entity"], json!("admission"));
        assert_eq!(
            value["question"]["consent"]["facts"][0]["label"],
            "Says it is"
        );
        assert_eq!(
            round_trip(&admission.key()),
            json!({ "entity": "admission", "id": CARD })
        );
    }

    #[test]
    fn an_origin_this_build_cant_read_keeps_the_key() {
        let read: AppKey = serde_json::from_value(json!({
            "id": AppKey::id_for(device(), PUBLIC),
            "device": DEVICE,
            "public_key": PUBLIC,
            "label": "MaxHQ on studio-mac",
            "added_at": 1_762_600_000_000_u64,
            "origin": "teleported",
            "proved_at": null,
        }))
        .unwrap();
        assert_eq!(read.origin, AppKeyOrigin::Unknown);
    }

    #[test]
    fn app_key_ids_depend_on_the_device_and_the_key() {
        let id = AppKey::id_for(device(), PUBLIC);
        assert_eq!(id, AppKey::id_for(device(), PUBLIC));
        assert_ne!(id, AppKey::id_for(DeviceId::derive("other"), PUBLIC));
        assert_ne!(id, AppKey::id_for(device(), "another-key"));
    }
}

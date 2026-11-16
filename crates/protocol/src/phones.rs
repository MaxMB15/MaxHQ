// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Phones. A phone pairs through one device, which keeps its [`PhoneBook`]
//! of the devices it may reach. A phone can pair with several Macs, each
//! with a book of its own. Each device that lets the phone's key in
//! publishes a [`Phone`], and pushes to it with the [`PushToken`] the phone
//! sends over its own connection.
//!
//! The phone's key on a device runs `maxd proxy --phone <phone>`, and the
//! one-time pairing key runs `maxd proxy --pair <pair>`. Each sends a first
//! frame of its own, [`crate::ClientMessage::Phone`] or
//! [`crate::ClientMessage::Pair`], before the phone's bytes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{DeviceId, PhoneId};
use crate::netaccess::PhoneNetwork;
use crate::secret::Secret;
use crate::time::Timestamp;

/// A paired phone, as one device that lets its key in has it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Phone {
    /// The phone.
    pub id: PhoneId,
    /// The device that publishes this.
    pub device: DeviceId,
    /// Your name for the phone, such as "iPhone".
    pub name: String,
    /// The phone's own SSH key, as `ssh-ed25519 <base64>`.
    pub ssh_public: String,
    /// The phone's X25519 key that pushes are encrypted to, as base64 of
    /// its 32 bytes.
    pub push_public: String,
    /// When the phone paired.
    pub paired_at: Timestamp,
    /// The kinds this device pushes to the phone. Empty until the phone
    /// sends a token.
    pub push: Vec<PushKind>,
    /// Whether the phone may do what only the Mac does here, short of
    /// letting other keys in. Only the Mac turns it on.
    #[serde(default)]
    pub everything: bool,
}

/// The devices a phone may reach, as the device it paired through keeps
/// them. A change reaches the phone as this entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneBook {
    /// The phone.
    pub phone: PhoneId,
    /// The device it paired through, which keeps the book.
    pub device: DeviceId,
    /// Every device the phone may reach, in your order.
    pub devices: Vec<PhoneBookEntry>,
    /// The Mac that made the book, which the phone names it by. A book
    /// from an older Mac leaves it out, and the phone names the keeper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<PhoneMac>,
}

/// The Mac that made a [`PhoneBook`]. It is often not the keeper, which
/// can be any device the Mac paired the phone through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneMac {
    /// The Mac's own device.
    pub device: DeviceId,
    /// Its name, such as "Home Mac".
    pub name: String,
}

/// One device in a [`PhoneBook`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneBookEntry {
    /// The device.
    pub device: DeviceId,
    /// Your name for it.
    pub name: String,
    /// Where the phone dials it, in the order it tries them. The Mac
    /// resolves SSH aliases first, since the phone has no SSH config.
    pub routes: Vec<PhoneRoute>,
    /// The device's SSH host key, as `ssh-ed25519 <base64>`. The phone
    /// refuses a host that shows another.
    pub host_key: String,
    /// How the phone reaches the device over the network, when its
    /// network access is on. A book from before leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<PhoneNetwork>,
}

/// An SSH address a phone dials, with no alias or jump host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneRoute {
    /// The user to log in as, such as `dev`.
    pub user: String,
    /// A host name or address, such as `devbox.local`.
    pub host: String,
    /// The SSH port.
    pub port: u16,
}

/// Where and what a device pushes to a phone. The phone sends it to every
/// device it links to, and again when anything in it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PushToken {
    /// The APNs device token, as hex.
    pub token: Secret,
    /// The app's bundle ID, which APNs takes as the topic, such as
    /// `com.example.maxhq`.
    pub topic: String,
    /// Which APNs host the token belongs to.
    pub environment: PushEnvironment,
    /// What the phone wants pushed. Empty turns pushes off.
    pub kinds: Vec<PushKind>,
}

/// The APNs host a token belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushEnvironment {
    /// The sandbox host, for development builds.
    Sandbox,
    /// The production host, for TestFlight and App Store builds.
    Production,
}

/// What a push can be about. Each one is a switch in the phone's settings.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Type,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    /// An agent waits on you.
    NeedsInput,
    /// An agent stopped with an error.
    Error,
    /// An agent finished a turn.
    Finished,
    /// An agent hit a usage limit.
    Limit,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_book_and_a_token_read_back() {
        let book = PhoneBook {
            phone: PhoneId::derive("phone"),
            device: DeviceId::derive("devbox"),
            devices: vec![PhoneBookEntry {
                device: DeviceId::derive("devbox"),
                name: "devbox".into(),
                routes: vec![PhoneRoute {
                    user: "dev".into(),
                    host: "devbox.local".into(),
                    port: 22,
                }],
                host_key: "ssh-ed25519 AAAAexample".into(),
                network: None,
            }],
            mac: None,
        };
        let value = serde_json::to_value(&book).unwrap();
        assert_eq!(
            value["devices"][0]["routes"][0],
            json!({ "user": "dev", "host": "devbox.local", "port": 22 })
        );
        assert!(value["devices"][0].get("network").is_none());
        assert!(value.get("mac").is_none());
        assert_eq!(serde_json::from_value::<PhoneBook>(value).unwrap(), book);

        // A book that names its Mac reads back.
        let named = PhoneBook {
            mac: Some(PhoneMac {
                device: DeviceId::derive("mac"),
                name: "Home Mac".into(),
            }),
            ..book.clone()
        };
        let value = serde_json::to_value(&named).unwrap();
        assert_eq!(value["mac"]["name"], json!("Home Mac"));
        assert_eq!(serde_json::from_value::<PhoneBook>(value).unwrap(), named);

        // A book with a network part reads back, and its code stays out
        // of logs.
        let mut entry = book.devices[0].clone();
        entry.network = Some(PhoneNetwork {
            hosts: vec!["192.168.1.20".into()],
            port: 7466,
            pin: "pin".into(),
            code: Some(Secret::new("test-not-a-real-code".into())),
        });
        assert!(!format!("{entry:?}").contains("test-not-a-real-code"));
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["network"]["code"], json!("test-not-a-real-code"));
        assert_eq!(
            serde_json::from_value::<PhoneBookEntry>(value).unwrap(),
            entry
        );

        let token = PushToken {
            token: Secret::new("test-not-a-real-token".into()),
            topic: "com.example.maxhq".into(),
            environment: PushEnvironment::Sandbox,
            kinds: vec![PushKind::NeedsInput, PushKind::Limit],
        };
        let printed = format!("{token:?}");
        assert!(!printed.contains("test-not-a-real-token"), "{printed}");
        let value = serde_json::to_value(&token).unwrap();
        assert_eq!(value["environment"], json!("sandbox"));
        assert_eq!(value["kinds"], json!(["needs_input", "limit"]));
        assert_eq!(serde_json::from_value::<PushToken>(value).unwrap(), token);
    }

    #[test]
    fn a_phone_from_before_the_switch_reads_off() {
        let phone: Phone = serde_json::from_value(json!({
            "id": PhoneId::derive("phone"),
            "device": DeviceId::derive("devbox"),
            "name": "iPhone",
            "ssh_public": "ssh-ed25519 AAAAexample",
            "push_public": "cHVzaA==",
            "paired_at": 1,
            "push": [],
        }))
        .unwrap();
        assert!(!phone.everything);
    }
}

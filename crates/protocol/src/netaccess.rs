// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Network access. With its switch on, a device's `maxd` listens on its
//! private addresses for phones, over TLS with its own certificate, which
//! the phone pins at pairing. A phone trades a one-time code, or its
//! pairing, for a token bound to its key, and opens a WebSocket that
//! carries the same bytes `maxd proxy --phone` does.
//!
//! Every request carries a DPoP proof made with the phone's own SSH key,
//! in the `DPoP` header. A request with a token carries it as
//! `Authorization: DPoP <token>` too.
//!
//! - `POST /pair` takes a [`NetPair`] and answers a [`NetPaired`].
//! - `POST /token` takes a [`NetGrant`] and answers a [`NetToken`].
//! - `GET /link` upgrades to the WebSocket, whose binary messages carry
//!   the link's bytes.
//!
//! A refusal is a [`NetRefusal`], with the status [`NetError::status`]
//! gives.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{DeviceId, PairId, PhoneId, PhoneSessionId};
use crate::phones::PhoneBookEntry;
use crate::secret::Secret;
use crate::time::Timestamp;

/// The port a device listens on unless you pick another.
pub const DEFAULT_PORT: u16 = 7466;
/// How long a token works.
pub const TOKEN_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A phone refreshes its token on connect once less than this is left.
pub const REFRESH_WITHIN: Duration = Duration::from_secs(15 * 24 * 60 * 60);
/// How long a one-time code from `AddPhone` works. The Mac makes a new one
/// when you ask, for a phone that has no token yet.
pub const CODE_FOR: Duration = Duration::from_secs(10 * 60);
/// The largest request body `maxd` reads.
pub const MAX_BODY: usize = 16 * 1024;
/// The largest WebSocket message or frame either end of a link takes.
/// Both ends send at most 64 KiB at a time.
pub const MAX_LINK_MESSAGE: usize = 1024 * 1024;
/// The paths.
pub const PAIR_PATH: &str = "/pair";
/// See [`PAIR_PATH`].
pub const TOKEN_PATH: &str = "/token";
/// See [`PAIR_PATH`].
pub const LINK_PATH: &str = "/link";

/// Network access in a device's settings. Off unless you turn it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NetworkAccessSettings {
    /// Whether phones may reach this device over the network.
    #[serde(default)]
    pub on: bool,
    /// The TCP port it listens on.
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for NetworkAccessSettings {
    fn default() -> Self {
        Self {
            on: false,
            port: DEFAULT_PORT,
        }
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

/// How a device's network access stands now. The app builds the phone's
/// network routes from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NetworkAccess {
    /// The device.
    pub device: DeviceId,
    /// Whether the switch is on.
    pub on: bool,
    /// The port from the settings.
    pub port: u16,
    /// The private addresses it listens on now, such as `192.168.1.20`
    /// or `fd7a:115c:a1e0::1`. Empty while it's off.
    pub hosts: Vec<String>,
    /// The pin of its TLS certificate: base64url, without padding, of the
    /// SHA-256 of the certificate's DER. None when it has none.
    pub pin: Option<String>,
    /// What keeps it from listening, such as a port in use, as a sentence
    /// to show.
    pub problem: Option<String>,
}

/// A token one device holds for one phone, until it expires or you
/// revoke it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneSession {
    /// The session. It keeps its ID when the phone refreshes its token.
    pub id: PhoneSessionId,
    /// The device that holds it.
    pub device: DeviceId,
    /// The phone.
    pub phone: PhoneId,
    /// When the phone first got a token.
    pub created: Timestamp,
    /// When the current token stops working.
    pub expires: Timestamp,
    /// When the phone last used it, if it has.
    pub last_used: Option<Timestamp>,
}

/// How a phone reaches one device of its book over the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PhoneNetwork {
    /// The addresses it tries, in order.
    pub hosts: Vec<String>,
    /// The port.
    pub port: u16,
    /// The pin of the device's TLS certificate, as [`NetworkAccess::pin`].
    pub pin: String,
    /// A one-time code bound to the phone, to trade at `POST /token`.
    /// The book drops it once the phone has a token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Secret>,
}

/// The body of `POST /pair`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPair {
    /// The pairing from the QR code.
    pub pair: PairId,
    /// The one-time key's 32-byte seed from the QR code, as base64.
    pub seed: Secret,
    /// Your name for the phone.
    pub name: String,
    /// The phone's own SSH key, as `ssh-ed25519 <base64>`. The proof is
    /// made with it.
    pub ssh_public: String,
    /// The phone's X25519 push key, as base64 of its 32 bytes.
    pub push_public: String,
}

/// The answer to `POST /pair`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPaired {
    /// The phone's ID.
    pub phone: PhoneId,
    /// The devices it may reach.
    pub book: Vec<PhoneBookEntry>,
    /// Its token for this device.
    pub token: NetToken,
}

/// The body of `POST /token`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "grant", rename_all = "snake_case")]
pub enum NetGrant {
    /// Trades a one-time code from the book. The proof is made with the
    /// key of the phone the code is bound to.
    Code {
        /// The code.
        code: Secret,
    },
    /// Swaps the token in `Authorization` for a new one. The old one stops
    /// working once the new one is made, and keeps working if it isn't.
    Refresh,
}

/// A token, from `POST /pair` or `POST /token`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetToken {
    /// The token, for `Authorization: DPoP <token>`.
    pub token: Secret,
    /// The session it belongs to.
    pub session: PhoneSessionId,
    /// When it stops working.
    pub expires_at: Timestamp,
}

/// Every refusal's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRefusal {
    /// Why.
    pub error: NetError,
}

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetError {
    /// The token or code is past its time. The phone pairs again.
    Expired,
    /// The token is unknown, revoked or refreshed already. The phone pairs
    /// again.
    Revoked,
    /// The DPoP proof is missing, broken, replayed, or made with another
    /// key.
    BadProof,
    /// The code or the pairing is wrong, used or over.
    BadCode,
    /// The phone isn't paired with this device.
    NotPaired,
    /// The body or the headers aren't what the path takes.
    BadRequest,
    /// No such path.
    NotFound,
    /// `maxd` couldn't take the link now. The phone tries again later.
    Unavailable,
}

impl NetError {
    /// The HTTP status that goes with it.
    pub fn status(self) -> u16 {
        match self {
            Self::Expired | Self::Revoked | Self::BadProof => 401,
            Self::BadCode | Self::NotPaired => 403,
            Self::BadRequest => 400,
            Self::NotFound => 404,
            Self::Unavailable => 503,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn old_settings_read_as_off_and_the_bodies_have_their_shapes() {
        let settings: NetworkAccessSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(settings, NetworkAccessSettings::default());
        assert_eq!(settings.port, 7466);

        let grant = serde_json::to_value(NetGrant::Code {
            code: Secret::new("test-not-a-real-code".into()),
        })
        .unwrap();
        assert_eq!(
            grant,
            json!({ "grant": "code", "code": "test-not-a-real-code" })
        );
        assert_eq!(
            serde_json::to_value(NetGrant::Refresh).unwrap(),
            json!({ "grant": "refresh" })
        );
        let refusal = NetRefusal {
            error: NetError::BadProof,
        };
        assert_eq!(
            serde_json::to_value(refusal).unwrap(),
            json!({ "error": "bad_proof" })
        );
        assert_eq!(NetError::Revoked.status(), 401);
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a connection may do before it proves it's the app, and the
//! requests that prove it and change the keys the device trusts.
//!
//! The hello carries a challenge. The app signs it, with the device, this
//! run of `maxd` and its own client ID, and sends `Prove`. With a key the
//! device trusts, the connection may do everything. Without one it may do
//! only what [`open`] lets through, which grants nothing: it can keep a
//! guest's SSH pipe open, or ask the device to trust the key it showed. A
//! program running as root may also wind the agents down, as `maxd host
//! quiesce` does at shutdown. Anything else asks for an admission card, and
//! your yes there lets it in until it disconnects.
//!
//! A phone's or a pairing's connection proves the same way, after the
//! first frame that names it: with the phone's SSH key, or with the
//! pairing's one-time key. A Phone or Pair frame on its own grants nothing.
//! A phone's card lets it in with what a phone may do. A pairing gets no
//! card, since only its one-time key lets a phone in. A Peer frame hands
//! the connection to the peer links, which take a proof with that device's
//! own key before they serve it.

use std::io;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_core::prove::{self, Kind, Statement};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::ClientMessage;
use tracing::{info, warn};

use super::Connection;
use crate::app_keys::{Asker, Decision, Removal};
use crate::codesign;

/// The answer when the app keys file can't be read or written.
pub(super) const KEYS_UNREAD: &str = "maxd couldn't read its app keys. Its log says why.";

/// Whether an unproven connection may send `message`. Nothing here reads
/// or changes state, starts work or lets anyone in: the editor's SSH pipe
/// to a guest still needs MaxHQ's SSH key, and a key only counts once the
/// device trusts it.
pub(super) fn open(message: &ClientMessage) -> bool {
    matches!(
        message,
        ClientMessage::Ping { .. }
            | ClientMessage::Prove { .. }
            | ClientMessage::EnrollAppKey { .. }
            | ClientMessage::GuestSshOpen { .. }
            | ClientMessage::TunnelCredit { .. }
            | ClientMessage::TunnelClose { .. }
    )
}

/// Whether a program running as root may send `message` unproven. That's
/// `maxd host quiesce`, which the system runs as root as it shuts down, to
/// wind the agents down. Root can stop `maxd` or change its files anyway,
/// so this lets it do nothing it couldn't.
pub(super) fn open_to_root(message: &ClientMessage) -> bool {
    open(message) || matches!(message, ClientMessage::WindDown { .. })
}

/// The refusal for anything else an unproven connection asks `device` for.
pub(super) fn unproven(device: &str) -> String {
    format!(
        "This program hasn't shown {device} that it's MaxHQ, so {device} won't do that. If it's \
         an older MaxHQ, update it, or allow it from the card on another device."
    )
}

pub(super) fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// Runs `f`, which reads or writes the app keys file, off the async
/// threads.
async fn on_file<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|err| Err(io::Error::other(err)))
}

impl Connection {
    /// Whether this connection may ask for more than [`open`] lets
    /// through: it proved an app key, or the key its Phone or Pair frame
    /// named, you let it in from its admission card, or the gate leaves it
    /// to its own rules, as `maxd mcp`, whose agent key is its proof. A
    /// phone let in from its card still may do only what a phone may.
    pub(super) fn past_the_gate(&self) -> bool {
        self.app.is_some() || self.frame_proven || self.seat.admitted() || self.mcp
    }

    /// Checks a proof of the key `public_key`: an app key, or after a Phone
    /// or Pair frame the key that frame names. A good signature with an
    /// app key the device doesn't trust is refused, but the connection
    /// holds the key from then on, so `EnrollAppKey` can ask for it. A
    /// proven connection stays proven whatever a later proof says.
    pub(super) async fn prove(&mut self, public_key: &str, signature: &str) -> ActionOutcome {
        let (Some(challenge), Some(client)) = (self.challenge, self.client_id) else {
            return refused("maxd had no challenge for this connection. Its log says why.");
        };
        // A phone's or a pairing's connection never proves an app key.
        let kind = match (self.phone, self.pairs) {
            (Some(_), _) => Kind::Phone,
            (None, true) => Kind::Pair,
            (None, false) => Kind::App,
        };
        let statement = Statement {
            kind,
            device: self.hub.device().id,
            instance: self.hub.instance(),
            client,
            challenge,
        };
        let key = match prove::verify(public_key, signature, &statement) {
            Ok(key) => key,
            Err(err) => return refused(format!("Bad proof: {err}.")),
        };
        if kind != Kind::App {
            return self.prove_frame(key);
        }
        let key = URL_SAFE_NO_PAD.encode(key);
        self.held = Some(key.clone());
        // Listening before the check, so a removal right after it still
        // closes the connection.
        let revoked = self.keys.removed();
        let (keys, proved) = (self.keys.clone(), key.clone());
        match on_file(move || keys.proved(&proved)).await {
            Ok(true) => {
                self.now_proven(key, revoked);
                ActionOutcome::Done
            }
            Ok(false) => refused(format!(
                "Unknown key: {} doesn't trust this app key yet.",
                self.hub.device().name
            )),
            Err(err) => {
                warn!(%err, "couldn't check an app key");
                refused(KEYS_UNREAD)
            }
        }
    }

    /// Takes `key`, which signed this connection's challenge, when it's
    /// the key its first frame named: the phone's SSH key as the phones
    /// file holds it, or the one-time key of the pairing it took.
    fn prove_frame(&mut self, key: [u8; 32]) -> ActionOutcome {
        if let Some(phone) = self.phone {
            let named = crate::phones::installed()
                .and_then(|phones| phones.ssh_public(phone))
                .and_then(|line| crate::sshkey::ed25519_key(&line));
            if named != Some(key) {
                return refused(format!(
                    "Unknown key: {} doesn't know this phone's key.",
                    self.hub.device().name
                ));
            }
        } else if let Err(reason) = self.take_pairing(key) {
            return refused(reason);
        }
        self.frame_proven = true;
        // Its card, if it raised one, goes. Neither is the desktop app, so
        // the alert still shows for others.
        self.seat.proven(true);
        ActionOutcome::Done
    }

    /// Takes the pairing this connection's Pair frame named, now that it
    /// proved `key`, when that's the pairing's one-time key. A proof with
    /// the same key again, once taken, still holds.
    fn take_pairing(&mut self, key: [u8; 32]) -> Result<(), &'static str> {
        const UNKNOWN: &str = "Unknown key: that isn't this pairing's key.";
        let Some((pair, public)) = self.pair else {
            return Err(UNKNOWN);
        };
        if public != key {
            return Err(UNKNOWN);
        }
        if self.frame_proven {
            return Ok(());
        }
        let phones = crate::phones::installed().ok_or(crate::phones::NOT_YET)?;
        self.pairing = Some(phones.take(pair)?);
        Ok(())
    }

    /// Asks the device to trust the key this connection last proved it
    /// holds, as [`crate::app_keys::decide`] rules. A connection that
    /// hasn't proved a trusted key yet asks with what the Mac's signature
    /// check says about the program on the other end.
    pub(super) async fn enroll(&mut self, label: String) -> ActionOutcome {
        let Some(key) = self.held.clone() else {
            return refused("Prove the key first. This connection hasn't shown it holds one.");
        };
        let asker = if self.app.is_some() {
            Asker::App
        } else {
            let now = codesign::token(&self.reader);
            Asker::Connection(codesign::check(self.caller, now).await)
        };
        let revoked = self.keys.removed();
        let (keys, enrolled, asked) = (self.keys.clone(), key.clone(), label.clone());
        match on_file(move || keys.enroll(&enrolled, &label, asker)).await {
            Ok(decision @ (Decision::Trusted | Decision::Add(_))) => {
                if let Decision::Add(origin) = decision {
                    info!(?origin, %key, "trusts a new app key");
                }
                self.now_proven(key, revoked);
                ActionOutcome::Done
            }
            Ok(Decision::Ask) => self.ask_enrollment(key, asked, revoked),
            Err(err) => {
                warn!(%err, "couldn't add an app key");
                refused(KEYS_UNREAD)
            }
        }
    }

    /// Stops trusting `public_key`. Connections proven with it close. A key
    /// the device doesn't trust counts as removed, so a resend is safe.
    pub(super) async fn remove_app_key(&mut self, public_key: String) -> ActionOutcome {
        let keys = self.keys.clone();
        match on_file(move || keys.remove(&public_key)).await {
            Ok(Removal::Removed | Removal::Absent) => ActionOutcome::Done,
            Ok(Removal::Last) => refused(format!(
                "That's the only app key {} trusts, so it stays. Add another first.",
                self.hub.device().name
            )),
            Err(err) => {
                warn!(%err, "couldn't remove an app key");
                refused(KEYS_UNREAD)
            }
        }
    }
}

#[cfg(test)]
mod tests;

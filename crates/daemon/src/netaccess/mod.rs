// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Network access: with the device's switch on, phones reach `maxd` over
//! TLS on its private addresses, with no SSH. `maxhq_protocol::netaccess`
//! has the endpoints.
//!
//! A phone gets a token by pairing through `POST /pair`, or by trading a
//! one-time code from `AddPhone` at `POST /token`. Each token belongs to a
//! [`PhoneSession`], works for 30 days, and is bound to the phone's own
//! SSH key: every request carries a DPoP proof made with it, and a proof
//! works once. `GET /link` then joins a WebSocket to `maxd`'s own socket as
//! that phone, as `maxd proxy --phone` does.
//!
//! The database keeps only the SHA-256 of each token and code. Revoking a
//! session or removing the phone ends it, and closes its link.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_core::dpop;
use maxhq_protocol::ids::{DeviceId, PhoneId, PhoneSessionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::netaccess::{
    NetError, NetToken, NetworkAccessSettings, PhoneSession, CODE_FOR, TOKEN_FOR,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::Secret;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::agents::hash;
use crate::db::Db;
use crate::hub::Hub;
use crate::session::clock::Clock;

pub mod addrs;
pub mod listener;
mod store;
pub mod tls;

#[cfg(test)]
mod tests;

/// The folder under `maxd`'s data folder that holds the certificate.
pub const DIR: &str = "netaccess";

/// How long a proof's ID is kept, to refuse it again: twice the window a
/// proof works in, either side of now.
const REPLAY_FOR: Duration = Duration::from_secs(2 * dpop::MAX_CLOCK_SKEW as u64);

/// The tokens, codes and proofs of network access. Clones share them.
#[derive(Clone)]
pub struct Access(Arc<Inner>);

struct Inner {
    hub: Hub,
    db: Db,
    clock: Arc<dyn Clock>,
    /// Each proof seen lately, by its key's thumbprint and its ID.
    proofs: Mutex<HashMap<(String, String), Instant>>,
    /// Each session that ended, so its link closes.
    ended: broadcast::Sender<PhoneSessionId>,
}

impl Access {
    /// Starts with the sessions `db` keeps, drops those that are over, and
    /// publishes the rest.
    pub fn open(hub: Hub, db: Db, clock: Arc<dyn Clock>) -> Self {
        let me = hub.device().id;
        if let Err(err) = db.drop_expired_phone_access(clock.wall()) {
            warn!(%err, "couldn't drop the phone sessions that are over");
        }
        match db.phone_sessions(me) {
            Ok(sessions) => hub.apply(sessions.into_iter().map(|session| Change::Upsert {
                value: Entity::PhoneSession(session),
            })),
            Err(err) => warn!(%err, "couldn't read the phone sessions"),
        }
        Self(Arc::new(Inner {
            hub,
            db,
            clock,
            proofs: Mutex::new(HashMap::new()),
            ended: broadcast::channel(16).0,
        }))
    }

    fn me(&self) -> DeviceId {
        self.0.hub.device().id
    }

    /// The hub the listener publishes on.
    pub fn hub(&self) -> &Hub {
        &self.0.hub
    }

    /// The clock the listener rechecks with.
    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.0.clock
    }

    /// The switch and port, as this device's settings have them.
    pub fn settings(&self) -> NetworkAccessSettings {
        match self.0.hub.get(&EntityKey::DeviceSettings(self.me())) {
            Some(Entity::DeviceSettings(settings)) => settings.network,
            _ => NetworkAccessSettings::default(),
        }
    }

    /// Each session that ends from now on.
    pub fn ended(&self) -> broadcast::Receiver<PhoneSessionId> {
        self.0.ended.subscribe()
    }

    /// Whether `session` still works.
    pub fn live(&self, session: PhoneSessionId) -> bool {
        self.0.db.has_phone_session(session).unwrap_or(false)
    }

    /// What `AddPhone` answers once `phone` is let in: a one-time code
    /// bound to it while the switch is on, or done.
    pub fn added(&self, phone: PhoneId) -> ActionOutcome {
        if !self.settings().on {
            return ActionOutcome::Done;
        }
        let Some(code) = random() else {
            return refused("This device gave no random bytes for the phone's code.");
        };
        let expires = self.0.clock.wall().after(CODE_FOR);
        if let Err(err) = self.0.db.put_phone_code(&hash(&code), phone, expires) {
            return refused(format!("This device couldn't keep the phone's code: {err}"));
        }
        ActionOutcome::PhoneAdded {
            code: Secret::new(code),
        }
    }

    /// Ends every session of `phone` and drops its codes, as removing it
    /// does.
    pub fn forget(&self, phone: PhoneId) -> Result<(), String> {
        let ended = self
            .0
            .db
            .forget_phone_access(phone)
            .map_err(|err| format!("This device couldn't end the phone's sessions: {err}"))?;
        self.close(ended);
        Ok(())
    }

    /// Answers `RevokePhoneSession`. A session that isn't here is done
    /// already.
    pub fn revoke(&self, session: PhoneSessionId) -> ActionOutcome {
        match self.0.db.remove_phone_session(session) {
            Ok(_) => {
                self.close(vec![session]);
                ActionOutcome::Done
            }
            Err(err) => refused(format!("This device couldn't end the session: {err}")),
        }
    }

    /// Takes each of `sessions` out of the state and closes its link.
    fn close(&self, sessions: Vec<PhoneSessionId>) {
        if sessions.is_empty() {
            return;
        }
        for session in &sessions {
            info!(%session, "a phone session ended");
            let _ = self.0.ended.send(*session);
        }
        self.0
            .hub
            .apply(sessions.into_iter().map(|id| Change::Remove {
                key: EntityKey::PhoneSession(id),
            }));
    }

    /// Checks `proof` for `method` on `url`, made with `key` and naming
    /// `token` when there is one, and keeps its ID so it works once. A
    /// proof that names a certificate pin must name `pin`, this listener's.
    /// The caller finds `key` first, from what only a paired phone has, so
    /// a stranger's proofs never fill the list.
    fn proof(
        &self,
        proof: Option<&str>,
        method: &str,
        url: &str,
        key: &[u8; 32],
        token: Option<&str>,
        pin: &str,
    ) -> Result<(), NetError> {
        let thumbprint = dpop::thumbprint(key);
        let expected = dpop::Expected {
            method,
            url,
            now: i64::try_from(self.0.clock.wall().0 / 1000).unwrap_or(i64::MAX),
            thumbprint: Some(&thumbprint),
            access_token: token,
            pin: Some(pin),
        };
        let verified = dpop::verify(proof.unwrap_or_default(), &expected).map_err(|failure| {
            debug!(%failure, "refused a phone's proof");
            NetError::BadProof
        })?;
        let now = self.0.clock.now();
        let mut proofs = self.0.proofs.lock().unwrap_or_else(PoisonError::into_inner);
        proofs.retain(|_, seen| now.saturating_duration_since(*seen) < REPLAY_FOR);
        if proofs
            .insert((verified.thumbprint, verified.jti), now)
            .is_some()
        {
            debug!("refused a phone's proof it sent before");
            return Err(NetError::BadProof);
        }
        Ok(())
    }

    /// A new session for `phone`, which just paired.
    fn start(&self, phone: PhoneId) -> Result<NetToken, NetError> {
        let (session, token) = self.new_session(phone)?;
        self.0
            .db
            .put_phone_session(&session, &hash(token.expose()))
            .map_err(|err| store_failed(&err))?;
        Ok(self.started(session, token))
    }

    /// Trades `code` for a session, when `proof` is made with the key of
    /// the phone it's bound to.
    fn trade(
        &self,
        code: &Secret,
        proof: Option<&str>,
        url: &str,
        pin: &str,
    ) -> Result<NetToken, NetError> {
        let code = hash(code.expose());
        let found = self
            .0
            .db
            .phone_code(&code)
            .map_err(|err| store_failed(&err))?
            .ok_or(NetError::BadCode)?;
        let now = self.0.clock.wall();
        if found.expires <= now {
            return Err(NetError::Expired);
        }
        let key = phone_key(found.phone)?;
        self.proof(proof, "POST", url, &key, None, pin)?;
        let (session, token) = self.new_session(found.phone)?;
        let traded = self
            .0
            .db
            .trade_phone_code(&code, now, &session, &hash(token.expose()))
            .map_err(|err| store_failed(&err))?;
        if !traded {
            return Err(NetError::BadCode);
        }
        Ok(self.started(session, token))
    }

    /// The session `token` belongs to, when `proof` is made for it with
    /// its phone's key. A use notes when.
    fn session(
        &self,
        token: &str,
        proof: Option<&str>,
        method: &str,
        url: &str,
        pin: &str,
    ) -> Result<PhoneSession, NetError> {
        let mut session = self
            .0
            .db
            .phone_session(&hash(token), self.me())
            .map_err(|err| store_failed(&err))?
            .ok_or(NetError::Revoked)?;
        let now = self.0.clock.wall();
        if session.expires <= now {
            return Err(NetError::Expired);
        }
        let key = phone_key(session.phone)?;
        self.proof(proof, method, url, &key, Some(token), pin)?;
        if self.0.db.touch_phone_session(session.id, now).is_ok() {
            session.last_used = Some(now);
            self.publish(&session);
        }
        Ok(session)
    }

    /// Swaps `token` for a new one of the same session, in one step.
    fn refresh(
        &self,
        token: &str,
        proof: Option<&str>,
        url: &str,
        pin: &str,
    ) -> Result<NetToken, NetError> {
        let mut session = self.session(token, proof, "POST", url, pin)?;
        let new = random().map(Secret::new).ok_or(NetError::Unavailable)?;
        session.expires = self.0.clock.wall().after(TOKEN_FOR);
        let swapped = self
            .0
            .db
            .refresh_phone_session(
                session.id,
                &hash(token),
                &hash(new.expose()),
                session.expires,
            )
            .map_err(|err| store_failed(&err))?;
        if !swapped {
            return Err(NetError::Revoked);
        }
        Ok(self.started(session, new))
    }

    fn new_session(&self, phone: PhoneId) -> Result<(PhoneSession, Secret), NetError> {
        let token = random().map(Secret::new).ok_or(NetError::Unavailable)?;
        let now = self.0.clock.wall();
        let session = PhoneSession {
            id: PhoneSessionId::generate(),
            device: self.me(),
            phone,
            created: now,
            expires: now.after(TOKEN_FOR),
            last_used: Some(now),
        };
        Ok((session, token))
    }

    /// Publishes `session` and answers with `token` for it.
    fn started(&self, session: PhoneSession, token: Secret) -> NetToken {
        self.publish(&session);
        NetToken {
            token,
            session: session.id,
            expires_at: session.expires,
        }
    }

    fn publish(&self, session: &PhoneSession) {
        self.0.hub.apply([Change::Upsert {
            value: Entity::PhoneSession(session.clone()),
        }]);
    }
}

/// The SSH key of `phone`, which its proofs are made with.
fn phone_key(phone: PhoneId) -> Result<[u8; 32], NetError> {
    crate::phones::installed()
        .and_then(|phones| phones.ssh_public(phone))
        .and_then(|line| crate::sshkey::ed25519_key(&line))
        .ok_or(NetError::NotPaired)
}

/// 32 random bytes as base64url, for a token or a code.
fn random() -> Option<String> {
    let mut bytes = zeroize::Zeroizing::new([0u8; 32]);
    SystemRandom::new().fill(bytes.as_mut()).ok()?;
    Some(URL_SAFE_NO_PAD.encode(bytes.as_ref()))
}

fn store_failed(err: &rusqlite::Error) -> NetError {
    warn!(%err, "network access couldn't use the database");
    NetError::Unavailable
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// The service the free functions reach, once `maxd` has started it.
static ACCESS: OnceLock<Access> = OnceLock::new();

/// Makes `access` the service the free functions reach.
pub fn install(access: Access) {
    let _ = ACCESS.set(access);
}

/// Runs `f` on the service, or refuses on a device without it.
pub fn with(f: impl FnOnce(&Access) -> ActionOutcome) -> ActionOutcome {
    match ACCESS.get() {
        Some(access) => f(access),
        None => refused("Network access isn't set up on this device."),
    }
}

/// What `AddPhone` answers once `phone` is let in, as [`Access::added`].
pub fn added(phone: PhoneId) -> ActionOutcome {
    ACCESS
        .get()
        .map_or(ActionOutcome::Done, |access| access.added(phone))
}

/// Ends `phone`'s sessions and codes, as [`Access::forget`]. A device
/// without the service has none.
pub fn forget(phone: PhoneId) -> Result<(), String> {
    ACCESS.get().map_or(Ok(()), |access| access.forget(phone))
}

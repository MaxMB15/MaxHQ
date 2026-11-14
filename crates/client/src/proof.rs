// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A connection's proof that it holds a key, given right after `maxd`'s
//! hello and before anything else.
//!
//! A client is given a [`Prover`] for each kind of connection it opens,
//! through [`crate::Client::proving`]. When a hello carries a challenge,
//! the link hands the prover a [`Proving`], which signs with the core
//! helper and sends each proof, enrollment or removal as a keyed action,
//! the way every other action goes, and waits for its result. The
//! connection opens once the prover's verdict is [`Verdict::Proven`]. A
//! `maxd` whose hello has no challenge is older and takes no proof, so the
//! link opens at once, as it did before.
//!
//! [`crate::Client::reprove`] has each open connection prove again on the
//! same challenge, such as after the app makes a new key.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
pub use maxhq_core::prove::Kind;
use maxhq_core::prove::{self, Statement};
use maxhq_protocol::ids::{DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::ClientMessage;
use tokio::sync::{mpsc, Notify};

use crate::requests::{Book, Reply};
use crate::{LinkError, Waiting};

/// Proves one kind of connection to each `maxd` that asks.
pub trait Prover: Send + Sync + 'static {
    /// The kind of connection it proves.
    fn kind(&self) -> Kind;

    /// Proves the connection `proving` names. The future may outlive a
    /// dropped connection, whose [`Proving`] then fails with
    /// [`LinkError::NotConnected`].
    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict>;
}

/// How a proof ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The device takes the connection as proven, so it opens.
    Proven,
    /// The device turned the connection down. Carries what the app shows,
    /// such as "devbox doesn't trust this Mac's app key: unknown key".
    Refused(String),
    /// The connection dropped or went quiet before the device answered.
    Lost,
}

/// One connection's proof while it runs: what the signature names, and the
/// way to the device's `maxd`.
pub struct Proving {
    /// What a proof signs.
    statement: Statement,
    /// The link's name, for what the app shows.
    name: String,
    /// The connection's control queue.
    queue: mpsc::Sender<ClientMessage>,
    /// Where the results come in.
    book: Arc<Book>,
    /// How long an answer may take.
    wait: Duration,
    /// Woken when the device waits for a yes given somewhere else.
    asked: Arc<Notify>,
    /// The Sync that may go right behind the first proof.
    pipeline: Arc<Mutex<Pipeline>>,
}

/// The Sync a connection sends, and whether it went out behind a proof
/// the device took.
pub(crate) struct Pipeline {
    /// The Sync to send behind the first proof that asks for it, until it
    /// goes.
    pub sync: Option<ClientMessage>,
    /// The proof it went behind.
    behind: Option<IdempotencyKey>,
    /// Whether that proof was taken, so the Sync reached `maxd` proven.
    pub proven: bool,
}

impl Pipeline {
    /// A pipeline holding the connection's Sync, if it sends one.
    pub(crate) fn new(sync: Option<ClientMessage>) -> Self {
        Self {
            sync,
            behind: None,
            proven: false,
        }
    }
}

impl Proving {
    pub(crate) fn new(
        statement: Statement,
        name: String,
        queue: mpsc::Sender<ClientMessage>,
        book: Arc<Book>,
        wait: Duration,
        asked: Arc<Notify>,
        pipeline: Arc<Mutex<Pipeline>>,
    ) -> Self {
        Self {
            statement,
            name,
            queue,
            book,
            wait,
            asked,
            pipeline,
        }
    }

    /// The device the connection reached.
    pub fn device(&self) -> DeviceId {
        self.statement.device
    }

    /// The link's name, as the app shows it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The kind of connection being proven.
    pub fn kind(&self) -> Kind {
        self.statement.kind
    }

    /// Proves the connection holds the key whose seed is `seed`, and gives
    /// the device's answer: Done when it trusts the key, Refused with the
    /// reason otherwise.
    ///
    /// With `then_sync`, the link's Sync goes right behind the first proof
    /// that asks for it, so a device that takes the proof answers it with
    /// no extra wait. Ask only when the device is expected to trust the
    /// key, since `maxd` refuses a Sync from a connection it hasn't taken.
    /// Otherwise the link sends the Sync once the verdict is
    /// [`Verdict::Proven`].
    pub async fn prove(
        &self,
        seed: &[u8; 32],
        then_sync: bool,
    ) -> Result<ActionOutcome, LinkError> {
        let Some(proof) = prove::sign(seed, &self.statement) else {
            return Ok(ActionOutcome::Refused {
                reason: "this app couldn't sign with its key".to_owned(),
            });
        };
        let key = IdempotencyKey::generate();
        let message = ClientMessage::Prove {
            key,
            public_key: proof.public_key,
            signature: proof.signature,
        };
        let outcome = self.act(key, message, then_sync).await?;
        if outcome == ActionOutcome::Done {
            let mut pipeline = self.pipeline();
            if pipeline.behind == Some(key) {
                pipeline.proven = true;
            }
        }
        Ok(outcome)
    }

    /// Asks the device to trust the key this connection last proved, under
    /// `label`. While the device waits for a yes given somewhere else, the
    /// link shows that, and this waits for as long as it takes.
    pub async fn enroll(&self, label: &str) -> Result<ActionOutcome, LinkError> {
        let key = IdempotencyKey::generate();
        let message = ClientMessage::EnrollAppKey {
            key,
            label: label.to_owned(),
        };
        self.act(key, message, false).await
    }

    /// Asks the device to stop trusting the app key `public_key`.
    pub async fn remove(&self, public_key: &str) -> Result<ActionOutcome, LinkError> {
        let key = IdempotencyKey::generate();
        let message = ClientMessage::RemoveAppKey {
            key,
            public_key: public_key.to_owned(),
        };
        self.act(key, message, false).await
    }

    /// Sends a keyed action on this connection and waits for its result.
    /// It never goes again on another connection, since a proof names this
    /// one. An `Asked` wakes the link and waits on, with no time limit.
    async fn act(
        &self,
        key: IdempotencyKey,
        message: ClientMessage,
        then_sync: bool,
    ) -> Result<ActionOutcome, LinkError> {
        let (sender, mut results) = mpsc::unbounded_channel();
        self.book
            .lock()
            .wait(key, message.clone(), Reply::Proof(sender))?;
        // Stops waiting however this ends, including when the link drops
        // the future.
        let _waiting = Waiting {
            book: &self.book,
            key,
        };
        self.queue
            .send(message)
            .await
            .map_err(|_| LinkError::NotConnected)?;
        if then_sync {
            let sync = {
                let mut pipeline = self.pipeline();
                let sync = pipeline.sync.take();
                if sync.is_some() {
                    pipeline.behind = Some(key);
                }
                sync
            };
            if let Some(sync) = sync {
                self.queue
                    .send(sync)
                    .await
                    .map_err(|_| LinkError::NotConnected)?;
            }
        }
        let mut patient = false;
        loop {
            let next = if patient {
                results.recv().await
            } else {
                tokio::time::timeout(self.wait, results.recv())
                    .await
                    .map_err(|_| LinkError::TimedOut)?
            };
            match next {
                // The connection ended.
                None => return Err(LinkError::NotConnected),
                Some(ActionOutcome::Asked) => {
                    self.asked.notify_one();
                    patient = true;
                }
                Some(outcome) => return Ok(outcome),
            }
        }
    }

    fn pipeline(&self) -> std::sync::MutexGuard<'_, Pipeline> {
        self.pipeline.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The proof a link to another device gives once it's in: a signature
//! with the device's own SSH key, the one the other device let in. The
//! other `maxd` serves the link only once it checks that signature, so a
//! program there that writes a Peer frame of its own gets nothing.

use std::path::PathBuf;

use futures::future::BoxFuture;
use maxhq_client::proof::{Kind, Prover, Proving, Verdict};
use maxhq_protocol::message::ActionOutcome;
use tracing::{debug, warn};

use super::key;
use crate::secrets::files::Folder;

/// Proves each link this device dials with the key in `folder`, read at
/// each proof, so a new key counts from the next link on.
pub(super) struct PeerProver {
    pub folder: PathBuf,
}

impl Prover for PeerProver {
    fn kind(&self) -> Kind {
        Kind::Peer
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        let folder = self.folder.clone();
        Box::pin(async move {
            let seed = match Folder::open(folder).and_then(|folder| key::seed(&folder)) {
                Ok(Some(seed)) => seed,
                Ok(None) => {
                    return Verdict::Refused("this device has no SSH key to prove with".into())
                }
                Err(err) => {
                    warn!(%err, "couldn't read the device's key for a peer proof");
                    return Verdict::Refused("this device couldn't read its SSH key".into());
                }
            };
            match proving.prove(&seed, false).await {
                Ok(ActionOutcome::Done) => Verdict::Proven,
                Ok(ActionOutcome::Refused { reason }) => Verdict::Refused(format!(
                    "{} didn't take this device's key: {reason}",
                    proving.name()
                )),
                Ok(other) => Verdict::Refused(format!(
                    "{} answered the proof with {other:?}",
                    proving.name()
                )),
                Err(err) => {
                    debug!(%err, "a peer proof ended with its connection");
                    Verdict::Lost
                }
            }
        })
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pushing files on this device into a container, as its guest's credit
//! allows.

use maxhq_protocol::guest::HostToGuest;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::message::ErrorCode;
use maxhq_transfer::{Failure, Source, Walk};
use tracing::debug;

use super::disk::Pump;
use super::{Event, FromGuest, GuestLink, GuestStored, Task, STALL};

/// Why a push stopped short.
pub(super) enum Stop {
    /// It failed.
    Failed(Failure),
    /// The client gave up.
    Cancelled,
}

/// The failure for a guest that went away.
pub(super) fn unreachable() -> Failure {
    Failure::new(
        ErrorCode::Unreachable,
        None,
        "the workspace's container went away",
    )
}

/// Sends what `walk` found to the guest, and gives where it landed.
pub(super) async fn push(
    task: &mut Task,
    link: &GuestLink,
    agent: Option<AgentId>,
    walk: &Walk,
) -> Result<GuestStored, Stop> {
    let transfer = task.transfer;
    let down = || Stop::Failed(unreachable());
    let begin = HostToGuest::UploadBegin {
        transfer,
        agent,
        entries: walk.entries.clone(),
    };
    link.message(&begin).await.map_err(|_| down())?;
    let mut pump = Pump::new(Source::new(walk));
    // The first credit after a begin says where the guest picks up from.
    let mut fresh = true;
    loop {
        while !task.has_event() {
            match pump.next().await {
                Ok(Some(chunks)) => link.chunks(transfer, chunks).await.map_err(|_| down())?,
                Ok(None) => break,
                Err(failure) => {
                    link.cancel(transfer).await;
                    return Err(Stop::Failed(failure));
                }
            }
        }
        let Some(event) = task.next_within(STALL).await else {
            link.cancel(transfer).await;
            return Err(Stop::Failed(Failure::new(
                ErrorCode::Unreachable,
                None,
                "the workspace's container stopped answering",
            )));
        };
        match event {
            Event::Guest(FromGuest::Credit {
                committed,
                window_end,
            }) => {
                let taken = if fresh {
                    pump.resume(committed, window_end)
                } else {
                    pump.credit(committed, window_end)
                };
                fresh = false;
                if let Err(failure) = taken {
                    link.cancel(transfer).await;
                    return Err(Stop::Failed(failure));
                }
            }
            Event::Guest(FromGuest::Stored(stored)) => return Ok(stored),
            Event::Guest(FromGuest::Failed(failure)) if failure.code == ErrorCode::HashMismatch => {
                if !pump.restart() {
                    link.cancel(transfer).await;
                    return Err(Stop::Failed(failure));
                }
                link.message(&begin).await.map_err(|_| down())?;
                fresh = true;
            }
            Event::Guest(FromGuest::Failed(failure)) => return Err(Stop::Failed(failure)),
            Event::GuestDown => return Err(down()),
            Event::Cancel => {
                link.cancel(transfer).await;
                return Err(Stop::Cancelled);
            }
            Event::Again { route, key } => task.again(route, key),
            Event::Guest(FromGuest::Manifest(_))
            | Event::GuestChunk(_)
            | Event::ClientChunk(_)
            | Event::ClientCredit { .. } => debug!("dropped an event a push doesn't take"),
        }
    }
}

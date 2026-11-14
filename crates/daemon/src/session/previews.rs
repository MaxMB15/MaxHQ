// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Previews of the pictures in tool results. A picture under the decoding
//! cap can still take seconds to decode, and one tool result holds up to
//! sixteen, so they're made off the loop, one at a time, and fill in on the
//! timeline when they're done. Past the limits here a picture keeps the
//! placeholder it shows until then.

use std::collections::VecDeque;

use maxhq_protocol::ids::{AgentId, AttachmentId};

use super::{Input, Manager};
use crate::drive::pictures::ForPreview;
use crate::thumbnail;

/// The most pictures waiting for a preview.
const MAX_WAITING: usize = 256;

/// The most bytes of pictures waiting for a preview.
const MAX_WAITING_BYTES: usize = 256 * 1024 * 1024;

/// The pictures waiting for a preview.
#[derive(Default)]
pub(super) struct Previews {
    waiting: VecDeque<ForPreview>,
    bytes: usize,
    running: bool,
}

impl Previews {
    /// Adds `job`, unless the same picture is already waiting or there's no
    /// room left.
    fn add(&mut self, job: ForPreview) {
        let bytes = self.bytes + job.bytes.len();
        if self.waiting.len() >= MAX_WAITING
            || bytes > MAX_WAITING_BYTES
            || self
                .waiting
                .iter()
                .any(|other| other.agent == job.agent && other.id == job.id)
        {
            return;
        }
        self.bytes = bytes;
        self.waiting.push_back(job);
    }

    fn next(&mut self) -> Option<ForPreview> {
        let job = self.waiting.pop_front()?;
        self.bytes -= job.bytes.len();
        Some(job)
    }
}

impl Manager {
    /// Queues the previews a driver's pictures need, and starts on them.
    pub(super) fn make_previews(&mut self, jobs: Vec<ForPreview>) {
        for job in jobs {
            self.previews.add(job);
        }
        self.next_preview();
    }

    fn next_preview(&mut self) {
        if self.previews.running {
            return;
        }
        let Some(job) = self.previews.next() else {
            return;
        };
        self.previews.running = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ForPreview {
                agent,
                id,
                mime,
                bytes,
            } = job;
            let made =
                tokio::task::spawn_blocking(move || thumbnail::thumbnail_of(&bytes, Some(mime)))
                    .await;
            let preview = made.ok().flatten();
            let _ = tx.send(Input::Preview { agent, id, preview });
        });
    }

    /// Shows a finished preview to everyone with the timeline open, and
    /// starts on the next.
    pub(super) fn preview_made(
        &mut self,
        agent: AgentId,
        id: AttachmentId,
        preview: Option<String>,
    ) {
        self.previews.running = false;
        if let Some(preview) = preview
            && let Some(timeline) = self.timelines.get_mut(&agent)
        {
            let since = timeline.seq();
            timeline.show_preview(&id, &preview);
            self.broadcast(agent, since);
        }
        self.next_preview();
    }
}

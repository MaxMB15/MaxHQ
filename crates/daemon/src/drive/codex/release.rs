// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Letting an idle thread go so the app-server keeps nothing for it. The
//! next message resumes it, as it does after `thread/unsubscribe` anywhere.

use serde_json::json;

use super::actions::Call;
use super::thread::Thread;
use super::{CodexServer, Phase};
use crate::drive::Effects;
use maxhq_protocol::ids::AgentId;

impl CodexServer {
    /// Whether `agent`'s thread and its sub-agents' threads are idle: no
    /// turn, message, prompt, open item or request of theirs is under way.
    pub(super) fn releasable(&self, agent: AgentId) -> bool {
        let Some(id) = self.agents.get(&agent) else {
            return false;
        };
        if self.phase != Phase::Ready || !self.tool_calls.is_empty() {
            return false;
        }
        let family: Vec<&Thread> = self.family(id).collect();
        let Some(lead) = family.iter().find(|thread| &thread.id == id) else {
            return false;
        };
        let ids: Vec<&str> = family.iter().map(|thread| thread.id.as_str()).collect();
        !lead.stopped
            && family.iter().all(|thread| idle(thread))
            && !self
                .prompts
                .values()
                .any(|prompt| ids.contains(&prompt.thread.as_str()))
            && !self
                .rpc
                .waiting(|call| call.thread().is_some_and(|thread| ids.contains(&thread)))
    }

    /// Lets `agent`'s thread and its sub-agents' threads go. They stay
    /// known, so the next message resumes the thread.
    pub(super) fn release_thread(&mut self, agent: AgentId, fx: &mut Effects) {
        let Some(id) = self.agents.get(&agent).cloned() else {
            return;
        };
        let loaded: Vec<String> = self
            .family(&id)
            .filter(|thread| thread.loaded)
            .map(|thread| thread.id.clone())
            .collect();
        for thread in loaded {
            if let Some(known) = self.threads.get_mut(&thread) {
                known.loaded = false;
            }
            let call = Call::Release {
                thread: thread.clone(),
            };
            self.rpc.send(
                "thread/unsubscribe",
                json!({ "threadId": thread }),
                call,
                fx,
            );
        }
    }

    /// The thread `id` and the threads it spawned.
    fn family<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Thread> + 'a {
        self.threads
            .values()
            .filter(move |thread| thread.id == id || thread.parent.as_deref() == Some(id))
    }
}

/// Whether nothing of a thread's is under way.
fn idle(thread: &Thread) -> bool {
    !thread.busy()
        && !thread.resuming
        && !thread.interrupt_wanted
        && thread.limited.is_none()
        && thread.queue.is_empty()
        && thread.sent.is_empty()
        && thread.steers.is_empty()
        && thread.open.is_empty()
}

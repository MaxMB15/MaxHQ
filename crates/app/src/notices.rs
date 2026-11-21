// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notification inbox. Each `maxd` keeps the alerts about its own
//! agents; the app asks every connected device and merges their pages by
//! time. A device too old to keep them is left out.

use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notices::{NoticeCall, NoticePage, NOTICES_KEPT};
use maxhq_protocol::state::Entity;
use maxhq_protocol::ClientMessage;

use crate::devices::{agent_device, device_connected, Connector};
use crate::merge::Merge;
use crate::request::{lookup_failed, CommandError};
use crate::App;

impl<C: Connector> App<C> {
    /// The newest `limit` notices across every connected device, and how
    /// many they hold. A device that doesn't answer is left out.
    pub async fn list_notices(&self, limit: u32) -> NoticePage {
        let limit = limit.min(NOTICES_KEPT);
        let pages = self
            .ask_notices(NoticeCall::List { limit }, None)
            .await
            .into_iter()
            .filter_map(|answer| match answer {
                Ok(ActionOutcome::Notices { page }) => Some(page),
                _ => None,
            })
            .collect();
        merge_pages(pages, limit)
    }

    /// Marks the agent's notices read on its device, or every notice on
    /// every device with no agent.
    pub async fn read_notices(&self, agent: Option<AgentId>) -> Result<(), CommandError> {
        let device = match agent {
            Some(agent) => match self.view.read(|merge| agent_device(merge, agent)) {
                Some(device) => Some(device),
                // An agent the view no longer shows has no device to ask.
                None => return Ok(()),
            },
            None => None,
        };
        self.settle_notices(NoticeCall::Read { agent }, device)
            .await
    }

    /// Empties every connected device's log.
    pub async fn clear_notices(&self) -> Result<(), CommandError> {
        self.settle_notices(NoticeCall::Clear, None).await
    }

    /// Sends a call that changes the logs, failing as the first device
    /// that refuses it.
    async fn settle_notices(
        &self,
        call: NoticeCall,
        device: Option<DeviceId>,
    ) -> Result<(), CommandError> {
        for answer in self.ask_notices(call, device).await {
            match answer? {
                ActionOutcome::Done => {}
                other => return Err(lookup_failed(other)),
            }
        }
        Ok(())
    }

    /// Asks `device`, or every connected device, and gives each answer
    /// but a too old `maxd`'s.
    async fn ask_notices(
        &self,
        call: NoticeCall,
        device: Option<DeviceId>,
    ) -> Vec<Result<ActionOutcome, CommandError>> {
        let on = match device {
            Some(device) => vec![device],
            None => self.view.read(connected_devices),
        };
        let mut answers = Vec::with_capacity(on.len());
        for device in on {
            let action = ClientMessage::Notices {
                key: IdempotencyKey::generate(),
                device,
                call: call.clone(),
            };
            match self.devices.act_on_unless_old(device, action).await {
                Ok(Some(outcome)) => answers.push(Ok(outcome)),
                Ok(None) => {}
                Err(error) => answers.push(Err(error)),
            }
        }
        answers
    }
}

/// Every device a link reaches now, once each.
fn connected_devices(merge: &Merge) -> Vec<DeviceId> {
    let mut devices: Vec<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::DeviceLink(link) => link.device,
            _ => None,
        })
        .filter(|&device| device_connected(merge, device))
        .collect();
    devices.sort();
    devices.dedup();
    devices
}

/// One page from each device's, the newest `limit` first, with the counts
/// summed.
fn merge_pages(pages: Vec<NoticePage>, limit: u32) -> NoticePage {
    let mut merged = NoticePage::default();
    for page in pages {
        merged.total += page.total;
        merged.unread += page.unread;
        merged.notices.extend(page.notices);
    }
    merged
        .notices
        .sort_by_key(|n| std::cmp::Reverse((n.at, n.device, n.id)));
    merged.notices.truncate(limit as usize);
    merged
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::notices::{Notice, NoticeKind};
    use maxhq_protocol::time::Timestamp;

    use super::*;

    fn notice(device: &str, id: u32, at: u64) -> Notice {
        Notice {
            id,
            device: DeviceId::derive(device),
            agent: AgentId::derive("a"),
            kind: NoticeKind::Finished,
            title: "Fix the login".into(),
            project: None,
            summary: None,
            at: Timestamp(at),
            read_at: None,
        }
    }

    #[test]
    fn pages_from_each_device_merge_newest_first_with_their_counts() {
        let mac = NoticePage {
            notices: vec![notice("Home Mac", 3, 30), notice("Home Mac", 2, 10)],
            total: 3,
            unread: 1,
        };
        let devbox = NoticePage {
            notices: vec![notice("devbox", 9, 20), notice("devbox", 8, 5)],
            total: 40,
            unread: 4,
        };
        let merged = merge_pages(vec![mac, devbox], 3);
        let order: Vec<(DeviceId, u32)> = merged
            .notices
            .iter()
            .map(|notice| (notice.device, notice.id))
            .collect();
        assert_eq!(
            order,
            vec![
                (DeviceId::derive("Home Mac"), 3),
                (DeviceId::derive("devbox"), 9),
                (DeviceId::derive("Home Mac"), 2),
            ]
        );
        assert_eq!((merged.total, merged.unread), (43, 5));
    }
}

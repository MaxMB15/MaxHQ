// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving a held agent to another vendor. The agent keeps its ID, timeline
//! and folder. Its old session stops, a new one starts under the account
//! you picked, and the
//! conversation so far goes in front of the next message the new session
//! gets. Moving back takes the old session up again, with only what
//! happened since it last saw the conversation.

use std::borrow::Cow;
use std::path::PathBuf;

use maxhq_core::handoff::{self, Strategy, Usage};
use maxhq_protocol::ids::{AccountId, AgentId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Activity, AgentPermission, PermissionMode, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineItem};
use tracing::warn;
use uuid::Uuid;

use super::held::{internal_key, Handoff, Internal, Native, Server, ServerKey, Target, Written};
use super::requests::{guest_folder, STOPPING, UNREACHABLE};
use super::{refused, Manager};
use crate::drive::{Action, ActionKey};
use crate::skills::deliver;

impl Manager {
    /// Whether a model picked under `account` moves the agent off the
    /// account it runs under.
    pub(super) fn moves(&self, agent: AgentId, account: AccountId) -> bool {
        self.held
            .get(&self.lead_of(agent))
            .is_some_and(|held| held.account != account)
    }

    /// Moves a held agent to `account`'s vendor, running `model`, or the
    /// account's default when it's empty. The new session starts now if
    /// the old one was running, or with the next message.
    pub(super) fn switch_vendor(
        &mut self,
        agent: AgentId,
        account: AccountId,
        model: String,
    ) -> ActionOutcome {
        let lead = self.lead_of(agent);
        if lead != agent {
            return refused("a sub-agent runs on its lead's vendor");
        }
        let Some(held) = self.held.get(&lead) else {
            return ActionOutcome::UnknownAgent;
        };
        let Some(Entity::Account(to)) = self.hub.get(&EntityKey::Account(account)) else {
            return refused("that account isn't on the agent's device");
        };
        if to.device != self.device {
            return refused("that account isn't on the agent's device");
        }
        if self.is_dispatcher(lead) && !matches!(to.vendor, Vendor::Claude | Vendor::Codex) {
            return refused("The dispatcher runs as a Claude or Codex account on this Mac.");
        }
        if held.guest.is_some_and(|guest| !self.link_up(guest)) {
            return refused(UNREACHABLE);
        }
        if self.is_stopping(lead) {
            return refused(STOPPING);
        }
        if self.cut(lead).busy {
            return refused(format!(
                "wait for the turn to end before moving the agent to {}",
                to.vendor.name()
            ));
        }
        let guest = held.guest;
        let config_dir = match guest {
            Some(_) => guest_folder(to.vendor, to.id),
            None => PathBuf::from(&to.config_dir),
        };
        let login = self
            .login_for(
                to.vendor,
                to.id,
                config_dir.clone(),
                to.login,
                guest.is_some(),
            )
            .and_then(|login| self.check_sign_in(&login, guest.is_some()).map(|()| login));
        let login = match login {
            Ok(login) => login,
            Err(reason) => return refused(reason),
        };
        // A Codex agent runs while its thread is loaded, the others while
        // their CLI does.
        let running = held.paused.is_none() && !held.adopt && self.session_of(held).is_some();
        // A container links the skills into each vendor's home as an agent
        // of that vendor starts there.
        if let (Some(guest), Some(cwd)) = (guest, held.cwd.to_str()) {
            let homes = deliver::guest_homes(to.vendor, &config_dir);
            let cwd = cwd.to_owned();
            tokio::spawn(async move {
                if let Err(err) = deliver::ensure_guest(guest, cwd, homes).await {
                    warn!(%err, "couldn't link the worktree's skills");
                }
            });
        }
        self.leave(lead);
        if to.vendor == Vendor::Codex {
            let server = ServerKey {
                account: to.id,
                workspace: guest,
            };
            self.servers
                .entry(server)
                .or_insert_with(|| Server::new(login));
        }
        self.arrive(lead, &to, config_dir, model);
        if running {
            self.revive(lead);
        }
        ActionOutcome::Done
    }

    /// Stops the agent's session the way a provider switch does,
    /// keeping where it was so a move back takes it up again.
    fn leave(&mut self, lead: AgentId) {
        let last = self
            .timelines
            .get(&lead)
            .and_then(|timeline| timeline.items().next_back())
            .map(|item| item.id.clone());
        let codex_session = self
            .held
            .get(&lead)
            .filter(|held| !held.own_process() && !held.adopt)
            .and_then(|held| self.session_of(held).map(|_| held.server_key()));
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        // A session that never got its handoff saw only what it had before.
        let pending = held.handoff.take();
        let seen = match &pending {
            Some(pending) => pending.since.clone(),
            None => last,
        };
        if let Some(seen) = seen.filter(|_| !held.vendor_session.is_empty()) {
            held.natives.retain(|native| native.account != held.account);
            held.natives.push(Native {
                account: held.account,
                vendor_session: held.vendor_session.clone(),
                seen,
                context: held.agent.context,
            });
        }
        let session = held.session.take();
        if held.own_process()
            && let Some(session) = session
            && let Some(hold) = self.holds.get_mut(&session)
        {
            // The holder no longer runs the agent, so nothing it says from
            // here on reaches it.
            hold.record.agent = None;
            let record = hold.record.clone();
            self.store_holder(session, &record);
            self.stop_hold(session);
        }
        if let Some(server) = codex_session {
            let key = internal_key();
            self.internal.insert(key, Internal::Plain);
            self.work
                .push_back((Target::Codex(server), Action::Stop { key, agent: lead }));
        }
        self.idle_ends.remove(&lead);
        self.limit_waits.remove(&lead);
        if let Some(pending) = pending {
            self.settle_marker(lead, &pending.marker, ItemStatus::Cancelled);
        }
    }

    /// Makes the agent `to`'s, with the marker where it moved and the
    /// handoff its next message carries.
    fn arrive(&mut self, lead: AgentId, to: &Account, config_dir: PathBuf, model: String) {
        let now = self.clock.wall();
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        let model = Some(model).filter(|model| !model.is_empty());
        let from_vendor = held.vendor();
        let from_model = held.agent.model.clone().or_else(|| held.model.clone());
        let native = held
            .natives
            .iter()
            .position(|native| native.account == to.id)
            .map(|at| held.natives.remove(at));
        let (vendor_session, since, context) = match native {
            Some(native) => (native.vendor_session, Some(native.seen), native.context),
            // A Claude session is named before it starts.
            None if to.vendor == Vendor::Claude => (Uuid::now_v7().to_string(), None, None),
            None => (String::new(), None, None),
        };
        // Codex has no plan mode.
        if to.vendor == Vendor::Codex && held.mode == PermissionMode::Plan {
            held.mode = PermissionMode::Ask;
        }
        if held.title.is_none() {
            held.title = Some(held.agent.title.clone());
        }
        held.agent.vendor = to.vendor;
        held.agent.model.clone_from(&model);
        held.agent.permission = Some(AgentPermission {
            requested: held.mode,
            effective: None,
        });
        held.agent.context = None;
        held.agent.activity = Activity::Idle;
        held.agent.vendor_session_id = None;
        held.account = to.id;
        held.config_dir = config_dir;
        held.login = to.login;
        held.model = model.clone();
        held.vendor_session = vendor_session;
        held.resumes = Default::default();
        held.fault = None;
        held.owed = None;
        held.resuming = false;
        held.resume_after = None;
        held.down_since = None;
        held.adopt = to.vendor == Vendor::Codex;

        let marker = ItemId(format!("maxhq-handoff-{}", Uuid::now_v7()));
        let item = TimelineItem {
            id: marker.clone(),
            agent: lead,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Pending,
            started_at: Some(now),
            completed_at: None,
            body: ItemBody::Handoff {
                from_vendor,
                from_model,
                to_vendor: to.vendor,
                to_model: model,
            },
        };
        self.timeline_events(vec![(lead, TimelineEvent::Upsert { item })]);
        self.timeline_mut(lead).freeze();
        let earlier = self.timeline_mut(lead).earlier();
        let (items, _) = carried(before(&earlier, &marker), since.as_ref());
        if handoff::carries(items) {
            if let Some(held) = self.held.get_mut(&lead) {
                held.handoff = Some(Handoff {
                    marker,
                    since,
                    context,
                    brief: None,
                });
            }
            self.keep_earlier(lead);
        } else {
            // With nothing to carry the new session starts as it is when
            // the agent has no turns yet.
            self.settle_marker(lead, &marker, ItemStatus::Completed);
        }
        self.save(lead);
        self.publish(lead);
    }

    /// `action` with the conversation in front of its message, when it's
    /// the first the agent's new session gets, or none when it can't
    /// carry it, which fails the action.
    pub(super) fn handed(&mut self, lead: AgentId, mut action: Action) -> Option<Action> {
        match self.hand_off(lead, &mut action) {
            Ok(()) => Some(action),
            Err(reason) => {
                self.result(action.key(), refused(reason), Written::default());
                None
            }
        }
    }

    /// Puts the handoff in front of the message after a
    /// provider switch: as much of the conversation as fits
    /// what the new session's window leaves.
    fn hand_off(&mut self, lead: AgentId, action: &mut Action) -> Result<(), String> {
        let Action::Send {
            agent,
            text,
            attachments,
            ..
        } = action
        else {
            return Ok(());
        };
        let pending = self.held.get(&lead).and_then(|held| held.handoff.clone());
        let Some(pending) = pending.filter(|_| *agent == lead) else {
            return Ok(());
        };
        let kinds: Vec<_> = attachments
            .iter()
            .map(|staged| staged.attachment.kind)
            .collect();
        let history = if let Some(brief) = pending.brief {
            // An agent that moved here starts from the summary the device
            // it left made, since its conversation stayed there.
            Some(brief).filter(|brief| {
                handoff::fits(brief, handoff::budget(text, &kinds, Usage::default()))
            })
        } else {
            let earlier = self
                .timelines
                .get(&lead)
                .map(|timeline| timeline.earlier())
                .unwrap_or_default();
            let (items, strategy) =
                carried(before(&earlier, &pending.marker), pending.since.as_ref());
            let usage = match (strategy, pending.context) {
                (Strategy::Delta, Some(context)) => Usage {
                    used: context.used_tokens.into(),
                    window: Some(context.window_tokens.into()).filter(|window| *window > 0),
                },
                _ => Usage::default(),
            };
            let budget = handoff::budget(text, &kinds, usage);
            handoff::deliver(lead, items, strategy, budget)
        };
        let history = history.ok_or_else(|| handoff::TOO_LITTLE.to_owned())?;
        *text = handoff::wrap(&history, text);
        if let Some(held) = self.held.get_mut(&lead) {
            held.handoff = None;
        }
        self.settle_marker(lead, &pending.marker, ItemStatus::Completed);
        self.save(lead);
        Ok(())
    }

    /// Ends the marker of a move with `status`.
    fn settle_marker(&mut self, lead: AgentId, marker: &ItemId, status: ItemStatus) {
        let item = self
            .timelines
            .get(&lead)
            .and_then(|timeline| timeline.get(marker))
            .map(Cow::into_owned);
        if let Some(mut item) = item {
            item.status = status;
            item.completed_at = Some(self.clock.wall());
            self.timeline_events(vec![(lead, TimelineEvent::Upsert { item })]);
        }
        self.keep_earlier(lead);
    }

    /// Stores the items from before the agent's moves, which no vendor's
    /// history has.
    fn keep_earlier(&self, lead: AgentId) {
        let Some(timeline) = self.timelines.get(&lead) else {
            return;
        };
        if let Err(err) = self.db.put_earlier(lead, &timeline.earlier()) {
            warn!(%err, agent = %lead, "couldn't store the agent's earlier items");
        }
    }
}

/// The items of `earlier` before the marker of a move.
fn before<'a>(earlier: &'a [TimelineItem], marker: &ItemId) -> &'a [TimelineItem] {
    let at = earlier
        .iter()
        .position(|item| item.id == *marker)
        .unwrap_or(earlier.len());
    &earlier[..at]
}

/// What of `items` goes to a session that last saw `since`: what came
/// after it, or all of it to a session that saw none of it.
fn carried<'a>(
    items: &'a [TimelineItem],
    since: Option<&ItemId>,
) -> (&'a [TimelineItem], Strategy) {
    let after = since.and_then(|since| items.iter().position(|item| item.id == *since));
    match after {
        Some(at) => (&items[at + 1..], Strategy::Delta),
        None => (items, Strategy::Full),
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Each lead agent's PR links. The manager is the only writer: it links a
//! PR an agent's command opened, one whose branch is the agent's, one the
//! agent or you link, and keeps what the watcher reads of each. A watched
//! PR whose checks, remarks or merge state changed wakes its agent with a
//! message, and one that merged or closed may settle it, the way
//! auto-settle does: settling stops only a process MaxHQ holds.
//!
//! A link lives only in the agent's own repository, the one its project
//! names, since that's the only one the watcher reads.

use std::sync::Arc;

use maxhq_core::inbox;
use maxhq_core::pulls::{self, settle, watch};
use maxhq_core::tools::pulls::Numbered;
use maxhq_core::tools::Call;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::pulls::{LinkSource, PullKey, PullLink, PullState};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{ItemBody, ItemId};
use serde_json::json;
use tracing::warn;

use super::agent_tools::Calling;
use super::{refused, Input, Manager};
use crate::drive;
use crate::pulls::source::PullSource;
use crate::pulls::watcher::{self, Found, ReadPull};
use crate::secrets::RepoName;

/// The refusal for a repository that doesn't parse.
const NOT_A_REPO: &str = "That isn't a GitHub repository.";
/// The refusal for an agent whose project names no repository.
const NO_REPO: &str = "This agent's project names no GitHub repository.";
/// The refusal for a PR tool called by something that isn't an agent.
const NO_AGENT: &str = "Only an agent has pull requests.";

impl Manager {
    /// Shows the stored links.
    pub(super) fn load_pulls(&mut self) {
        match self.db.pull_links() {
            Ok(all) => {
                for (agent, links) in all {
                    self.hub.set_pulls(agent, links);
                }
            }
            Err(err) => warn!(%err, "couldn't load the PR links"),
        }
    }

    /// Starts the watcher on `source`.
    pub(super) fn watch_pulls(&mut self, source: Arc<dyn PullSource>) {
        self.dispatcher.pulls = Some(source.clone());
        let tx = self.tx.clone();
        watcher::start(
            self.hub.clone(),
            source,
            self.clock.clone(),
            self.pulls_due.clone(),
            move |found| {
                let _ = tx.send(Input::Pulls(found));
            },
        );
    }

    /// Takes in what the watcher found.
    pub(super) fn pulls_found(&mut self, found: Vec<Found>) {
        for found in found {
            match found {
                Found::Matched { agent, key } => self.link(agent, key, LinkSource::Branch),
                Found::Read(read) => self.pull_read(&read),
                Found::Missing(key) => self.pull_missing(&key),
                Found::Failed(key) => self.pull_failed(&key),
            }
        }
    }

    /// Answers `PullsDue`.
    pub(super) fn pulls_due(&mut self, repo: &str) -> ActionOutcome {
        let Some(repo) = RepoName::parse(repo) else {
            return refused(NOT_A_REPO);
        };
        self.pulls_due.mark(&repo);
        ActionOutcome::Done
    }

    /// Answers `LinkPull`.
    pub(super) fn link_by_you(&mut self, agent: AgentId, number: u64) -> ActionOutcome {
        let lead = self.lead_of(agent);
        let Some(repo) = self.agent_repo(lead) else {
            return refused(NO_REPO);
        };
        self.link(lead, pulls::key(&repo.key(), number), LinkSource::You);
        self.pulls_due.mark(&repo);
        ActionOutcome::Done
    }

    /// Answers `UnlinkPull`.
    pub(super) fn unlink_by_you(
        &mut self,
        agent: AgentId,
        repo: &str,
        number: u64,
    ) -> ActionOutcome {
        let Some(repo) = RepoName::parse(repo) else {
            return refused(NOT_A_REPO);
        };
        self.unlink(self.lead_of(agent), pulls::key(&repo.key(), number));
        ActionOutcome::Done
    }

    /// Takes in a command an agent finished: one that opened a PR links
    /// it, and one that pushed, merged or closed makes the repository due.
    pub(super) fn pull_command(&mut self, agent: AgentId, item: &ItemId) {
        let (created, changes) = {
            let item = self
                .timelines
                .get(&agent)
                .and_then(|timeline| timeline.get(item));
            match item.as_deref().map(|item| &item.body) {
                Some(ItemBody::Command {
                    command, output, ..
                }) => (
                    pulls::created_by(command, output),
                    pulls::changes_pulls(command),
                ),
                _ => return,
            }
        };
        if created.is_empty() && !changes {
            return;
        }
        let lead = self.lead_of(agent);
        let Some(repo) = self.agent_repo(lead) else {
            return;
        };
        let here = repo.key();
        for key in created.into_iter().filter(|key| key.repo == here) {
            self.link(lead, key, LinkSource::Created);
        }
        self.pulls_due.mark(&repo);
    }

    /// Answers a PR tool.
    pub(super) fn pull_tool(&mut self, caller: Calling, call: Call) -> Result<String, String> {
        let lead = caller.agent.ok_or(NO_AGENT)?;
        let repo = self.agent_repo(lead).ok_or(NO_REPO)?;
        let key = |number| pulls::key(&repo.key(), number);
        let linked = |manager: &Self, key: &PullKey| {
            manager
                .hub
                .pulls(lead)
                .into_iter()
                .find(|link| link.key == *key && pulls::shown(link))
        };
        match call {
            Call::LinkPullRequest(Numbered { number }) => {
                self.link(lead, key(number), LinkSource::Agent);
                self.pulls_due.mark(&repo);
                Ok(format!("Linked PR #{number} in {repo}."))
            }
            Call::UnlinkPullRequest(Numbered { number }) => {
                self.unlink(lead, key(number));
                Ok(format!("Unlinked PR #{number}."))
            }
            Call::ListPullRequests => {
                let links: Vec<PullLink> = self
                    .hub
                    .pulls(lead)
                    .into_iter()
                    .filter(pulls::shown)
                    .collect();
                Ok(json!({ "pull_requests": links }).to_string())
            }
            Call::WatchPullRequest(Numbered { number }) => {
                let key = key(number);
                let mut link = match linked(self, &key) {
                    Some(link) => link,
                    None => {
                        self.link(lead, key.clone(), LinkSource::Agent);
                        linked(self, &key).ok_or(NO_REPO)?
                    }
                };
                if let Some(state) = link.snapshot.as_ref().map(|snapshot| snapshot.state)
                    && state != PullState::Open
                {
                    return Err(format!(
                        "PR #{number} has ended, so there's nothing to watch."
                    ));
                }
                if link.watch.is_none() {
                    link.watch = Some(watch::start(self.clock.wall()));
                    self.put_link(lead, link);
                }
                self.pulls_due.mark(&repo);
                Ok(format!(
                    "Watching PR #{number}. MaxHQ wakes you when checks fail or pass, new \
                     comments or reviews arrive, or the PR conflicts. End your turn after this \
                     call."
                ))
            }
            Call::UnwatchPullRequest(Numbered { number }) => {
                let mut link = linked(self, &key(number))
                    .ok_or_else(|| format!("PR #{number} isn't linked to you."))?;
                link.watch = None;
                self.put_link(lead, link);
                Ok(format!("Stopped watching PR #{number}."))
            }
            _ => unreachable!("not a PR tool"),
        }
    }

    /// Forgets the links of an agent that's gone.
    pub(super) fn forget_pulls(&mut self, agent: AgentId) {
        if let Err(err) = self.db.remove_pull_links(agent, None) {
            warn!(%err, %agent, "couldn't forget the agent's PR links");
        }
        self.hub.set_pulls(agent, Vec::new());
    }

    /// Settles a lead agent whose linked PRs all ended, when one of them
    /// settles it and nothing keeps it busy.
    pub(super) fn settle_ended(&mut self, lead: AgentId) {
        let Some(agent) = self.hub.reported(lead) else {
            return;
        };
        if agent.pulls.is_empty() {
            return;
        }
        let settle_on_merge = self
            .project_settings(&agent)
            .is_none_or(|settings| settings.settle_on_merge);
        let now = self.clock.wall();
        let Some(settled) = settle::settles(&agent, settle_on_merge, self.last_message(lead), now)
        else {
            return;
        };
        if self.busy(&agent).is_some() {
            return;
        }
        let mut next = self.hub.inbox(lead);
        inbox::settle(&mut next, settled);
        self.write_inbox(lead, next);
        self.stop_settled(lead);
    }

    /// When you last sent the agent a message, as its timeline shows.
    fn last_message(&self, lead: AgentId) -> Option<maxhq_protocol::time::Timestamp> {
        let timeline = self.timelines.get(&lead)?;
        let item = timeline
            .items()
            .rev()
            .find(|item| matches!(item.body, ItemBody::UserMessage { .. }))?;
        item.started_at.or(item.completed_at)
    }

    /// Takes in a linked PR the watcher read: wakes each agent that
    /// watches it for what changed, keeps the snapshot, and settles the
    /// agents once it ended.
    fn pull_read(&mut self, read: &ReadPull) {
        let ended = read.snapshot.state != PullState::Open;
        for lead in self.hub.linking(&read.key) {
            let Some(mut link) = self
                .hub
                .pulls(lead)
                .into_iter()
                .find(|link| link.key == read.key && pulls::shown(link))
            else {
                continue;
            };
            if let Some(watching) = link.watch.take() {
                let report = read.evaluate(&watching, link.snapshot.as_ref(), self.clock.wall());
                if !report.changes.is_empty() {
                    let text = watch::message(
                        read.key.number,
                        &read.url,
                        &read.snapshot.base,
                        &read.snapshot.head_sha,
                        &report,
                        &read.known,
                        &crate::hooks::deliver::frame(),
                    );
                    self.wake(lead, text);
                }
                link.watch = (!report.exhausted && !ended).then_some(report.next);
            }
            link.snapshot = Some(read.snapshot.clone());
            self.put_link(lead, link);
            if ended {
                self.settle_ended(lead);
            }
        }
    }

    /// Sends a held agent a message about its PR, which brings it back as
    /// one you send does. One MaxHQ doesn't hold can't be told.
    fn wake(&mut self, lead: AgentId, text: String) {
        if !self.held.contains_key(&lead) {
            return;
        }
        self.send_own(
            lead,
            drive::Message {
                text,
                refs: Vec::new(),
                attachments: Vec::new(),
            },
        );
        self.message_sent(lead);
    }

    /// Drops the links to a PR that isn't there and was never read.
    fn pull_missing(&mut self, key: &PullKey) {
        for lead in self.hub.linking(key) {
            let mut links = self.hub.pulls(lead);
            let before = links.len();
            links.retain(|link| link.key != *key || link.snapshot.is_some());
            if links.len() == before {
                continue;
            }
            if let Err(err) = self.db.remove_pull_links(lead, Some(key)) {
                warn!(%err, %lead, "couldn't forget a missing PR's link");
            }
            self.send_waiting(lead);
            self.hub.set_pulls(lead, links);
        }
    }

    /// Counts a failed read on each watch of a PR, which ends after too
    /// many in a row.
    fn pull_failed(&mut self, key: &PullKey) {
        for lead in self.hub.linking(key) {
            let link = self
                .hub
                .pulls(lead)
                .into_iter()
                .find(|link| link.key == *key);
            if let Some(mut link) = link
                && let Some(watching) = &link.watch
            {
                link.watch = watch::failed(watching);
                self.put_link(lead, link);
            }
        }
    }

    /// Links PR `key` to a lead agent. A link it has stays as it is, and
    /// one you unlinked comes back only when you or the agent link it.
    fn link(&mut self, lead: AgentId, key: PullKey, source: LinkSource) {
        let old = self
            .hub
            .pulls(lead)
            .into_iter()
            .find(|link| link.key == key);
        let dismissed = old
            .as_ref()
            .is_some_and(|old| old.source == LinkSource::Dismissed);
        let relink = matches!(source, LinkSource::Agent | LinkSource::You);
        if old.is_some() && !(dismissed && relink) {
            return;
        }
        let link = PullLink {
            key,
            source,
            at: self.clock.wall(),
            snapshot: old.and_then(|old| old.snapshot),
            watch: None,
        };
        self.put_link(lead, link);
    }

    /// Unlinks PR `key` from a lead agent, and keeps a branch match from
    /// bringing it back.
    fn unlink(&mut self, lead: AgentId, key: PullKey) {
        let old = self
            .hub
            .pulls(lead)
            .into_iter()
            .find(|link| link.key == key);
        let link = match old {
            Some(old) => PullLink {
                source: LinkSource::Dismissed,
                watch: None,
                ..old
            },
            None => PullLink {
                key,
                source: LinkSource::Dismissed,
                at: self.clock.wall(),
                snapshot: None,
                watch: None,
            },
        };
        self.put_link(lead, link);
    }

    /// Stores a link and shows it. One that didn't change is left alone.
    fn put_link(&mut self, lead: AgentId, link: PullLink) {
        let mut links = self.hub.pulls(lead);
        match links.iter_mut().find(|old| old.key == link.key) {
            Some(old) if *old == link => return,
            Some(old) => *old = link.clone(),
            None => links.push(link.clone()),
        }
        if let Err(err) = self.db.put_pull_link(lead, &link) {
            warn!(%err, %lead, "couldn't store a PR link");
        }
        self.send_waiting(lead);
        self.hub.set_pulls(lead, links);
    }

    /// The repository of a lead agent's project.
    fn agent_repo(&self, lead: AgentId) -> Option<RepoName> {
        let Some(Entity::Agent(agent)) = self.hub.get(&EntityKey::Agent(lead)) else {
            return None;
        };
        RepoName::parse(&self.project_repo(agent.workspace)?)
    }
}

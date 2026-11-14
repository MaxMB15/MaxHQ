// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the follower reads from the hub: the folders to follow, each
//! project's agents as the engine's [`Agents`], and where summaries go.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use maxhq_protocol::guest::GuestAgent;
use maxhq_protocol::ids::{AgentId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{Activity, Agent, WorkspaceLocation};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::WorkflowRun;
use maxhq_workflow::{AgentFacts, AgentState, Agents};

use super::follow::{Root, World};
use super::guest::{guest_state, MAX_LIST};
use super::notes::Notes;
use crate::hub::Hub;
use crate::session::clock::Clock;

/// The most bytes of one agent's commands a guest gets, newest kept.
const GUEST_COMMANDS: usize = 16 * 1024;
/// A guest needs an agent's last write only for the ten minute quiet check,
/// so it gets it to the half minute, and the list doesn't change on every
/// write.
const GUEST_WRITE_GRAIN: u64 = 30_000;

/// The hub, as the follower sees it.
pub struct HubWorld {
    pub hub: Hub,
    pub notes: Notes,
    pub clock: Arc<dyn Clock>,
}

impl World for HubWorld {
    fn roots(&self, open: &HashSet<ProjectId>) -> Vec<Root> {
        let device = self.hub.device().id;
        let leads: Vec<(WorkspaceId, AgentId)> = self.hub.pick(|e| match e {
            Entity::Agent(a) if a.parent.is_none() => Some((a.workspace, a.id)),
            _ => None,
        });
        self.hub.pick(|e| {
            let Entity::Workspace(w) = e else {
                return None;
            };
            let (WorkspaceLocation::Folder { path }, Some(project)) = (&w.location, w.project)
            else {
                return None;
            };
            let here: Vec<AgentId> = leads
                .iter()
                .filter(|(at, _)| *at == w.id)
                .map(|(_, lead)| *lead)
                .collect();
            (w.device == device && (!here.is_empty() || open.contains(&project))).then(|| Root {
                path: PathBuf::from(path),
                project,
                leads: here,
            })
        })
    }

    fn agents(&self, project: ProjectId) -> Box<dyn Agents + '_> {
        let workspaces: HashSet<WorkspaceId> = self
            .hub
            .pick(|e| match e {
                Entity::Workspace(w) if w.project == Some(project) => Some(w.id),
                _ => None,
            })
            .into_iter()
            .collect();
        let facts = self.hub.pick(|e| match e {
            Entity::Agent(a) if workspaces.contains(&a.workspace) => Some(facts(a)),
            _ => None,
        });
        Box::new(HubAgents {
            facts,
            notes: &self.notes,
            now: self.clock.wall(),
        })
    }

    fn guest_project(&self, workspace: WorkspaceId) -> Option<ProjectId> {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(w))
                if matches!(w.location, WorkspaceLocation::Container { .. }) =>
            {
                w.project
            }
            _ => None,
        }
    }

    fn forwarded(&self, workspace: WorkspaceId) -> Vec<u16> {
        self.hub.pick(|e| match e {
            Entity::ListeningPort(p) if p.workspace == workspace && p.gone_since.is_none() => {
                Some(p.port)
            }
            _ => None,
        })
    }

    fn guest_agents(&self, workspace: WorkspaceId) -> Vec<GuestAgent> {
        let mut agents: Vec<Agent> = self.hub.pick(|e| match e {
            Entity::Agent(a) if a.workspace == workspace => Some(a.clone()),
            _ => None,
        });
        // The newest, when there are too many.
        agents.sort_by_key(|a| (std::cmp::Reverse(a.started_at), a.id));
        agents.truncate(MAX_LIST);
        agents
            .iter()
            .map(|a| {
                let f = facts(a);
                GuestAgent {
                    id: f.id,
                    lead: f.lead,
                    branch: a.branch.clone(),
                    title: f.title,
                    pid: f.pid,
                    state: guest_state(f.state),
                    started: f.started,
                    last_write: f
                        .last_write
                        .map(|t| Timestamp(t.0 - t.0 % GUEST_WRITE_GRAIN)),
                    prompt_head: self.notes.prompt_head(a.id),
                    commands: self.commands(a.id),
                }
            })
            .collect()
    }

    fn publish(&self, runs: Vec<WorkflowRun>) {
        self.hub.replace(
            |e| matches!(e, Entity::WorkflowRun(_)),
            runs.into_iter().map(Entity::WorkflowRun),
        );
        // Notes of agents that left the hub are no use to anyone.
        let agents: HashSet<AgentId> = self
            .hub
            .pick(|e| match e {
                Entity::Agent(a) => Some(a.id),
                _ => None,
            })
            .into_iter()
            .collect();
        self.notes.retain(|a| agents.contains(&a));
    }
}

impl HubWorld {
    /// An agent's last commands, oldest first, up to [`GUEST_COMMANDS`]
    /// bytes.
    fn commands(&self, agent: AgentId) -> Vec<String> {
        let mut all = Vec::new();
        self.notes
            .each_command(agent, &mut |c| all.push(c.to_owned()));
        let mut bytes = 0;
        let keep = all
            .iter()
            .rev()
            .take_while(|c| {
                bytes += c.len();
                bytes <= GUEST_COMMANDS
            })
            .count();
        all.split_off(all.len() - keep)
    }
}

/// One project's agents, as the hub had them when the tick began.
struct HubAgents<'a> {
    facts: Vec<AgentFacts>,
    notes: &'a Notes,
    now: Timestamp,
}

impl Agents for HubAgents<'_> {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn list(&self) -> Vec<AgentFacts> {
        self.facts.clone()
    }

    fn prompt_head(&self, agent: AgentId) -> Option<String> {
        self.notes.prompt_head(agent)
    }

    fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str)) {
        self.notes.each_command(agent, visit);
    }
}

/// What the engine needs to know of one agent.
fn facts(agent: &Agent) -> AgentFacts {
    AgentFacts {
        id: agent.id,
        lead: agent.parent,
        title: (!agent.title.is_empty()).then(|| agent.title.clone()),
        pid: agent.pid,
        state: state(&agent.activity),
        started: Some(agent.started_at),
        last_write: Some(agent.last_activity_at),
        tool_open: false,
    }
}

/// How an agent stands, in the engine's words.
fn state(activity: &Activity) -> AgentState {
    match activity {
        Activity::Starting | Activity::Busy | Activity::BusyInSubagents => AgentState::Busy,
        Activity::NeedsInput { .. } => AgentState::Waiting,
        Activity::DoneUnread
        | Activity::IdleCacheWarm { .. }
        | Activity::Idle
        | Activity::WaitingForLimitReset { .. }
        | Activity::Paused { .. } => AgentState::Idle,
        Activity::Error { .. } | Activity::NotAccessible => AgentState::Gone,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use maxhq_protocol::guest::GuestAgentState;
    use maxhq_protocol::ids::{BootId, DeviceId};
    use maxhq_protocol::model::{AgentLink, Control, Device, Vendor, Workspace};
    use maxhq_protocol::state::Change;
    use maxhq_protocol::timeline::ItemStatus;

    use super::*;
    use crate::session::clock::FakeClock;
    use crate::workflow::notes::tests::ran;

    fn world() -> HubWorld {
        let hub = Hub::new(
            Device {
                id: DeviceId::derive("devbox"),
                name: "devbox".into(),
                os: "macos".into(),
                boot_id: BootId("boot-1".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        );
        HubWorld {
            hub,
            notes: Notes::default(),
            clock: Arc::new(FakeClock::new()),
        }
    }

    fn folder(w: &HubWorld, name: &str) -> (WorkspaceId, ProjectId) {
        let (id, project) = (WorkspaceId::derive(name), ProjectId::derive(name));
        w.hub.apply([Change::Upsert {
            value: Entity::Workspace(Workspace {
                id,
                project: Some(project),
                device: w.hub.device().id,
                location: WorkspaceLocation::Folder {
                    path: format!("/work/{name}"),
                },
                managed: None,
                clone: None,
            }),
        }]);
        (id, project)
    }

    fn agent(
        w: &HubWorld,
        name: &str,
        at: WorkspaceId,
        parent: Option<AgentId>,
        activity: Activity,
    ) -> AgentId {
        let id = AgentId::derive(name);
        w.hub.apply([Change::Upsert {
            value: Entity::Agent(Agent {
                id,
                workspace: at,
                parent,
                vendor: Vendor::Claude,
                account: None,
                model: None,
                permission: None,
                session: None,
                vendor_session_id: Some(name.into()),
                control: Control::Observed,
                pid: Some(42),
                link: AgentLink::Live,
                title: name.into(),
                activity,
                context: None,
                started_at: Timestamp(1),
                last_activity_at: Timestamp(2),
                branch: None,
                inbox: Default::default(),
                origin: None,
                pulls: Vec::new(),
                taking_over: false,
                asleep: false,
                effort: None,
                running: None,
            }),
        }]);
        id
    }

    #[test]
    fn folders_with_a_lead_or_an_open_run_are_followed() {
        let w = world();
        let (busy, project) = folder(&w, "app");
        let (_, quiet) = folder(&w, "other");
        let lead = agent(&w, "lead", busy, None, Activity::Busy);
        let sub = agent(&w, "sub", busy, Some(lead), Activity::Idle);

        let roots = w.roots(&HashSet::new());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, PathBuf::from("/work/app"));
        assert_eq!(roots[0].project, project);
        assert_eq!(roots[0].leads, [lead]);
        assert_eq!(w.roots(&HashSet::from([quiet])).len(), 2);

        let agents = w.agents(project);
        let mut facts = agents.list();
        facts.sort_by_key(|f| f.id != lead);
        assert_eq!(facts.len(), 2);
        assert_eq!(
            (facts[0].id, facts[0].lead, facts[0].state),
            (lead, None, AgentState::Busy)
        );
        assert_eq!(
            (facts[1].id, facts[1].lead, facts[1].state),
            (sub, Some(lead), AgentState::Idle)
        );
        assert_eq!(facts[0].last_write, Some(Timestamp(2)));
    }

    #[test]
    fn a_guest_gets_only_its_own_containers_agents() {
        let w = world();
        let (box_, _) = folder(&w, "box");
        let (other, _) = folder(&w, "other");
        let lead = agent(&w, "lead", box_, None, Activity::Idle);
        let sub = agent(&w, "sub", box_, Some(lead), Activity::Busy);
        agent(&w, "elsewhere", other, None, Activity::Busy);

        let mut agents = w.guest_agents(box_);
        agents.sort_by_key(|a| a.id != lead);
        let ids: Vec<AgentId> = agents.iter().map(|a| a.id).collect();
        assert_eq!(ids, [lead, sub]);
        assert_eq!(agents[0].state, GuestAgentState::Idle);
        assert_eq!(agents[1].lead, Some(lead));
        assert_eq!(
            agents[0].last_write,
            Some(Timestamp(0)),
            "to the half minute"
        );
        assert!(w.guest_agents(WorkspaceId::derive("nowhere")).is_empty());
    }

    #[test]
    fn a_guest_gets_an_agents_newest_commands_only() {
        let w = world();
        let (box_, _) = folder(&w, "box");
        let lead = agent(&w, "lead", box_, None, Activity::Busy);
        assert!(w.commands(lead).is_empty());
        for i in 0..10 {
            let text = format!("{i}{}", "x".repeat(4000));
            let item = ran(lead, &text, ItemStatus::Completed);
            w.notes.item(lead, &item, true);
        }
        let kept = w.commands(lead);
        assert_eq!(kept.len(), 4);
        assert!(kept[0].starts_with('6') && kept[3].starts_with('9'));
    }
}

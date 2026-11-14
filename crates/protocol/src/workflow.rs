// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Workflow runs. A project's descriptor names its workflows, and the
//! `maxd` that owns the project follows each run's files and derives what
//! the canvas shows. Each run is published as a [`WorkflowRun`], and a client
//! that opens one gets its [`RunSnapshot`] and then [`RunPatch`]es as it
//! changes, much like a timeline.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId, ProjectId, WorkflowRunId};
use crate::time::Timestamp;

/// Names one run of one workflow in one project on one device.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunKey {
    /// The device whose `maxd` follows it.
    pub device: DeviceId,
    /// The project it runs in.
    pub project: ProjectId,
    /// The workflow, by its id in the descriptor.
    pub workflow: String,
    /// The run, by the id the workflow gave it.
    pub run: String,
}

impl RunKey {
    /// The id its [`WorkflowRun`] is published under. The same key always
    /// gives the same id.
    pub fn id(&self) -> WorkflowRunId {
        WorkflowRunId::derive(&format!(
            "{}/{}/{}/{}",
            self.device, self.project, self.workflow, self.run
        ))
    }
}

/// Where a node stands, in the standard's words. A workflow maps its own
/// words onto these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// This run leaves it out.
    Omitted,
    /// It waits for the nodes it needs.
    Queued,
    /// Everything it needs is done, and it can start.
    Ready,
    /// It's running.
    Busy,
    /// It waits for a child run or agent.
    WaitingChild,
    /// It waits for you.
    WaitingHuman,
    /// It finished.
    Done,
    /// It was skipped.
    Skipped,
    /// It will be skipped when the run reaches it.
    WillSkip,
    /// It failed.
    Failed,
    /// Something it needs failed, so it can't run.
    Blocked,
    /// It says it's busy, but no live agent works on it.
    Stale,
    /// The workflow wrote a word the standard doesn't know.
    Unknown,
}

/// How a badge or a header reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Plain.
    Neutral,
    /// Worth a look.
    Info,
    /// Went well.
    Good,
    /// Needs care.
    Warn,
    /// Went wrong.
    Bad,
}

/// A short label on a node, such as a tier or a chunk count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Badge {
    /// What it says.
    pub text: String,
    /// How it reads.
    pub tone: Tone,
}

/// One attempt at a node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AttemptSpan {
    /// When it started.
    pub start: Timestamp,
    /// When it ended, none while it runs.
    pub end: Option<Timestamp>,
    /// How it ended, once it has.
    pub end_status: Option<NodeStatus>,
    /// The agent that made it, once bound.
    pub agent: Option<AgentId>,
    /// When it last began waiting on you, while it still does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<Timestamp>,
}

/// A loop between two nodes, such as a review sending work back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LoopState {
    /// The node it goes back from.
    pub from: String,
    /// The node it goes back to.
    pub to: String,
    /// What it's called, such as "review".
    pub label: Option<String>,
    /// How many times it went round.
    pub count: u32,
    /// How many times it may, when the workflow says.
    pub budget: Option<u32>,
}

/// A link a node offers, such as its PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Link {
    /// What it says.
    pub label: String,
    /// Where it goes.
    pub url: String,
}

/// Where one node of a run stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NodeState {
    /// The node, by its id in the shape.
    pub id: String,
    /// Where it stands.
    pub status: NodeStatus,
    /// Whether `maxd` guessed the status rather than read it.
    pub provisional: bool,
    /// The workflow's last note on it.
    pub note: Option<String>,
    /// Its badges, in order.
    pub badges: Vec<Badge>,
    /// Its attempts, oldest first.
    pub attempts: Vec<AttemptSpan>,
    /// The agents bound to it.
    pub agents: Vec<AgentId>,
    /// Whether nothing has happened on it for a while.
    pub quiet: bool,
    /// The links it offers.
    pub links: Vec<Link>,
}

/// One node of a run's shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NodeShape {
    /// Its id, unique in the run.
    pub id: String,
    /// What the canvas calls it.
    pub label: String,
    /// The group it sits in, if any.
    pub group: Option<String>,
    /// The lane it sits in, if the workflow names lanes.
    pub lane: Option<String>,
    /// The device or container it runs on, when that isn't the run's own.
    pub host: Option<String>,
}

/// One edge of a run's shape: `to` needs `from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct EdgeShape {
    /// The node needed.
    pub from: String,
    /// The node that needs it.
    pub to: String,
}

/// A group of nodes the canvas can fold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GroupShape {
    /// Its id, unique in the run.
    pub id: String,
    /// What the canvas calls it.
    pub label: String,
    /// Whether it starts folded.
    pub collapsed: bool,
}

/// What a run looks like, apart from where it stands. Sent again only
/// when it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunShape {
    /// Its nodes, in the workflow's order.
    pub nodes: Vec<NodeShape>,
    /// Its edges.
    pub edges: Vec<EdgeShape>,
    /// Its groups.
    pub groups: Vec<GroupShape>,
    /// A hash of the rest, so a client can tell a shape it has.
    pub hash: String,
}

/// The line above the canvas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunHeader {
    /// What the run is called.
    pub label: String,
    /// The run's own status, in the standard's words.
    pub status: NodeStatus,
    /// How many nodes are done, and how many there are.
    pub progress: Progress,
    /// The descriptor's header lines.
    #[serde(default)]
    pub lines: Vec<Badge>,
}

/// How far a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Progress {
    /// Nodes done or skipped.
    pub done: u32,
    /// Nodes in the run, leaving out omitted ones.
    pub total: u32,
}

/// Something wrong with how `maxd` reads a run, such as a file that
/// doesn't parse. It keeps the last good state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunNotice {
    /// What's wrong, in a sentence.
    pub text: String,
    /// How it reads.
    pub tone: Tone,
}

/// The whole of a run, sent when a client opens it and when its shape
/// changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunSnapshot {
    /// The run.
    pub key: RunKey,
    /// Counts up with each change, so a patch applies to the snapshot it
    /// follows.
    #[specta(type = specta_typescript::Number)]
    pub rev: u64,
    /// What it looks like.
    pub shape: RunShape,
    /// Where each node stands.
    pub nodes: Vec<NodeState>,
    /// Its loops.
    pub loops: Vec<LoopState>,
    /// The line above the canvas.
    pub header: RunHeader,
    /// What's wrong with reading it, if anything.
    pub notice: Option<RunNotice>,
}

/// A change to an open run, with the same shape and notice. A change to
/// either comes as a reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunPatch {
    /// The run.
    pub key: RunKey,
    /// The revision it brings the run to, one past the last.
    #[specta(type = specta_typescript::Number)]
    pub rev: u64,
    /// The nodes that changed, whole.
    pub nodes: Vec<NodeState>,
    /// Every loop, when any changed.
    pub loops: Option<Vec<LoopState>>,
    /// The header, when it changed.
    pub header: Option<RunHeader>,
}

/// What a client gets about an open run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", content = "run", rename_all = "snake_case")]
pub enum WorkflowUpdate {
    /// The whole run, to replace what the client has.
    Reset(RunSnapshot),
    /// A change to what it has.
    Patch(RunPatch),
}

/// One list in a node's detail, such as the files it touched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DetailList {
    /// What the list is.
    pub label: String,
    /// What's in it.
    pub items: Vec<String>,
}

/// More about one node, asked for when you open it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NodeDetail {
    /// Its lists.
    pub lists: Vec<DetailList>,
}

/// One run, as the view lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct WorkflowRun {
    /// The run's id, from [`RunKey::id`].
    pub id: WorkflowRunId,
    /// The run.
    pub key: RunKey,
    /// What it's called.
    pub label: String,
    /// How far it is.
    pub progress: Progress,
    /// The nodes busy now.
    pub current: Vec<String>,
    /// Whether a node, or the run's lead, waits for you.
    pub needs_you: bool,
    /// The agents that lead it.
    pub orchestrators: Vec<AgentId>,
    /// When anything last happened.
    pub last_activity: Option<Timestamp>,
    /// Whether it finished.
    pub done: bool,
    /// Whether it finished by failing. Sent since
    /// [`WORKFLOW_RUN_AGENTS_SINCE`](crate::message::WORKFLOW_RUN_AGENTS_SINCE).
    #[serde(default)]
    pub failed: bool,
    /// The agents bound to its nodes, other than its leads. Sent since
    /// [`WORKFLOW_RUN_AGENTS_SINCE`](crate::message::WORKFLOW_RUN_AGENTS_SINCE).
    #[serde(default)]
    pub agents: Vec<AgentId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key() -> RunKey {
        RunKey {
            device: DeviceId::derive("devbox"),
            project: ProjectId::derive("git:github.com/owner/repo"),
            workflow: "devflow".into(),
            run: "run-1".into(),
        }
    }

    #[test]
    fn a_run_id_depends_on_every_part_of_its_key() {
        let id = key().id();
        assert_eq!(id, key().id());
        let other = RunKey {
            run: "run-2".into(),
            ..key()
        };
        assert_ne!(id, other.id());
        let other = RunKey {
            workflow: "release".into(),
            ..key()
        };
        assert_ne!(id, other.id());
    }

    #[test]
    fn statuses_use_the_standard_words() {
        assert_eq!(
            serde_json::to_value(NodeStatus::WaitingHuman).unwrap(),
            json!("waiting_human")
        );
        assert_eq!(
            serde_json::to_value(NodeStatus::WillSkip).unwrap(),
            json!("will_skip")
        );
    }

    #[test]
    fn a_wait_time_is_left_out_when_none_and_read_when_missing() {
        let span = AttemptSpan {
            start: Timestamp(1_000),
            end: None,
            end_status: None,
            agent: None,
            waiting_since: None,
        };
        let value = serde_json::to_value(&span).unwrap();
        assert!(
            value.get("waiting_since").is_none(),
            "older peers see no change"
        );
        assert_eq!(serde_json::from_value::<AttemptSpan>(value).unwrap(), span);
        let waiting = AttemptSpan {
            waiting_since: Some(Timestamp(2_000)),
            ..span
        };
        let value = serde_json::to_value(&waiting).unwrap();
        assert_eq!(value["waiting_since"], 2_000);
        assert_eq!(
            serde_json::from_value::<AttemptSpan>(value).unwrap(),
            waiting
        );
    }

    #[test]
    fn updates_round_trip() {
        let header = RunHeader {
            label: "Add sign-in".into(),
            status: NodeStatus::Busy,
            progress: Progress { done: 1, total: 3 },
            lines: Vec::new(),
        };
        let reset = WorkflowUpdate::Reset(RunSnapshot {
            key: key(),
            rev: 1,
            shape: RunShape {
                nodes: vec![NodeShape {
                    id: "plan".into(),
                    label: "Plan".into(),
                    group: None,
                    lane: None,
                    host: None,
                }],
                edges: vec![EdgeShape {
                    from: "plan".into(),
                    to: "build".into(),
                }],
                groups: Vec::new(),
                hash: "abc".into(),
            },
            nodes: vec![NodeState {
                id: "plan".into(),
                status: NodeStatus::Done,
                provisional: false,
                note: None,
                badges: vec![Badge {
                    text: "tier 2".into(),
                    tone: Tone::Info,
                }],
                attempts: Vec::new(),
                agents: Vec::new(),
                quiet: false,
                links: Vec::new(),
            }],
            loops: Vec::new(),
            header: header.clone(),
            notice: None,
        });
        let value = serde_json::to_value(&reset).unwrap();
        assert_eq!(value["kind"], "reset");
        assert_eq!(
            serde_json::from_value::<WorkflowUpdate>(value).unwrap(),
            reset
        );

        let patch = WorkflowUpdate::Patch(RunPatch {
            key: key(),
            rev: 2,
            nodes: Vec::new(),
            loops: None,
            header: Some(header),
        });
        let value = serde_json::to_value(&patch).unwrap();
        assert_eq!(value["kind"], "patch");
        assert_eq!(
            serde_json::from_value::<WorkflowUpdate>(value).unwrap(),
            patch
        );
    }
}

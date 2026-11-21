// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Scheduled tasks. A task lives on one device, its home, whose `maxd`
//! runs it: on a timer, on each request to its webhook, or on events in a
//! GitHub repository. Each run starts an agent or sends one a message, as a
//! caller of the agent tools under the task's [`Permission`].
//!
//! [`Permission`]: crate::agents::Permission

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::agents::AgentOrigin;
use crate::ids::{AgentId, DeviceId, PermissionId, TaskId};
use crate::message::AgentSpec;
use crate::time::Timestamp;

/// A scheduled task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Task {
    /// The task.
    pub id: TaskId,
    /// The name you gave it.
    pub name: String,
    /// What the run sends the agent. For a webhook or GitHub events, a
    /// template that takes values from the request.
    pub prompt: String,
    /// What makes it run.
    pub trigger: Trigger,
    /// The agent each run reaches.
    pub target: TaskTarget,
    /// Whether it runs. A paused task keeps its schedule and its webhook.
    pub enabled: bool,
    /// The permission each run is checked against, as it stands then.
    pub permission: PermissionId,
    /// Who made it.
    pub made_by: Maker,
    /// When the timer fires next, set by `maxd`. None for a webhook, GitHub
    /// events or a paused task.
    pub next_run: Option<Timestamp>,
    /// How the last run went, set by `maxd`.
    pub last_run: Option<RunSummary>,
    /// When `maxd` asked you to approve it, while it waits for you, set by
    /// `maxd`. A task an agent makes, turns on or changes waits here, off,
    /// until you approve it on its card or turn it on yourself. A peer
    /// that leaves this out has none.
    #[serde(default)]
    pub approval_asked: Option<Timestamp>,
    /// Set by `maxd` for a task you made that the dispatcher saved at
    /// once, for a request of yours, without a plan you confirmed. Its
    /// runs say the dispatcher made it. One from an older `maxd` has none.
    /// Since protocol 55.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_dispatcher: bool,
}

/// What makes a task run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// Every so many minutes.
    Every {
        /// At least 1.
        minutes: u32,
    },
    /// At a time of day, on some days of the week.
    At {
        /// The time, in `zone`.
        time: TimeOfDay,
        /// The days. None means every day.
        days: Days,
        /// The time zone the time is in.
        zone: Zone,
    },
    /// On each request to the task's webhook.
    Hook(Hook),
    /// On events in a GitHub repository, which `maxd` polls for.
    GithubEvents {
        /// The repository, as "owner/name".
        repo: String,
        /// The event types, such as `issues` or `pull_request`, as GitHub's
        /// webhooks name them. `maxd` maps each polled event to these.
        events: Vec<String>,
        /// Which events run the task, the same way a webhook's do.
        rules: HookRules,
    },
}

/// A time of day, to the minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TimeOfDay {
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
}

/// The days of the week a task runs on. None means every day.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(transparent)]
pub struct Days(pub Vec<Weekday>);

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

/// An IANA time zone name, such as `Europe/Amsterdam`. The app takes it
/// from the Mac when you make the task, so a task on a device set to UTC
/// still runs at the time you typed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(transparent)]
pub struct Zone(pub String);

/// The agent a task's runs reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskTarget {
    /// An agent that already runs. Each run queues the prompt as a message,
    /// which it takes when its turn ends.
    Agent {
        /// The device the agent is on.
        device: DeviceId,
        /// The agent.
        agent: AgentId,
    },
    /// A new agent for each run, with the prompt as its first message.
    New {
        /// The device it starts on.
        device: DeviceId,
        /// What it should be. `first` and `origin` are set by the run.
        spec: Box<AgentSpec>,
    },
}

/// Who made a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Maker {
    /// You, in the app.
    User,
    /// An agent or an access key, through the agent tools.
    Tools {
        /// The caller and the permission it called under.
        origin: AgentOrigin,
    },
}

/// A task's webhook. Its URL holds a token that `maxd` keeps only a hash
/// of, so the URL shows once, when it's made or rotated. A new one is off,
/// unsigned and holds nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Hook {
    /// Whether the webhook takes requests, set with
    /// [`crate::message::ClientMessage::SetHookOn`].
    pub on: bool,
    /// The signature each request must carry, if any.
    pub signature: Option<SignatureCheck>,
    /// Which requests run the task.
    pub rules: HookRules,
    /// Whether to hold requests up to 24 hours while the target's device is
    /// offline.
    pub hold: bool,
}

/// An HMAC-SHA256 check on a request's raw body. The secret is set with
/// [`crate::message::ClientMessage::SetHookSecret`] and never comes back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SignatureCheck {
    /// The header with the signature, such as `x-hub-signature-256`.
    pub header: String,
    /// How the digest is written.
    pub encoding: DigestEncoding,
    /// The text before the digest, such as `sha256=`. Empty for none.
    pub prefix: String,
    /// Whether the device holds a secret for it, set by `maxd`.
    pub has_secret: bool,
}

/// How a signature's digest is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DigestEncoding {
    /// Lowercase hex.
    Hex,
    /// Standard base64.
    Base64,
}

/// Which requests or events run a task, for a webhook and GitHub events
/// alike. Each is off when unset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct HookRules {
    /// All must match for a request to run the task.
    pub filters: Vec<Filter>,
    /// A template whose value, seen once in the last 7 days, makes a
    /// repeat a duplicate.
    pub replay_key: Option<String>,
    /// A template whose value groups requests that arrive within
    /// `coalesce_secs` into one run.
    pub coalesce_key: Option<String>,
    /// How long a coalesce window stays open, in seconds.
    pub coalesce_secs: Option<u32>,
    /// Whether each request that joins a coalesce window moves its run to
    /// `coalesce_secs` after that request. A task from before it leaves
    /// this out, and its windows keep the time they opened with.
    #[serde(default)]
    pub coalesce_restart: bool,
    /// The longest a window that restarts waits after its first request,
    /// in seconds. None means two hours.
    #[serde(default)]
    pub coalesce_max_secs: Option<u32>,
    /// Skip a request older than this many minutes, when its sender stamps
    /// a time or it was held.
    pub max_age_minutes: Option<u32>,
    /// The most runs a day. None means no cap.
    pub daily_runs: Option<u32>,
    /// The free text fields the prompt may hold, named as its placeholders
    /// name them, such as `body.pull_request.title`. Each fills in framed
    /// as untrusted data. A GitHub event's structural fields, such as
    /// numbers, logins and URLs, need no place here. Any other webhook's
    /// fields, headers and query included, are all free text. A hook from
    /// before the list leaves it out and fills in every field framed, and
    /// a GitHub events task reads that as none.
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    /// The author associations, such as `OWNER`, that a GitHub event's author
    /// must have for it to run the task. Empty lets anyone's events run it, as a
    /// task from before the list does. An event with no author, such as a
    /// push or a star, runs either way.
    #[serde(default)]
    pub authors: Vec<String>,
}

/// The keys of a GitHub event whose values are structural: numbers, logins,
/// SHAs, URLs, actions, repository names and times. Any other key, such as
/// a title, a body, a comment or a branch name, holds free text, which
/// whoever opened the PR or issue wrote. A value that doesn't look like
/// what its key holds, such as a URL off the repository, fills in framed.
pub const STRUCTURAL_KEYS: &[&str] = &[
    "action",
    "additions",
    "after",
    "author_association",
    "before",
    "changed_files",
    "commit_id",
    "commits",
    "comments",
    "conclusion",
    "created",
    "deleted",
    "deletions",
    "distinct",
    "draft",
    "fork",
    "forced",
    "full_name",
    "head",
    "id",
    "locked",
    "login",
    "merged",
    "mergeable",
    "mergeable_state",
    "number",
    "private",
    "ref_type",
    "review_comments",
    "sha",
    "size",
    "state",
    "status",
    "timestamp",
    "type",
    "url",
];

/// The endings that make any other GitHub event key structural, such as
/// `html_url`, `merged_at`, `merge_commit_sha` and `node_id`.
pub const STRUCTURAL_SUFFIXES: &[&str] = &["_url", "_at", "_sha", "_id"];

/// One test on a request, such as `headers.x-github-event` equals `push`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Filter {
    /// What it tests, starting `body.`, `headers.` or `query.`. A `[]`
    /// segment matches any item of a list.
    pub path: String,
    /// How it tests it.
    pub test: FilterTest,
}

/// How a [`Filter`] tests its value. Values compare as text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FilterTest {
    /// It equals `value`.
    Equals {
        /// The value.
        value: String,
    },
    /// No item equals `value`.
    NotEquals {
        /// The value.
        value: String,
    },
    /// It starts with `value`.
    StartsWith {
        /// The start.
        value: String,
    },
    /// It equals one of `values`.
    In {
        /// The values.
        values: Vec<String>,
    },
    /// It's there.
    Exists,
}

/// How a run went, in short, for the task list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunSummary {
    /// When it started.
    pub at: Timestamp,
    /// How it went.
    pub outcome: RunOutcome,
}

/// One run of a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Run {
    /// The task.
    pub task: TaskId,
    /// What made it run.
    pub cause: RunCause,
    /// When it started.
    pub started: Timestamp,
    /// When it ended, once it has.
    pub ended: Option<Timestamp>,
    /// How it went.
    pub outcome: RunOutcome,
    /// The agent it started or sent the prompt to, so you can open it.
    pub agent: Option<AgentId>,
    /// The webhook request or GitHub event behind it.
    pub delivery: Option<String>,
    /// Why it failed, for a person to read.
    pub error: Option<String>,
}

/// What made a run happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunCause {
    /// Its timer.
    Timer,
    /// Run now, from the app or a tool.
    Now,
    /// A request to its webhook.
    Hook,
    /// A GitHub event.
    Github,
}

/// How a run went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// The agent's turn hasn't ended yet.
    Running,
    /// The turn ended.
    Done,
    /// The run or the turn failed.
    Failed,
    /// The turn was stopped.
    Stopped,
    /// The device was asleep or off when it was due, so it didn't run.
    Missed,
}

/// One request to a webhook, or one GitHub event, and what came of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TaskDelivery {
    /// The request, or the GitHub event's ID.
    pub id: String,
    /// The task.
    pub task: TaskId,
    /// When it arrived.
    pub received: Timestamp,
    /// The HTTP method.
    pub method: String,
    /// Its content type, if it named one.
    pub content_type: Option<String>,
    /// How big its body was.
    pub body_bytes: u32,
    /// What came of it.
    pub outcome: DeliveryOutcome,
    /// Whether its signature checked out.
    pub signature_verified: bool,
    /// The template's fields it lacked.
    pub missing_fields: Vec<String>,
    /// The template's free text fields the task doesn't allow, which the
    /// prompt left out.
    #[serde(default)]
    pub refused_fields: Vec<String>,
    /// What went wrong, for a person to read.
    pub error: Option<String>,
}

/// What came of a delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    /// It queued a run.
    Accepted,
    /// It queued a run, which then failed to start.
    DispatchFailed,
    /// Its replay key was seen in the last 7 days.
    Duplicate,
    /// It joined an open coalesce window.
    Coalesced,
    /// Its signature didn't check out.
    RejectedSignature,
    /// The task is paused.
    Disabled,
    /// The task had too many requests this minute or runs today.
    RateLimited,
    /// It was older than the task takes, or held too long.
    Expired,
    /// A filter didn't match.
    Filtered,
    /// The prompt it made was too long.
    PromptTooLong,
    /// Too many runs were waiting.
    QueueFull,
    /// The target's device is offline, and the request waits for it.
    Held,
    /// The target's device is offline, and the hook doesn't hold requests.
    TargetOffline,
}

/// How a device's webhooks are reached, which each device picks for
/// itself. A hook's URL is this base and `/maxhq/<task>/<token>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookAddress {
    /// Only from the device itself, at `http://127.0.0.1:<port>`.
    #[default]
    Local,
    /// Over Tailscale Funnel, which `maxd` turns on for its own paths.
    Funnel,
    /// Under a base URL you point at the hooks port, such as a tunnel's or a
    /// proxy's. `maxd` can't check that it reaches the device.
    Own {
        /// Such as `https://hooks.example.com`.
        base: String,
    },
}

/// Whether a device's webhooks can be reached, at the address it picked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FunnelState {
    /// No webhook is on.
    Off,
    /// Webhooks are reachable under `url`.
    On {
        /// The base of the device's hook URLs, such as
        /// `https://<device>.<tailnet>.ts.net`.
        url: String,
    },
    /// Something needs setting up first.
    NeedsSetup {
        /// What to do, in order.
        steps: Vec<FunnelStep>,
    },
}

/// One thing to set up before the address works.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FunnelStep {
    /// What's missing, for a person to read.
    pub reason: String,
    /// A command that sets it up, for you to run, if there is one.
    pub command: Option<String>,
}

/// The tasks a device holds, and whether its webhooks can be reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DeviceTasks {
    /// The device.
    pub device: DeviceId,
    /// Its tasks.
    pub tasks: Vec<Task>,
    /// How its webhooks are reached. A `maxd` from before the choice
    /// leaves this out, and reads as this device only.
    #[serde(default)]
    pub address: HookAddress,
    /// The port its hook listener takes requests on, for a tunnel or proxy
    /// to point at.
    #[serde(default)]
    pub port: Option<u16>,
    /// Whether they can be reached there.
    pub funnel: FunnelState,
    /// Why some of its tasks can't run now, such as the GitHub App missing
    /// from a task's repository.
    pub problems: Vec<TaskProblem>,
    /// How each enabled GitHub events task hears of events. A `maxd` from
    /// before webhooks leaves this out.
    #[serde(default)]
    pub webhooks: Vec<TaskWebhook>,
}

/// Why a task can't run now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TaskProblem {
    /// The task.
    pub task: TaskId,
    /// Why, in words for the user.
    pub reason: String,
    /// A page that fixes it, such as the GitHub App's permissions.
    #[serde(default)]
    pub link: Option<String>,
}

/// How a GitHub events task hears of events: at once through a repository
/// webhook `maxd` made, or by polling every minute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TaskWebhook {
    /// The task.
    pub task: TaskId,
    /// Whether the webhook delivered a verified request in the last hour,
    /// so events come at once and polling slows to a backstop.
    pub instant: bool,
    /// When it last did.
    pub last_delivery: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_task_round_trips() {
        let task = Task {
            id: TaskId::derive("t"),
            name: "Triage".into(),
            prompt: "Look at {{body.issue.title}}".into(),
            trigger: Trigger::Hook(Hook {
                rules: HookRules {
                    filters: vec![Filter {
                        path: "headers.x-github-event".into(),
                        test: FilterTest::Equals {
                            value: "issues".into(),
                        },
                    }],
                    ..HookRules::default()
                },
                ..Hook::default()
            }),
            target: TaskTarget::Agent {
                device: DeviceId::derive("d"),
                agent: AgentId::derive("a"),
            },
            enabled: true,
            permission: PermissionId::derive("p"),
            made_by: Maker::User,
            next_run: None,
            last_run: None,
            approval_asked: None,
            by_dispatcher: false,
        };
        let json = serde_json::to_value(&task).expect("it serializes");
        assert_eq!(json["trigger"]["type"], "hook");
        assert_eq!(json["trigger"]["hold"], false);
        assert_eq!(
            json["trigger"]["rules"]["filters"][0]["test"]["op"],
            "equals"
        );
        let back: Task = serde_json::from_value(json).expect("it reads back");
        assert_eq!(back, task);
    }
}

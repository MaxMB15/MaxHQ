// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns what a workspace container shows into the project, workspace and
//! agent the hub holds. Nothing here reads anything, so tests drive it with
//! fixtures and a fixed clock.

use std::collections::HashMap;
use std::time::Duration;

use maxhq_core::names;
use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId, DeviceId};
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, Control, Project, Vendor, Workspace, WorkspaceLocation,
};
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;

use super::api::Instance;
use super::time;

/// How long after a container starts its agent counts as starting while
/// the hook hasn't written a status. Agents start at boot, and their
/// SessionStart hook runs about ten seconds in.
const START_GRACE: Duration = Duration::from_secs(60);

/// How long the vendor keeps a finished conversation cached, by the same
/// rule the Claude observer follows.
const CACHE_WARM: Duration = Duration::from_secs(5 * 60);

/// How long Claude sits at the prompt before it sends the `idle_prompt`
/// reminder.
const IDLE_PROMPT_DELAY: Duration = Duration::from_secs(60);

/// How far a reported time may sit before the turn end the reminder
/// implies and still be that turn's end. The hook writes whole seconds.
const SLACK: Duration = Duration::from_secs(5);

/// The agent's home in every workspace image, for a session with no folder.
const HOME: &str = "/home/agent";

/// What the hook writes to `/run/agent-state/status.json` on every event.
/// The file lives on a tmpfs, so a container starts without one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StatusFile {
    /// `claude` or `codex`.
    pub agent: String,
    /// `working`, `idle`, `blocked`, `exited` or `unknown`.
    pub status: String,
    /// The hook event that wrote it, such as `Stop`.
    pub event: String,
    /// A notification's type, such as `permission_prompt`. Empty for other
    /// events.
    pub notification: String,
    /// When the hook ran, from `date -Is`.
    pub updated: String,
}

/// What the hook writes to session.json whenever an event carries the
/// session ID. The file outlives restarts, since resuming needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SessionFile {
    /// `claude` or `codex`.
    pub agent: String,
    /// The CLI's own session or thread ID.
    pub session_id: String,
    /// The folder the CLI runs in, inside the container.
    pub cwd: String,
    /// When the hook last wrote it.
    pub updated: String,
}

/// Everything read about one workspace container in a pass.
pub struct Seen<'a> {
    /// The container.
    pub instance: &'a Instance,
    /// Its session file.
    pub session: &'a SessionFile,
    /// Its status file. None when there isn't one, and always None for a
    /// container that isn't running.
    pub status: Option<&'a StatusFile>,
    /// False when a read failed and an older copy stands in.
    pub fresh: bool,
    /// When the container stopped, if the observer saw it go.
    pub down_since: Option<Timestamp>,
}

/// The entities one container adds to the hub.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The repository the container holds.
    pub project: Project,
    /// The folder the agent runs in.
    pub workspace: Workspace,
    /// The agent.
    pub agent: Agent,
    /// The prompt it's waiting on, while it waits.
    pub attention: Option<AttentionEvent>,
}

/// The entities for one container on `device`. `history` holds the last
/// activity time each agent reported, so a file that says nothing new
/// doesn't move it. None when the container names no vendor this knows or
/// its session file has no session ID.
pub fn report(
    device: DeviceId,
    seen: &Seen<'_>,
    history: &HashMap<AgentId, Timestamp>,
    now: Timestamp,
) -> Option<Report> {
    let instance = seen.instance;
    let session = seen.session;
    let vendor = [
        instance.config("user.agent"),
        Some(session.agent.as_str()),
        seen.status.map(|s| s.agent.as_str()),
    ]
    .into_iter()
    .flatten()
    .find_map(vendor)?;
    if session.session_id.is_empty() {
        return None;
    }
    let id = names::agent(vendor, &session.session_id);
    let title = instance.config("user.title").unwrap_or(&instance.name);

    let project = project(device, instance);
    let path = if session.cwd.starts_with('/') {
        session.cwd.clone()
    } else {
        HOME.to_owned()
    };
    let location = WorkspaceLocation::Container {
        instance: instance.name.clone(),
        path,
    };
    let workspace = Workspace {
        id: names::workspace(device, &location),
        project: Some(project.id),
        device,
        location,
        managed: None,
        clone: None,
    };

    let state = state(seen, id, title, history.get(&id).copied(), now);
    let last = state.last_activity_at;
    let started_at = instance
        .started_at()
        .or_else(|| instance.config("user.created").and_then(time::parse))
        .unwrap_or(last)
        .min(last);
    let agent = Agent {
        id,
        workspace: workspace.id,
        parent: None,
        vendor,
        account: None,
        model: None,
        permission: None,
        session: None,
        vendor_session_id: Some(session.session_id.clone()),
        control: Control::Observed,
        pid: None,
        link: state.link,
        title: title.to_owned(),
        activity: state.activity,
        context: None,
        started_at,
        last_activity_at: last,
        branch: None,
        inbox: Default::default(),
        origin: None,
        pulls: Vec::new(),
        taking_over: false,
        asleep: false,
        effort: None,
        running: None,
    };
    Some(Report {
        project,
        workspace,
        agent,
        attention: state.attention,
    })
}

/// The project for a container's `user.repo`, which holds `owner/name` on
/// GitHub, as `new-workspace` clones it. It goes through the same naming
/// as a checkout's remote, so a container and a Mac checkout of one repo
/// share a project.
pub fn project(device: DeviceId, instance: &Instance) -> Project {
    let repo = instance.config("user.repo").map(|repo| {
        if repo.contains("://") || repo.contains('@') {
            repo.to_owned()
        } else {
            format!("https://github.com/{repo}.git")
        }
    });
    let named = names::project(device, &instance.name, repo.as_deref());
    Project {
        id: named.id,
        name: named.name,
        repo,
        default_vendor: None,
        default_account: None,
        default_workspace: None,
    }
}

/// The vendor a `user.agent` or state file `agent` value names.
fn vendor(name: &str) -> Option<Vendor> {
    match name {
        "claude" => Some(Vendor::Claude),
        "codex" => Some(Vendor::Codex),
        _ => None,
    }
}

/// An agent's state, from its container and status file.
struct State {
    activity: Activity,
    link: AgentLink,
    last_activity_at: Timestamp,
    attention: Option<AttentionEvent>,
}

/// Maps the hook's status to an activity. The hook writes `working` from
/// a prompt to the end of the turn, `idle` at session start and turn end,
/// `blocked` for a notification other than the idle reminder, `exited` at
/// session end and `unknown` for an event it doesn't know.
fn state(
    seen: &Seen<'_>,
    id: AgentId,
    title: &str,
    previous: Option<Timestamp>,
    now: Timestamp,
) -> State {
    let instance = seen.instance;
    // The last activity when the files don't give a newer one: what was
    // reported before, or else when the session file last changed.
    let known = previous
        .or_else(|| time::parse(&seen.session.updated))
        .or_else(|| instance.started_at())
        .unwrap_or(now);
    let plain = |activity, link| State {
        activity,
        link,
        last_activity_at: known,
        attention: None,
    };

    if !instance.is_running() {
        // The agent runs inside the container, so it can't be reached
        // until the container starts again, and decision N-2 shows the
        // device answering with an agent that doesn't as purple.
        let reason = match instance.status.as_str() {
            "Stopped" => "container stopped".to_owned(),
            other => format!("container {}", other.to_lowercase()),
        };
        let link = AgentLink::Down {
            reason,
            since: seen.down_since,
        };
        return plain(Activity::NotAccessible, link);
    }

    let Some(status) = seen.status else {
        // No status file means the hook hasn't run since the container
        // started, since the file lives on a tmpfs. Shortly after a start
        // that's the agent launching. Later, the agent can't be busy or
        // waiting on a prompt, because the hook writes the file at session
        // start, at every prompt and on every notification. Idle is the
        // closest state that claims nothing more, and the link says
        // unknown rather than live, since nothing has shown the CLI runs.
        let launching = instance
            .started_at()
            .is_some_and(|at| now < at.after(START_GRACE));
        let activity = if launching {
            Activity::Starting
        } else {
            Activity::Idle
        };
        return plain(activity, AgentLink::Unknown);
    };

    let at = time::parse(&status.updated).unwrap_or(known);
    let warm = |end: Timestamp| {
        let until = end.after(CACHE_WARM);
        if now < until {
            Activity::IdleCacheWarm { warm_until: until }
        } else {
            Activity::Idle
        }
    };
    let mut state = match (
        status.status.as_str(),
        status.event.as_str(),
        status.notification.as_str(),
    ) {
        ("working", _, _) => State {
            activity: Activity::Busy,
            link: AgentLink::Live,
            last_activity_at: at,
            attention: None,
        },
        // A new or resumed session hasn't finished a turn, so the time
        // stays where it was, and a restart doesn't mark the agent unread.
        ("idle", "SessionStart", _) => plain(Activity::Idle, AgentLink::Live),
        // The idle reminder comes a minute after the turn ended. When the
        // turn's own end was already reported, that time stands, so the
        // reminder doesn't mark a turn you've seen unread again.
        ("idle", "Notification", _) => {
            let end = Timestamp(at.0.saturating_sub(millis(IDLE_PROMPT_DELAY)));
            let end = match previous {
                Some(reported) if reported.after(SLACK) >= end => reported.min(end),
                _ => end,
            };
            State {
                activity: warm(end),
                link: AgentLink::Live,
                last_activity_at: end,
                attention: None,
            }
        }
        // The end of a turn, from Stop, TurnComplete or TurnAborted.
        ("idle", _, _) => State {
            activity: warm(at),
            link: AgentLink::Live,
            last_activity_at: at,
            attention: None,
        },
        // Claude says it logged in. Nothing waits on you.
        ("blocked", _, "auth_success") => plain(Activity::Idle, AgentLink::Live),
        // A permission prompt or a question. The hook has no event for an
        // answer, so this stays until the turn ends, even while the agent
        // works on after you answered in its terminal.
        ("blocked", _, kind) => {
            let attention = attention(id, title, kind, &status.updated, at);
            State {
                activity: Activity::NeedsInput {
                    attention: attention.id,
                },
                link: AgentLink::Live,
                last_activity_at: at,
                attention: Some(attention),
            }
        }
        ("exited", _, _) => State {
            activity: Activity::Error {
                reason: "exited".to_owned(),
            },
            link: AgentLink::Down {
                reason: "exited".to_owned(),
                since: Some(at),
            },
            last_activity_at: at,
            attention: None,
        },
        // The hook ran for an event it doesn't map, so the CLI runs but
        // its state isn't known.
        (other, _, _) => {
            tracing::debug!(status = other, "unknown workspace agent status");
            plain(Activity::Idle, AgentLink::Unknown)
        }
    };
    if !seen.fresh {
        state.link = AgentLink::Unknown;
    }
    state
}

/// The attention event for a blocked agent. Its ID comes from the agent
/// and the time the hook wrote, so it stays the same on every pass.
fn attention(
    agent: AgentId,
    title: &str,
    kind: &str,
    updated: &str,
    at: Timestamp,
) -> AttentionEvent {
    let (kind, summary) = if kind == "permission_prompt" {
        (
            AttentionKind::Permission,
            format!("{title} is asking for permission"),
        )
    } else {
        (
            AttentionKind::Question,
            format!("{title} is waiting for your answer"),
        )
    };
    AttentionEvent {
        id: AttentionId::derive(&format!("incus:{agent}:{updated}")),
        agent,
        kind,
        summary,
        // You answer in the agent's own terminal, so there's nothing to
        // pick here.
        options: Vec::new(),
        accepts_text: false,
        created_at: at,
        expires_at: None,
    }
}

/// A duration in milliseconds, for adding to a timestamp.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCES: &str = include_str!("../../../tests/fixtures/incus/instances.json");

    fn node() -> DeviceId {
        DeviceId::derive("devbox")
    }

    fn instance(name: &str) -> Instance {
        #[derive(Deserialize)]
        struct Sync {
            metadata: Vec<Instance>,
        }
        let list: Sync = serde_json::from_str(INSTANCES).unwrap();
        list.metadata
            .into_iter()
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("no {name} in the fixture"))
    }

    fn session(name: &str) -> SessionFile {
        let text = match name {
            "harbor" => include_str!("../../../tests/fixtures/incus/session-harbor.json"),
            "lantern" => include_str!("../../../tests/fixtures/incus/session-lantern.json"),
            "quill" => include_str!("../../../tests/fixtures/incus/session-quill.json"),
            "tidepool" => include_str!("../../../tests/fixtures/incus/session-tidepool.json"),
            _ => panic!("no session for {name}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn status(status: &str, event: &str, notification: &str, updated: &str) -> StatusFile {
        StatusFile {
            agent: "claude".into(),
            status: status.into(),
            event: event.into(),
            notification: notification.into(),
            updated: updated.into(),
        }
    }

    fn seen<'a>(
        instance: &'a Instance,
        session: &'a SessionFile,
        status: Option<&'a StatusFile>,
    ) -> Seen<'a> {
        Seen {
            instance,
            session,
            status,
            fresh: true,
            down_since: None,
        }
    }

    fn at(text: &str) -> Timestamp {
        time::parse(text).unwrap()
    }

    #[test]
    fn reads_the_state_files() {
        let status: StatusFile = serde_json::from_str(include_str!(
            "../../../tests/fixtures/incus/status-lantern.json"
        ))
        .unwrap();
        assert_eq!(
            status,
            self::status(
                "idle",
                "Notification",
                "idle_prompt",
                "2026-10-24T09:01:03+00:00"
            )
        );
        let lantern = session("lantern");
        assert_eq!(lantern.agent, "claude");
        assert_eq!(lantern.session_id, "0c9b8a7d-6e5f-4a3b-8c2d-1e0f9a8b7c6d");
        assert_eq!(lantern.cwd, "/home/agent/Lantern/docs/plans");
        assert_eq!(lantern.updated, "2026-10-24T09:01:03+00:00");
        // Fields the hook may add later don't break the read.
        let newer: SessionFile =
            serde_json::from_str(r#"{"session_id":"s","pid":7,"cwd":"/w"}"#).unwrap();
        assert_eq!(newer.session_id, "s");
    }

    #[test]
    fn a_running_container_is_an_observed_agent() {
        let lantern = instance("ws-lantern");
        let session = session("lantern");
        let file = status(
            "working",
            "UserPromptSubmit",
            "",
            "2026-10-24T09:00:00+00:00",
        );
        let now = at("2026-10-24T09:00:01Z");
        let report = report(
            node(),
            &seen(&lantern, &session, Some(&file)),
            &HashMap::new(),
            now,
        )
        .unwrap();

        let agent = &report.agent;
        assert_eq!(agent.id, names::agent(Vendor::Claude, &session.session_id));
        assert_eq!(agent.vendor, Vendor::Claude);
        assert_eq!(agent.control, Control::Observed);
        assert_eq!(
            agent.vendor_session_id.as_deref(),
            Some(session.session_id.as_str())
        );
        assert_eq!(agent.title, "lantern");
        assert_eq!(agent.activity, Activity::Busy);
        assert_eq!(agent.link, AgentLink::Live);
        assert_eq!(agent.last_activity_at, at("2026-10-24T09:00:00Z"));
        // The container last started before this turn.
        assert_eq!(agent.started_at, at("2026-10-23T04:03:13.744Z"));
        assert_eq!(agent.workspace, report.workspace.id);

        assert_eq!(
            report.workspace.location,
            WorkspaceLocation::Container {
                instance: "ws-lantern".into(),
                path: "/home/agent/Lantern/docs/plans".into(),
            }
        );
        assert_eq!(report.workspace.device, node());
        assert_eq!(report.workspace.project, Some(report.project.id));
        assert_eq!(report.project.name, "lantern");
        assert_eq!(
            report.project.repo.as_deref(),
            Some("https://github.com/example-org/Lantern.git")
        );
    }

    #[test]
    fn maps_each_hook_status() {
        let lantern = instance("ws-lantern");
        let session = session("lantern");
        let updated = "2026-10-24T09:00:00+00:00";
        let then = at(updated);
        let minute = Duration::from_secs(60);
        let soon = then.after(Duration::from_secs(10));
        let later = then.after(10 * minute);
        let cases = [
            (
                status("working", "UserPromptSubmit", "", updated),
                soon,
                Activity::Busy,
                then,
            ),
            (
                status("idle", "Stop", "", updated),
                soon,
                Activity::IdleCacheWarm {
                    warm_until: then.after(5 * minute),
                },
                then,
            ),
            (
                status("idle", "Stop", "", updated),
                later,
                Activity::Idle,
                then,
            ),
            (
                status("idle", "TurnAborted", "", updated),
                later,
                Activity::Idle,
                then,
            ),
            // The reminder a minute after the turn ended.
            (
                status("idle", "Notification", "idle_prompt", updated),
                soon,
                Activity::IdleCacheWarm {
                    warm_until: then.after(4 * minute),
                },
                Timestamp(then.0 - 60_000),
            ),
            (
                status("blocked", "Notification", "auth_success", updated),
                soon,
                Activity::Idle,
                at(&session.updated),
            ),
            (
                status("unknown", "PreCompact", "", updated),
                soon,
                Activity::Idle,
                at(&session.updated),
            ),
        ];
        for (file, now, activity, last) in cases {
            let report = report(
                node(),
                &seen(&lantern, &session, Some(&file)),
                &HashMap::new(),
                now,
            )
            .unwrap();
            assert_eq!(report.agent.activity, activity, "{file:?}");
            assert_eq!(report.agent.last_activity_at, last, "{file:?}");
            assert_eq!(report.attention, None, "{file:?}");
        }
    }

    #[test]
    fn session_start_and_the_reminder_keep_the_turn_end() {
        let harbor = instance("ws-harbor");
        let session = session("harbor");
        let file = status("idle", "SessionStart", "", "2026-10-23T04:03:23+00:00");
        let now = at("2026-10-23T04:03:30Z");
        let earlier = at("2026-10-22T20:00:00Z");
        let history = HashMap::from([(names::agent(Vendor::Claude, &session.session_id), earlier)]);

        let first = report(
            node(),
            &seen(&harbor, &session, Some(&file)),
            &HashMap::new(),
            now,
        )
        .unwrap();
        assert_eq!(first.agent.activity, Activity::Idle);
        assert_eq!(first.agent.last_activity_at, at("2026-10-23T04:03:23Z"));
        let resumed = report(node(), &seen(&harbor, &session, Some(&file)), &history, now).unwrap();
        assert_eq!(resumed.agent.last_activity_at, earlier);

        let stop = at("2026-10-23T05:00:00Z");
        let history = HashMap::from([(first.agent.id, stop)]);
        let reminder = status(
            "idle",
            "Notification",
            "idle_prompt",
            "2026-10-23T05:01:01+00:00",
        );
        let report = report(
            node(),
            &seen(&harbor, &session, Some(&reminder)),
            &history,
            at("2026-10-23T05:01:02Z"),
        )
        .unwrap();
        assert_eq!(report.agent.last_activity_at, stop);
    }

    #[test]
    fn a_blocked_agent_needs_input() {
        let lantern = instance("ws-lantern");
        let session = session("lantern");
        let updated = "2026-10-24T09:00:00+00:00";
        let now = at("2026-10-24T09:00:05Z");
        for (notification, kind, summary) in [
            (
                "permission_prompt",
                AttentionKind::Permission,
                "lantern is asking for permission",
            ),
            (
                "elicitation_dialog",
                AttentionKind::Question,
                "lantern is waiting for your answer",
            ),
        ] {
            let file = status("blocked", "Notification", notification, updated);
            let report = report(
                node(),
                &seen(&lantern, &session, Some(&file)),
                &HashMap::new(),
                now,
            )
            .unwrap();
            let attention = report.attention.unwrap();
            assert_eq!(
                report.agent.activity,
                Activity::NeedsInput {
                    attention: attention.id
                }
            );
            assert_eq!(attention.agent, report.agent.id);
            assert_eq!(attention.kind, kind);
            assert_eq!(attention.summary, summary);
            assert_eq!(attention.created_at, at(updated));
            assert!(attention.options.is_empty());
            assert!(!attention.accepts_text);
        }
    }

    #[test]
    fn an_exited_agent_is_down() {
        let lantern = instance("ws-lantern");
        let session = session("lantern");
        let file = status("exited", "SessionEnd", "", "2026-10-24T09:00:00+00:00");
        let report = report(
            node(),
            &seen(&lantern, &session, Some(&file)),
            &HashMap::new(),
            at("2026-10-24T09:10:00Z"),
        )
        .unwrap();
        assert_eq!(
            report.agent.activity,
            Activity::Error {
                reason: "exited".into()
            }
        );
        assert_eq!(
            report.agent.link,
            AgentLink::Down {
                reason: "exited".into(),
                since: Some(at("2026-10-24T09:00:00Z")),
            }
        );
    }

    #[test]
    fn a_stopped_container_is_down() {
        let tidepool = instance("ws-tidepool");
        let session = session("tidepool");
        let since = at("2026-10-25T01:17:05Z");
        let report = report(
            node(),
            &Seen {
                down_since: Some(since),
                ..seen(&tidepool, &session, None)
            },
            &HashMap::new(),
            at("2026-10-25T01:20:00Z"),
        )
        .unwrap();
        assert_eq!(report.agent.activity, Activity::NotAccessible);
        assert_eq!(
            report.agent.link,
            AgentLink::Down {
                reason: "container stopped".into(),
                since: Some(since),
            }
        );
        assert_eq!(report.agent.title, "TIDEPOOL");
        assert_eq!(report.agent.last_activity_at, at(&session.updated));
        assert_eq!(report.project.name, "tidepool");
    }

    #[test]
    fn a_missing_status_file_is_starting_then_unknown() {
        let quill = instance("ws-quill");
        let session = session("quill");
        let started = quill.started_at().unwrap();
        let look =
            |now| report(node(), &seen(&quill, &session, None), &HashMap::new(), now).unwrap();

        let booting = look(started.after(Duration::from_secs(10)));
        assert_eq!(booting.agent.activity, Activity::Starting);
        assert_eq!(booting.agent.link, AgentLink::Unknown);

        let settled = look(started.after(Duration::from_secs(3600)));
        assert_eq!(settled.agent.activity, Activity::Idle);
        assert_eq!(settled.agent.link, AgentLink::Unknown);
        assert_eq!(settled.agent.last_activity_at, at(&session.updated));
        assert_eq!(
            settled.workspace.location,
            WorkspaceLocation::Container {
                instance: "ws-quill".into(),
                path: "/home/agent".into(),
            }
        );
    }

    #[test]
    fn a_stale_read_makes_the_link_unknown() {
        let lantern = instance("ws-lantern");
        let session = session("lantern");
        let file = status(
            "working",
            "UserPromptSubmit",
            "",
            "2026-10-24T09:00:00+00:00",
        );
        let report = report(
            node(),
            &Seen {
                fresh: false,
                ..seen(&lantern, &session, Some(&file))
            },
            &HashMap::new(),
            at("2026-10-24T09:00:05Z"),
        )
        .unwrap();
        assert_eq!(report.agent.activity, Activity::Busy);
        assert_eq!(report.agent.link, AgentLink::Unknown);
    }

    #[test]
    fn merges_with_a_mac_checkout_of_the_same_repo() {
        let mac = DeviceId::derive("mac");
        let checkout = names::project(
            mac,
            "/Users/someone/Code/Lantern",
            Some("git@github.com:example-org/Lantern.git"),
        );
        let container = project(node(), &instance("ws-lantern"));
        assert_eq!(container.id, checkout.id);
        assert_eq!(container.name, checkout.name);
    }

    #[test]
    fn needs_a_vendor_and_a_session() {
        let lantern = instance("ws-lantern");
        let mut session = session("lantern");
        session.session_id.clear();
        let now = at("2026-10-24T09:00:05Z");
        assert_eq!(
            report(
                node(),
                &seen(&lantern, &session, None),
                &HashMap::new(),
                now
            ),
            None
        );

        let mut vault = instance("vault");
        let session = self::session("lantern");
        vault.status = "Running".into();
        let unnamed = SessionFile {
            agent: String::new(),
            ..session.clone()
        };
        assert_eq!(
            report(node(), &seen(&vault, &unnamed, None), &HashMap::new(), now),
            None
        );
        // With no user keys, the session names the vendor and the instance
        // name stands in for the title and project.
        let named = report(node(), &seen(&vault, &session, None), &HashMap::new(), now).unwrap();
        assert_eq!(named.agent.title, "vault");
        assert_eq!(named.project.name, "vault");
        assert_eq!(named.project.repo, None);
    }
}

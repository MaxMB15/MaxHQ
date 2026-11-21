// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which changes to an agent alert you, shared by the desktop's
//! notifications and `maxd`'s pushes to phones. The app owns every alert,
//! and the CLIs' own alerts stay off.
//!
//! An agent that needs input alerts at once, unless its pane has focus. A
//! finished turn, a usage limit and a resume after the limit alert only
//! when no pane shows the agent, in any window. Each alert shows once per
//! prompt or per state, and goes away when the state it was about clears.
//! `maxd` has no panes, so for it nothing covers an alert.
//!
//! The rules live in [`Notifier`], and a [`NotificationSender`] puts the
//! result on screen or on its way to a phone, so tests check the rules
//! with a fake sender.

use std::collections::{BTreeMap, HashMap, HashSet};

use maxhq_protocol::attention::{show_hidden, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId};
use maxhq_protocol::model::{Activity, Agent, Vendor};
use maxhq_protocol::phones::PushKind;
use maxhq_protocol::state::{Change, Delta, Entity, EntityKey};

use crate::store::Store;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// One notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// Stable for what it's about, so showing it again replaces it.
    pub id: String,
    /// The agent it's about, for the click.
    pub agent: AgentId,
    /// The agent's title.
    pub title: String,
    /// Where the agent runs, such as `maxhq on devbox`.
    pub subtitle: Option<String>,
    /// What happened.
    pub body: String,
    /// Whether it plays a sound, which only prompts do.
    pub sound: bool,
}

/// Puts notifications on screen and takes them off.
pub trait NotificationSender: Send {
    /// Shows a notification, replacing any with the same ID.
    fn deliver(&self, notification: &Notification);
    /// Shows an alert about an agent's state. A sender that needs to know
    /// what the alert is about, as `maxd`'s pushes and the desktop's
    /// settings do, reads `alert` and its `kind`, and the rest just deliver
    /// it.
    fn alert(&self, notification: &Notification, alert: Alert, kind: PushKind) {
        let _ = (alert, kind);
        self.deliver(notification);
    }
    /// Takes these notifications off screen, if they're still there.
    fn remove(&self, ids: &[String]);
    /// Takes every notification from this app off screen.
    fn remove_all(&self);
}

/// What an alert is about. Each kind holds while one state does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// The agent is waiting on this prompt.
    NeedsInput(AttentionId),
    /// The agent finished a turn you haven't looked at.
    Done,
    /// The agent hit a usage limit.
    Limit,
    /// The agent is working again after a usage limit.
    Resume,
}

impl Alert {
    /// Which switch, on the phone or on the Mac, turns this alert off. A
    /// prompt that's an error is an error, and a resume is about a limit.
    pub fn kind(self, attention: Option<AttentionKind>) -> PushKind {
        match (self, attention) {
            (Self::NeedsInput(_), Some(AttentionKind::Error)) => PushKind::Error,
            (Self::NeedsInput(_), _) => PushKind::NeedsInput,
            (Self::Done, _) => PushKind::Finished,
            (Self::Limit | Self::Resume, _) => PushKind::Limit,
        }
    }

    /// The notification ID. A prompt has its own, and the other kinds have
    /// one per agent.
    fn id(self, agent: AgentId) -> String {
        match self {
            Self::NeedsInput(attention) => format!("needs-input:{attention}"),
            Self::Done => format!("done:{agent}"),
            Self::Limit => format!("limit:{agent}"),
            Self::Resume => format!("resume:{agent}"),
        }
    }

    /// Whether the state this alert is about still holds.
    fn holds(self, activity: &Activity) -> bool {
        match self {
            Self::NeedsInput(id) => {
                matches!(activity, Activity::NeedsInput { attention } if *attention == id)
            }
            Self::Done => *activity == Activity::DoneUnread,
            Self::Limit => matches!(activity, Activity::WaitingForLimitReset { .. }),
            Self::Resume => working(activity),
        }
    }
}

/// What the app's panes show, across every window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panes {
    /// The agent whose pane has focus, in the window that has focus.
    pub focused: Option<AgentId>,
    /// Every agent whose pane shows in a window on screen.
    pub visible: HashSet<AgentId>,
}

impl Panes {
    /// Whether the panes already tell you what an alert would. A prompt
    /// waits on you, so it still alerts unless its pane has focus.
    fn cover(&self, agent: AgentId, alert: Alert) -> bool {
        self.focused == Some(agent)
            || (!matches!(alert, Alert::NeedsInput(_)) && self.visible.contains(&agent))
    }
}

/// Decides which notifications to show and clear as the view changes.
pub struct Notifier {
    sender: Box<dyn NotificationSender>,
    /// Each agent's last activity the app could see. An agent on a link
    /// that's down keeps the one from before, so its alerts neither clear
    /// nor repeat while the link is down.
    seen: HashMap<AgentId, Activity>,
    /// What's on screen now, by ID.
    shown: BTreeMap<String, (AgentId, Alert)>,
}

impl Notifier {
    /// Starts with nothing on screen. Notifications left from an earlier
    /// run are taken off, since this run doesn't know what they were for.
    pub fn new(sender: Box<dyn NotificationSender>) -> Self {
        sender.remove_all();
        Self {
            sender,
            seen: HashMap::new(),
            shown: BTreeMap::new(),
        }
    }

    /// Shows and clears notifications for a change to the view. `store` is
    /// the view after the change, and `panes` what the panes show.
    pub fn observe(&mut self, delta: &Delta, store: &Store, panes: &Panes) {
        for change in &delta.changes {
            match change {
                Change::Upsert {
                    value: Entity::Agent(agent),
                } => self.agent(agent, store, panes),
                Change::Remove {
                    key: EntityKey::Agent(agent),
                } => {
                    self.seen.remove(agent);
                    self.clear(|id, _| id == *agent);
                }
                Change::Remove {
                    key: EntityKey::Attention(attention),
                } => self.clear(|_, alert| alert == Alert::NeedsInput(*attention)),
                _ => {}
            }
        }
    }

    /// Takes notifications off screen once the panes show what they were
    /// about. A pane that gains focus takes all of its agent's, and one
    /// that only shows takes all but a prompt. They don't come back.
    pub fn look(&mut self, panes: &Panes) {
        self.clear(|agent, alert| panes.cover(agent, alert));
    }

    /// Shows and clears notifications for an agent's new state.
    fn agent(&mut self, agent: &Agent, store: &Store, panes: &Panes) {
        // Only lead agents have panes. A sub-agent's prompt and limit show
        // on its lead too, so alerting for both would say it twice.
        if agent.parent.is_some() || agent.activity == Activity::NotAccessible {
            return;
        }
        let now = &agent.activity;
        let was = self.seen.insert(agent.id, now.clone());
        self.clear(|id, alert| id == agent.id && !alert.holds(now));

        let alert = match (was.as_ref(), now) {
            // A prompt alerts even the first time the app sees it, such as
            // one already waiting when the app starts.
            (was, Activity::NeedsInput { attention }) if was != Some(now) => {
                Some(Alert::NeedsInput(*attention))
            }
            // The rest are changes, so an agent the app has only just seen
            // never alerts for them.
            (Some(was), Activity::DoneUnread) if *was != Activity::DoneUnread => Some(Alert::Done),
            (Some(was), Activity::WaitingForLimitReset { .. })
                if !matches!(was, Activity::WaitingForLimitReset { .. }) =>
            {
                Some(Alert::Limit)
            }
            (Some(Activity::WaitingForLimitReset { .. }), now) if working(now) => {
                Some(Alert::Resume)
            }
            _ => None,
        };
        if let Some(alert) = alert
            && !panes.cover(agent.id, alert)
        {
            self.show(alert, agent, store);
        }
    }

    /// Puts an alert about an agent on screen.
    fn show(&mut self, alert: Alert, agent: &Agent, store: &Store) {
        let id = alert.id(agent.id);
        let notification = Notification {
            id: id.clone(),
            agent: agent.id,
            title: show_hidden(&title(agent)),
            subtitle: place(agent, store).as_deref().map(show_hidden),
            body: show_hidden(&body(alert, store)),
            sound: matches!(alert, Alert::NeedsInput(_)),
        };
        let attention = match alert {
            Alert::NeedsInput(id) => match store.get(&EntityKey::Attention(id)) {
                Some(Entity::Attention(event)) => Some(event.kind),
                _ => None,
            },
            _ => None,
        };
        self.sender
            .alert(&notification, alert, alert.kind(attention));
        self.shown.insert(id, (agent.id, alert));
    }

    /// Shows a notification that isn't about an agent's state, so nothing
    /// clears it but you.
    pub fn deliver(&self, notification: &Notification) {
        self.sender.deliver(notification);
    }

    /// Takes every shown alert that `gone` picks off screen.
    fn clear(&mut self, gone: impl Fn(AgentId, Alert) -> bool) {
        let ids: Vec<String> = self
            .shown
            .iter()
            .filter(|(_, (agent, alert))| gone(*agent, *alert))
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            self.shown.remove(id);
        }
        self.sender.remove(&ids);
    }
}

/// Whether the agent is working on a turn.
fn working(activity: &Activity) -> bool {
    matches!(
        activity,
        Activity::Starting | Activity::Busy | Activity::BusyInSubagents
    )
}

/// The agent's title, or its CLI's name while it has none.
pub fn title(agent: &Agent) -> String {
    let title = agent.title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    match agent.vendor {
        Vendor::Claude => "Claude Code".to_owned(),
        Vendor::Codex => "Codex".to_owned(),
        Vendor::Cursor => "Cursor".to_owned(),
        Vendor::Acp => "ACP agent".to_owned(),
    }
}

/// The name of the agent's project, as far as the view knows it.
pub fn project_name(agent: &Agent, store: &Store) -> Option<String> {
    let Some(Entity::Workspace(workspace)) = store.get(&EntityKey::Workspace(agent.workspace))
    else {
        return None;
    };
    match store.get(&EntityKey::Project(workspace.project?)) {
        Some(Entity::Project(project)) => Some(project.name.clone()),
        _ => None,
    }
}

/// The agent's project and device, as far as the view knows them.
pub fn place(agent: &Agent, store: &Store) -> Option<String> {
    let Some(Entity::Workspace(workspace)) = store.get(&EntityKey::Workspace(agent.workspace))
    else {
        return None;
    };
    let project = project_name(agent, store);
    let device = match store.get(&EntityKey::Device(workspace.device)) {
        Some(Entity::Device(device)) => Some(device.name.clone()),
        _ => None,
    };
    match (project, device) {
        (Some(project), Some(device)) => Some(format!("{project} on {device}")),
        (project, device) => project.or(device),
    }
}

/// What the notification says happened.
fn body(alert: Alert, store: &Store) -> String {
    match alert {
        Alert::NeedsInput(id) => match store.get(&EntityKey::Attention(id)) {
            Some(Entity::Attention(event)) if !event.summary.trim().is_empty() => {
                event.summary.trim().to_owned()
            }
            Some(Entity::Attention(event)) if event.kind == AttentionKind::Permission => {
                "Wants your permission to go on".to_owned()
            }
            Some(Entity::Attention(event)) if event.kind == AttentionKind::Question => {
                "Has a question for you".to_owned()
            }
            _ => "Needs your input".to_owned(),
        },
        Alert::Done => "Finished its turn".to_owned(),
        Alert::Limit => "Hit a usage limit, and carries on when it resets".to_owned(),
        Alert::Resume => "The usage limit reset, so it's working again".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakeSender, Sent};
    use super::*;

    use maxhq_protocol::attention::AttentionEvent;
    use maxhq_protocol::ids::{BootId, DeviceId, ProjectId, WorkspaceId};
    use maxhq_protocol::model::{
        AgentLink, Control, Device, Project, Workspace, WorkspaceLocation,
    };
    use maxhq_protocol::time::Timestamp;

    struct Harness {
        store: Store,
        notifier: Notifier,
        sender: FakeSender,
        panes: Panes,
    }

    fn harness() -> Harness {
        let sender = FakeSender::default();
        let notifier = Notifier::new(Box::new(sender.clone()));
        assert_eq!(sender.take(), [Sent::RemoveAll]);
        let mut store = Store::new(0);
        store.apply([
            Change::Upsert {
                value: Entity::Device(Device {
                    id: DeviceId::derive("nl"),
                    name: "devbox".into(),
                    os: "linux".into(),
                    boot_id: BootId("b".into()),
                    maxd_version: "0.1.0".into(),
                    winding_down_until: None,
                    workspace_host: None,
                    build: None,
                    target: None,
                }),
            },
            Change::Upsert {
                value: Entity::Project(Project {
                    id: ProjectId::derive("maxhq"),
                    name: "maxhq".into(),
                    repo: None,
                    default_vendor: None,
                    default_account: None,
                    default_workspace: None,
                }),
            },
            Change::Upsert {
                value: Entity::Workspace(Workspace {
                    id: WorkspaceId::derive("w"),
                    project: Some(ProjectId::derive("maxhq")),
                    device: DeviceId::derive("nl"),
                    location: WorkspaceLocation::Folder {
                        path: "/src/maxhq".into(),
                    },
                    managed: None,
                    clone: None,
                }),
            },
        ]);
        Harness {
            store,
            notifier,
            sender,
            panes: Panes::default(),
        }
    }

    fn id() -> AgentId {
        AgentId::derive("a")
    }

    fn agent(activity: Activity) -> Agent {
        Agent {
            id: id(),
            workspace: WorkspaceId::derive("w"),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: None,
            control: Control::Observed,
            pid: None,
            link: AgentLink::Live,
            title: "Fix the merge".into(),
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
            moved: None,
            local: None,
        }
    }

    fn prompt(name: &str) -> (AttentionId, Change) {
        prompt_of(name, AttentionKind::Permission)
    }

    fn prompt_of(name: &str, kind: AttentionKind) -> (AttentionId, Change) {
        let id = AttentionId::derive(name);
        let event = AttentionEvent {
            id,
            agent: AgentId::derive("a"),
            kind,
            summary: format!("Run {name}?"),
            options: Vec::new(),
            accepts_text: false,
            created_at: Timestamp(3),
            expires_at: None,
        };
        (
            id,
            Change::Upsert {
                value: Entity::Attention(event),
            },
        )
    }

    /// Another lead agent, with its own ID.
    fn other(activity: Activity) -> Agent {
        Agent {
            id: AgentId::derive("b"),
            ..agent(activity)
        }
    }

    fn needs(attention: AttentionId) -> Activity {
        Activity::NeedsInput { attention }
    }

    fn limit() -> Activity {
        Activity::WaitingForLimitReset { resets_at: None }
    }

    impl Harness {
        fn apply(&mut self, changes: Vec<Change>) {
            if let Some(delta) = self.store.apply(changes) {
                self.notifier.observe(&delta, &self.store, &self.panes);
            }
        }

        fn set(&mut self, agent: Agent) {
            self.apply(vec![Change::Upsert {
                value: Entity::Agent(agent),
            }]);
        }

        fn to(&mut self, activity: Activity) {
            self.set(agent(activity));
        }

        /// Says which pane has focus and which panes show.
        fn look(&mut self, focused: Option<AgentId>, visible: &[AgentId]) {
            self.panes = Panes {
                focused,
                visible: visible.iter().copied().collect(),
            };
            self.notifier.look(&self.panes);
        }

        /// Focuses one pane, which shows too, or none.
        fn focus(&mut self, agent: Option<AgentId>) {
            self.look(agent, agent.as_slice());
        }
    }

    #[test]
    fn a_prompt_alerts_at_once_with_a_sound_even_at_startup() {
        let mut h = harness();
        let (attention, event) = prompt("tests");
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        assert_eq!(
            h.sender.take(),
            [Sent::Deliver(Notification {
                id: format!("needs-input:{attention}"),
                agent: id(),
                title: "Fix the merge".into(),
                subtitle: Some("maxhq on devbox".into()),
                body: "Run tests?".into(),
                sound: true,
            })]
        );
    }

    #[test]
    fn an_alert_spells_out_what_its_text_would_hide() {
        let mut h = harness();
        let (attention, event) = prompt("rm -rf ~\u{202e}\u{200b}");
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        let sent = h.sender.take();
        let [Sent::Deliver(note)] = sent.as_slice() else {
            panic!("expected one alert, got {sent:?}");
        };
        assert_eq!(note.body, "Run rm -rf ~[U+202E][U+200B]?");
    }

    #[test]
    fn a_prompt_alerts_once_however_often_it_is_reported() {
        let mut h = harness();
        let (first, _) = prompt("first");
        let (second, _) = prompt("second");
        h.to(Activity::Busy);
        h.to(needs(first));
        assert_eq!(h.sender.delivered().len(), 1);

        let mut renamed = agent(needs(first));
        renamed.title = "Renamed".into();
        h.set(renamed);
        assert!(h.sender.take().is_empty(), "same prompt, no new alert");

        h.to(needs(second));
        assert_eq!(
            h.sender.take(),
            [
                Sent::Remove(vec![format!("needs-input:{first}")]),
                Sent::Deliver(Notification {
                    id: format!("needs-input:{second}"),
                    agent: id(),
                    title: "Fix the merge".into(),
                    subtitle: Some("maxhq on devbox".into()),
                    body: "Needs your input".into(),
                    sound: true,
                }),
            ]
        );
    }

    #[test]
    fn done_limit_and_resume_alert_only_when_no_pane_shows_the_agent() {
        let mut h = harness();
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        h.to(Activity::Busy);
        h.to(limit());
        h.to(Activity::Busy);
        h.to(Activity::Idle);
        assert_eq!(
            h.sender.take(),
            [
                Sent::Deliver(Notification {
                    id: format!("done:{}", id()),
                    agent: id(),
                    title: "Fix the merge".into(),
                    subtitle: Some("maxhq on devbox".into()),
                    body: "Finished its turn".into(),
                    sound: false,
                }),
                Sent::Remove(vec![format!("done:{}", id())]),
                Sent::Deliver(Notification {
                    id: format!("limit:{}", id()),
                    agent: id(),
                    title: "Fix the merge".into(),
                    subtitle: Some("maxhq on devbox".into()),
                    body: "Hit a usage limit, and carries on when it resets".into(),
                    sound: false,
                }),
                Sent::Remove(vec![format!("limit:{}", id())]),
                Sent::Deliver(Notification {
                    id: format!("resume:{}", id()),
                    agent: id(),
                    title: "Fix the merge".into(),
                    subtitle: Some("maxhq on devbox".into()),
                    body: "The usage limit reset, so it's working again".into(),
                    sound: false,
                }),
                Sent::Remove(vec![format!("resume:{}", id())]),
            ]
        );

        h.focus(Some(id()));
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        h.to(Activity::Busy);
        h.to(limit());
        h.to(Activity::Starting);
        assert!(h.sender.take().is_empty(), "the focused pane stays quiet");

        // A pane that shows without focus is just as quiet.
        h.look(
            Some(AgentId::derive("other")),
            &[AgentId::derive("other"), id()],
        );
        h.to(Activity::DoneUnread);
        h.to(Activity::Busy);
        h.to(limit());
        h.to(Activity::Busy);
        assert!(h.sender.take().is_empty(), "a visible pane stays quiet");

        // Focus on another agent, or on none, doesn't mute this one.
        h.focus(Some(AgentId::derive("other")));
        h.to(Activity::DoneUnread);
        h.focus(None);
        h.to(limit());
        assert_eq!(
            h.sender.delivered(),
            [format!("done:{}", id()), format!("limit:{}", id())]
        );
    }

    #[test]
    fn a_prompt_alerts_for_a_pane_that_shows_without_focus() {
        let mut h = harness();
        let (attention, event) = prompt("tests");
        h.look(
            Some(AgentId::derive("other")),
            &[AgentId::derive("other"), id()],
        );
        h.to(Activity::Busy);
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        assert_eq!(h.sender.delivered(), [format!("needs-input:{attention}")]);
    }

    #[test]
    fn done_alerts_for_an_agent_no_pane_shows() {
        let mut h = harness();
        h.look(Some(AgentId::derive("other")), &[AgentId::derive("other")]);
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        assert_eq!(h.sender.delivered(), [format!("done:{}", id())]);
    }

    #[test]
    fn a_pane_that_shows_takes_all_but_a_prompt_off_screen() {
        let mut h = harness();
        let (attention, event) = prompt("tests");
        h.to(Activity::Busy);
        h.set(other(Activity::Busy));
        h.set(other(Activity::DoneUnread));
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        let b = AgentId::derive("b");
        assert_eq!(
            h.sender.delivered(),
            [format!("done:{b}"), format!("needs-input:{attention}")]
        );

        h.look(None, &[id(), b]);
        assert_eq!(h.sender.take(), [Sent::Remove(vec![format!("done:{b}")])]);
        h.look(None, &[id(), b]);
        assert!(h.sender.take().is_empty(), "the prompt stays");

        h.look(Some(id()), &[id(), b]);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("needs-input:{attention}")])]
        );
    }

    #[test]
    fn a_pane_that_shows_takes_a_limit_and_a_resume_off_screen() {
        let mut h = harness();
        h.to(Activity::Busy);
        h.to(limit());
        h.sender.take();
        h.look(None, &[id()]);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("limit:{}", id())])]
        );

        h.look(None, &[]);
        h.to(Activity::Busy);
        assert_eq!(h.sender.delivered(), [format!("resume:{}", id())]);
        h.look(None, &[id()]);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("resume:{}", id())])]
        );
    }

    #[test]
    fn the_focused_agent_doesnt_alert_for_a_prompt_either() {
        let mut h = harness();
        let (attention, event) = prompt("tests");
        h.focus(Some(id()));
        h.to(Activity::Busy);
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        assert!(h.sender.take().is_empty());
        // You saw it in the pane, so moving away doesn't alert for it.
        h.focus(None);
        h.to(needs(attention));
        assert!(h.sender.take().is_empty());
    }

    #[test]
    fn changes_only_alert_once_the_agent_has_been_seen() {
        let mut h = harness();
        h.to(Activity::DoneUnread);
        assert!(h.sender.take().is_empty(), "done at startup is old news");
        let mut other = agent(limit());
        other.id = AgentId::derive("b");
        h.set(other);
        assert!(h.sender.take().is_empty(), "so is a limit");
    }

    #[test]
    fn alerts_clear_when_their_state_clears() {
        let mut h = harness();
        let (attention, event) = prompt("tests");
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        h.sender.take();
        h.to(Activity::Busy);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("needs-input:{attention}")])]
        );

        // Marking it seen turns done-unread into idle, which clears it.
        h.to(Activity::DoneUnread);
        h.sender.take();
        h.to(Activity::Idle);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("done:{}", id())])]
        );

        // A prompt that goes away clears with it, even before the agent's
        // own state catches up.
        let (attention, event) = prompt("again");
        h.apply(vec![
            Change::Upsert {
                value: Entity::Agent(agent(needs(attention))),
            },
            event,
        ]);
        h.sender.take();
        h.apply(vec![Change::Remove {
            key: EntityKey::Attention(attention),
        }]);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("needs-input:{attention}")])]
        );
    }

    #[test]
    fn an_agent_that_goes_away_takes_its_alerts_with_it() {
        let mut h = harness();
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        h.sender.take();
        h.apply(vec![Change::Remove {
            key: EntityKey::Agent(id()),
        }]);
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("done:{}", id())])]
        );
        h.to(Activity::DoneUnread);
        assert!(h.sender.take().is_empty(), "back again counts as new");
    }

    #[test]
    fn a_link_that_drops_neither_clears_nor_repeats_alerts() {
        let mut h = harness();
        let (attention, _) = prompt("tests");
        h.to(needs(attention));
        h.sender.take();
        h.to(Activity::NotAccessible);
        h.to(needs(attention));
        assert!(h.sender.take().is_empty());

        // A turn that ends while the link is down alerts when it's back.
        h.to(Activity::Busy);
        h.sender.take();
        h.to(Activity::NotAccessible);
        h.to(Activity::DoneUnread);
        assert_eq!(h.sender.delivered(), [format!("done:{}", id())]);
    }

    #[test]
    fn focusing_a_pane_takes_its_alerts_off_screen() {
        let mut h = harness();
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        h.sender.take();
        h.focus(Some(id()));
        assert_eq!(
            h.sender.take(),
            [Sent::Remove(vec![format!("done:{}", id())])]
        );
        h.focus(None);
        h.to(Activity::DoneUnread);
        assert!(h.sender.take().is_empty(), "it doesn't come back");
    }

    #[test]
    fn sub_agents_leave_alerts_to_their_lead() {
        let mut h = harness();
        let (attention, _) = prompt("tests");
        let mut sub = agent(Activity::Busy);
        sub.parent = Some(AgentId::derive("lead"));
        h.set(sub.clone());
        sub.activity = needs(attention);
        h.set(sub.clone());
        sub.activity = limit();
        h.set(sub);
        assert!(h.sender.take().is_empty());
    }

    #[test]
    fn titles_and_places_fall_back_to_what_is_known() {
        let mut nameless = agent(Activity::Idle);
        nameless.title = "  ".into();
        assert_eq!(title(&nameless), "Claude Code");
        nameless.vendor = Vendor::Codex;
        assert_eq!(title(&nameless), "Codex");
        assert_eq!(place(&nameless, &Store::new(0)), None);
    }

    /// Records the kind each alert came with.
    #[derive(Clone, Default)]
    struct Kinds(std::sync::Arc<std::sync::Mutex<Vec<(String, PushKind)>>>);

    impl NotificationSender for Kinds {
        fn deliver(&self, _: &Notification) {}

        fn alert(&self, notification: &Notification, _: Alert, kind: PushKind) {
            self.0.lock().unwrap().push((notification.id.clone(), kind));
        }

        fn remove(&self, _: &[String]) {}

        fn remove_all(&self) {}
    }

    #[test]
    fn each_alert_comes_with_the_switch_that_turns_it_off() {
        let attention = AttentionId::derive("x");
        let prompt = Alert::NeedsInput(attention);
        assert_eq!(prompt.kind(None), PushKind::NeedsInput);
        assert_eq!(
            prompt.kind(Some(AttentionKind::Question)),
            PushKind::NeedsInput
        );
        assert_eq!(prompt.kind(Some(AttentionKind::Error)), PushKind::Error);
        assert_eq!(Alert::Done.kind(None), PushKind::Finished);
        assert_eq!(Alert::Limit.kind(None), PushKind::Limit);
        assert_eq!(Alert::Resume.kind(None), PushKind::Limit);

        // The notifier reads the prompt's kind from the view.
        let mut h = harness();
        let kinds = Kinds::default();
        h.notifier = Notifier::new(Box::new(kinds.clone()));
        let (id, change) = prompt_of("broke", AttentionKind::Error);
        h.apply(vec![change]);
        h.to(needs(id));
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        assert_eq!(
            *kinds.0.lock().unwrap(),
            [
                (format!("needs-input:{id}"), PushKind::Error),
                (format!("done:{}", AgentId::derive("a")), PushKind::Finished),
            ]
        );
    }
}

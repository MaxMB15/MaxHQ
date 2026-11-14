// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Desktop notifications about agents, following decision C-8. Which
//! changes alert lives in [`maxhq_core::notify`], shared with `maxd`'s
//! pushes. Clicking one focuses the agent's pane.
//!
//! An inbox change the outbox held while a device was away, and that the
//! device then refused, gets a notification of its own.
//!
//! Your settings for them, [`NotificationPrefs`], apply in [`Filtered`],
//! which sits between the notifier and the system's sender. The notifier
//! keeps its rules whatever they say, so an alert you turned off still
//! counts as shown and still gets taken off screen when its state clears.

use std::sync::{Arc, PoisonError, RwLock};

use maxhq_core::store::Store;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::InboxChange;
use maxhq_protocol::phones::PushKind;
use maxhq_protocol::state::{Entity, EntityKey};
use serde::{Deserialize, Serialize};
use specta::Type;

pub use maxhq_core::notify::{
    place, title, Alert, Notification, NotificationSender, Notifier, Panes,
};

use crate::registry::{Queued, Registry, RegistryError};

/// The setting the notification settings are saved under.
const PREFS: &str = "notifications";

/// Every kind of alert, in the order the phone's settings show them.
const ALL_KINDS: [PushKind; 4] = [
    PushKind::NeedsInput,
    PushKind::Error,
    PushKind::Finished,
    PushKind::Limit,
];

/// Which notifications this Mac shows, and which of them play a sound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NotificationPrefs {
    /// Whether the app shows notifications at all.
    pub show: bool,
    /// The kinds of alert that show, by the names the phone's switches
    /// use.
    pub kinds: Vec<PushKind>,
    /// Whether an alert that waits on you plays a sound: a prompt or an
    /// error.
    pub prompt_sound: bool,
    /// Whether the other alerts play one.
    pub other_sound: bool,
}

impl Default for NotificationPrefs {
    /// Everything shows, and only what waits on you plays a sound, as
    /// before there were settings.
    fn default() -> Self {
        Self {
            show: true,
            kinds: ALL_KINDS.to_vec(),
            prompt_sound: true,
            other_sound: false,
        }
    }
}

impl NotificationPrefs {
    /// The settings saved in `registry`, or the defaults when there are
    /// none or they don't read.
    pub fn load(registry: &Registry) -> Self {
        match registry.setting(PREFS) {
            Ok(saved) => saved.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "couldn't read the notification settings");
                Self::default()
            }
        }
    }

    /// Saves the settings in `registry`.
    pub fn save(&self, registry: &Registry) -> Result<(), RegistryError> {
        registry.set_setting(PREFS, self)
    }

    /// Whether an alert plays a sound. `waits` says whether it's one that
    /// waits on you.
    fn sound(&self, waits: bool) -> bool {
        if waits {
            self.prompt_sound
        } else {
            self.other_sound
        }
    }
}

/// The notification settings as they are now. Clones share them, so a
/// change from the settings page reaches [`Filtered`] at once.
#[derive(Clone, Debug, Default)]
pub struct LivePrefs(Arc<RwLock<NotificationPrefs>>);

impl LivePrefs {
    /// Starts with `prefs`.
    pub fn new(prefs: NotificationPrefs) -> Self {
        Self(Arc::new(RwLock::new(prefs)))
    }

    /// The settings now.
    pub fn get(&self) -> NotificationPrefs {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Puts new settings in place.
    pub fn set(&self, prefs: NotificationPrefs) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = prefs;
    }
}

/// A sender that shows only what your settings let through, with the
/// sound they ask for. Taking notifications off screen always goes
/// through, so one that showed before you turned its kind off still goes
/// when its state clears.
pub struct Filtered {
    inner: Arc<dyn NotificationSender + Sync>,
    prefs: LivePrefs,
}

impl Filtered {
    /// Puts `prefs` in front of `inner`.
    pub fn new(inner: Arc<dyn NotificationSender + Sync>, prefs: LivePrefs) -> Self {
        Self { inner, prefs }
    }
}

impl NotificationSender for Filtered {
    fn deliver(&self, notification: &Notification) {
        if self.prefs.get().show {
            self.inner.deliver(notification);
        } else {
            tracing::debug!(id = notification.id, "notifications are off");
        }
    }

    fn alert(&self, notification: &Notification, alert: Alert, kind: PushKind) {
        let prefs = self.prefs.get();
        if !prefs.show || !prefs.kinds.contains(&kind) {
            tracing::debug!(id = notification.id, ?kind, "this kind of alert is off");
            return;
        }
        let notification = Notification {
            sound: prefs.sound(matches!(alert, Alert::NeedsInput(_))),
            ..notification.clone()
        };
        self.inner.alert(&notification, alert, kind);
    }

    fn remove(&self, ids: &[String]) {
        self.inner.remove(ids);
    }

    fn remove_all(&self) {
        self.inner.remove_all();
    }
}

/// The ID of the notification the settings page sends to try them out.
pub const TEST_ID: &str = "test";

/// What the settings page sends to try notifications out. It sounds as a
/// prompt would. A click on it opens nothing, since no agent has its ID.
pub fn test_notification(prefs: &NotificationPrefs) -> Notification {
    Notification {
        id: TEST_ID.to_owned(),
        agent: AgentId::derive(TEST_ID),
        title: "MaxHQ".to_owned(),
        subtitle: None,
        body: "Notifications from MaxHQ look like this.".to_owned(),
        sound: prefs.prompt_sound,
    }
}

/// Writes notifications to the log, for builds that can't show them.
pub struct LogSender;

impl NotificationSender for LogSender {
    fn deliver(&self, notification: &Notification) {
        tracing::info!(id = notification.id, "would show a notification");
    }

    fn remove(&self, ids: &[String]) {
        tracing::debug!(?ids, "would remove notifications");
    }

    fn remove_all(&self) {}
}

/// Says that `device` refused a change the outbox held for it, and why.
/// The change is already off the agent by then, so nothing clears this
/// one but you.
pub fn refused(notifier: &Notifier, queued: &Queued, device: &str, reason: &str, store: &Store) {
    let agent = match store.get(&EntityKey::Agent(queued.agent)) {
        Some(Entity::Agent(agent)) => Some(agent),
        _ => None,
    };
    let name = agent.map_or_else(|| queued.title.clone(), title);
    let notification = Notification {
        id: format!("inbox:{}", queued.key),
        agent: queued.agent,
        subtitle: agent.and_then(|agent| place(agent, store)),
        body: format!(
            "Couldn't {} on {device}: {reason}",
            undone(&queued.change, &name)
        ),
        title: name,
        sound: false,
    };
    notifier.deliver(&notification);
}

/// What an inbox change set out to do to the agent called `title`, to
/// follow "Couldn't".
fn undone(change: &InboxChange, title: &str) -> String {
    match change {
        InboxChange::Settle => format!("settle {title}"),
        InboxChange::Unsettle => format!("bring back {title}"),
        InboxChange::Snooze { .. } => format!("snooze {title}"),
        InboxChange::Wake => format!("wake {title}"),
        InboxChange::DismissWoke => format!("clear the wake-up marker on {title}"),
        InboxChange::Pin { .. } => format!("pin {title}"),
        InboxChange::Unpin => format!("unpin {title}"),
        InboxChange::AutoSettle { on: true } => format!("turn auto-settle on for {title}"),
        InboxChange::AutoSettle { on: false } => format!("turn auto-settle off for {title}"),
        InboxChange::ContinueAtLimit { on: true } => {
            format!("have {title} continue after the usage limit")
        }
        InboxChange::ContinueAtLimit { on: false } => {
            format!("stop {title} continuing after the usage limit")
        }
        InboxChange::Restore { .. } => format!("undo the change to {title}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) use maxhq_core::notify::testing::{FakeSender, Sent};

    use super::*;
    use maxhq_protocol::ids::{AgentId, BootId, DeviceId, ProjectId, WorkspaceId};
    use maxhq_protocol::model::{
        Activity, Agent, AgentLink, Control, Device, Project, Vendor, Workspace, WorkspaceLocation,
    };
    use maxhq_protocol::state::Change;
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
        }
    }

    impl Harness {
        fn to(&mut self, activity: Activity) {
            let changes = vec![Change::Upsert {
                value: Entity::Agent(agent(activity)),
            }];
            if let Some(delta) = self.store.apply(changes) {
                self.notifier.observe(&delta, &self.store, &self.panes);
            }
        }
    }

    fn queued(change: InboxChange) -> Queued {
        Queued {
            key: maxhq_protocol::ids::IdempotencyKey::generate(),
            link: maxhq_protocol::ids::LinkId::derive("nl"),
            device: DeviceId::derive("nl"),
            agent: id(),
            title: "Fix the merge, as queued".into(),
            change,
            at: Timestamp(5),
        }
    }

    /// A filter in front of a fake sender, with settings to change.
    fn filtered() -> (Filtered, FakeSender, LivePrefs) {
        let sender = FakeSender::default();
        let prefs = LivePrefs::default();
        (
            Filtered::new(Arc::new(sender.clone()), prefs.clone()),
            sender,
            prefs,
        )
    }

    fn alerted(id: &str, sound: bool) -> Notification {
        Notification {
            id: id.into(),
            agent: AgentId::derive("a"),
            title: "Fix the merge".into(),
            subtitle: None,
            body: "Needs your input".into(),
            sound,
        }
    }

    #[test]
    fn the_defaults_show_everything_and_sound_for_prompts() {
        let prefs = NotificationPrefs::default();
        assert!(prefs.show);
        assert_eq!(prefs.kinds, ALL_KINDS);
        assert!(prefs.prompt_sound);
        assert!(!prefs.other_sound);

        let (filter, sender, _) = filtered();
        let prompt = Alert::NeedsInput(maxhq_protocol::ids::AttentionId::derive("p"));
        filter.alert(&alerted("p", true), prompt, PushKind::NeedsInput);
        filter.alert(&alerted("e", true), prompt, PushKind::Error);
        filter.alert(&alerted("d", false), Alert::Done, PushKind::Finished);
        filter.alert(&alerted("l", false), Alert::Limit, PushKind::Limit);
        filter.alert(&alerted("r", false), Alert::Resume, PushKind::Limit);
        assert_eq!(
            sender.take(),
            [
                Sent::Deliver(alerted("p", true)),
                Sent::Deliver(alerted("e", true)),
                Sent::Deliver(alerted("d", false)),
                Sent::Deliver(alerted("l", false)),
                Sent::Deliver(alerted("r", false)),
            ]
        );
    }

    #[test]
    fn a_kind_turned_off_stays_quiet_but_still_clears() {
        let (filter, sender, prefs) = filtered();
        let prompt = Alert::NeedsInput(maxhq_protocol::ids::AttentionId::derive("p"));
        filter.alert(&alerted("d", false), Alert::Done, PushKind::Finished);
        prefs.set(NotificationPrefs {
            kinds: vec![PushKind::NeedsInput, PushKind::Limit],
            ..NotificationPrefs::default()
        });
        filter.alert(&alerted("d2", false), Alert::Done, PushKind::Finished);
        filter.alert(&alerted("e", true), prompt, PushKind::Error);
        filter.alert(&alerted("p", true), prompt, PushKind::NeedsInput);
        filter.remove(&["d".into()]);
        filter.remove_all();
        assert_eq!(
            sender.take(),
            [
                Sent::Deliver(alerted("d", false)),
                Sent::Deliver(alerted("p", true)),
                Sent::Remove(vec!["d".into()]),
                Sent::RemoveAll,
            ]
        );
    }

    #[test]
    fn turning_notifications_off_stops_every_one() {
        let (filter, sender, prefs) = filtered();
        prefs.set(NotificationPrefs {
            show: false,
            ..NotificationPrefs::default()
        });
        let prompt = Alert::NeedsInput(maxhq_protocol::ids::AttentionId::derive("p"));
        filter.alert(&alerted("p", true), prompt, PushKind::NeedsInput);
        filter.deliver(&alerted("inbox", false));
        filter.remove(&["p".into()]);
        assert_eq!(sender.take(), [Sent::Remove(vec!["p".into()])]);

        prefs.set(NotificationPrefs::default());
        filter.deliver(&alerted("inbox", false));
        assert_eq!(sender.take(), [Sent::Deliver(alerted("inbox", false))]);
    }

    #[test]
    fn the_sound_follows_the_settings() {
        let (filter, sender, prefs) = filtered();
        prefs.set(NotificationPrefs {
            prompt_sound: false,
            other_sound: true,
            ..NotificationPrefs::default()
        });
        let prompt = Alert::NeedsInput(maxhq_protocol::ids::AttentionId::derive("p"));
        filter.alert(&alerted("p", true), prompt, PushKind::NeedsInput);
        filter.alert(&alerted("e", true), prompt, PushKind::Error);
        filter.alert(&alerted("d", false), Alert::Done, PushKind::Finished);
        filter.alert(&alerted("l", false), Alert::Limit, PushKind::Limit);
        assert_eq!(
            sender.take(),
            [
                Sent::Deliver(alerted("p", false)),
                Sent::Deliver(alerted("e", false)),
                Sent::Deliver(alerted("d", true)),
                Sent::Deliver(alerted("l", true)),
            ]
        );
    }

    #[test]
    fn a_notifier_behind_the_filter_keeps_its_rules() {
        let (filter, sender, prefs) = filtered();
        let mut h = harness();
        h.notifier = Notifier::new(Box::new(filter));
        assert_eq!(sender.take(), [Sent::RemoveAll]);
        h.to(Activity::Busy);
        h.to(Activity::DoneUnread);
        assert_eq!(sender.delivered(), [format!("done:{}", id())]);

        // Turned off while it shows, it still goes once you've looked.
        prefs.set(NotificationPrefs {
            kinds: Vec::new(),
            ..NotificationPrefs::default()
        });
        h.to(Activity::Busy);
        assert_eq!(
            sender.take(),
            [Sent::Remove(vec![format!("done:{}", id())])]
        );
        h.to(Activity::DoneUnread);
        assert!(sender.take().is_empty());
    }

    #[test]
    fn settings_save_and_load_through_the_registry() {
        let registry = Registry::in_memory().unwrap();
        assert_eq!(
            NotificationPrefs::load(&registry),
            NotificationPrefs::default()
        );
        let prefs = NotificationPrefs {
            show: true,
            kinds: vec![PushKind::Error],
            prompt_sound: false,
            other_sound: true,
        };
        prefs.save(&registry).unwrap();
        assert_eq!(NotificationPrefs::load(&registry), prefs);

        // A setting that no longer reads, such as one with a kind this
        // build doesn't know, gives the defaults.
        for unread in [
            serde_json::json!({"show": false}),
            serde_json::json!({
                "show": false,
                "kinds": ["someday"],
                "prompt_sound": false,
                "other_sound": false,
            }),
        ] {
            registry.set_setting(PREFS, &unread).unwrap();
            assert_eq!(
                NotificationPrefs::load(&registry),
                NotificationPrefs::default()
            );
        }
    }

    #[test]
    fn the_test_notification_sounds_like_a_prompt() {
        let mut prefs = NotificationPrefs::default();
        assert!(test_notification(&prefs).sound);
        prefs.prompt_sound = false;
        assert!(!test_notification(&prefs).sound);
        assert_eq!(test_notification(&prefs).id, TEST_ID);
    }

    #[test]
    fn a_refused_change_says_what_it_was_and_why() {
        let mut h = harness();
        h.to(Activity::Idle);
        let settle = queued(InboxChange::Settle);
        refused(&h.notifier, &settle, "devbox", "it's working", &h.store);
        assert_eq!(
            h.sender.take(),
            [Sent::Deliver(Notification {
                id: format!("inbox:{}", settle.key),
                agent: id(),
                title: "Fix the merge".into(),
                subtitle: Some("maxhq on devbox".into()),
                body: "Couldn't settle Fix the merge on devbox: it's working".into(),
                sound: false,
            })]
        );

        // An agent that's gone goes by the title it had when it was queued.
        let gone = Store::new(0);
        for (change, body) in [
            (InboxChange::Unsettle, "bring back"),
            (
                InboxChange::Snooze {
                    until: Timestamp(9),
                },
                "snooze",
            ),
            (InboxChange::Unpin, "unpin"),
            (
                InboxChange::AutoSettle { on: false },
                "turn auto-settle off for",
            ),
        ] {
            refused(&h.notifier, &queued(change), "devbox", "no", &gone);
            let [Sent::Deliver(notification)] = &h.sender.take()[..] else {
                panic!("one notification each");
            };
            assert_eq!(notification.title, "Fix the merge, as queued");
            assert_eq!(notification.subtitle, None);
            assert_eq!(
                notification.body,
                format!("Couldn't {body} Fix the merge, as queued on devbox: no")
            );
        }
    }
}

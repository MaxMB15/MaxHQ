// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Scheduled tasks. Each lives on its home device, whose `maxd` runs it
//! and publishes its tasks and its Funnel as `DeviceTasks`, so the app
//! reads both from the view and sends each change to the task's home. A
//! webhook's URL and secret pass through without being kept.

use maxhq_core::agents::{target_device, task_permission, user_permission};
use maxhq_protocol::agents::Permission;
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, TaskId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::tasks::{FunnelState, HookAddress, Maker, Run, Task, TaskDelivery};
use maxhq_protocol::ClientMessage;

use crate::devices::{is_local, Connector};
use crate::fleet::{check_can, Needs};
use crate::merge::Merge;
use crate::request::{lookup_failed, CommandError};
use crate::App;

impl<C: Connector> App<C> {
    /// The tasks one device holds, or those of every device the view has.
    pub fn list_tasks(&self, device: Option<DeviceId>) -> Result<Vec<Task>, CommandError> {
        self.view.read(|merge| {
            if let Some(device) = device {
                check_can(merge, device, Needs::Tasks)?;
            }
            let mut tasks = Vec::new();
            for entity in merge.store().entities() {
                match entity {
                    Entity::DeviceTasks(held) if device.is_none_or(|one| one == held.device) => {
                        tasks.extend(held.tasks.iter().cloned());
                    }
                    _ => {}
                }
            }
            Ok(tasks)
        })
    }

    /// Whether a device's webhooks reach the internet. A device that
    /// hasn't published its tasks has no webhook on.
    pub fn funnel_state(&self, device: DeviceId) -> Result<FunnelState, CommandError> {
        self.view.read(|merge| {
            check_can(merge, device, Needs::Tasks)?;
            Ok(match merge.store().get(&EntityKey::DeviceTasks(device)) {
                Some(Entity::DeviceTasks(held)) => held.funnel.clone(),
                _ => FunnelState::Off,
            })
        })
    }

    /// Stores a task on `device`, its home. The outcome holds a new
    /// webhook's URL, which goes back to the caller alone.
    ///
    /// A task the user made whose target is on another device runs there
    /// under its own permission, which that device checks against its own
    /// copy, so the permission goes to it too.
    pub async fn save_task(
        &self,
        device: DeviceId,
        task: Task,
    ) -> Result<ActionOutcome, CommandError> {
        self.view
            .read(|merge| check_can(merge, device, Needs::Tasks))?;
        let action = ClientMessage::SaveTask {
            key: IdempotencyKey::generate(),
            task: Box::new(task.clone()),
        };
        let outcome = self.devices.act_on(device, action).await?;
        // The permission comes from the task sent, never from the answer,
        // since a device could answer with a target it wants power over.
        if let ActionOutcome::TaskSaved { task: saved, .. } = &outcome {
            if !stored_as_sent(&task, saved) {
                tracing::warn!(task = %task.id, %device, "a device stored a different task from the one it was sent");
                return Err(CommandError::Failed {
                    message: "That device stored something other than this task".to_owned(),
                });
            }
            if runs_elsewhere(&task, device)
                && let Err(error) = self.grant_permission(permission_for(&task, device)).await
            {
                // The outcome may hold a webhook URL that shows only once, so
                // the save still goes back. Saving again sends it again.
                tracing::warn!(%error, task = %task.id, "couldn't give a task's target its permission");
            }
        }
        Ok(outcome)
    }

    /// Turns a task `device` holds on or off. The device changes only that,
    /// so no permission moves. An older `maxd` reads only a whole save, which
    /// gives it back its own row with the switch changed, and grants nothing.
    pub async fn set_task_enabled(
        &self,
        device: DeviceId,
        task: TaskId,
        enabled: bool,
    ) -> Result<ActionOutcome, CommandError> {
        let held = self.view.read(|merge| {
            check_can(merge, device, Needs::Tasks)?;
            Ok::<_, CommandError>(match merge.store().get(&EntityKey::DeviceTasks(device)) {
                Some(Entity::DeviceTasks(held)) => {
                    held.tasks.iter().find(|kept| kept.id == task).cloned()
                }
                _ => None,
            })
        })?;
        let switch = ClientMessage::SetTaskEnabled {
            key: IdempotencyKey::generate(),
            task,
            enabled,
        };
        if let Some(outcome) = self.devices.act_on_unless_old(device, switch).await? {
            return Ok(outcome);
        }
        let Some(mut whole) = held else {
            return Err(CommandError::NotFound {
                what: format!("task {task}"),
            });
        };
        whole.enabled = enabled;
        let save = ClientMessage::SaveTask {
            key: IdempotencyKey::generate(),
            task: Box::new(whole),
        };
        self.devices.act_on(device, save).await
    }

    /// Deletes a task, and takes its permission off the device its target
    /// is on.
    pub async fn delete_task(&self, task: TaskId) -> Result<ActionOutcome, CommandError> {
        let found = self.view.read(|merge| {
            merge.store().entities().find_map(|entity| match entity {
                Entity::DeviceTasks(held) => held
                    .tasks
                    .iter()
                    .find(|kept| kept.id == task && runs_elsewhere(kept, held.device))
                    .map(|_| user_permission(task)),
                _ => None,
            })
        });
        let outcome = self
            .act_on_task(task, |key| ClientMessage::DeleteTask { key, task })
            .await?;
        if let (ActionOutcome::Done, Some(permission)) = (&outcome, found)
            && let Err(error) = self.revoke_permission(permission).await
        {
            tracing::warn!(%error, %task, "couldn't take a deleted task's permission back");
        }
        Ok(outcome)
    }

    /// Stores the secret a task's webhook checks signatures with.
    pub async fn set_hook_secret(
        &self,
        task: TaskId,
        secret: Secret,
    ) -> Result<ActionOutcome, CommandError> {
        if secret.is_empty() {
            return Err(CommandError::Failed {
                message: "Enter the secret first".to_owned(),
            });
        }
        self.act_on_task(task, |key| ClientMessage::SetHookSecret {
            key,
            task,
            secret,
        })
        .await
    }

    /// Picks how `device`'s webhooks are reached.
    pub async fn set_hook_address(
        &self,
        device: DeviceId,
        address: HookAddress,
    ) -> Result<ActionOutcome, CommandError> {
        self.view
            .read(|merge| check_can(merge, device, Needs::Tasks))?;
        let action = ClientMessage::SetHookAddress {
            key: IdempotencyKey::generate(),
            address,
        };
        self.devices.act_on(device, action).await
    }

    /// A task's recent runs, newest first.
    pub async fn list_runs(&self, task: TaskId) -> Result<Vec<Run>, CommandError> {
        match self
            .act_on_task(task, |key| ClientMessage::ListRuns { key, task })
            .await?
        {
            ActionOutcome::Runs { runs } => Ok(runs),
            other => Err(lookup_failed(other)),
        }
    }

    /// A task's recent webhook requests or GitHub events, newest first.
    pub async fn list_deliveries(&self, task: TaskId) -> Result<Vec<TaskDelivery>, CommandError> {
        match self
            .act_on_task(task, |key| ClientMessage::ListDeliveries { key, task })
            .await?
        {
            ActionOutcome::Deliveries { deliveries } => Ok(deliveries),
            other => Err(lookup_failed(other)),
        }
    }

    /// Sends an action about a task to its home under a fresh key, once
    /// its `maxd` runs scheduled tasks.
    pub async fn act_on_task(
        &self,
        task: TaskId,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        let home = self.view.read(|merge| {
            let home = task_home(merge, task)?;
            check_can(merge, home, Needs::Tasks)?;
            Ok::<_, CommandError>(home)
        })?;
        let action = action(IdempotencyKey::generate());
        self.devices.act_on(home, action).await
    }
}

/// Whether a task the user made has its target on a device other than
/// its home. One an agent made runs under the agent's own permission,
/// which its devices already hold.
fn runs_elsewhere(task: &Task, home: DeviceId) -> bool {
    task.made_by == Maker::User && target_device(&task.target) != home
}

/// What a device that stored `sent` answers with. It fills in times and a
/// webhook, and leaves who made the task and what it reaches alone.
fn stored_as_sent(sent: &Task, saved: &Task) -> bool {
    saved.id == sent.id && saved.made_by == sent.made_by && saved.target == sent.target
}

/// The permission to send for a task the user made, under the ID its home
/// gives it, whatever ID the task came with.
fn permission_for(task: &Task, home: DeviceId) -> Permission {
    Permission {
        id: user_permission(task.id),
        ..task_permission(task, home)
    }
}

/// The device that holds a task. Any device can list any ID, and a change to
/// the task can carry a secret, so when two list it, only this machine's own
/// report settles which one you mean.
fn task_home(merge: &Merge, task: TaskId) -> Result<DeviceId, CommandError> {
    let homes: Vec<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::DeviceTasks(held) if held.tasks.iter().any(|kept| kept.id == task) => {
                Some(held.device)
            }
            _ => None,
        })
        .collect();
    match homes.as_slice() {
        [] => Err(CommandError::NotFound {
            what: "This task".to_owned(),
        }),
        [home] => Ok(*home),
        _ => homes
            .iter()
            .copied()
            .find(|home| is_local(merge, *home))
            .ok_or_else(|| CommandError::Failed {
                message: "Two devices list this task, so it's not clear which one you mean."
                    .to_owned(),
            }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use maxhq_client::{LinkEvent, LinkSpec};
    use maxhq_protocol::ids::{AgentId, PermissionId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::state::Change;
    use maxhq_protocol::tasks::{
        DeviceTasks, FunnelStep, Maker, RunCause, RunOutcome, TaskTarget, Trigger,
    };
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::devices::tests::{connect_doing, two_devices, Call, Fake, Harness};
    use crate::tests::{block_on, FakeShell};

    /// A task that runs every 15 minutes on an agent on `device`.
    pub(crate) fn task(name: &str, device: &str) -> Task {
        let device = DeviceId::derive(device);
        Task {
            id: TaskId::derive(name),
            name: name.to_owned(),
            prompt: "Check the build".to_owned(),
            trigger: Trigger::Every { minutes: 15 },
            target: TaskTarget::Agent {
                device,
                agent: AgentId::derive("a"),
            },
            enabled: true,
            permission: user_permission(TaskId::derive(name)),
            made_by: Maker::User,
            next_run: None,
            last_run: None,
            approval_asked: None,
        }
    }

    /// Has `link` publish `device`'s tasks and Funnel.
    pub(crate) fn publish(h: &Harness, link: &LinkSpec, held: DeviceTasks) {
        let value = Entity::DeviceTasks(held);
        h.report(link.id, LinkEvent::Changed(vec![Change::Upsert { value }]));
    }

    /// What both sample devices' `maxd` does.
    fn scheduled_tasks() -> Capabilities {
        Capabilities {
            scheduled_tasks: true,
            agent_tools: true,
            ..Capabilities::default()
        }
    }

    /// Devbox's Funnel, which needs setting up.
    fn needs_setup() -> FunnelState {
        FunnelState::NeedsSetup {
            steps: vec![FunnelStep {
                reason: "Funnel isn't on in the tailnet's policy".to_owned(),
                command: None,
            }],
        }
    }

    /// The app over this machine, with task "mac" on it, and devbox, with
    /// task "nl", whose Funnel needs setting up. Both run scheduled tasks.
    fn app() -> (App<Fake>, Fake, LinkSpec) {
        app_with(vec![task("mac", "mac")])
    }

    /// The same, with `tasks` on this machine.
    fn app_with(tasks: Vec<Task>) -> (App<Fake>, Fake, LinkSpec) {
        let (h, local, nl) = two_devices();
        connect_doing(&h, &local, "mac", scheduled_tasks());
        connect_doing(&h, &nl, "nl", scheduled_tasks());
        publish(
            &h,
            &local,
            DeviceTasks {
                device: DeviceId::derive("mac"),
                tasks,
                address: HookAddress::Local,
                port: None,
                funnel: FunnelState::Off,
                problems: Vec::new(),
                webhooks: Vec::new(),
            },
        );
        publish(
            &h,
            &nl,
            DeviceTasks {
                device: DeviceId::derive("nl"),
                tasks: vec![task("nl", "nl")],
                address: HookAddress::Funnel,
                port: None,
                funnel: needs_setup(),
                problems: Vec::new(),
                webhooks: Vec::new(),
            },
        );
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        (App::new(view, devices, FakeShell::default()), fake, nl)
    }

    /// Each message sent, with the link it went through.
    fn sent(fake: &Fake) -> Vec<(maxhq_protocol::ids::LinkId, ClientMessage)> {
        fake.calls()
            .into_iter()
            .filter_map(|(link, call)| match call {
                Call::Send(message) => Some((link, message)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tasks_and_the_funnel_come_from_the_view() {
        let (app, fake, _) = app();
        let (mac, nl) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let mut every = app.list_tasks(None).unwrap();
        every.sort_by_key(|task| task.id);
        let mut expected = vec![task("mac", "mac"), task("nl", "nl")];
        expected.sort_by_key(|task| task.id);
        assert_eq!(every, expected);
        assert_eq!(app.list_tasks(Some(nl)).unwrap(), [task("nl", "nl")]);
        assert_eq!(app.funnel_state(nl), Ok(needs_setup()));
        assert_eq!(app.funnel_state(mac), Ok(FunnelState::Off));
        assert!(sent(&fake).is_empty());
    }

    #[test]
    fn a_change_goes_to_the_task_s_home() {
        let (app, fake, nl) = app();
        let id = TaskId::derive("nl");
        let outcome = app.act_on_task(id, |key| ClientMessage::RunTaskNow { key, task: id });
        assert_eq!(block_on(outcome), Ok(ActionOutcome::Done));
        let calls = sent(&fake);
        let [(link, ClientMessage::RunTaskNow { task: ran, .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, *ran), (nl.id, id));

        let saved = task("new", "nl");
        block_on(app.save_task(DeviceId::derive("nl"), saved.clone())).unwrap();
        let calls = sent(&fake);
        let [(
            link,
            ClientMessage::SaveTask {
                task: sent_task, ..
            },
        )] = calls.as_slice()
        else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, sent_task.as_ref()), (nl.id, &saved));
    }

    #[test]
    fn a_task_for_another_device_gives_it_the_permission_and_takes_it_back() {
        let remote = task("remote", "nl");
        let (app, fake, nl) = app_with(vec![remote.clone()]);
        let mac = DeviceId::derive("mac");
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(remote.clone()),
            hook_url: None,
        });
        block_on(app.save_task(mac, remote.clone())).unwrap();
        let granted: Vec<_> = sent(&fake)
            .into_iter()
            .filter_map(|(link, message)| match message {
                ClientMessage::GrantPermission { permission, .. } => Some((link, *permission)),
                _ => None,
            })
            .collect();
        let permission = task_permission(&remote, mac);
        assert_eq!(permission.devices, [DeviceId::derive("nl")]);
        assert!(granted.contains(&(nl.id, permission)));

        block_on(app.delete_task(remote.id)).unwrap();
        let calls = sent(&fake);
        assert!(matches!(
            calls.first(),
            Some((_, ClientMessage::DeleteTask { .. }))
        ));
        assert!(calls.iter().any(|(link, message)| *link == nl.id
            && matches!(message, ClientMessage::RevokePermission { permission, .. }
                if *permission == remote.permission)));

        // One whose target is on its home sends no permission anywhere.
        let here = task("here", "mac");
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(here.clone()),
            hook_url: None,
        });
        block_on(app.save_task(mac, here)).unwrap();
        assert_eq!(sent(&fake).len(), 1);
    }

    /// What each `GrantPermission` sent names.
    fn granted(fake: &Fake) -> Vec<maxhq_protocol::agents::Permission> {
        sent(fake)
            .into_iter()
            .filter_map(|(_, message)| match message {
                ClientMessage::GrantPermission { permission, .. } => Some(*permission),
                _ => None,
            })
            .collect()
    }

    /// The app with devbox holding a task aimed at an agent on this machine,
    /// as a device could say it does without the owner ever saving it.
    fn app_with_a_row_aimed_at_the_mac() -> (App<Fake>, Fake, LinkSpec, Task) {
        let (h, local, nl) = two_devices();
        connect_doing(&h, &local, "mac", scheduled_tasks());
        connect_doing(&h, &nl, "nl", scheduled_tasks());
        let row = Task {
            target: TaskTarget::Agent {
                device: DeviceId::derive("mac"),
                agent: AgentId::derive("private"),
            },
            ..task("made-up", "nl")
        };
        publish(
            &h,
            &nl,
            DeviceTasks {
                device: DeviceId::derive("nl"),
                tasks: vec![row.clone()],
                address: HookAddress::Local,
                port: None,
                funnel: FunnelState::Off,
                problems: Vec::new(),
                webhooks: Vec::new(),
            },
        );
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        let app = App::new(view, devices, FakeShell::default());
        (app, fake, nl, row)
    }

    #[test]
    fn a_pause_or_resume_sends_only_the_switch_and_grants_nothing() {
        let (app, fake, nl, row) = app_with_a_row_aimed_at_the_mac();
        block_on(app.set_task_enabled(DeviceId::derive("nl"), row.id, true)).unwrap();
        let sent = sent(&fake);
        assert_eq!(sent.len(), 1, "only the switch goes: {sent:?}");
        assert!(matches!(
            &sent[0],
            (link, ClientMessage::SetTaskEnabled { task, enabled: true, .. })
                if *link == nl.id && *task == row.id
        ));
    }

    #[test]
    fn an_older_maxd_gets_its_own_row_back_and_nothing_is_granted() {
        let (app, fake, nl, row) = app_with_a_row_aimed_at_the_mac();
        fake.record().oldest.insert(nl.id);
        block_on(app.set_task_enabled(DeviceId::derive("nl"), row.id, false)).unwrap();
        let sent = sent(&fake);
        let saved: Vec<_> = sent
            .iter()
            .filter_map(|(link, message)| match message {
                ClientMessage::SaveTask { task, .. } => Some((*link, (**task).clone())),
                _ => None,
            })
            .collect();
        let off = Task {
            enabled: false,
            ..row
        };
        assert_eq!(saved, [(nl.id, off)]);
        let granted = sent
            .iter()
            .any(|(_, message)| matches!(message, ClientMessage::GrantPermission { .. }));
        assert!(!granted, "a switch grants nothing: {sent:?}");
    }

    #[test]
    fn an_older_maxd_can_t_switch_a_task_it_doesn_t_list() {
        let (app, fake, nl, _) = app_with_a_row_aimed_at_the_mac();
        fake.record().oldest.insert(nl.id);
        let nobody = TaskId::derive("nobody");
        assert!(matches!(
            block_on(app.set_task_enabled(DeviceId::derive("nl"), nobody, false)),
            Err(CommandError::NotFound { .. })
        ));
        assert!(!sent(&fake)
            .iter()
            .any(|(_, message)| matches!(message, ClientMessage::SaveTask { .. })));
    }

    /// A device that's the home of a task must not widen what the owner
    /// asked for by answering with another target.
    #[test]
    fn a_device_s_answer_can_t_change_what_the_app_grants() {
        let mine = task("mine", "nl");
        let (app, fake, _) = app_with(vec![mine.clone()]);
        let nl = DeviceId::derive("nl");
        let mac = DeviceId::derive("mac");
        let elsewhere = |task: &Task| Task {
            target: TaskTarget::Agent {
                device: mac,
                agent: AgentId::derive("private"),
            },
            ..task.clone()
        };

        // The task stays on its home, and the answer points it at the Mac.
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(elsewhere(&mine)),
            hook_url: None,
        });
        assert!(matches!(
            block_on(app.save_task(nl, mine.clone())),
            Err(CommandError::Failed { .. })
        ));
        assert_eq!(granted(&fake), [], "a changed target grants nothing");

        // Another ID, or an agent's task turned into the user's, is as bad.
        let by_agent = Task {
            made_by: Maker::Tools {
                origin: maxhq_protocol::agents::AgentOrigin {
                    permission: PermissionId::derive("agent's"),
                    by: maxhq_protocol::agents::Actor::Task {
                        task: TaskId::derive("mine"),
                    },
                    device: nl,
                    level: 0,
                    wake: true,
                    container: None,
                },
            },
            ..mine.clone()
        };
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(mine.clone()),
            hook_url: None,
        });
        assert!(block_on(app.save_task(nl, by_agent)).is_err());
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(task("other", "nl")),
            hook_url: None,
        });
        assert!(block_on(app.save_task(nl, mine)).is_err());
        assert_eq!(granted(&fake), []);
    }

    #[test]
    fn a_task_s_permission_id_comes_from_the_task_s_own_id() {
        let (app, fake, _) = app();
        let mac = DeviceId::derive("mac");
        let other_permission = PermissionId::derive("someone else's");
        let remote = Task {
            permission: other_permission,
            ..task("remote", "nl")
        };
        fake.record().acts.push_back(ActionOutcome::TaskSaved {
            task: Box::new(remote.clone()),
            hook_url: None,
        });
        block_on(app.save_task(mac, remote.clone())).unwrap();
        let ids: Vec<_> = granted(&fake).iter().map(|sent| sent.id).collect();
        assert!(!ids.is_empty());
        assert!(ids.iter().all(|id| *id == user_permission(remote.id)));
    }

    #[test]
    fn deleting_a_row_revokes_only_that_task_s_own_permission() {
        let other_permission = PermissionId::derive("someone else's");
        let row = Task {
            permission: other_permission,
            ..task("row", "nl")
        };
        let (app, fake, _) = app_with(vec![row.clone()]);
        block_on(app.delete_task(row.id)).unwrap();
        let revoked: Vec<_> = sent(&fake)
            .into_iter()
            .filter_map(|(_, message)| match message {
                ClientMessage::RevokePermission { permission, .. } => Some(permission),
                _ => None,
            })
            .collect();
        assert!(!revoked.is_empty());
        assert!(revoked.iter().all(|id| *id == user_permission(row.id)));
    }

    #[test]
    fn a_secret_doesn_t_go_to_a_device_that_lists_the_same_task() {
        let held = |device: &str, name: &str| DeviceTasks {
            device: DeviceId::derive(device),
            tasks: vec![task(name, device)],
            address: HookAddress::Local,
            port: None,
            funnel: needs_setup(),
            problems: Vec::new(),
            webhooks: Vec::new(),
        };
        let secret = || Secret::new("typed".to_owned());
        let id = task("real", "nl").id;

        // On this machine, its own report settles it, whichever sorts first.
        let (h, local, nl) = two_devices();
        connect_doing(&h, &local, "mac", scheduled_tasks());
        connect_doing(&h, &nl, "nl", scheduled_tasks());
        publish(&h, &local, held("mac", "real"));
        publish(&h, &nl, held("nl", "real"));
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        let app = App::new(view, devices, FakeShell::default());
        block_on(app.set_hook_secret(id, secret())).unwrap();
        let calls = sent(&fake);
        assert!(!calls.is_empty());
        assert!(calls.iter().all(|(link, _)| *link == local.id));

        // Between two others, it goes to neither.
        let registry = crate::registry::Registry::in_memory().unwrap();
        let one = registry.add("one").unwrap();
        let two = registry.add("two").unwrap();
        let h = crate::devices::tests::harness(registry, None);
        connect_doing(&h, &one, "one", scheduled_tasks());
        connect_doing(&h, &two, "two", scheduled_tasks());
        publish(&h, &one, held("one", "real"));
        publish(&h, &two, held("two", "real"));
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        let app = App::new(view, devices, FakeShell::default());
        let refused = block_on(app.set_hook_secret(id, secret()));
        assert!(matches!(refused, Err(CommandError::Failed { .. })));
        assert!(sent(&fake).is_empty(), "no secret left the app");
    }

    #[test]
    fn a_task_the_view_doesn_t_hold_is_gone() {
        let (app, fake, _) = app();
        let id = TaskId::derive("missing");
        let outcome = app.act_on_task(id, |key| ClientMessage::DeleteTask { key, task: id });
        assert_eq!(
            block_on(outcome).unwrap_err().to_string(),
            "This task is gone"
        );
        assert!(sent(&fake).is_empty());
    }

    #[test]
    fn runs_and_deliveries_come_back_as_lists() {
        let (app, fake, _) = app();
        let id = TaskId::derive("nl");
        let run = Run {
            task: id,
            cause: RunCause::Timer,
            started: Timestamp(1),
            ended: None,
            outcome: RunOutcome::Running,
            agent: Some(AgentId::derive("a")),
            delivery: None,
            error: None,
        };
        fake.record().acts.extend([
            ActionOutcome::Runs {
                runs: vec![run.clone()],
            },
            ActionOutcome::Refused {
                reason: "The task's log is locked".to_owned(),
            },
        ]);
        assert_eq!(block_on(app.list_runs(id)), Ok(vec![run]));
        assert_eq!(
            block_on(app.list_deliveries(id)),
            Err(CommandError::Failed {
                message: "The task's log is locked".to_owned()
            })
        );
        assert_eq!(block_on(app.list_deliveries(id)), Ok(Vec::new()));
    }

    #[test]
    fn a_hook_s_url_reaches_the_caller_and_no_log() {
        let (app, fake, _) = app();
        let id = TaskId::derive("nl");
        let url = "https://devbox.example.ts.net/hooks/test-not-a-real-token";
        fake.record().acts.push_back(ActionOutcome::HookUrl {
            url: Secret::new(url.to_owned()),
        });
        let outcome =
            block_on(app.act_on_task(id, |key| ClientMessage::RotateHookToken { key, task: id }))
                .unwrap();
        assert!(!format!("{outcome:?}").contains("test-not-a-real"));
        let ActionOutcome::HookUrl { url: shown } = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(shown.expose(), url);
    }

    #[test]
    fn a_hook_address_goes_to_its_device() {
        let (app, fake, nl) = app();
        fake.record().acts.push_back(ActionOutcome::Done);
        let local = block_on(app.set_hook_address(DeviceId::derive("nl"), HookAddress::Local));
        assert_eq!(local, Ok(ActionOutcome::Done));
        let calls = sent(&fake);
        let [(link, ClientMessage::SetHookAddress { address, .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, address), (nl.id, &HookAddress::Local));
    }

    #[test]
    fn an_empty_secret_never_leaves_the_app() {
        let (app, fake, nl) = app();
        let id = TaskId::derive("nl");
        let empty = block_on(app.set_hook_secret(id, Secret::new(String::new())));
        assert_eq!(empty.unwrap_err().to_string(), "Enter the secret first");
        assert!(sent(&fake).is_empty());

        let secret = Secret::new("test-not-a-real-secret".to_owned());
        block_on(app.set_hook_secret(id, secret.clone())).unwrap();
        let calls = sent(&fake);
        let [(
            link,
            ClientMessage::SetHookSecret {
                secret: carried, ..
            },
        )] = calls.as_slice()
        else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, carried), (nl.id, &secret));
    }

    #[test]
    fn a_maxd_from_before_scheduled_tasks_is_refused_in_words() {
        let (h, _, nl) = two_devices();
        connect_doing(&h, &nl, "nl", Capabilities::default());
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let device = DeviceId::derive("nl");
        for result in [
            app.list_tasks(Some(device)).map(drop),
            app.funnel_state(device).map(drop),
            block_on(app.save_task(device, task("t", "nl"))).map(drop),
        ] {
            let Err(CommandError::Failed { message }) = result else {
                panic!("{result:?}");
            };
            assert!(message.ends_with("maxd is too old to run scheduled tasks. Update it first"));
        }
        assert!(sent(&h.fake).is_empty());
    }
}

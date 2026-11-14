// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Permissions to run agents, and the access keys tools outside MaxHQ
//! use. Both live on the devices: a permission goes to its home and to
//! every device it names, since each checks the calls that reach it, and
//! the app reads them back from the devices instead of keeping a copy.
//! The defaults a new permission starts with are the app's own setting.

use maxhq_protocol::agents::{AccessKey, Actor, Limits, Permission, DEPTH};
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, PermissionId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::Entity;
use maxhq_protocol::ClientMessage;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::devices::Connector;
use crate::fleet::{check_can, device_name, Needs};
use crate::request::{lookup_failed, CommandError};
use crate::App;

/// The setting the defaults are saved under.
const DEFAULTS: &str = "agent_defaults";

/// What a new permission starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AgentDefaults {
    /// The limits, all off until you set them.
    pub limits: Limits,
    /// How many levels of agents it reaches.
    pub depth: u8,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            depth: DEPTH,
        }
    }
}

impl<C: Connector> App<C> {
    /// One device's permissions, or those of every connected device that
    /// holds them, each once and as its home has it. A device that doesn't
    /// answer is left out of the second.
    pub async fn list_permissions(
        &self,
        device: Option<DeviceId>,
    ) -> Result<Vec<Permission>, CommandError> {
        let found = self
            .gather(
                device,
                |key| ClientMessage::ListPermissions { key },
                |outcome| match outcome {
                    ActionOutcome::Permissions { permissions } => Ok(permissions),
                    other => Err(lookup_failed(other)),
                },
            )
            .await?;
        let mut permissions: Vec<Permission> = Vec::new();
        for (from, permission) in found {
            match permissions.iter_mut().find(|kept| kept.id == permission.id) {
                Some(kept) if from == permission.home => *kept = permission,
                Some(_) => {}
                None => permissions.push(permission),
            }
        }
        Ok(permissions)
    }

    /// One device's access keys, or those of every connected device that
    /// makes them. A device that doesn't answer is left out of the second.
    pub async fn list_keys(
        &self,
        device: Option<DeviceId>,
    ) -> Result<Vec<AccessKey>, CommandError> {
        let found = self
            .gather(
                device,
                |key| ClientMessage::ListKeys { key },
                |outcome| match outcome {
                    ActionOutcome::Keys { keys } => Ok(keys),
                    other => Err(lookup_failed(other)),
                },
            )
            .await?;
        Ok(found.into_iter().map(|(_, key)| key).collect())
    }

    /// Writes a permission to its home and each device it names, and
    /// takes it off the other connected devices, for one that names fewer
    /// than before. It's done once every device has it, and otherwise
    /// fails with each device that didn't.
    pub async fn grant_permission(
        &self,
        permission: Permission,
    ) -> Result<ActionOutcome, CommandError> {
        let to = holders(&permission);
        // A revoke still waiting would take the permission off again.
        for &device in &to {
            self.forget_revoke(device, permission.id);
        }
        let others: Vec<DeviceId> = self
            .known_agent_devices()
            .into_iter()
            .filter(|device| !to.contains(device))
            .collect();
        let mut results = self.grant_on(&to, &permission).await;
        results.extend(self.revoke_on(&others, permission.id).await);
        self.settle(results)
    }

    /// Takes a permission off every device. An access key that holds it
    /// stops working. A device that isn't connected gets the revoke once
    /// it connects.
    pub async fn revoke_permission(
        &self,
        permission: PermissionId,
    ) -> Result<ActionOutcome, CommandError> {
        let on = self.known_agent_devices();
        if on.is_empty() {
            return Err(CommandError::Failed {
                message: "No device holds permissions".to_owned(),
            });
        }
        let results = self.revoke_on(&on, permission).await;
        self.settle(results)
    }

    /// Makes an access key on `device`, which holds `permission`, and
    /// writes the permission to the other devices it names. The outcome
    /// says where the key file is, and never holds the key.
    pub async fn mint_key(
        &self,
        device: DeviceId,
        name: String,
        permission: Permission,
    ) -> Result<ActionOutcome, CommandError> {
        self.view
            .read(|merge| check_can(merge, device, Needs::AgentTools))?;
        let action = ClientMessage::MintKey {
            key: IdempotencyKey::generate(),
            name,
            permission: Box::new(permission.clone()),
        };
        let outcome = self.devices.act_on(device, action).await?;
        let ActionOutcome::KeyMinted { id, .. } = &outcome else {
            return Ok(outcome);
        };
        let held = Permission {
            holder: Actor::Key { key: *id },
            ..permission
        };
        let others: Vec<DeviceId> = holders(&held)
            .into_iter()
            .filter(|other| *other != device)
            .collect();
        let results = self.grant_on(&others, &held).await;
        if let Err(error) = self.settle(results) {
            return Err(CommandError::Failed {
                message: format!(
                    "The key is made, but not every device has its permission. {error}"
                ),
            });
        }
        Ok(outcome)
    }

    /// What a new permission starts with: what you saved, or no limits
    /// and [`DEPTH`] levels.
    pub fn agent_defaults(&self) -> Result<AgentDefaults, CommandError> {
        let saved = self.devices.registry().setting(DEFAULTS)?;
        Ok(saved.unwrap_or_default())
    }

    /// Saves what a new permission starts with. It reaches at least one
    /// level.
    pub fn set_agent_defaults(&self, defaults: AgentDefaults) -> Result<(), CommandError> {
        if defaults.depth == 0 {
            return Err(CommandError::Failed {
                message: "A permission reaches at least one level of agents".to_owned(),
            });
        }
        self.devices.registry().set_setting(DEFAULTS, &defaults)?;
        Ok(())
    }

    /// The connected devices whose `maxd` holds permissions, or might,
    /// before its hello says.
    fn agent_devices(&self) -> Vec<DeviceId> {
        self.devices_holding(true)
    }

    /// The devices whose `maxd` holds permissions, or might, connected or
    /// not.
    fn known_agent_devices(&self) -> Vec<DeviceId> {
        self.devices_holding(false)
    }

    /// The devices whose `maxd` holds permissions, or might, only those
    /// connected when `connected_only` says so.
    fn devices_holding(&self, connected_only: bool) -> Vec<DeviceId> {
        self.view.read(|merge| {
            let mut devices = Vec::new();
            for entity in merge.store().entities() {
                let Entity::DeviceLink(link) = entity else {
                    continue;
                };
                let connected =
                    !connected_only || matches!(link.state, LinkState::Connected { .. });
                let able = link.capabilities.is_none_or(|does| does.agent_tools);
                match link.device {
                    Some(device) if connected && able && !devices.contains(&device) => {
                        devices.push(device);
                    }
                    _ => {}
                }
            }
            devices
        })
    }

    /// Asks one device, or every device that holds permissions, for a
    /// list, and gives each item with the device it came from. One device
    /// fails as it answered, and a device among every one that fails is
    /// left out.
    async fn gather<T>(
        &self,
        device: Option<DeviceId>,
        ask: impl Fn(IdempotencyKey) -> ClientMessage,
        take: impl Fn(ActionOutcome) -> Result<Vec<T>, CommandError>,
    ) -> Result<Vec<(DeviceId, T)>, CommandError> {
        let on = match device {
            Some(device) => vec![device],
            None => self.agent_devices(),
        };
        let mut found = Vec::new();
        for (from, result) in self.ask_each(&on, ask).await {
            let listed = result.and_then(&take);
            match listed {
                Ok(items) => found.extend(items.into_iter().map(|item| (from, item))),
                Err(error) if device.is_some() => return Err(error),
                Err(_) => {}
            }
        }
        Ok(found)
    }

    /// Writes a permission to each device.
    async fn grant_on(
        &self,
        devices: &[DeviceId],
        permission: &Permission,
    ) -> Vec<(DeviceId, Result<ActionOutcome, CommandError>)> {
        let grant = |key| ClientMessage::GrantPermission {
            key,
            permission: Box::new(permission.clone()),
        };
        self.ask_each(devices, grant).await
    }

    /// Takes a permission off each device. One that isn't connected
    /// keeps the revoke waiting until it connects, and counts as done.
    async fn revoke_on(
        &self,
        devices: &[DeviceId],
        permission: PermissionId,
    ) -> Vec<(DeviceId, Result<ActionOutcome, CommandError>)> {
        let revoke = |key| ClientMessage::RevokePermission { key, permission };
        let mut results = self.ask_each(devices, revoke).await;
        for (device, result) in &mut results {
            match result {
                Ok(ActionOutcome::Done) => self.forget_revoke(*device, permission),
                Err(CommandError::NotConnected { .. }) => {
                    let kept = self.devices.registry().revoke_later(*device, permission);
                    if let Err(error) = kept {
                        tracing::error!(%error, "couldn't keep a revoke for a device that's away");
                        continue;
                    }
                    *result = Ok(ActionOutcome::Done);
                }
                _ => {}
            }
        }
        results
    }

    /// Lets go of a revoke waiting for `device`.
    fn forget_revoke(&self, device: DeviceId, permission: PermissionId) {
        let forgot = self.devices.registry().forget_revoke(device, permission);
        if let Err(error) = forgot {
            tracing::error!(%error, "couldn't let go of a waiting revoke");
        }
    }

    /// Sends an action to each device in turn under a fresh key, once its
    /// `maxd` is new enough, and gives each device's result.
    async fn ask_each(
        &self,
        devices: &[DeviceId],
        action: impl Fn(IdempotencyKey) -> ClientMessage,
    ) -> Vec<(DeviceId, Result<ActionOutcome, CommandError>)> {
        let mut results = Vec::with_capacity(devices.len());
        for &device in devices {
            let checked = self
                .view
                .read(|merge| check_can(merge, device, Needs::AgentTools));
            let result = match checked {
                Ok(()) => {
                    let action = action(IdempotencyKey::generate());
                    self.devices.act_on(device, action).await
                }
                Err(error) => Err(error),
            };
            results.push((device, result));
        }
        results
    }

    /// Done when every device is, and otherwise a failure that says why
    /// for each device that isn't.
    fn settle(
        &self,
        results: Vec<(DeviceId, Result<ActionOutcome, CommandError>)>,
    ) -> Result<ActionOutcome, CommandError> {
        let why: Vec<String> = results
            .into_iter()
            .filter_map(|(device, result)| match result {
                Ok(ActionOutcome::Done) => None,
                Ok(other) => {
                    let name = self.view.read(|merge| device_name(merge, device));
                    Some(format!("{name}: {}", lookup_failed(other)))
                }
                Err(error) => Some(error.to_string()),
            })
            .collect();
        if why.is_empty() {
            Ok(ActionOutcome::Done)
        } else {
            Err(CommandError::Failed {
                message: why.join(". "),
            })
        }
    }
}

/// The devices that hold a permission: its home, then each it names.
fn holders(permission: &Permission) -> Vec<DeviceId> {
    let mut devices = vec![permission.home];
    for &device in &permission.devices {
        if !devices.contains(&device) {
            devices.push(device);
        }
    }
    devices
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::agents::{LeaveContainer, McpCommand};
    use maxhq_protocol::ids::{AgentId, KeyId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::Vendor;

    use super::*;
    use maxhq_client::{LinkEvent, LinkSpec};

    use crate::devices::tests::{connect_doing, two_devices, wait_until, Call, Fake, Harness};
    use crate::merge::tests::{connected, link_state, unreachable};
    use crate::tests::{block_on, FakeShell};

    /// What both sample devices' `maxd` does.
    fn agent_tools() -> Capabilities {
        Capabilities {
            agent_tools: true,
            ..Capabilities::default()
        }
    }

    /// The app over this machine and devbox, both new enough for
    /// permissions, with the fake behind them.
    fn app() -> (App<Fake>, Fake) {
        let (app, fake, _) = app_and_devbox();
        (app, fake)
    }

    /// The app as `app` makes it, with devbox's link.
    fn app_and_devbox() -> (App<Fake>, Fake, LinkSpec) {
        let (h, local, nl) = two_devices();
        connect_doing(&h, &local, "mac", agent_tools());
        connect_doing(&h, &nl, "nl", agent_tools());
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        (App::new(view, devices, FakeShell::default()), fake, nl)
    }

    /// Has devbox's link drop, or come back, and waits for the view to
    /// show it.
    fn devbox_up(app: &App<Fake>, fake: &Fake, nl: &LinkSpec, up: bool) {
        let LinkEvent::Link(mut link) =
            link_state(nl, if up { connected() } else { unreachable() })
        else {
            unreachable!("link_state reports a link");
        };
        link.device = Some(DeviceId::derive("nl"));
        link.capabilities = Some(agent_tools());
        if up {
            fake.record().up.insert(nl.id);
        } else {
            fake.record().up.remove(&nl.id);
        }
        let events = fake.record().events[&nl.id].clone();
        block_on(events.send(LinkEvent::Link(link))).unwrap();
        let nl = DeviceId::derive("nl");
        wait_until("devbox never moved", || {
            app.agent_devices().contains(&nl) == up
        });
    }

    /// The revokes waiting for a device that's away.
    fn waiting(app: &App<Fake>) -> Vec<(DeviceId, PermissionId)> {
        app.devices.registry().pending_revokes().unwrap()
    }

    /// A permission for an agent, at home on `home` and naming `devices`.
    fn permission(id: &str, home: &str, devices: &[&str]) -> Permission {
        Permission {
            id: PermissionId::derive(id),
            holder: Actor::Agent {
                agent: AgentId::derive("lead"),
            },
            home: DeviceId::derive(home),
            devices: devices
                .iter()
                .map(|device| DeviceId::derive(device))
                .collect(),
            vendors: vec![Vendor::Claude],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Ask,
        }
    }

    /// Each action sent, with the device it went to.
    fn sent(app: &App<Fake>, fake: &Fake) -> Vec<(DeviceId, ClientMessage)> {
        let links = app.view.read(|merge| {
            merge
                .store()
                .entities()
                .filter_map(|entity| match entity {
                    Entity::DeviceLink(link) => Some((link.id, link.device?)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        fake.calls()
            .into_iter()
            .filter_map(|(link, call)| {
                let Call::Send(message) = call else {
                    return None;
                };
                let device = links.iter().find(|(id, _)| *id == link)?.1;
                Some((device, message))
            })
            .collect()
    }

    #[test]
    fn a_permission_goes_to_its_home_and_devices_and_leaves_the_rest() {
        let (app, fake) = app();
        let (mac, nl) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let both = permission("p", "mac", &["nl", "mac"]);
        assert_eq!(
            block_on(app.grant_permission(both.clone())),
            Ok(ActionOutcome::Done)
        );
        let grants: Vec<_> = sent(&app, &fake)
            .into_iter()
            .map(|(device, message)| match message {
                ClientMessage::GrantPermission { permission, .. } => (device, *permission),
                other => panic!("sent {other:?}"),
            })
            .collect();
        assert_eq!(grants, [(mac, both.clone()), (nl, both)]);

        let fewer = permission("p", "mac", &[]);
        block_on(app.grant_permission(fewer)).unwrap();
        let calls = sent(&app, &fake);
        assert!(matches!(
            calls.as_slice(),
            [
                (first, ClientMessage::GrantPermission { .. }),
                (second, ClientMessage::RevokePermission { permission, .. }),
            ] if *first == mac && *second == nl && *permission == PermissionId::derive("p")
        ));

        block_on(app.revoke_permission(PermissionId::derive("p"))).unwrap();
        let mut revoked: Vec<_> = sent(&app, &fake)
            .into_iter()
            .map(|(device, message)| {
                assert!(matches!(message, ClientMessage::RevokePermission { .. }));
                device
            })
            .collect();
        revoked.sort();
        let mut expected = vec![mac, nl];
        expected.sort();
        assert_eq!(revoked, expected);
    }

    #[test]
    fn a_device_that_refuses_is_named_in_the_failure() {
        let (app, fake) = app();
        fake.record().acts.extend([
            ActionOutcome::Done,
            ActionOutcome::Refused {
                reason: "couldn't store the permission: disk full".to_owned(),
            },
        ]);
        let result = block_on(app.grant_permission(permission("p", "mac", &["nl"])));
        assert_eq!(
            result,
            Err(CommandError::Failed {
                message: "devbox: couldn't store the permission: disk full".to_owned()
            })
        );
    }

    #[test]
    fn every_device_lists_each_permission_once_as_its_home_has_it() {
        let (app, fake) = app();
        let order = app.agent_devices();
        let home = DeviceId::derive("nl");
        let mut stale = permission("shared", "nl", &["mac", "nl"]);
        stale.depth = 1;
        let current = permission("shared", "nl", &["mac", "nl"]);
        let only_mac = permission("mac-only", "mac", &["mac"]);
        for device in &order {
            let permissions = if *device == home {
                vec![current.clone()]
            } else {
                vec![stale.clone(), only_mac.clone()]
            };
            fake.record()
                .acts
                .push_back(ActionOutcome::Permissions { permissions });
        }
        let mut listed = block_on(app.list_permissions(None)).unwrap();
        listed.sort_by_key(|permission| permission.id);
        let mut expected = vec![current, only_mac];
        expected.sort_by_key(|permission| permission.id);
        assert_eq!(listed, expected);

        assert_eq!(
            block_on(app.list_permissions(Some(home))).unwrap(),
            Vec::<Permission>::new()
        );
        assert_eq!(block_on(app.list_keys(None)).unwrap(), Vec::new());
    }

    #[test]
    fn a_minted_key_s_permission_reaches_the_other_devices_it_names() {
        let (app, fake) = app();
        let (mac, nl) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let key = KeyId::derive("ci");
        let path = "/home/dev/.local/share/maxhq/keys/ci.key".to_owned();
        let minted = ActionOutcome::KeyMinted {
            id: key,
            path: path.clone(),
            permission: PermissionId::derive("p"),
            command: McpCommand {
                command: "/home/dev/.local/bin/maxd".to_owned(),
                args: vec!["mcp".to_owned(), "--key-file".to_owned(), path],
            },
        };
        fake.record().acts.push_back(minted.clone());
        let asked = permission("p", "mac", &["mac", "nl"]);
        assert_eq!(
            block_on(app.mint_key(mac, "ci".to_owned(), asked.clone())),
            Ok(minted)
        );
        let calls = sent(&app, &fake);
        let [(first, ClientMessage::MintKey { name, .. }), (second, ClientMessage::GrantPermission { permission, .. })] =
            calls.as_slice()
        else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*first, name.as_str(), *second), (mac, "ci", nl));
        assert_eq!(permission.holder, Actor::Key { key });
        assert_eq!(permission.devices, asked.devices);
    }

    #[test]
    fn a_maxd_from_before_agent_tools_is_refused_in_words() {
        let (h, _, nl) = two_devices();
        connect_doing(&h, &nl, "nl", Capabilities::default());
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let device = DeviceId::derive("nl");
        for result in [
            block_on(app.list_keys(Some(device))).map(drop),
            block_on(app.mint_key(device, "ci".to_owned(), permission("p", "nl", &[]))).map(drop),
        ] {
            let Err(CommandError::Failed { message }) = result else {
                panic!("{result:?}");
            };
            assert!(message.ends_with("maxd is too old to let agents run agents. Update it first"));
        }
        assert!(!app.agent_devices().contains(&device));
    }

    #[test]
    fn a_device_that_is_away_gets_the_revoke_once_it_connects() {
        let (app, fake, link) = app_and_devbox();
        let (mac, nl) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let p = PermissionId::derive("p");
        devbox_up(&app, &fake, &link, false);
        drop(fake.calls());
        assert_eq!(block_on(app.revoke_permission(p)), Ok(ActionOutcome::Done));
        assert_eq!(waiting(&app), [(nl, p)]);
        let reached: Vec<DeviceId> = sent(&app, &fake).into_iter().map(|(to, _)| to).collect();
        assert!(reached.contains(&mac));

        devbox_up(&app, &fake, &link, true);
        wait_until("the revoke never went", || waiting(&app).is_empty());
        let replayed = sent(&app, &fake);
        assert!(replayed.iter().any(|(to, message)| *to == nl
            && matches!(message, ClientMessage::RevokePermission { permission, .. } if *permission == p)));
    }

    #[test]
    fn granting_a_device_again_lets_go_of_its_waiting_revoke() {
        let (app, fake, link) = app_and_devbox();
        let nl = DeviceId::derive("nl");
        let p = PermissionId::derive("p");
        devbox_up(&app, &fake, &link, false);
        block_on(app.revoke_permission(p)).unwrap();
        assert_eq!(waiting(&app), [(nl, p)]);

        // Devbox is away, so the grant fails for it, but the revoke must
        // not take it off once it's back.
        assert!(block_on(app.grant_permission(permission("p", "mac", &["nl"]))).is_err());
        assert!(waiting(&app).is_empty());
        drop(fake.calls());
        devbox_up(&app, &fake, &link, true);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(sent(&app, &fake)
            .iter()
            .all(|(_, message)| !matches!(message, ClientMessage::RevokePermission { .. })));
    }

    #[test]
    fn the_defaults_start_with_no_limits_and_keep_what_you_save() {
        let (app, _) = app();
        assert_eq!(
            app.agent_defaults().unwrap(),
            AgentDefaults {
                limits: Limits::default(),
                depth: DEPTH,
            }
        );
        let saved = AgentDefaults {
            limits: Limits {
                max_running: Some(2),
                ..Limits::default()
            },
            depth: 1,
        };
        app.set_agent_defaults(saved).unwrap();
        assert_eq!(app.agent_defaults().unwrap(), saved);
        assert!(app
            .set_agent_defaults(AgentDefaults { depth: 0, ..saved })
            .is_err());
        assert_eq!(app.agent_defaults().unwrap(), saved);
    }
}

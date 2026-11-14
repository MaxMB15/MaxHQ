// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Projects hidden from the sidebar. Each `maxd` keeps a list, and the
//! sidebar hides a project any list names, on every device it's on. A
//! hide goes to one device: a connected one first, then a Mac, since the
//! sidebar's state lives there, then this machine. Showing a project again
//! takes it off every list that names it.

use maxhq_protocol::ids::{DeviceId, IdempotencyKey, ProjectId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::ClientMessage;

use crate::devices::{device_connected, is_local, Connector, Devices};
use crate::merge::Merge;
use crate::request::CommandError;
use crate::view::View;

/// Hides `project` from the sidebar, or shows it again. The first device
/// that doesn't take the change answers for all of them.
pub async fn set_hidden<C: Connector>(
    devices: &Devices<C>,
    view: &View,
    project: ProjectId,
    hidden: bool,
) -> Result<ActionOutcome, CommandError> {
    let keepers = view.read(|merge| keepers(merge, project, hidden));
    if hidden && keepers.is_empty() {
        return Err(CommandError::Failed {
            message: "No device can hide projects. Update maxd first".to_owned(),
        });
    }
    for device in keepers {
        let action = ClientMessage::SetProjectHidden {
            key: IdempotencyKey::generate(),
            device,
            project,
            hidden,
        };
        let outcome = devices.act(action).await?;
        if outcome != ActionOutcome::Done {
            return Ok(outcome);
        }
    }
    Ok(ActionOutcome::Done)
}

/// The devices a change goes to: for a hide, the one that keeps it, and
/// for a show, each one whose list names the project.
fn keepers(merge: &Merge, project: ProjectId, hidden: bool) -> Vec<DeviceId> {
    let lists = merge.store().entities().filter_map(|entity| match entity {
        Entity::HiddenProjects(list) => Some(list),
        _ => None,
    });
    if !hidden {
        return lists
            .filter(|list| list.projects.contains(&project))
            .map(|list| list.device)
            .collect();
    }
    lists
        .map(|list| list.device)
        .min_by_key(|&device| {
            (
                !device_connected(merge, device),
                !is_mac(merge, device),
                !is_local(merge, device),
                device,
            )
        })
        .into_iter()
        .collect()
}

/// Whether the view shows the device as a Mac.
fn is_mac(merge: &Merge, device: DeviceId) -> bool {
    matches!(
        merge.store().get(&EntityKey::Device(device)),
        Some(Entity::Device(found)) if found.os == "macos"
    )
}

#[cfg(test)]
mod tests {
    use maxhq_client::LinkEvent;
    use maxhq_protocol::model::{Device, HiddenProjects};
    use maxhq_protocol::state::Change;

    use super::*;
    use crate::devices::tests::{disconnect, two_devices, Call, Harness};
    use crate::tests::block_on;

    fn list(device: &str, projects: &[ProjectId]) -> Entity {
        Entity::HiddenProjects(HiddenProjects {
            device: DeviceId::derive(device),
            projects: projects.to_vec(),
        })
    }

    fn device(id: &str, os: &str) -> Entity {
        Entity::Device(Device {
            id: DeviceId::derive(id),
            name: id.into(),
            os: os.into(),
            boot_id: maxhq_protocol::ids::BootId("b".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
        })
    }

    fn upsert(entities: Vec<Entity>) -> LinkEvent {
        LinkEvent::Changed(
            entities
                .into_iter()
                .map(|value| Change::Upsert { value })
                .collect(),
        )
    }

    /// Where each change went, as (device, project, hidden).
    fn sent(h: &Harness) -> Vec<(DeviceId, ProjectId, bool)> {
        h.fake
            .calls()
            .into_iter()
            .filter_map(|(_, call)| match call {
                Call::Send(ClientMessage::SetProjectHidden {
                    device,
                    project,
                    hidden,
                    ..
                }) => Some((device, project, hidden)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_hide_goes_to_the_mac_and_a_show_to_every_list_that_names_it() {
        let (h, mac, nl) = two_devices();
        let project = ProjectId::derive("scratch");
        let set = |hidden| block_on(set_hidden(&h.devices, &h.view, project, hidden));

        // A fleet whose maxd keeps no lists can't hide.
        assert!(set(true).is_err());
        assert!(sent(&h).is_empty());

        h.report(
            nl.id,
            upsert(vec![device("nl", "linux"), list("nl", &[project])]),
        );
        h.report(
            mac.id,
            upsert(vec![device("mac", "macos"), list("mac", &[])]),
        );
        assert_eq!(set(true).unwrap(), ActionOutcome::Done);
        assert_eq!(sent(&h), [(DeviceId::derive("mac"), project, true)]);

        h.report(mac.id, upsert(vec![list("mac", &[project])]));
        assert_eq!(set(false).unwrap(), ActionOutcome::Done);
        let mut shown = sent(&h);
        shown.sort();
        let mut each = vec![
            (DeviceId::derive("mac"), project, false),
            (DeviceId::derive("nl"), project, false),
        ];
        each.sort();
        assert_eq!(shown, each);
    }

    #[test]
    fn a_hide_goes_to_a_connected_device_when_the_mac_is_down() {
        let (h, mac, nl) = two_devices();
        let project = ProjectId::derive("scratch");
        h.report(nl.id, upsert(vec![device("nl", "linux"), list("nl", &[])]));
        h.report(
            mac.id,
            upsert(vec![device("mac", "macos"), list("mac", &[])]),
        );
        disconnect(&h, &mac, "mac");
        let hidden = block_on(set_hidden(&h.devices, &h.view, project, true));
        assert_eq!(hidden.unwrap(), ActionOutcome::Done);
        assert_eq!(sent(&h), [(DeviceId::derive("nl"), project, true)]);
    }
}

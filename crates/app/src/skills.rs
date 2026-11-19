// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Skill trees between devices. Only the device that holds the library
//! keeps its trees, so before a project's skills go into checkouts on
//! another device, that device says which trees of the project's lock and
//! the holder's always set it lacks. Each one comes from the holder's store
//! into a copy here, the way a file between agents does, and goes on into
//! the device's store. The device then keeps the holder's always set, so
//! its agents get the same always skills as the holder's.
//!
//! The library moves the same way: the trees of every source go from the
//! old holder's store into the new one's, the new holder takes the source
//! list, and only then do the settings say which device holds it.

use std::sync::Arc;

use maxhq_protocol::ids::{DeviceId, IdempotencyKey, ProjectId};
use maxhq_protocol::link::Route;
use maxhq_protocol::message::{ActionOutcome, UploadTarget};
use maxhq_protocol::model::DeviceSettings;
use maxhq_protocol::skills::{LockedSkill, SkillLibrary, SkillLock, SkillSource, TreeHash};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::ClientMessage;

use crate::devices::{device_connected, Connector, Devices, OnProgress};
use crate::fleet::{check_can, device_name, Needs};
use crate::merge::Merge;
use crate::request::{lookup_failed, CommandError};
use crate::view::View;

/// Brings `device` the trees the project's lock there and the holder's
/// always set name, and the always set itself, from the device that holds
/// the library. The holder has them already, and with no library in view
/// there's nothing to bring, so both leave it to the apply.
pub async fn bring_trees<C: Connector>(
    devices: &Devices<C>,
    view: &View,
    device: DeviceId,
    project: ProjectId,
) -> Result<(), CommandError> {
    let Some((library, lock)) = view.read(|merge| wanted(merge, device, project)) else {
        return Ok(());
    };
    let needed = ClientMessage::SkillsTreesNeeded {
        key: IdempotencyKey::generate(),
        lock,
    };
    let trees = match devices.act_on(device, needed).await? {
        ActionOutcome::SkillTrees { trees } => trees,
        other => return Err(lookup_failed(other)),
    };
    if !trees.is_empty() {
        devices.check_copies(library.device)?;
        devices.check_copies(device)?;
    }
    let quiet: OnProgress = Arc::new(|_| {});
    for tree in trees {
        send_tree(devices, library.device, device, tree, Arc::clone(&quiet)).await?;
    }
    let always = ClientMessage::SkillsSetAlways {
        key: IdempotencyKey::generate(),
        skills: library.always_skills,
    };
    match devices.act_on(device, always).await? {
        ActionOutcome::Done => Ok(()),
        other => Err(lookup_failed(other)),
    }
}

/// Copies one tree from `from`'s store into a copy here, and on into
/// `to`'s store. Both devices must take copies.
pub async fn send_tree<C: Connector>(
    devices: &Devices<C>,
    from: DeviceId,
    to: DeviceId,
    tree: TreeHash,
    on_progress: OnProgress,
) -> Result<(), CommandError> {
    let copied = devices
        .copy_tree(from, tree.clone(), Arc::clone(&on_progress))
        .await?;
    let into = UploadTarget::SkillStore { tree };
    let path = copied.path().to_owned();
    match devices.upload(to, into, path, on_progress).await? {
        ActionOutcome::Done => Ok(()),
        other => Err(lookup_failed(other)),
    }
}

/// Makes `to` the device that holds the skill library, and every other
/// device not. When another device holds it, `to` first gets the trees of
/// its sources and takes its source list, so their IDs stay the same. The
/// old holder stops publishing once `to` does. A move is refused while
/// the old holder is away or has commits it hasn't pushed.
pub async fn move_library<C: Connector>(
    devices: &Devices<C>,
    view: &View,
    to: DeviceId,
) -> Result<ActionOutcome, CommandError> {
    let moving = match view.read(|merge| moving(merge, to))? {
        Ok(moving) => moving,
        Err(reason) => return Ok(ActionOutcome::Refused { reason }),
    };
    if let Some((from, library)) = moving.from {
        let lock = SkillLock {
            skills: library.sources.iter().flat_map(trees_of).collect(),
            committed: false,
            pack: None,
        };
        let needed = ClientMessage::SkillsTreesNeeded {
            key: IdempotencyKey::generate(),
            lock,
        };
        let trees = match devices.act_on(to, needed).await? {
            ActionOutcome::SkillTrees { trees } => trees,
            other => return Err(lookup_failed(other)),
        };
        if !trees.is_empty() {
            devices.check_copies(library.device)?;
            devices.check_copies(to)?;
        }
        let quiet: OnProgress = Arc::new(|_| {});
        for tree in trees {
            send_tree(devices, library.device, to, tree, Arc::clone(&quiet)).await?;
        }
        let take = ClientMessage::SkillsTakeLibrary {
            key: IdempotencyKey::generate(),
            from,
            sources: library.sources,
        };
        match devices.act_on(to, take).await? {
            ActionOutcome::Done => {}
            other => return Ok(other),
        }
    }
    for settings in std::iter::once(moving.on).chain(moving.off) {
        match devices.save_settings(settings).await? {
            ActionOutcome::Done => {}
            other => return Ok(other),
        }
    }
    Ok(ActionOutcome::Done)
}

/// What a move of the library to a device does.
struct Moving {
    /// The device that holds the library now, by name, and the library.
    from: Option<(String, SkillLibrary)>,
    /// The new holder's settings, saying it holds it.
    on: DeviceSettings,
    /// Every other holder's settings, saying it doesn't.
    off: Vec<DeviceSettings>,
}

/// Works out a move of the library to `to`, or why it can't move.
fn moving(merge: &Merge, to: DeviceId) -> Result<Result<Moving, String>, CommandError> {
    let store = merge.store();
    let name = |device| device_name(merge, device);
    let settings = |device| match store.get(&EntityKey::DeviceSettings(device)) {
        Some(Entity::DeviceSettings(settings)) => Some(settings.clone()),
        _ => None,
    };
    let Some(mut on) = settings(to) else {
        return Ok(Err(format!(
            "{} hasn't sent its settings yet. Try again in a moment.",
            name(to)
        )));
    };
    on.skill_library = Some(true);
    let held = held_library(merge);
    let others = store
        .entities()
        .any(|entity| matches!(entity, Entity::SkillLibrary(library) if library.device != to));
    if held.is_none() && others {
        return Ok(Err(
            "It isn't clear which device holds the skill library, so it can't move yet.".to_owned(),
        ));
    }
    let library = held.filter(|library| library.device != to);
    let mut off = Vec::new();
    for entity in store.entities() {
        let Entity::DeviceSettings(held) = entity else {
            continue;
        };
        let publishes = library.as_ref().is_some_and(|l| l.device == held.device);
        if held.device == to || !(publishes || holds(merge, held)) {
            continue;
        }
        if !device_connected(merge, held.device) {
            let away = name(held.device);
            return Ok(Err(format!(
                "{away} holds the skill library and isn't connected, so it can't move. \
                Connect to {away} first."
            )));
        }
        off.push(DeviceSettings {
            skill_library: Some(false),
            ..held.clone()
        });
    }
    let from = match library {
        Some(library) => {
            check_can(merge, library.device, Needs::LibraryMoves)?;
            check_can(merge, to, Needs::LibraryMoves)?;
            if library.sources.iter().any(|source| source.unpushed) {
                return Ok(Err(format!(
                    "{} has approved skill changes it hasn't pushed. \
                    Push the library's approvals first.",
                    name(library.device)
                )));
            }
            Some((name(library.device), library))
        }
        None => None,
    };
    Ok(Ok(Moving { from, on, off }))
}

/// Whether a device's settings say it holds the library, where none set
/// means a Mac does.
fn holds(merge: &Merge, settings: &DeviceSettings) -> bool {
    settings.skill_library.unwrap_or_else(|| {
        matches!(
            merge.store().get(&EntityKey::Device(settings.device)),
            Some(Entity::Device(device)) if device.os == "macos"
        )
    })
}

/// The library the device that holds it publishes. Any device can publish
/// one, so a library counts only by what this Mac's own settings say: its
/// own library while it holds it, and once you've moved it off, the one
/// device left publishing. Where that's not one device, as on a phone that
/// sees two, there's no telling which is real, so there's none.
pub(crate) fn held_library(merge: &Merge) -> Option<SkillLibrary> {
    let store = merge.store();
    let library_of = |device| match store.get(&EntityKey::SkillLibrary(device)) {
        Some(Entity::SkillLibrary(library)) => Some(library.clone()),
        _ => None,
    };
    let here = store.entities().find_map(|entity| match entity {
        Entity::DeviceLink(link) if link.route == Route::Local => link.device,
        _ => None,
    });
    if let Some(here) = here {
        match store.get(&EntityKey::DeviceSettings(here)) {
            Some(Entity::DeviceSettings(settings)) if !holds(merge, settings) => {}
            // It holds the library, or hasn't said yet.
            _ => return library_of(here),
        }
    }
    let mut libraries = store.entities().filter_map(|entity| match entity {
        Entity::SkillLibrary(library) => Some(library),
        _ => None,
    });
    let library = libraries.next()?;
    libraries.next().is_none().then(|| library.clone())
}

/// A lock entry for each of a source's skill trees and its pack's, only
/// so the new holder can say which it lacks.
fn trees_of(source: &SkillSource) -> Vec<LockedSkill> {
    let skills = source
        .skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.tree.clone()));
    skills
        .chain(source.pack.clone().map(|tree| (source.name.clone(), tree)))
        .map(|(name, tree)| LockedSkill {
            name,
            source: source.id,
            path: String::new(),
            commit: source.commit.clone(),
            tree,
            pinned: false,
        })
        .collect()
}

/// The library another device holds, and a lock of what `device` needs
/// from it: the project's locked skills there and the always set.
fn wanted(
    merge: &Merge,
    device: DeviceId,
    project: ProjectId,
) -> Option<(SkillLibrary, SkillLock)> {
    let store = merge.store();
    let library = held_library(merge)?;
    if library.device == device {
        return None;
    }
    let mut lock = match store.get(&EntityKey::ProjectSkills { project, device }) {
        Some(Entity::ProjectSkills(skills)) => skills.lock.clone(),
        _ => SkillLock::default(),
    };
    lock.skills.extend(library.always_skills.iter().cloned());
    Some((library, lock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_client::LinkEvent;
    use maxhq_protocol::ids::SkillSourceId;
    use maxhq_protocol::model::DeviceSettingChange;
    use maxhq_protocol::skills::{LockedSkill, ProjectSkills, SkillPack, TreeHash};
    use maxhq_protocol::state::Change;

    use maxhq_client::LinkSpec;

    use crate::devices::tests::{two_devices, Call, Harness};
    use crate::tests::block_on;

    fn skill(name: &str, tree: &str) -> LockedSkill {
        LockedSkill {
            name: name.into(),
            source: SkillSourceId::derive("git:owner/repo"),
            path: format!("skills/{name}"),
            commit: None,
            tree: TreeHash(tree.into()),
            pinned: false,
        }
    }

    fn library(device: DeviceId, always_skills: Vec<LockedSkill>) -> LinkEvent {
        LinkEvent::Changed(vec![Change::Upsert {
            value: Entity::SkillLibrary(SkillLibrary {
                device,
                sources: Vec::new(),
                always: vec!["unslop".into()],
                always_skills,
            }),
        }])
    }

    #[test]
    fn a_device_gets_the_trees_it_lacks_from_the_holder_and_its_always_set() {
        let (h, mac, nl) = two_devices();
        let (holder, device) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let project = ProjectId::derive("comet");
        let unslop = skill("unslop", "a1");
        h.report(mac.id, library(holder, vec![unslop.clone()]));
        let lock = SkillLock {
            skills: vec![skill("review", "b2")],
            committed: false,
            pack: Some(SkillPack {
                source: SkillSourceId::derive("git:owner/devflow"),
                commit: "8be68f9".into(),
                tree: TreeHash("c3".into()),
            }),
        };
        h.report(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: Entity::ProjectSkills(ProjectSkills {
                    project,
                    device,
                    lock,
                    checkouts: Vec::new(),
                }),
            }]),
        );
        let missing = TreeHash("b2".into());
        h.fake.record().acts.push_back(ActionOutcome::SkillTrees {
            trees: vec![missing.clone()],
        });

        block_on(bring_trees(&h.devices, &h.view, device, project)).unwrap();
        let calls = h.fake.calls();
        let Some((_, Call::Send(ClientMessage::SkillsTreesNeeded { lock, .. }))) = calls.first()
        else {
            panic!("expected the device asked first, got {calls:?}");
        };
        let names: Vec<&str> = lock.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["review", "unslop"]);
        // The pack's tree goes the same way as the skills'.
        assert_eq!(lock.pack.as_ref().map(|pack| &pack.tree.0[..]), Some("c3"));
        assert_eq!(calls[1], (mac.id, Call::CopyTree(missing.clone())));
        let (link, Call::Upload(target, path)) = &calls[2] else {
            panic!("expected an upload, got {:?}", calls[2]);
        };
        assert_eq!(*link, nl.id);
        assert_eq!(*target, UploadTarget::SkillStore { tree: missing });
        assert!(path.ends_with("b2"), "{path:?}");
        assert!(matches!(
            &calls[3],
            (link, Call::Send(ClientMessage::SkillsSetAlways { skills, .. }))
                if *link == nl.id && *skills == [unslop.clone()]
        ));
        assert_eq!(calls.len(), 4, "{calls:?}");
    }

    fn device(id: &str, os: &str) -> Entity {
        Entity::Device(maxhq_protocol::model::Device {
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

    fn settings(id: &str, skill_library: Option<bool>) -> DeviceSettings {
        DeviceSettings {
            device: DeviceId::derive(id),
            default_permission: maxhq_protocol::model::PermissionMode::Ask,
            workspace_network: None,
            workspace_pool: None,
            auto_settle_days: 5,
            snooze_at_limit: false,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            placement: Default::default(),
            tool_updates: Vec::new(),
            cursor_usage: false,
            keep_awake: Default::default(),
            test_hosts: Default::default(),
            skill_library,
            network: Default::default(),
            idle_release_minutes: 60,
            memory_items: 256,
            history_mib: 256,
            claude_setup_token: false,
            shell_integration: true,
        }
    }

    fn source(name: &str, tree: &str, unpushed: bool) -> SkillSource {
        let id = SkillSourceId::derive(&format!("git:owner/{name}"));
        SkillSource {
            id,
            name: name.into(),
            kind: maxhq_protocol::skills::SourceKind::Git {
                url: format!("owner/{name}"),
            },
            default_ref: None,
            trust: maxhq_protocol::skills::SourceTrust::Own,
            commit: Some("8be68f9".into()),
            fetched_at: None,
            problem: None,
            skills: vec![maxhq_protocol::skills::SkillInfo {
                name: "review".into(),
                description: "Reviews.".into(),
                tree: TreeHash(tree.into()),
                path: "skills/review".into(),
                source: id,
                flags: Default::default(),
                listing_chars: 10,
            }],
            pack: None,
            unpushed,
        }
    }

    /// A Mac that holds the library with `sources`, and a Linux box that
    /// doesn't.
    fn fleet(sources: Vec<SkillSource>) -> (Harness, LinkSpec, LinkSpec) {
        let (h, mac, nl) = two_devices();
        let mut on_mac = vec![device("mac", "macos"), settings_entity("mac", None)];
        on_mac.push(Entity::SkillLibrary(SkillLibrary {
            device: DeviceId::derive("mac"),
            sources,
            always: vec!["unslop".into()],
            always_skills: Vec::new(),
        }));
        let changes = |entities: Vec<Entity>| {
            LinkEvent::Changed(
                entities
                    .into_iter()
                    .map(|value| Change::Upsert { value })
                    .collect(),
            )
        };
        h.report(mac.id, changes(on_mac));
        let on_nl = vec![device("nl", "linux"), settings_entity("nl", None)];
        h.report(nl.id, changes(on_nl));
        (h, mac, nl)
    }

    fn settings_entity(id: &str, skill_library: Option<bool>) -> Entity {
        Entity::DeviceSettings(settings(id, skill_library))
    }

    fn holder(h: &Harness) -> Option<DeviceId> {
        h.view
            .read(|merge| Ok::<_, CommandError>(held_library(merge).map(|library| library.device)))
            .unwrap()
    }

    #[test]
    fn the_library_s_holder_comes_from_this_mac_s_settings() {
        let (h, mac, nl) = two_devices();
        let (mac_id, nl_id) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let upsert = |value| LinkEvent::Changed(vec![Change::Upsert { value }]);
        h.report(mac.id, upsert(device("mac", "macos")));
        h.report(mac.id, upsert(settings_entity("mac", None)));
        h.report(nl.id, upsert(device("nl", "linux")));
        h.report(nl.id, upsert(settings_entity("nl", None)));
        h.report(nl.id, library(nl_id, vec![skill("planted", "e1")]));
        let wanted_here = || {
            h.view
                .read(|merge| Ok::<_, CommandError>(wanted(merge, mac_id, ProjectId::derive("p"))))
                .unwrap()
        };
        let move_to = |to| h.view.read(|merge| moving(merge, to)).unwrap();

        // This Mac holds it and hasn't published it yet, so devbox's isn't
        // what a device fetches or what a move copies.
        assert_eq!(holder(&h), None);
        assert_eq!(wanted_here(), None);
        assert!(move_to(mac_id).is_err(), "not clear yet");

        // Once it publishes, its own counts, whichever sorts first.
        h.report(mac.id, library(mac_id, Vec::new()));
        assert_eq!(holder(&h), Some(mac_id));
        assert_eq!(wanted_here(), None, "it's already here");
        let here = move_to(mac_id).unwrap();
        assert!(here.from.is_none(), "devbox's library isn't taken");

        // You moved it off this Mac, which still shows an old one. Two say
        // they hold it, so neither counts.
        h.report(mac.id, upsert(settings_entity("mac", Some(false))));
        assert_eq!(holder(&h), None);
        assert!(move_to(nl_id).is_err(), "not clear yet");

        // With this Mac's gone, only devbox says it holds it.
        let gone = Change::Remove {
            key: EntityKey::SkillLibrary(mac_id),
        };
        h.report(mac.id, LinkEvent::Changed(vec![gone]));
        assert_eq!(holder(&h), Some(nl_id));
    }

    #[test]
    fn the_library_moves_with_its_trees_and_then_the_settings() {
        let review = source("skills", "a1", false);
        let (h, mac, nl) = fleet(vec![review.clone()]);
        let missing = TreeHash("a1".into());
        h.fake.record().acts.push_back(ActionOutcome::SkillTrees {
            trees: vec![missing.clone()],
        });

        let moved = block_on(move_library(&h.devices, &h.view, DeviceId::derive("nl")));
        assert_eq!(moved.unwrap(), ActionOutcome::Done);
        let calls = h.fake.calls();
        assert!(matches!(
            &calls[0],
            (link, Call::Send(ClientMessage::SkillsTreesNeeded { lock, .. }))
                if *link == nl.id && lock.skills[0].tree == missing
        ));
        assert_eq!(calls[1], (mac.id, Call::CopyTree(missing.clone())));
        assert!(matches!(&calls[2], (link, Call::Upload(..)) if *link == nl.id));
        let (link, Call::Send(ClientMessage::SkillsTakeLibrary { from, sources, .. })) = &calls[3]
        else {
            panic!("expected the take, got {:?}", calls[3]);
        };
        assert_eq!((*link, from.as_str()), (nl.id, "mac"));
        assert_eq!(*sources, [review]);
        let set = |at: usize| match &calls[at] {
            (link, Call::Send(ClientMessage::UpdateDeviceSettings { changes, .. })) => {
                assert_eq!(changes.len(), 1, "only the holder changes: {changes:?}");
                let DeviceSettingChange::SkillLibrary { value } = changes[0] else {
                    panic!("expected the holder, got {changes:?}");
                };
                (*link, value)
            }
            other => panic!("expected a setting, got {other:?}"),
        };
        assert_eq!(set(4), (nl.id, Some(true)), "the new holder first");
        assert_eq!(set(5), (mac.id, Some(false)));
        assert_eq!(calls.len(), 6, "{calls:?}");
    }

    #[test]
    fn a_move_waits_for_pushes_and_for_the_old_holder() {
        let (h, mac, _) = fleet(vec![source("skills", "a1", true)]);
        let to = DeviceId::derive("nl");
        let refused = |h: &Harness| match block_on(move_library(&h.devices, &h.view, to)).unwrap() {
            ActionOutcome::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(
            refused(&h),
            "mac has approved skill changes it hasn't pushed. Push the library's approvals first."
        );
        crate::devices::tests::disconnect(&h, &mac, "mac");
        assert_eq!(
            refused(&h),
            "mac holds the skill library and isn't connected, so it can't move. Connect to mac first."
        );
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn with_no_library_yet_the_switch_only_changes_the_settings() {
        let (h, mac, nl) = two_devices();
        let changes = |entities: Vec<Entity>| {
            LinkEvent::Changed(
                entities
                    .into_iter()
                    .map(|value| Change::Upsert { value })
                    .collect(),
            )
        };
        h.report(
            mac.id,
            changes(vec![
                device("mac", "macos"),
                settings_entity("mac", Some(false)),
            ]),
        );
        h.report(
            nl.id,
            changes(vec![device("nl", "linux"), settings_entity("nl", None)]),
        );
        let moved = block_on(move_library(&h.devices, &h.view, DeviceId::derive("nl")));
        assert_eq!(moved.unwrap(), ActionOutcome::Done);
        let calls = h.fake.calls();
        assert!(matches!(
            &calls[..],
            [(link, Call::Send(ClientMessage::UpdateDeviceSettings { changes, .. }))]
                if *link == nl.id
                    && *changes == [DeviceSettingChange::SkillLibrary { value: Some(true) }]
        ));
    }

    #[test]
    fn the_holder_and_a_fleet_without_a_library_bring_nothing() {
        let (h, mac, _) = two_devices();
        let project = ProjectId::derive("comet");
        block_on(bring_trees(
            &h.devices,
            &h.view,
            DeviceId::derive("nl"),
            project,
        ))
        .unwrap();
        let holder = DeviceId::derive("mac");
        h.report(mac.id, library(holder, Vec::new()));
        block_on(bring_trees(&h.devices, &h.view, holder, project)).unwrap();
        assert!(h.fake.calls().is_empty());
    }
}

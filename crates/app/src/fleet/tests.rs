// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the cases
// follow the tests of the update-everything logic.

use std::sync::Mutex;

use maxhq_protocol::model::Tool;
use maxhq_protocol::state::{HelperTool, ToolKind, ToolOwner, ToolPlace};

use super::*;

const TARGET: &str = "x86_64-unknown-linux-musl";
const NEW: &str = "aaaaaaaaaaaaaaaa";

/// Binaries for one target, by kind.
struct Fixed(Vec<(UpdateKind, &'static str, Binary)>);

impl Binaries for Fixed {
    fn find(&self, kind: UpdateKind, target: &str) -> Option<Binary> {
        self.0
            .iter()
            .find(|(k, t, _)| *k == kind && *t == target)
            .map(|(_, _, binary)| binary.clone())
    }
}

/// A binary a release signed. The device checks the signature, so the app
/// only carries it, and these stand-ins needn't be real.
fn binary(name: &str) -> Binary {
    Binary {
        path: PathBuf::from(format!("/bin/{name}")),
        sha256: format!("{NEW}{}", "0".repeat(48)),
        release: Some(release()),
    }
}

fn release() -> SignedRelease {
    SignedRelease {
        manifest: "{\"version\":\"0.2.0\",\"binaries\":{}}".into(),
        signature: "c2lnbmF0dXJl".into(),
    }
}

/// A binary built from source, with no release beside it.
fn from_source(name: &str) -> Binary {
    Binary {
        release: None,
        ..binary(name)
    }
}

fn fleet_binaries() -> Fixed {
    Fixed(vec![
        (UpdateKind::Maxd, TARGET, binary("maxd")),
        (UpdateKind::Guest, TARGET, binary("guest")),
    ])
}

fn machine(name: &str, build: &str) -> Machine {
    Machine {
        device: DeviceId::derive(name),
        name: name.to_owned(),
        connected: true,
        winding_down: false,
        capabilities: Some(Capabilities {
            updates: true,
            tools: true,
            load: true,
            ..Capabilities::default()
        }),
        build: Some(build.to_owned()),
        version: None,
        target: Some(TARGET.to_owned()),
        local: false,
        workspaces: false,
        tools: Vec::new(),
    }
}

fn claude_on_host() -> ToolTarget {
    ToolTarget {
        tool: Tool::Claude,
        place: ToolPlace::Host,
    }
}

#[test]
fn updates_devices_behind_and_names_every_skipped_one() {
    let old = "bbbbbbbbbbbbbbbb";
    let machines = [
        Machine {
            local: true,
            ..machine("this-mac", old)
        },
        machine("devbox", old),
        machine("current", NEW),
        Machine {
            connected: false,
            ..machine("offline", old)
        },
        Machine {
            connected: false,
            ..machine("current-offline", NEW)
        },
        Machine {
            winding_down: true,
            ..machine("restarting", old)
        },
        Machine {
            capabilities: Some(Capabilities::default()),
            ..machine("old-protocol", old)
        },
        Machine {
            build: None,
            target: None,
            ..machine("before-builds", old)
        },
        Machine {
            target: Some("aarch64-unknown-linux-musl".into()),
            ..machine("arm", old)
        },
        Machine {
            workspaces: true,
            ..machine("buildbox", old)
        },
    ];
    let plan = plan(&machines, &fleet_binaries(), false, true, false);
    let targets: Vec<_> = plan
        .targets
        .iter()
        .map(|t| (t.name.as_str(), t.maxd.is_some(), t.guest.is_some()))
        .collect();
    assert_eq!(targets, [("devbox", true, false), ("buildbox", true, true)]);
    let skipped: Vec<_> = plan
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [
            (
                "this-mac",
                "runs from your source tree, so restart it with Start maxd"
            ),
            ("offline", "not connected"),
            ("restarting", "winding down"),
            (
                "old-protocol",
                "its maxd is too old to update itself, so set it up again"
            ),
            (
                "before-builds",
                "its maxd is too old to update itself, so set it up again"
            ),
            ("arm", "this app has no maxd for aarch64-unknown-linux-musl"),
        ]
    );
}

#[test]
fn a_current_device_still_gets_its_tools_when_asked() {
    let machines = [Machine {
        tools: vec![claude_on_host()],
        ..machine("devbox", NEW)
    }];
    assert!(plan(&machines, &fleet_binaries(), false, false, false)
        .targets
        .is_empty());
    let with_tools = plan(&machines, &fleet_binaries(), true, false, false);
    assert_eq!(with_tools.targets.len(), 1);
    assert_eq!(with_tools.targets[0].tools, [claude_on_host()]);
    assert_eq!(with_tools.targets[0].maxd, None);
}

#[test]
fn a_newer_maxd_is_never_taken_back_to_the_apps_build() {
    let old = "bbbbbbbbbbbbbbbb";
    let at = |name: &str, version: &str| Machine {
        version: Some(version.to_owned()),
        ..machine(name, old)
    };
    let machines = [
        at("newer", "99.0.0"),
        at("same", env!("CARGO_PKG_VERSION")),
        at("older", "0.0.1"),
        at("no-version", ""),
        Machine {
            tools: vec![claude_on_host()],
            ..at("newer-with-tools", "99.0.0")
        },
    ];
    let plan = plan(&machines, &fleet_binaries(), true, false, false);
    let targets: Vec<_> = plan
        .targets
        .iter()
        .map(|t| (t.name.as_str(), t.maxd.is_some()))
        .collect();
    assert_eq!(
        targets,
        [
            ("same", true),
            ("older", true),
            ("no-version", true),
            ("newer-with-tools", false),
        ]
    );
    assert!(plan.skipped.is_empty(), "{plan:?}");
    assert!(newer_than_app(Some("99.0.0")));
    assert!(!newer_than_app(Some(env!("CARGO_PKG_VERSION"))));
    assert!(!newer_than_app(Some("0.0.1")));
    assert!(!newer_than_app(Some("not a version")));
    assert!(!newer_than_app(None));
}

#[test]
fn a_bundled_app_leaves_its_own_macs_maxd_to_move_with_it() {
    let machines = [Machine {
        local: true,
        ..machine("this-mac", "bbbbbbbbbbbbbbbb")
    }];
    assert_eq!(
        plan(&machines, &fleet_binaries(), false, false, false),
        Plan::default()
    );
}

#[test]
fn this_mac_s_clis_update_while_its_maxd_moves_with_the_app() {
    for dev_tree in [false, true] {
        let machines = [Machine {
            local: true,
            tools: vec![claude_on_host()],
            ..machine("this-mac", "bbbbbbbbbbbbbbbb")
        }];
        let plan = plan(&machines, &fleet_binaries(), true, dev_tree, false);
        assert!(plan.skipped.is_empty(), "{plan:?}");
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].tools, [claude_on_host()]);
        assert_eq!(plan.targets[0].maxd, None);
        assert_eq!(plan.targets[0].guest, None);
    }
}

fn info(update: ToolUpdate, version: &str, latest: &str, state: ToolState) -> ToolInfo {
    ToolInfo {
        kind: ToolKind::Agent { tool: Tool::Claude },
        place: ToolPlace::Host,
        path: Some("/home/dev/.local/bin/claude".into()),
        version: Some(version.into()),
        latest: Some(latest.into()),
        owner: ToolOwner::User,
        method: "native".into(),
        update,
        state,
    }
}

#[test]
fn only_allowed_tools_that_are_behind_update() {
    let tools = [
        info(ToolUpdate::Allowed, "1.0.0", "1.1.0", ToolState::Idle),
        info(ToolUpdate::Auto, "1.0.0", "1.0.0", ToolState::Idle),
        info(ToolUpdate::NeedsTick, "1.0.0", "1.1.0", ToolState::Idle),
        info(ToolUpdate::Never, "1.0.0", "1.1.0", ToolState::Idle),
        info(
            ToolUpdate::Manual {
                command: "brew upgrade claude".into(),
            },
            "1.0.0",
            "1.1.0",
            ToolState::Idle,
        ),
        info(
            ToolUpdate::Auto,
            "1.0.0",
            "1.1.0",
            ToolState::running("Updating".into()),
        ),
        // A helper isn't an agent CLI, so this run leaves it alone.
        ToolInfo {
            kind: ToolKind::Helper {
                tool: HelperTool::Node,
            },
            ..info(ToolUpdate::Auto, "22.0.0", "22.1.0", ToolState::Idle)
        },
    ];
    assert_eq!(tools_to_update(&tools, &[]), [claude_on_host()]);
}

#[test]
fn a_ticked_cli_waiting_for_its_tick_updates() {
    let tools = [info(
        ToolUpdate::NeedsTick,
        "1.0.0",
        "1.1.0",
        ToolState::Idle,
    )];
    assert_eq!(tools_to_update(&tools, &[Tool::Claude]), [claude_on_host()]);
    assert!(tools_to_update(&tools, &[Tool::Codex]).is_empty());
}

/// Devices that answer as told, and remember what they were asked.
#[derive(Default)]
struct FakeFleet {
    calls: Mutex<Vec<String>>,
    refuse_tools: bool,
    refuse_install: bool,
    never_back: bool,
}

impl FakeFleet {
    fn call(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

impl FleetOps for FakeFleet {
    async fn update_tools(
        &self,
        _device: DeviceId,
        tools: Vec<ToolTarget>,
    ) -> Result<ActionOutcome, CommandError> {
        self.call(format!("tools {}", tools.len()));
        Ok(if self.refuse_tools {
            ActionOutcome::Refused {
                reason: "npm failed".into(),
            }
        } else {
            ActionOutcome::Started {
                agent: maxhq_protocol::ids::AgentId::generate(),
            }
        })
    }

    async fn upload(&self, _device: DeviceId, path: PathBuf) -> Result<AttachmentId, CommandError> {
        self.call(format!("upload {}", path.display()));
        Ok(AttachmentId::generate())
    }

    async fn install(
        &self,
        _device: DeviceId,
        _staged: AttachmentId,
        kind: UpdateKind,
        _sha256: String,
        release: Option<SignedRelease>,
        unsigned: bool,
    ) -> Result<ActionOutcome, CommandError> {
        let how = match (release.is_some(), unsigned) {
            (true, false) => "signed",
            (false, true) => "unsigned",
            (false, false) => "bare",
            (true, true) => "signed and marked",
        };
        self.call(format!("install {kind:?} {how}"));
        Ok(if self.refuse_install {
            ActionOutcome::Refused {
                reason: "The binary doesn't match its sum".into(),
            }
        } else {
            ActionOutcome::Done
        })
    }

    async fn wait_for_build(&self, _device: DeviceId, build: String, _within: Duration) -> bool {
        self.call(format!("wait {build}"));
        !self.never_back
    }
}

async fn statuses(plan: Plan, fleet: &FakeFleet) -> Vec<FleetStatus> {
    let seen = Mutex::new(Vec::new());
    run(plan, fleet, &|progress| {
        seen.lock().unwrap().push(progress.status)
    })
    .await;
    seen.into_inner().unwrap()
}

fn full_target() -> Plan {
    Plan {
        targets: vec![Target {
            device: DeviceId::derive("devbox"),
            name: "devbox".into(),
            tools: vec![claude_on_host()],
            maxd: Some(binary("maxd")),
            guest: Some(binary("guest")),
            unsigned: false,
        }],
        skipped: vec![Skipped {
            device: DeviceId::derive("offline"),
            name: "offline".into(),
            reason: "not connected".into(),
        }],
    }
}

#[tokio::test]
async fn tools_go_first_then_maxd_then_the_guest() {
    let fleet = FakeFleet::default();
    assert_eq!(
        statuses(full_target(), &fleet).await,
        [
            FleetStatus::Skipped {
                reason: "not connected".into()
            },
            FleetStatus::Waiting,
            FleetStatus::Tools,
            FleetStatus::Uploading,
            FleetStatus::Restarting,
            FleetStatus::Guests,
            FleetStatus::Done,
        ]
    );
    assert_eq!(
        *fleet.calls.lock().unwrap(),
        [
            "tools 1",
            "upload /bin/maxd",
            "install Maxd signed",
            &format!("wait {NEW}"),
            "upload /bin/guest",
            "install Guest signed",
        ]
    );
}

#[test]
fn a_build_from_source_goes_out_only_once_you_agree() {
    let machines = [
        Machine {
            workspaces: true,
            ..machine("devbox", "bbbbbbbbbbbbbbbb")
        },
        Machine {
            tools: vec![claude_on_host()],
            ..machine("current", NEW)
        },
    ];
    let source = Fixed(vec![
        (UpdateKind::Maxd, TARGET, from_source("maxd")),
        (UpdateKind::Guest, TARGET, from_source("guest")),
    ]);
    // Without your say the device that needs the build is left out, and
    // one that only needs its CLIs still gets them.
    let refused = plan(&machines, &source, true, false, false);
    assert_eq!(
        refused
            .skipped
            .iter()
            .map(|s| (s.name.as_str(), s.reason.as_str()))
            .collect::<Vec<_>>(),
        [("devbox", UNSIGNED_SKIP)]
    );
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(refused.targets[0].name, "current");
    assert!(!refused.targets[0].unsigned);

    let agreed = plan(&machines, &source, true, false, true);
    assert!(agreed.skipped.is_empty(), "{agreed:?}");
    assert_eq!(agreed.targets[0].maxd, Some(from_source("maxd")));
    assert!(agreed.targets[0].unsigned);

    // A guest without a release holds the device back too.
    let mixed = Fixed(vec![
        (UpdateKind::Maxd, TARGET, binary("maxd")),
        (UpdateKind::Guest, TARGET, from_source("guest")),
    ]);
    assert_eq!(
        plan(&machines[..1], &mixed, false, false, false)
            .skipped
            .len(),
        1
    );
}

#[tokio::test]
async fn only_an_upload_no_release_signed_goes_out_marked() {
    let mut target = full_target();
    target.targets[0].unsigned = true;
    target.targets[0].guest = Some(from_source("guest"));
    let fleet = FakeFleet::default();
    statuses(target, &fleet).await;
    let calls = fleet.calls.lock().unwrap().clone();
    assert!(
        calls.contains(&"install Maxd signed".to_owned()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"install Guest unsigned".to_owned()),
        "{calls:?}"
    );

    // Without your say nothing goes out marked, even if a plan held one.
    let mut target = full_target();
    target.targets[0].maxd = Some(from_source("maxd"));
    let fleet = FakeFleet::default();
    statuses(target, &fleet).await;
    assert!(fleet
        .calls
        .lock()
        .unwrap()
        .contains(&"install Maxd bare".to_owned()));
}

#[tokio::test]
async fn a_failed_tool_update_still_updates_maxd_but_ends_failed() {
    let fleet = FakeFleet {
        refuse_tools: true,
        ..FakeFleet::default()
    };
    let seen = statuses(full_target(), &fleet).await;
    assert!(seen.contains(&FleetStatus::Guests));
    assert_eq!(
        seen.last(),
        Some(&FleetStatus::Failed {
            reason: "The agent CLIs didn't update: npm failed".into()
        })
    );
}

#[tokio::test]
async fn a_refused_install_stops_that_device() {
    let fleet = FakeFleet {
        refuse_install: true,
        ..FakeFleet::default()
    };
    let seen = statuses(full_target(), &fleet).await;
    assert!(!seen.contains(&FleetStatus::Restarting));
    assert_eq!(
        seen.last(),
        Some(&FleetStatus::Failed {
            reason: "The binary doesn't match its sum".into()
        })
    );
}

#[tokio::test]
async fn a_device_that_never_comes_back_fails_without_the_guest() {
    let fleet = FakeFleet {
        never_back: true,
        ..FakeFleet::default()
    };
    let seen = statuses(full_target(), &fleet).await;
    assert!(!seen.contains(&FleetStatus::Guests));
    assert_eq!(
        seen.last(),
        Some(&FleetStatus::Failed {
            reason: "maxd didn't come back on the new build within 90 seconds".into()
        })
    );
}

#[cfg(feature = "desktop")]
#[test]
fn binaries_come_from_the_first_folder_and_match_their_sums() {
    let first = tempdir();
    let second = tempdir();
    std::fs::write(second.join(format!("maxd-{TARGET}")), b"second").unwrap();
    let dirs = BinaryDirs::new(vec![first.clone(), second.clone()]);
    let found = dirs.find(UpdateKind::Maxd, TARGET).unwrap();
    assert_eq!(found.path, second.join(format!("maxd-{TARGET}")));
    assert_eq!(found.sha256, sha256_file(&found.path).unwrap());
    // A musl maxd is the guest too, when no guest file is there.
    assert_eq!(
        dirs.find(UpdateKind::Guest, TARGET).unwrap().path,
        found.path
    );
    assert!(dirs
        .find(UpdateKind::Guest, "x86_64-unknown-linux-gnu")
        .is_none());
    // A target can't lead out of the folder, to a file that is there.
    std::fs::create_dir(first.join("maxd-..")).unwrap();
    std::fs::write(first.join("outside"), b"outside").unwrap();
    assert!(dirs.find(UpdateKind::Maxd, "../../outside").is_none());

    std::fs::write(first.join(format!("maxd-{TARGET}")), b"first").unwrap();
    std::fs::write(
        first.join("SHA256SUMS"),
        format!("{}  maxd-{TARGET}\n", "0".repeat(64)),
    )
    .unwrap();
    // The first folder's file doesn't match its listing, so it's passed over.
    assert_eq!(
        dirs.find(UpdateKind::Maxd, TARGET).unwrap().path,
        second.join(format!("maxd-{TARGET}"))
    );
    let sum = sha256_file(&first.join(format!("maxd-{TARGET}"))).unwrap();
    std::fs::write(
        first.join("SHA256SUMS"),
        format!("{sum} *dist/maxd-{TARGET}\n"),
    )
    .unwrap();
    assert_eq!(
        dirs.find(UpdateKind::Maxd, TARGET).unwrap().path,
        first.join(format!("maxd-{TARGET}"))
    );
    // What the app carries lists each target once, guests and sums aside.
    std::fs::write(second.join(format!("maxd-guest-{TARGET}")), b"guest").unwrap();
    assert_eq!(
        dirs.bundled(),
        [BundledBuild {
            target: TARGET.to_owned(),
            build: sum[..16].to_owned(),
            signed: false,
        }]
    );
    // A release's manifest and signature go with the binaries beside them,
    // and only when both are there.
    assert_eq!(dirs.find(UpdateKind::Maxd, TARGET).unwrap().release, None);
    std::fs::write(first.join(RELEASE_MANIFEST), "manifest").unwrap();
    assert_eq!(dirs.find(UpdateKind::Maxd, TARGET).unwrap().release, None);
    std::fs::write(first.join(RELEASE_SIGNATURE), "signature").unwrap();
    assert_eq!(
        dirs.find(UpdateKind::Maxd, TARGET).unwrap().release,
        Some(SignedRelease {
            manifest: "manifest".into(),
            signature: "signature".into(),
        })
    );
    assert!(dirs.bundled()[0].signed);
    // The second folder's binaries don't borrow the first one's release.
    std::fs::remove_file(first.join(format!("maxd-{TARGET}"))).unwrap();
    assert_eq!(dirs.find(UpdateKind::Maxd, TARGET).unwrap().release, None);
    let _ = std::fs::remove_dir_all(first);
    let _ = std::fs::remove_dir_all(second);
}

#[test]
fn a_maxd_build_is_the_first_16_hex_characters() {
    assert_eq!(binary("maxd").build(), NEW);
}

/// A fresh folder under the system temp folder.
#[cfg(feature = "desktop")]
fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("maxhq-fleet-{}", IdempotencyKey::generate().0));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_device_can_t_report_another_device_s_build() {
    use maxhq_client::{LinkEvent, LinkSpec};
    use maxhq_protocol::ids::LinkId;
    use maxhq_protocol::link::{DeviceLink, LinkState, Route};
    use maxhq_protocol::model::Device;
    use maxhq_protocol::state::Entity;

    fn spec(name: &str) -> LinkSpec {
        LinkSpec {
            id: LinkId::derive(name),
            name: name.to_owned(),
            route: Route::Ssh {
                host: name.to_owned(),
            },
            fallbacks: Vec::new(),
            last_good: std::collections::BTreeMap::new(),
        }
    }
    fn hello(spec: &LinkSpec, device: &str) -> LinkEvent {
        LinkEvent::Link(Box::new(DeviceLink {
            id: spec.id,
            name: spec.name.clone(),
            device: Some(DeviceId::derive(device)),
            route: spec.route.clone(),
            routes: Vec::new(),
            state: LinkState::Unknown,
            network: None,
            peer: None,
            via: Vec::new(),
            capabilities: None,
            protocol: None,
            protocol_mismatch: None,
            waiting: Vec::new(),
            claim: None,
        }))
    }
    fn device(id: &str, build: &str) -> Entity {
        Entity::Device(Device {
            id: DeviceId::derive(id),
            name: id.to_owned(),
            os: "linux".to_owned(),
            boot_id: maxhq_protocol::ids::BootId("b".to_owned()),
            maxd_version: "1".to_owned(),
            winding_down_until: None,
            workspace_host: None,
            build: Some(build.to_owned()),
            target: None,
            wsl: None,
        })
    }

    let mut merge = Merge::new();
    let (victim, hostile) = (spec("victim"), spec("hostile"));
    for link in [&victim, &hostile] {
        merge.add_link(link);
    }
    merge.link_event(victim.id, hello(&victim, "victim"));
    merge.link_event(hostile.id, hello(&hostile, "hostile"));
    merge.link_event(victim.id, LinkEvent::Synced(vec![device("victim", "old")]));
    merge.link_event(hostile.id, LinkEvent::Synced(vec![device("victim", NEW)]));

    let machines = machines(&merge, None);
    let [machine] = machines.as_slice() else {
        panic!("{machines:?}");
    };
    assert_eq!(machine.device, DeviceId::derive("victim"));
    assert_eq!(machine.build.as_deref(), Some("old"));
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::os::unix::fs::PermissionsExt;

use maxhq_protocol::model::{DeviceSettings, Tool};

use super::*;
use crate::testing::TempDir;

/// A registry that knows one version of every package.
struct Fixed(&'static str);

impl Registry for Fixed {
    fn latest<'a>(&'a self, _package: &'a str) -> BoxFuture<'a, Option<String>> {
        let version = self.0.to_owned();
        async move { Some(version) }.boxed()
    }
}

/// A home with a fake native Claude at `~/.local/bin/claude`. Its update
/// moves it to 2.2.0.
fn home() -> (TempDir, OsString) {
    let dir = TempDir::new();
    let bin = dir.path().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let version = dir.path().join("version");
    fs::write(&version, "2.1.0\n").unwrap();
    let claude = bin.join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '%s (Claude Code)\\n' \"$(cat {v})\"; exit 0; fi\n\
             if [ \"$1\" = update ]; then echo 'Updating to 2.2.0'; echo 2.2.0 > {v}; fi\n",
            v = version.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    let search = bin.into_os_string();
    (dir, search)
}

fn tools(search: OsString) -> Arc<Tools> {
    let mut tools = Tools::new(
        crate::secrets::testing::hub(),
        None,
        Arc::new(Fixed("2.2.0")),
        Some(search),
    );
    // Never this machine's own installs.
    tools.home = None;
    tools.android = android::Sdk {
        on_mac: false,
        licences: None,
        javas: Vec::new(),
        ask_macos: false,
        timeout: Duration::from_secs(30),
    };
    Arc::new(tools)
}

fn published(tools: &Tools) -> DeviceTools {
    match tools
        .hub
        .get(&EntityKey::DeviceTools(tools.hub.device().id))
    {
        Some(Entity::DeviceTools(found)) => found,
        other => panic!("no tools entity: {other:?}"),
    }
}

fn tick(tools: &Tools, tool: Tool) {
    tools.hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(DeviceSettings {
            tool_updates: vec![tool],
            ..crate::hub::default_settings(tools.hub.device())
        }),
    }]);
}

#[tokio::test]
async fn the_device_s_cli_is_found_and_waits_for_a_tick() {
    let (_dir, search) = home();
    let tools = tools(search);
    tools.check_latest().await;
    tools.scan().await;

    let found = published(&tools);
    assert_eq!(found.tools.len(), 1, "{:?}", found.tools);
    let claude = &found.tools[0];
    assert_eq!(claude.tool(), Some(Tool::Claude));
    assert_eq!(claude.place, ToolPlace::Host);
    assert_eq!(claude.version.as_deref(), Some("2.1.0"));
    assert_eq!(claude.latest.as_deref(), Some("2.2.0"));
    assert_eq!(claude.owner, ToolOwner::User);
    assert_eq!(claude.method, "native");
    assert_eq!(claude.update, ToolUpdate::NeedsTick);

    let target = ToolTarget {
        tool: Tool::Claude,
        place: ToolPlace::Host,
    };
    let outcome = tools.update(vec![target]).await;
    assert!(
        matches!(outcome, ActionOutcome::Refused { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_ticked_cli_updates_and_shows_its_new_version() {
    let (_dir, search) = home();
    let tools = tools(search);
    tick(&tools, Tool::Claude);
    tools.check_latest().await;
    tools.scan().await;
    assert_eq!(published(&tools).tools[0].update, ToolUpdate::Allowed);

    let target = ToolTarget {
        tool: Tool::Claude,
        place: ToolPlace::Host,
    };
    assert_eq!(tools.update(vec![target]).await, ActionOutcome::Done);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let claude = published(&tools).tools[0].clone();
        if claude.version.as_deref() == Some("2.2.0") {
            assert_eq!(claude.state, ToolState::Idle);
            break;
        }
        assert!(
            !matches!(claude.state, ToolState::Failed { .. }),
            "{claude:?}"
        );
        assert!(tokio::time::Instant::now() < deadline, "{claude:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_tick_lets_a_cli_update_without_a_scan() {
    let (_dir, search) = home();
    let tools = tools(search);
    tools.follow_ticks();
    tools.scan().await;
    assert_eq!(published(&tools).tools[0].update, ToolUpdate::NeedsTick);

    tick(&tools, Tool::Claude);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while published(&tools).tools[0].update != ToolUpdate::Allowed {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the tick wasn't followed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_pinned_container_cli_is_refused_rather_than_skipped() {
    let (_dir, search) = home();
    let tools = tools(search);
    let target = ToolTarget {
        tool: Tool::Codex,
        place: ToolPlace::Workspace {
            workspace: maxhq_protocol::ids::WorkspaceId::derive("w"),
        },
    };
    let outcome = tools.update(vec![target]).await;
    assert!(
        matches!(&outcome, ActionOutcome::Refused { reason } if reason.contains("moves with maxd")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn host_exec_never_runs_as_root() {
    let refused = HostExec.run(&["true"], ExecUser::Root, None).await;
    assert!(
        matches!(refused, Err(ExecError::Conflict(_))),
        "{refused:?}"
    );
    let sudo = HostExec.run(&["sudo", "true"], ExecUser::Agent, None).await;
    assert!(matches!(sudo, Err(ExecError::Conflict(_))), "{sudo:?}");
    let ran = HostExec
        .run(
            &["sh", "-c", "cat; echo out"],
            ExecUser::Agent,
            Some(b"in "),
        )
        .await
        .unwrap();
    assert!(ran.success());
    assert_eq!(ran.stdout, b"in out\n");
}

/// One call: the tool, the place, the version, and true for an install.
type Call = (ToolKind, ToolPlace, Option<String>, bool);

/// An installer that says what it was asked and fails when told to.
#[derive(Default)]
struct Asked {
    calls: Mutex<Vec<Call>>,
    fail: bool,
}

impl install::Installer for Asked {
    fn ensure<'a>(
        &'a self,
        def: &'a install::InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<install::Installed, install::InstallError>> {
        lock(&self.calls).push((def.kind.clone(), *place, version.map(str::to_owned), true));
        let fail = self.fail;
        async move {
            if fail {
                return Err(install::InstallError::NoTarget("riscv64".into()));
            }
            Ok(install::Installed {
                version: "1.0.0".into(),
                path: "/nowhere/gh".into(),
                root: "/nowhere".into(),
            })
        }
        .boxed()
    }

    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), install::InstallError>> {
        lock(&self.calls).push((kind.clone(), *place, None, false));
        async { Ok(()) }.boxed()
    }
}

/// Waits until the published entry for `kind` on the host passes `done`.
async fn until(tools: &Tools, kind: &ToolKind, done: impl Fn(Option<&ToolInfo>) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let found = published(tools);
        if done(found.tools.iter().find(|info| info.kind == *kind)) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{found:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_named_version_that_isnt_one_never_reaches_the_installer() {
    let (_dir, search) = home();
    let tools = tools(search);
    let asked = Arc::new(Asked::default());
    tools.set_installer(asked.clone());
    let gh = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::Gh,
    };
    for version in [
        "../../../../../Documents",
        "..",
        "1.0/../..",
        "https://example.invalid/tool.tgz",
        "github:someone/tool",
        "$(touch nope)",
        "1.0\"; touch nope; \"",
        "",
    ] {
        assert!(
            matches!(
                tools.install(gh.clone(), ToolPlace::Host, Some(version.into())),
                ActionOutcome::Refused { .. }
            ),
            "{version}"
        );
    }
    assert!(lock(&asked.calls).is_empty());
    for version in ["1.2.3", "v1.2.3", "0.116.0-beta.1"] {
        assert_eq!(
            tools.install(gh.clone(), ToolPlace::Host, Some(version.into())),
            ActionOutcome::Done,
            "{version}"
        );
        until(&tools, &gh, |info| info.is_none()).await;
    }
    assert_eq!(lock(&asked.calls).len(), 3);
}

#[tokio::test]
async fn an_install_shows_while_it_runs_and_goes_through_the_installer() {
    let (_dir, search) = home();
    let tools = tools(search);
    let asked = Arc::new(Asked::default());
    tools.set_installer(asked.clone());
    let gh = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::Gh,
    };
    assert_eq!(
        tools.install(gh.clone(), ToolPlace::Host, Some("2.0.0".into())),
        ActionOutcome::Done
    );
    until(&tools, &gh, |info| info.is_none()).await;
    assert_eq!(
        lock(&asked.calls).as_slice(),
        [(gh.clone(), ToolPlace::Host, Some("2.0.0".into()), true)]
    );
    assert_eq!(
        tools.remove(gh.clone(), ToolPlace::Host),
        ActionOutcome::Done
    );
    until(&tools, &gh, |info| info.is_none()).await;
    assert!(!lock(&asked.calls)[1].3);
}

#[tokio::test]
async fn a_failed_install_says_why_and_others_are_refused() {
    let (_dir, search) = home();
    let tools = tools(search);
    let unset = tools.install(
        ToolKind::Helper {
            tool: maxhq_protocol::state::HelperTool::Gh,
        },
        ToolPlace::Host,
        None,
    );
    assert!(matches!(unset, ActionOutcome::Refused { .. }), "{unset:?}");
    tools.set_installer(Arc::new(Asked {
        fail: true,
        ..Asked::default()
    }));
    let uv = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::Uv,
    };
    assert_eq!(
        tools.install(uv.clone(), ToolPlace::Host, None),
        ActionOutcome::Done
    );
    until(&tools, &uv, |info| {
        info.is_some_and(|info| {
            info.state
                == ToolState::Failed {
                    reason: "There's no build for riscv64.".into(),
                }
                && info.owner == ToolOwner::MaxHq
        })
    })
    .await;

    let android = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::AndroidSdk,
    };
    assert_eq!(
        tools.install(android, ToolPlace::Host, None),
        refused("MaxHQ installs the Android SDK on a Mac only.".to_owned())
    );
    let agent_device = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::AgentDevice,
    };
    assert_eq!(
        tools.install(agent_device, ToolPlace::Host, None),
        refused("MaxHQ can't install that yet.".to_owned())
    );
    let elsewhere = ToolPlace::Workspace {
        workspace: maxhq_protocol::ids::WorkspaceId::derive("ws-alpha"),
    };
    assert!(matches!(
        tools.install(uv, elsewhere, None),
        ActionOutcome::Refused { .. }
    ));
}

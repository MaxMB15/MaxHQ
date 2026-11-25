// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The check's items. Each reads the system as the service user, with no
//! root: files under [`Env::root`], the cgroup tree, and Incus through its
//! socket. The egress table itself needs root to read, so its item reads
//! the files that load it, and a workspace's egress probe checks the rest.

use maxhq_core::host_spec::{HostSpec, Size};
use maxhq_protocol::hosts::{HostFix, HostItem, HostItemState};

use super::system::{self, Want};
use super::{Env, Os, HOST_CHANGE, INCUS_GROUP};
use crate::service::MODULE_UNIT;

/// The NixOS module's options that render each item, for its fix there.
const NIX_INCUS_ADMIN: &str = "services.maxd.incusAdmin = true;";
const NIX_INCUS: &str = "services.maxd.host.incus.enable = true;";
const NIX_PARENT: &str = "services.maxd.host.caps.manage_parent = true;";
const NIX_ZRAM: &str = "services.maxd.host.caps.zram = true;";
const NIX_ENABLE: &str = "services.maxd.enable = true;";

/// Every item that applies to this device.
pub async fn items(env: &Env, spec: &HostSpec) -> Vec<HostItem> {
    let mut items = Vec::new();
    if let Some(item) = service_user(env, spec) {
        items.push(item);
    }
    if env.os.is_linux() {
        if spec.incus.enable {
            incus(env, spec, &mut items).await;
        }
        caps(env, spec, &mut items);
        items.push(present(
            env,
            "linger",
            "maxd runs with nobody logged in",
            &format!("/var/lib/systemd/linger/{}", env.user.name),
            NIX_ENABLE,
        ));
        for want in system::files(env, spec) {
            items.push(files(env, spec, &want));
        }
        if let Some(item) = data_dir(env, spec) {
            items.push(item);
        }
    }
    items.push(user_service(env));
    tools(env, spec, &mut items);
    items
}

/// An item, with its fix only when it isn't fine.
fn item(id: &str, label: &str, state: HostItemState, fix: Option<HostFix>) -> HostItem {
    let fix = if state == HostItemState::Ok {
        None
    } else {
        fix
    };
    HostItem {
        id: id.to_owned(),
        label: label.to_owned(),
        state,
        fix,
    }
}

/// Ok or missing.
fn ok_if(ok: bool) -> HostItemState {
    if ok {
        HostItemState::Ok
    } else {
        HostItemState::Missing
    }
}

/// A file the root part makes, which must be there. `change` is the NixOS
/// option that makes it.
fn present(env: &Env, id: &str, label: &str, path: &str, change: &str) -> HostItem {
    let state = ok_if(env.at(path).exists());
    item(id, label, state, env.root_fix(change))
}

/// The spec names someone other than who runs maxd.
fn service_user(env: &Env, spec: &HostSpec) -> Option<HostItem> {
    let want = spec.service.user.as_ref()?;
    let state = if *want == env.user.name {
        HostItemState::Ok
    } else {
        HostItemState::Drift {
            want: want.clone(),
            have: env.user.name.clone(),
        }
    };
    Some(item("service.user", "Who runs maxd", state, None))
}

/// Incus, the group that may use it, and the spec's pool and bridge.
async fn incus(env: &Env, spec: &HostSpec, items: &mut Vec<HostItem>) {
    let member = in_group(env, INCUS_GROUP, &env.user.name);
    items.push(item(
        "incus.group",
        &format!("{} is in {INCUS_GROUP}", env.user.name),
        ok_if(member),
        env.root_fix(NIX_INCUS_ADMIN),
    ));
    let answer = match &env.incus {
        Some(incus) => incus.ping().await.map(|()| incus),
        None => Err(crate::incus::IncusError::NotYet),
    };
    let incus = match answer {
        Ok(incus) => {
            items.push(item("incus", "Incus answers", HostItemState::Ok, None));
            Some(incus)
        }
        Err(err) => {
            let gone = !env.at(crate::incus::SOCKET).exists();
            let state = if gone {
                HostItemState::Missing
            } else {
                HostItemState::Unknown {
                    why: err.to_string(),
                }
            };
            items.push(item(
                "incus",
                "Incus answers",
                state,
                env.root_fix(NIX_INCUS),
            ));
            None
        }
    };
    let silent = || HostItemState::Unknown {
        why: "Incus doesn't answer".to_owned(),
    };
    if let Some(pool) = &spec.incus.pool {
        let label = format!("Storage pool {}", pool.name);
        let state = match incus {
            None => silent(),
            Some(incus) => match incus.pools().await {
                Err(err) => HostItemState::Unknown {
                    why: err.to_string(),
                },
                Ok(pools) => match pools.into_iter().find(|p| p.name == pool.name) {
                    None => HostItemState::Missing,
                    Some(have) if have.driver != pool.driver => HostItemState::Drift {
                        want: pool.driver.clone(),
                        have: have.driver,
                    },
                    Some(_) => HostItemState::Ok,
                },
            },
        };
        let fix = (state == HostItemState::Missing).then_some(HostFix::Now);
        items.push(item("incus.pool", &label, state, fix));
        if let Some(mount) = &pool.needs_mount {
            items.push(item(
                "incus.pool.mount",
                &format!("{mount} is mounted"),
                ok_if(mounted(env, mount)),
                None,
            ));
        }
    }
    if let Some(bridge) = &spec.incus.bridge {
        let label = format!("Bridge {}", bridge.name);
        let state = match incus {
            None => silent(),
            Some(incus) => match incus.networks().await {
                Err(err) => HostItemState::Unknown {
                    why: err.to_string(),
                },
                Ok(networks) => match networks.into_iter().find(|n| n.name == bridge.name) {
                    None => HostItemState::Missing,
                    Some(have) if !have.is_managed_bridge() => HostItemState::Drift {
                        want: "a bridge Incus manages".to_owned(),
                        have: format!("a {} network", have.kind),
                    },
                    Some(have) => match &bridge.subnet {
                        Some(want) if have.ipv4() != Some(want.as_str()) => HostItemState::Drift {
                            want: want.clone(),
                            have: have.ipv4().unwrap_or("no address").to_owned(),
                        },
                        _ => HostItemState::Ok,
                    },
                },
            },
        };
        let fix = (state == HostItemState::Missing).then_some(HostFix::Now);
        items.push(item("incus.bridge", &label, state, fix));
    }
}

/// True when `user` is a member of `group` in `/etc/group`.
pub fn in_group(env: &Env, group: &str, user: &str) -> bool {
    let groups = env.read("/etc/group").unwrap_or_default();
    groups.lines().any(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        fields.len() == 4 && fields[0] == group && fields[3].split(',').any(|m| m == user)
    })
}

/// True when something is mounted at `path`.
fn mounted(env: &Env, path: &str) -> bool {
    let mounts = env.read("/proc/self/mountinfo").unwrap_or_default();
    let path = path.trim_end_matches('/');
    mounts
        .lines()
        .any(|line| line.split(' ').nth(4) == Some(path))
}

/// The parent cgroup and its limits, the memory kept for the host's own
/// services, and zram.
fn caps(env: &Env, spec: &HostSpec, items: &mut Vec<HostItem>) {
    let caps = &spec.caps;
    if let Some(parent) = &caps.parent {
        let dir = format!("/sys/fs/cgroup/{parent}");
        let fix = if caps.manage_parent {
            env.root_fix(NIX_PARENT)
        } else {
            None
        };
        let there = env.at(&dir).is_dir();
        items.push(item(
            "caps.parent",
            &format!("Workspace cgroup {parent}"),
            ok_if(there),
            fix.clone(),
        ));
        let limits = [
            (
                "caps.memory_high",
                "Memory cap on all workspaces",
                "memory.high",
                caps.memory_high.map(|size| size.0.to_string()),
            ),
            (
                "caps.cpu_max",
                "CPU cap on all workspaces",
                "cpu.max",
                caps.cpu_max.map(system::cpu_max),
            ),
            (
                "caps.cpu_weight",
                "CPU weight of all workspaces",
                "cpu.weight",
                caps.cpu_weight.map(|weight| weight.to_string()),
            ),
        ];
        for (id, label, file, want) in limits {
            let Some(want) = want else {
                continue;
            };
            let have = env.read(&format!("{dir}/{file}"));
            items.push(item(id, label, limit(file, &want, have), fix.clone()));
        }
    }
    if let Some(low) = caps.system_memory_low {
        let have = env.read("/sys/fs/cgroup/system.slice/memory.low");
        items.push(item(
            "caps.system_memory_low",
            "Memory kept for the host's services",
            limit("memory.low", &low.0.to_string(), have),
            env.root_fix(&format!(
                "services.maxd.host.caps.system_memory_low = \"{low}\";"
            )),
        ));
    }
    if caps.zram {
        let swaps = env.read("/proc/swaps").unwrap_or_default();
        let on = swaps.lines().any(|line| line.starts_with("/dev/zram"));
        items.push(item(
            "zram",
            "Compressed swap in memory",
            ok_if(on),
            env.root_fix(NIX_ZRAM),
        ));
    }
}

/// How a cgroup file stands against what it should hold.
fn limit(file: &str, want: &str, have: Option<String>) -> HostItemState {
    let Some(have) = have else {
        return HostItemState::Missing;
    };
    let have = have.trim();
    if have == want {
        return HostItemState::Ok;
    }
    HostItemState::Drift {
        want: shown(file, want),
        have: shown(file, have),
    }
}

/// A cgroup value as a person reads it.
fn shown(file: &str, value: &str) -> String {
    match file {
        "memory.high" | "memory.low" => match value.parse::<u64>() {
            Ok(0) => "none".to_owned(),
            Ok(bytes) => Size(bytes).to_string(),
            Err(_) => "no limit".to_owned(),
        },
        "cpu.max" => {
            let mut parts = value.split(' ');
            match (parts.next().map(str::parse::<u64>), parts.next()) {
                (Some(Ok(quota)), Some(period)) => match period.parse::<u64>() {
                    Ok(period) if period > 0 => {
                        let cpus = quota as f64 / period as f64;
                        format!("{cpus} CPUs")
                    }
                    _ => value.to_owned(),
                },
                _ => "no limit".to_owned(),
            }
        }
        _ => value.to_owned(),
    }
}

/// Files the root part writes. On NixOS the module writes its own, so
/// only that they're there counts.
fn files(env: &Env, spec: &HostSpec, want: &Want) -> HostItem {
    let change = match (want.id, &spec.incus.bridge) {
        ("units.prepare", _) => NIX_PARENT.to_owned(),
        ("egress", Some(bridge)) => {
            format!(
                "services.maxd.host.incus.bridge.name = \"{}\";",
                bridge.name
            )
        }
        _ => HOST_CHANGE.to_owned(),
    };
    let fix = env.root_fix(&change);
    let mut state = HostItemState::Ok;
    for (path, text) in &want.files {
        match env.read(path) {
            None => {
                state = HostItemState::Missing;
                break;
            }
            Some(have) if env.os != Os::Nix && have != *text => {
                state = HostItemState::Drift {
                    want: "what maxd writes".to_owned(),
                    have: format!("other text in {path}"),
                };
            }
            Some(_) => {}
        }
        // The module names only the unit, so the first file is enough.
        if env.os == Os::Nix {
            break;
        }
    }
    item(want.id, want.label, state, fix)
}

/// The spec's data folder, which must belong to the service user alone.
fn data_dir(env: &Env, spec: &HostSpec) -> Option<HostItem> {
    let dir = spec.service.data_dir.as_ref()?;
    let state = match std::fs::metadata(env.at(dir)) {
        Err(_) => HostItemState::Missing,
        Ok(meta) if !meta.is_dir() => HostItemState::Drift {
            want: "a folder".to_owned(),
            have: "a file".to_owned(),
        },
        Ok(meta) if super::ids::owner(&meta) != env.user.uid => HostItemState::Drift {
            want: format!("owned by {}", env.user.name),
            have: format!("owned by uid {}", super::ids::owner(&meta)),
        },
        Ok(meta) if maxhq_os::fsmode::mode_of(&meta) & 0o077 != 0 => HostItemState::Drift {
            want: "mode 0700".to_owned(),
            have: format!("mode {:04o}", maxhq_os::fsmode::mode_of(&meta) & 0o7777),
        },
        Ok(_) => HostItemState::Ok,
    };
    Some(item(
        "data_dir",
        &format!("Data folder {dir}"),
        state,
        env.root_fix(&format!("services.maxd.host.service.data_dir = \"{dir}\";")),
    ))
}

/// The user service that keeps `maxd run` going. Where the NixOS module
/// writes the unit, one from `maxd service install` shadows it.
fn user_service(env: &Env) -> HostItem {
    const LABEL: &str = "maxd runs as a user service";
    let module = env.at(MODULE_UNIT).exists();
    if env.os == Os::Nix && !module {
        let fix = HostFix::NixChange {
            change: NIX_ENABLE.to_owned(),
        };
        return item("service", LABEL, HostItemState::Missing, Some(fix));
    }
    if module {
        let shadow = env.service.as_ref().filter(|setup| setup.file().exists());
        let Some(setup) = shadow else {
            return item("service", LABEL, HostItemState::Ok, None);
        };
        // The command runs as you, and the report has no fix kind for
        // that. A root command would run uninstall in root's session, so
        // the command goes in the text and the item has no fix.
        let state = HostItemState::Drift {
            want: "the NixOS module's unit alone".to_owned(),
            have: format!(
                "{} too, which shadows it. Remove it with `{}`",
                setup.file().display(),
                setup.unshadow_command()
            ),
        };
        return item("service", LABEL, state, None);
    }
    let Some(setup) = &env.service else {
        let why = "maxd can't tell where its service file goes here".to_owned();
        return item("service", LABEL, HostItemState::Unknown { why }, None);
    };
    let state = match std::fs::read_to_string(setup.file()) {
        Err(_) => HostItemState::Missing,
        Ok(text) if text == setup.contents() => HostItemState::Ok,
        Ok(_) => HostItemState::Drift {
            want: "the service this maxd writes".to_owned(),
            have: "an older one".to_owned(),
        },
    };
    item("service", LABEL, state, Some(HostFix::Now))
}

/// The tools the spec names. MaxHQ's installer puts the user's in place,
/// and the system's need root.
fn tools(env: &Env, spec: &HostSpec, items: &mut Vec<HostItem>) {
    for name in &spec.tools.user {
        let binary = binary(name);
        items.push(item(
            &format!("tools.{name}"),
            &format!("{binary} for {}", env.user.name),
            ok_if((env.find)(binary).is_some()),
            None,
        ));
    }
    for name in &spec.tools.system {
        let fix = match env.os {
            Os::Nix => Some(HostFix::NixChange {
                change: format!("environment.systemPackages = [ pkgs.{name} ];"),
            }),
            _ => env.root_fix(HOST_CHANGE),
        };
        items.push(item(
            &format!("tools.system.{name}"),
            &format!("{name} for everyone"),
            ok_if((env.find)(name).is_some()),
            fix,
        ));
    }
}

/// The command a tool in the spec answers to.
fn binary(name: &str) -> &str {
    match name {
        "cursor" => crate::tools::defs::def(maxhq_protocol::model::Tool::Cursor).binary,
        "agent_device" => "agent-device",
        "android_sdk" => "sdkmanager",
        "jdk" => "java",
        other => other,
    }
}

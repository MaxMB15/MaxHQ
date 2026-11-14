// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The part of apply that needs no root: the spec's Incus pool and bridge,
//! made through the socket as a member of `incus-admin`, and the user
//! service.

use std::collections::BTreeMap;
use std::time::Duration;

use maxhq_core::host_spec::HostSpec;
use maxhq_protocol::hosts::{HostFix, HostItem, HostItemState};

use super::Env;

/// When the service fix restarts maxd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restart {
    /// Right away, from the command line.
    Now,
    /// A moment later, so the answer to the app goes out first.
    Later,
}

/// How long [`Restart::Later`] waits.
const RESTART_DELAY: Duration = Duration::from_millis(500);

/// Fixes the `items` maxd can fix itself, or the ones of them `wanted`
/// names. Returns what it did.
pub async fn apply_user(
    env: &Env,
    spec: &HostSpec,
    items: &[HostItem],
    wanted: Option<&[String]>,
    restart: Restart,
) -> Result<Vec<String>, String> {
    if env.user.uid == 0 {
        return Err(format!(
            "Run maxd host apply as the service user, not root. The root part is {}.",
            env.root_command()
        ));
    }
    let fixable = |item: &&HostItem| item.fix == Some(HostFix::Now);
    let chosen: Vec<&HostItem> = match wanted {
        None => items.iter().filter(fixable).collect(),
        Some(ids) => {
            let mut chosen = Vec::new();
            for id in ids {
                match items.iter().find(|item| item.id == *id) {
                    Some(item) if item.state == HostItemState::Ok => {}
                    Some(item) if fixable(&item) => chosen.push(item),
                    Some(item) => {
                        return Err(format!("maxd can't fix \"{}\" by itself", item.label));
                    }
                    None => return Err(format!("This host has no item {id}")),
                }
            }
            chosen
        }
    };
    let mut done = Vec::new();
    let mut service = false;
    for item in chosen {
        match item.id.as_str() {
            "incus.pool" => done.push(make_pool(env, spec).await?),
            "incus.bridge" => done.push(make_bridge(env, spec).await?),
            "service" => service = true,
            other => return Err(format!("maxd doesn't know how to fix {other}")),
        }
    }
    // Last, since it restarts maxd.
    if service {
        done.extend(install_service(env, restart).await?);
    }
    Ok(done)
}

/// Makes the spec's storage pool.
async fn make_pool(env: &Env, spec: &HostSpec) -> Result<String, String> {
    let pool = spec.incus.pool.as_ref().ok_or("The spec names no pool")?;
    let incus = env.incus.as_ref().ok_or("Incus isn't on this host")?;
    let mut config = BTreeMap::new();
    if let Some(image) = &pool.image {
        config.insert("source".to_owned(), image.clone());
    }
    if let Some(size) = pool.size {
        config.insert("size".to_owned(), size.0.to_string());
    }
    incus
        .create_pool(&pool.name, &pool.driver, config)
        .await
        .map_err(|err| format!("Incus didn't make pool {}: {err}", pool.name))?;
    Ok(format!("Made storage pool {}.", pool.name))
}

/// Makes the spec's bridge, with NAT and no IPv6.
async fn make_bridge(env: &Env, spec: &HostSpec) -> Result<String, String> {
    let bridge = spec
        .incus
        .bridge
        .as_ref()
        .ok_or("The spec names no bridge")?;
    let incus = env.incus.as_ref().ok_or("Incus isn't on this host")?;
    let mut config = BTreeMap::from([
        ("ipv4.nat".to_owned(), "true".to_owned()),
        ("ipv6.address".to_owned(), "none".to_owned()),
    ]);
    if let Some(subnet) = &bridge.subnet {
        config.insert("ipv4.address".to_owned(), subnet.clone());
    }
    incus
        .create_bridge(&bridge.name, config)
        .await
        .map_err(|err| format!("Incus didn't make bridge {}: {err}", bridge.name))?;
    Ok(format!("Made bridge {}.", bridge.name))
}

/// Writes the user service and restarts maxd. Where the NixOS module owns
/// the unit, it says so and changes nothing.
async fn install_service(env: &Env, restart: Restart) -> Result<Vec<String>, String> {
    let setup = env
        .service
        .clone()
        .ok_or("maxd can't tell where its service file goes here")?;
    if setup.module_owns_unit() {
        return Ok(crate::service::owned_by_module(&setup));
    }
    let note = setup.left_out_note();
    let install = move || {
        crate::service::install(&setup, &mut crate::service::System).map_err(|e| e.to_string())
    };
    match restart {
        Restart::Now => tokio::task::spawn_blocking(install)
            .await
            .map_err(|_| "The service install stopped".to_owned())?,
        Restart::Later => {
            tokio::spawn(async move {
                tokio::time::sleep(RESTART_DELAY).await;
                match tokio::task::spawn_blocking(install).await {
                    Ok(Err(err)) => tracing::warn!("the service install failed: {err}"),
                    Err(_) => tracing::warn!("the service install stopped"),
                    Ok(Ok(_)) => {}
                }
            });
            let mut report = vec!["Installing the service, which restarts maxd.".to_owned()];
            report.extend(note);
            Ok(report)
        }
    }
}

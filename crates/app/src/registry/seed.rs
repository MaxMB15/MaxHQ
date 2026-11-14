// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Ties each saved link to its device when the app first keeps who each
//! link reaches, from what the app already recorded about it: the links
//! its device links list under each device, and the changes queued for a
//! device through a link. A link gets a device only when every record
//! agrees and no other link is tied to that device, so a link that once
//! named another's device gets nothing and asks at its next hello.

use std::collections::{BTreeMap, BTreeSet};

use maxhq_protocol::ids::{DeviceId, LinkId};
use rusqlite::{params, OptionalExtension, Transaction};
use rusqlite_migration::HookResult;
use serde::Deserialize;

/// The setting device links are kept under, as `devices::peers` saves it.
const DEVICE_LINKS: &str = "device_links";

/// The part of the device links setting this reads.
#[derive(Deserialize)]
struct Kept {
    #[serde(default)]
    devices: BTreeMap<DeviceId, Known>,
}

#[derive(Deserialize)]
struct Known {
    #[serde(default)]
    links: BTreeSet<LinkId>,
}

/// Fills in the device of each SSH link that has none, where the records
/// agree on one.
pub(super) fn link_devices(tx: &Transaction) -> HookResult {
    let column = |sql: &str| -> rusqlite::Result<BTreeSet<String>> {
        let mut statement = tx.prepare(sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    };
    let saved = column("SELECT link FROM device")?;
    let ssh = column("SELECT link FROM device WHERE kind = 'ssh'")?;

    let mut ties: BTreeSet<(String, String)> = BTreeSet::new();
    {
        let mut statement = tx.prepare("SELECT link, device FROM link_device")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            ties.insert(row?);
        }
    }
    let pinned: BTreeSet<String> = ties.iter().map(|(link, _)| link.clone()).collect();
    {
        let mut statement = tx.prepare("SELECT DISTINCT link, device FROM outbox")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            ties.insert(row?);
        }
    }
    let kept: Option<String> = tx
        .query_row(
            "SELECT value FROM setting WHERE name = ?1",
            params![DEVICE_LINKS],
            |row| row.get(0),
        )
        .optional()?;
    let kept = kept.and_then(|value| serde_json::from_str::<Kept>(&value).ok());
    for (device, known) in kept.map(|kept| kept.devices).unwrap_or_default() {
        for link in known.links {
            ties.insert((link.to_string(), device.to_string()));
        }
    }
    ties.retain(|(link, _)| saved.contains(link));

    let mut by_link: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut by_device: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (link, device) in &ties {
        by_link.entry(link).or_default().insert(device);
        by_device.entry(device).or_default().insert(link);
    }
    for (link, devices) in &by_link {
        let mut each = devices.iter();
        let (Some(device), None) = (each.next(), each.next()) else {
            continue;
        };
        let alone = by_device.get(device).is_some_and(|links| links.len() == 1);
        if alone
            && ssh.contains(*link)
            && !pinned.contains(*link)
            && device.parse::<DeviceId>().is_ok()
        {
            tx.execute(
                "INSERT INTO link_device (link, device) VALUES (?1, ?2)",
                params![link, device],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::time::Timestamp;
    use rusqlite::Connection;
    use rusqlite_migration::Migrations;
    use serde_json::json;

    use super::*;
    use crate::registry::{local_link, migrations, steps, Registry};

    /// A registry from just before links kept when they first reached
    /// their device, with `ssh` devices, the device links setting and
    /// changes queued through a link, opened now.
    fn upgraded(
        ssh: &[LinkId],
        kept: serde_json::Value,
        queued: &[(LinkId, DeviceId)],
        pinned: &[(LinkId, DeviceId)],
    ) -> Registry {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let mut old = steps();
        old.truncate(5);
        Migrations::new(old).to_latest(&mut conn).unwrap();
        {
            conn.execute(
                "INSERT INTO device (link, name, kind, host, added_at)
                 VALUES (?1, 'This Mac', 'local', NULL, 0)",
                params![local_link().to_string()],
            )
            .unwrap();
            for (n, link) in ssh.iter().enumerate() {
                conn.execute(
                    "INSERT INTO device (link, name, kind, host, added_at)
                     VALUES (?1, ?2, 'ssh', ?2, 1)",
                    params![link.to_string(), format!("devbox{n}")],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO setting (name, value) VALUES (?1, ?2)",
                params![DEVICE_LINKS, kept.to_string()],
            )
            .unwrap();
            for (n, (link, device)) in queued.iter().enumerate() {
                conn.execute(
                    "INSERT INTO outbox (key, link, device, agent, title, change, queued_at)
                     VALUES (?1, ?2, ?3, 'agent', 'Agent', '{}', 1)",
                    params![format!("key{n}"), link.to_string(), device.to_string()],
                )
                .unwrap();
            }
            for (link, device) in pinned {
                conn.execute(
                    "INSERT INTO link_device (link, device) VALUES (?1, ?2)",
                    params![link.to_string(), device.to_string()],
                )
                .unwrap();
            }
        }
        migrations().to_latest(&mut conn).unwrap();
        Registry::start(conn).unwrap()
    }

    fn kept(devices: &[(DeviceId, &[LinkId])]) -> serde_json::Value {
        let devices: serde_json::Map<String, serde_json::Value> = devices
            .iter()
            .map(|(device, links)| {
                (
                    device.to_string(),
                    json!({ "links": links, "key": null, "book": null, "let_in": {} }),
                )
            })
            .collect();
        json!({ "devices": devices, "off": [], "revokes": [] })
    }

    #[test]
    fn saved_links_take_the_device_the_records_agree_on() {
        let (devbox, laptop) = (LinkId::generate(), LinkId::generate());
        let (one, two) = (DeviceId::derive("devbox"), DeviceId::derive("laptop"));
        let registry = upgraded(
            &[devbox, laptop],
            kept(&[(one, &[devbox])]),
            &[(laptop, two), (devbox, one)],
            &[],
        );
        assert_eq!(registry.known_device(devbox).unwrap(), Some(one));
        assert_eq!(registry.known_device(laptop).unwrap(), Some(two));
        // It reached its device before the app kept the time.
        assert_eq!(registry.first_seen(devbox).unwrap(), Some(Timestamp(1)));
    }

    #[test]
    fn a_link_the_records_tie_to_another_links_device_gets_none() {
        let (real, hostile, mixed) = (LinkId::generate(), LinkId::generate(), LinkId::generate());
        let (victim, other) = (DeviceId::derive("victim"), DeviceId::derive("other"));
        let registry = upgraded(
            &[real, hostile, mixed],
            // The hostile link once named the victim, so both sit under it.
            kept(&[(victim, &[real, hostile])]),
            // One link's records name two devices.
            &[(mixed, other), (mixed, DeviceId::derive("third"))],
            &[],
        );
        for link in [real, hostile, mixed] {
            assert_eq!(registry.known_device(link).unwrap(), None);
        }
    }

    #[test]
    fn a_device_another_link_reaches_already_or_this_mac_isnt_given_out() {
        let (pinned, unpinned, local) =
            (LinkId::generate(), LinkId::generate(), LinkId::generate());
        let (device, mac) = (DeviceId::derive("devbox"), DeviceId::derive("mac"));
        let registry = upgraded(
            &[pinned, unpinned, local],
            kept(&[(mac, &[local_link(), local])]),
            &[(unpinned, device)],
            &[(pinned, device)],
        );
        assert_eq!(registry.known_device(pinned).unwrap(), Some(device));
        assert_eq!(registry.known_device(unpinned).unwrap(), None);
        assert_eq!(registry.known_device(local).unwrap(), None);
    }

    #[test]
    fn a_registry_with_nothing_recorded_opens_as_before() {
        let link = LinkId::generate();
        let registry = upgraded(&[link], json!("unreadable"), &[], &[]);
        assert_eq!(registry.known_device(link).unwrap(), None);
        assert_eq!(registry.list().unwrap().len(), 2);
    }
}

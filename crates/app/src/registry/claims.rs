// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Links held because their hello named a device another link knows, and
//! your answers about them. A held link has no device in the registry, so
//! nothing goes to that device through it until you say it's the same one.

use maxhq_protocol::ids::{DeviceId, LinkId};
use maxhq_protocol::link::{LinkClaim, LinkClaimState};
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, OptionalExtension};

use super::{link_key, Registry, RegistryError};

impl Registry {
    /// What `link`'s held hello named, while it waits for you or after you
    /// said it's a different device.
    pub fn claim(&self, link: LinkId) -> Result<Option<LinkClaim>, RegistryError> {
        let row: Option<(String, Option<String>, bool, i64)> = self
            .conn
            .query_row(
                "SELECT device, known_by, different, at FROM link_claim WHERE link = ?1",
                params![link_key(link)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        Ok(row.and_then(|(device, known_by, different, at)| {
            Some(LinkClaim {
                device: device.parse().ok()?,
                known_by: known_by.and_then(|known| known.parse().ok()),
                state: if different {
                    LinkClaimState::Different
                } else {
                    LinkClaimState::Asking
                },
                since: Timestamp(u64::try_from(at).unwrap_or_default()),
            })
        }))
    }

    /// Holds `link`, whose hello named `device`, which `known_by` already
    /// knows. A link held for the same device keeps when it first said
    /// so. Only an SSH device in the registry can be held, so this gives
    /// None for any other link.
    pub fn hold(
        &self,
        link: LinkId,
        device: DeviceId,
        known_by: Option<LinkId>,
    ) -> Result<Option<LinkClaim>, RegistryError> {
        self.conn.execute(
            "INSERT INTO link_claim (link, device, known_by, different, at)
             SELECT link, ?2, ?3, 0, ?4 FROM device WHERE link = ?1 AND kind = 'ssh'
             ON CONFLICT (link) DO UPDATE SET
                 device = excluded.device, known_by = excluded.known_by,
                 different = 0, at = excluded.at
             WHERE link_claim.device != excluded.device",
            params![
                link_key(link),
                device.to_string(),
                known_by.map(link_key),
                now()
            ],
        )?;
        self.claim(link)
    }

    /// A link other than `link` that knows `device`: one that reaches it,
    /// one waiting for your answer about it, or one with changes queued
    /// for it. Else the first of `kept`, links the device links kept
    /// under it, that's still a device you added.
    pub fn known_elsewhere(
        &self,
        device: DeviceId,
        link: LinkId,
        kept: &[LinkId],
    ) -> Result<Option<LinkId>, RegistryError> {
        const KNOWING: [&str; 3] = [
            "SELECT link FROM link_device WHERE device = ?1 AND link != ?2
             ORDER BY link LIMIT 1",
            "SELECT link FROM link_claim WHERE device = ?1 AND link != ?2 AND different = 0
             ORDER BY at, link LIMIT 1",
            "SELECT outbox.link FROM outbox JOIN device ON device.link = outbox.link
             WHERE outbox.device = ?1 AND outbox.link != ?2 ORDER BY outbox.link LIMIT 1",
        ];
        for sql in KNOWING {
            let other: Option<String> = self
                .conn
                .query_row(sql, params![device.to_string(), link_key(link)], |row| {
                    row.get(0)
                })
                .optional()?;
            if let Some(other) = other.and_then(|other| other.parse().ok()) {
                return Ok(Some(other));
            }
        }
        for &other in kept.iter().filter(|&&other| other != link) {
            let saved: Option<i64> = self
                .conn
                .query_row(
                    "SELECT 1 FROM device WHERE link = ?1",
                    params![link_key(other)],
                    |row| row.get(0),
                )
                .optional()?;
            if saved.is_some() {
                return Ok(Some(other));
            }
        }
        Ok(None)
    }

    /// Asks you again about `link`, after you said it's a different
    /// device. False when it isn't held.
    pub fn reask_claim(&self, link: LinkId) -> Result<bool, RegistryError> {
        let changed = self.conn.execute(
            "UPDATE link_claim SET different = 0, at = ?2 WHERE link = ?1",
            params![link_key(link), now()],
        )?;
        Ok(changed > 0)
    }

    /// Holds `link` until you say it's `device`, whatever it reaches now,
    /// as setup does before it keeps new host keys under a device's name.
    /// None when it isn't an SSH device you added.
    pub fn ask_about(
        &self,
        link: LinkId,
        device: DeviceId,
        known_by: Option<LinkId>,
    ) -> Result<Option<LinkClaim>, RegistryError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM link_device WHERE link = ?1
                 AND EXISTS (SELECT 1 FROM device WHERE link = ?1 AND kind = 'ssh')",
            params![link_key(link)],
        )?;
        tx.execute(
            "INSERT INTO link_claim (link, device, known_by, different, at)
             SELECT link, ?2, ?3, 0, ?4 FROM device WHERE link = ?1 AND kind = 'ssh'
             ON CONFLICT (link) DO UPDATE SET
                 device = excluded.device, known_by = excluded.known_by,
                 different = 0, at = excluded.at",
            params![
                link_key(link),
                device.to_string(),
                known_by.map(link_key),
                now()
            ],
        )?;
        tx.commit()?;
        self.claim(link)
    }

    /// You said `link` is the device its hello named: it reaches that
    /// device from now on. Gives the device, or None when the link isn't
    /// held.
    pub fn confirm_claim(&self, link: LinkId) -> Result<Option<DeviceId>, RegistryError> {
        let Some(claim) = self.claim(link)? else {
            return Ok(None);
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO link_device (link, device, at) VALUES (?1, ?2, ?3)
             ON CONFLICT (link) DO UPDATE SET device = excluded.device, at = excluded.at",
            params![link_key(link), claim.device.to_string(), now()],
        )?;
        tx.execute(
            "DELETE FROM link_claim WHERE link = ?1",
            params![link_key(link)],
        )?;
        tx.commit()?;
        Ok(Some(claim.device))
    }

    /// You said `link` isn't the device its hello named, so it stays held
    /// whatever it says until you ask again. False when it isn't held.
    pub fn refuse_claim(&self, link: LinkId) -> Result<bool, RegistryError> {
        let changed = self.conn.execute(
            "UPDATE link_claim SET different = 1 WHERE link = ?1",
            params![link_key(link)],
        )?;
        Ok(changed > 0)
    }

    /// Forgets which device `link` reaches and any answer about it, so its
    /// next hello counts as its first: it reaches the device it names
    /// unless another link knows that device, which asks you again.
    pub fn forget_identity(&self, link: LinkId) -> Result<(), RegistryError> {
        let tx = self.conn.unchecked_transaction()?;
        for table in ["link_device", "link_claim"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE link = ?1"),
                params![link_key(link)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// When `link` first reached its device, or when you added it for a
    /// link that reached one before the app kept the time.
    pub fn first_seen(&self, link: LinkId) -> Result<Option<Timestamp>, RegistryError> {
        let at: Option<i64> = self
            .conn
            .query_row(
                "SELECT COALESCE(link_device.at, device.added_at)
                 FROM link_device JOIN device USING (link) WHERE link = ?1",
                params![link_key(link)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(at.map(|at| Timestamp(u64::try_from(at).unwrap_or_default())))
    }
}

/// Now, as the registry keeps times.
pub(super) fn now() -> i64 {
    i64::try_from(Timestamp::now().0).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::local_link;

    #[test]
    fn a_held_link_keeps_when_it_first_said_so_until_it_names_another() {
        let registry = Registry::in_memory().unwrap();
        let (real, hostile) = (
            registry.add("devbox").unwrap(),
            registry.add("other").unwrap(),
        );
        let victim = DeviceId::derive("victim");
        registry.keep_device(real.id, victim).unwrap();
        assert_eq!(
            registry.known_elsewhere(victim, hostile.id, &[]).unwrap(),
            Some(real.id)
        );
        assert_eq!(
            registry.known_elsewhere(victim, real.id, &[]).unwrap(),
            None
        );

        let held = registry
            .hold(hostile.id, victim, Some(real.id))
            .unwrap()
            .unwrap();
        assert_eq!(held.device, victim);
        assert_eq!(held.known_by, Some(real.id));
        assert_eq!(held.state, LinkClaimState::Asking);
        let again = registry.hold(hostile.id, victim, None).unwrap().unwrap();
        assert_eq!(again, held);
        // Nothing reaches the device through it meanwhile.
        assert_eq!(registry.known_device(hostile.id).unwrap(), None);

        let elsewhere = DeviceId::derive("elsewhere");
        let moved = registry.hold(hostile.id, elsewhere, None).unwrap().unwrap();
        assert_eq!(moved.device, elsewhere);
        assert_eq!(moved.known_by, None);
    }

    #[test]
    fn a_link_waiting_about_a_device_or_kept_under_it_knows_it_too() {
        let registry = Registry::in_memory().unwrap();
        let (first, second, kept) = (
            registry.add("devbox").unwrap(),
            registry.add("other").unwrap(),
            registry.add("kept").unwrap(),
        );
        let device = DeviceId::derive("devbox");
        let gone = LinkId::generate();
        assert_eq!(
            registry
                .known_elsewhere(device, second.id, &[gone, second.id])
                .unwrap(),
            None,
            "a removed link and the link itself don't count"
        );
        assert_eq!(
            registry
                .known_elsewhere(device, second.id, &[kept.id])
                .unwrap(),
            Some(kept.id)
        );

        registry.hold(first.id, device, None).unwrap();
        assert_eq!(
            registry.known_elsewhere(device, second.id, &[]).unwrap(),
            Some(first.id)
        );
        // One you said is a different device doesn't, until you ask again.
        registry.refuse_claim(first.id).unwrap();
        assert_eq!(
            registry.known_elsewhere(device, second.id, &[]).unwrap(),
            None
        );
        assert!(registry.reask_claim(first.id).unwrap());
        let claim = registry.claim(first.id).unwrap().unwrap();
        assert_eq!(claim.state, LinkClaimState::Asking);
        assert!(!registry.reask_claim(second.id).unwrap());
    }

    #[test]
    fn asking_about_a_link_lets_go_of_its_device_until_you_answer() {
        let registry = Registry::in_memory().unwrap();
        let (real, second) = (
            registry.add("devbox").unwrap(),
            registry.add("devbox.lan").unwrap(),
        );
        let device = DeviceId::derive("devbox");
        registry.keep_device(real.id, device).unwrap();
        registry.keep_device(second.id, device).unwrap();
        let claim = registry
            .ask_about(second.id, device, Some(real.id))
            .unwrap()
            .unwrap();
        assert_eq!(claim.state, LinkClaimState::Asking);
        assert_eq!(registry.known_device(second.id).unwrap(), None);
        assert_eq!(registry.confirm_claim(second.id).unwrap(), Some(device));
        assert_eq!(registry.known_device(second.id).unwrap(), Some(device));
        assert_eq!(
            registry.ask_about(local_link(), device, None).unwrap(),
            None
        );
    }

    #[test]
    fn only_a_device_you_added_can_be_held() {
        let registry = Registry::in_memory().unwrap();
        let device = DeviceId::derive("mac");
        assert_eq!(registry.hold(local_link(), device, None).unwrap(), None);
        assert_eq!(
            registry.hold(LinkId::generate(), device, None).unwrap(),
            None
        );
    }

    #[test]
    fn same_device_pins_it_and_different_device_keeps_it_held() {
        let registry = Registry::in_memory().unwrap();
        let (real, second, hostile) = (
            registry.add("devbox").unwrap(),
            registry.add("devbox.lan").unwrap(),
            registry.add("other").unwrap(),
        );
        let victim = DeviceId::derive("victim");
        registry.keep_device(real.id, victim).unwrap();
        registry.hold(second.id, victim, Some(real.id)).unwrap();
        registry.hold(hostile.id, victim, Some(real.id)).unwrap();

        assert_eq!(registry.confirm_claim(second.id).unwrap(), Some(victim));
        assert_eq!(registry.known_device(second.id).unwrap(), Some(victim));
        assert_eq!(registry.claim(second.id).unwrap(), None);
        assert!(registry.first_seen(second.id).unwrap().is_some());

        assert!(registry.refuse_claim(hostile.id).unwrap());
        let refused = registry.claim(hostile.id).unwrap().unwrap();
        assert_eq!(refused.state, LinkClaimState::Different);
        assert_eq!(registry.known_device(hostile.id).unwrap(), None);
        // There's nothing to answer for a link that isn't held.
        assert_eq!(registry.confirm_claim(real.id).unwrap(), None);
        assert!(!registry.refuse_claim(real.id).unwrap());
    }

    #[test]
    fn forgetting_a_links_identity_drops_its_device_and_answer() {
        let registry = Registry::in_memory().unwrap();
        let (real, hostile) = (
            registry.add("devbox").unwrap(),
            registry.add("other").unwrap(),
        );
        let victim = DeviceId::derive("victim");
        registry.keep_device(real.id, victim).unwrap();
        registry.hold(hostile.id, victim, Some(real.id)).unwrap();
        registry.refuse_claim(hostile.id).unwrap();

        registry.forget_identity(real.id).unwrap();
        registry.forget_identity(hostile.id).unwrap();
        assert_eq!(registry.known_device(real.id).unwrap(), None);
        assert_eq!(registry.claim(hostile.id).unwrap(), None);
        assert_eq!(registry.first_seen(real.id).unwrap(), None);
    }

    #[test]
    fn keeping_a_device_lets_go_of_the_question() {
        let registry = Registry::in_memory().unwrap();
        let link = registry.add("devbox").unwrap();
        let device = DeviceId::derive("devbox");
        registry.hold(link.id, device, None).unwrap();
        registry.keep_device(link.id, device).unwrap();
        assert_eq!(registry.claim(link.id).unwrap(), None);
        assert_eq!(registry.known_device(link.id).unwrap(), Some(device));
    }
}

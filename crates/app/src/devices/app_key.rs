// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This Mac's app key, as Settings shows and changes it. The client proves
//! the key on each connection through [`AppKeys`]. Here the devices keep
//! its record in the app's database, name the devices a new key moves to,
//! remove a key a device trusts, and make a new one.

use std::sync::{Arc, Weak};

use maxhq_protocol::ids::{DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::ClientMessage;

use super::{failed, lock, Connector, Core, Devices, LinkControl};
use crate::app_keys::{AppKeyState, AppKeys, Keeper, Record, SETTING};
use crate::request::CommandError;

/// Keeps the app key's record in the database, and names the devices in
/// the device list that the app has reached.
struct Kept<L>(Weak<Core<L>>);

impl<L: LinkControl> Keeper for Kept<L> {
    fn save(&self, record: &Record) {
        let Some(core) = self.0.upgrade() else {
            return;
        };
        if let Err(error) = lock(&core.registry).set_setting(SETTING, record) {
            tracing::error!(%error, "couldn't save what the app knows of its key");
        }
    }

    fn devices(&self) -> Vec<DeviceId> {
        let Some(core) = self.0.upgrade() else {
            return Vec::new();
        };
        let registry = lock(&core.registry);
        let specs = registry.list().unwrap_or_else(|error| {
            tracing::warn!(%error, "couldn't read the device list");
            Vec::new()
        });
        let mut devices = Vec::new();
        for spec in specs {
            if let Ok(Some(device)) = registry.known_device(spec.id)
                && !devices.contains(&device)
            {
                devices.push(device);
            }
        }
        devices
    }
}

impl<C: Connector> Devices<C> {
    /// Takes this Mac's app key, which the client proves with already, and
    /// keeps what the app knows of it in the database from now on.
    pub fn keep_app_key(&self, keys: AppKeys) {
        keys.keep(Box::new(Kept(Arc::downgrade(&self.core))));
        if self.core.app_key.set(keys).is_err() {
            tracing::warn!("the devices already had this Mac's app key");
        }
    }

    /// This Mac's app key and how the devices stand with it.
    pub fn app_key(&self) -> Result<AppKeyState, CommandError> {
        Ok(self.app_keys()?.view())
    }

    /// The public key and label setup hands a new device, so it trusts
    /// this Mac from the start. None when the Mac has no app key.
    pub fn app_key_for_setup(&self) -> Option<(String, String)> {
        let keys = self.core.app_key.get()?;
        Some((keys.public_key(), keys.label().to_owned()))
    }

    /// Has `device` stop trusting an app key. This Mac's own key, and the
    /// one before it while devices move off it, stay.
    pub async fn remove_app_key(
        &self,
        device: DeviceId,
        public_key: String,
    ) -> Result<ActionOutcome, CommandError> {
        if self.app_keys()?.is_own(&public_key) {
            return Err(failed(
                "This Mac's own app key can't be removed here".into(),
            ));
        }
        let key = IdempotencyKey::generate();
        let action = ClientMessage::RemoveAppKey { key, public_key };
        self.act_on(device, action).await
    }

    /// Makes a new app key, and has each connected device move to it now.
    /// One that's offline moves when the app next reaches it.
    pub fn rotate_app_key(&self) -> Result<(), CommandError> {
        self.app_keys()?.rotate().map_err(failed)?;
        self.connector.reprove();
        Ok(())
    }

    fn app_keys(&self) -> Result<&AppKeys, CommandError> {
        self.core
            .app_key
            .get()
            .ok_or_else(|| failed("This Mac has no app key".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::tests::harness;
    use crate::keys::MemoryStore;
    use crate::registry::Registry;
    use crate::tests::block_on;

    #[test]
    fn the_key_waits_on_each_device_the_mac_reached_and_its_own_keys_stay() {
        let registry = Registry::in_memory().unwrap();
        let devbox = DeviceId::derive("devbox");
        let spec = registry.add("devbox").unwrap();
        registry.keep_device(spec.id, devbox).unwrap();
        registry.add("never-reached").unwrap();
        let h = harness(registry, None);
        assert_eq!(h.devices.app_key_for_setup(), None);
        let store = Box::new(MemoryStore::default());
        let keys = AppKeys::open(store, None, "MaxHQ on studio-mac".into()).unwrap();
        h.devices.keep_app_key(keys.clone());
        assert_eq!(
            h.devices.app_key_for_setup(),
            Some((keys.public_key(), "MaxHQ on studio-mac".into()))
        );
        let saved = || {
            lock(&h.devices.core.registry)
                .setting::<Record>(SETTING)
                .unwrap()
                .expect("the record is in the database")
        };
        let first = saved();

        let old = keys.public_key();
        h.devices.rotate_app_key().unwrap();
        let state = h.devices.app_key().unwrap();
        assert_ne!(state.public_key, old);
        assert_eq!(state.previous.as_deref(), Some(old.as_str()));
        assert_eq!(
            state.waiting,
            vec![devbox],
            "only a device the Mac reached waits"
        );
        assert_ne!(saved(), first);

        for own in [old, state.public_key] {
            let removed = block_on(h.devices.remove_app_key(devbox, own));
            assert!(
                matches!(removed, Err(CommandError::Failed { .. })),
                "{removed:?}"
            );
        }
    }
}

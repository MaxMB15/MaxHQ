// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The WSL distros on this Windows machine, listed on its device, so the
//! app offers each as a device of its own, reached through `maxd proxy
//! --wsl`. Elsewhere it lists none and the device says nothing.

use std::time::Duration;

use maxhq_protocol::model::WslDistro;
use maxhq_protocol::state::{Change, Entity, EntityKey};

use crate::hub::Hub;

/// How often it looks again, for a distro added or removed since.
const EVERY: Duration = Duration::from_secs(5 * 60);

/// Lists the distros now and every few minutes after, on Windows only.
pub async fn publish(hub: Hub) {
    if !cfg!(windows) {
        return;
    }
    loop {
        put(&hub, maxhq_client::wsl::list().await.distros());
        tokio::time::sleep(EVERY).await;
    }
}

/// Puts `distros` on this device, `None` when WSL isn't there, unless the
/// device says so already.
fn put(hub: &Hub, distros: Option<Vec<WslDistro>>) {
    let mut device = match hub.get(&EntityKey::Device(hub.device().id)) {
        Some(Entity::Device(device)) => device,
        _ => hub.device().clone(),
    };
    if device.wsl == distros {
        return;
    }
    device.wsl = distros;
    hub.apply([Change::Upsert {
        value: Entity::Device(device),
    }]);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use maxhq_protocol::ids::{BootId, DeviceId};
    use maxhq_protocol::model::Device;
    use uuid::Uuid;

    use super::*;

    fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId(Uuid::from_u128(1)),
                name: "pc".into(),
                os: "windows".into(),
                boot_id: BootId("test".into()),
                maxd_version: "0.0.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
                wsl: None,
            },
            HashMap::new(),
        )
    }

    fn listed(hub: &Hub) -> Option<Vec<WslDistro>> {
        match hub.get(&EntityKey::Device(hub.device().id)) {
            Some(Entity::Device(device)) => device.wsl,
            _ => None,
        }
    }

    #[test]
    fn the_device_lists_its_distros() {
        let hub = hub();
        let ubuntu = WslDistro {
            name: "Ubuntu".into(),
            default: true,
            version: Some(2),
        };
        put(&hub, Some(vec![ubuntu.clone()]));
        assert_eq!(listed(&hub), Some(vec![ubuntu]));
        put(&hub, Some(Vec::new()));
        assert_eq!(listed(&hub), Some(Vec::new()));
        put(&hub, None);
        assert_eq!(listed(&hub), None);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn only_windows_lists_distros() {
        let hub = hub();
        publish(hub.clone()).await;
        assert_eq!(listed(&hub), None);
    }
}

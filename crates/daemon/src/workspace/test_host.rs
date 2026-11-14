// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A test host's container, such as the `linux` one. It's made the way a
//! workspace is, from its own recipe, and kept stopped at its [`CLEAN`]
//! snapshot between leases. It's no workspace: it has no record, and the
//! rebuild passes over it.

use maxhq_core::names;
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::model::WorkspaceLocation;

use super::create::{egress_problem, IMAGE};
use super::exec::IncusExec;
use super::manager::Manager;
use crate::incus::{Instance, InstanceName, InstanceSpec, StateAction};
use crate::recipe::{self, Step, AGENT_HOME};

/// The key that marks a test host's container, set to its kind.
pub(super) const TEST_HOST_KEY: &str = "user.maxhq.test_host";

/// The snapshot each lease starts from.
pub(super) const CLEAN: &str = "clean";

impl Manager {
    /// The ID the test host `name`'s guest link goes by.
    pub fn test_link(&self, name: &InstanceName) -> WorkspaceId {
        names::workspace(
            self.inner.device,
            &WorkspaceLocation::Container {
                instance: name.to_string(),
                path: AGENT_HOME.to_owned(),
            },
        )
    }

    /// Makes the test host `name` of `kind` from nothing: a container
    /// that passes the egress check, with `steps` done, stopped at its
    /// [`CLEAN`] snapshot. An old test host of that name goes first; any
    /// other instance there stays, and the build fails.
    pub async fn build_test_host(
        &self,
        name: &InstanceName,
        kind: &str,
        steps: &[Step],
    ) -> Result<(), String> {
        let incus = self.incus();
        let timing = self.config().timing;
        self.config().links.close(self.test_link(name));
        if let Some(old) = self.test_host(name).await? {
            if old.is_running() {
                incus.force_stop(name).await.map_err(|e| e.to_string())?;
            }
            incus.delete(name).await.map_err(|e| e.to_string())?;
        }
        self.remove_socket(name);

        let (network, pool) = self.place().await?;
        let binary = self.guest_binary()?;
        self.ensure_guests_dir().map_err(|e| e.to_string())?;
        let caps = &self.config().caps;
        caps.check_parent()?;
        let mut config = caps.create_keys(name.as_str(), caps.memory(None));
        // A test host goes back to its clean snapshot between leases, so
        // Incus's own schedule has nothing to keep.
        config.retain(|key, _| !key.starts_with(super::caps::SNAPSHOT_KEYS));
        config.insert(TEST_HOST_KEY.to_owned(), kind.to_owned());
        let spec = InstanceSpec {
            image: IMAGE.to_owned(),
            config,
            pool,
            network: network.clone(),
            guest_socket: self.socket(name),
        };
        incus.create(name, spec).await.map_err(|e| e.to_string())?;
        incus
            .set_state(name, StateAction::Start, timing.state_timeout)
            .await
            .map_err(|e| e.to_string())?;
        let exec = IncusExec::new(incus.clone(), name.clone());
        self.wait_ready(&exec).await?;
        super::install::install(incus, name, &binary)
            .await
            .map_err(|e| e.to_string())?;
        let (check, note) = self.egress(&exec, &network).await;
        if let Some(problem) = egress_problem(note.as_deref(), &check) {
            return Err(problem);
        }
        recipe::run(&exec, steps).await.map_err(|e| e.to_string())?;
        incus
            .set_state(name, StateAction::Stop, timing.state_timeout)
            .await
            .map_err(|e| e.to_string())?;
        incus.snapshot(name, CLEAN).await.map_err(|e| e.to_string())
    }

    /// Starts the test host `name` and waits for its guest to link.
    pub async fn start_test_host(&self, name: &InstanceName) -> Result<(), String> {
        self.made_test_host(name).await?;
        let incus = self.incus();
        incus
            .set_state(name, StateAction::Start, self.config().timing.state_timeout)
            .await
            .map_err(|e| e.to_string())?;
        let links = &self.config().links;
        let link = self.test_link(name);
        links.open(link, name, None);
        match links.wait_up(link, self.config().timing.guest_wait).await? {
            Some(_) => Ok(()),
            None => Err("this maxd doesn't link to guests yet".to_owned()),
        }
    }

    /// Stops the test host `name` and takes it back to its [`CLEAN`]
    /// snapshot.
    pub async fn clean_test_host(&self, name: &InstanceName) -> Result<(), String> {
        let instance = self.made_test_host(name).await?;
        let incus = self.incus();
        self.config().links.close(self.test_link(name));
        if instance.is_running() {
            incus.force_stop(name).await.map_err(|e| e.to_string())?;
        }
        incus.restore(name, CLEAN).await.map_err(|e| e.to_string())
    }

    /// The test host `name`, which a build made.
    async fn made_test_host(&self, name: &InstanceName) -> Result<Instance, String> {
        self.test_host(name)
            .await?
            .ok_or_else(|| format!("{name} isn't there. Build it again."))
    }

    /// The test host `name`, or none when nothing is there. Any other
    /// instance of that name is an error, so it's left alone.
    async fn test_host(&self, name: &InstanceName) -> Result<Option<Instance>, String> {
        match self.incus().instance(name).await {
            Ok(Some(instance)) if instance.is_managed() && is_test_host(&instance) => {
                Ok(Some(instance))
            }
            Ok(Some(_)) => Err(format!(
                "{name} is already there and isn't a test host this maxd made, so it stays."
            )),
            Ok(None) => Ok(None),
            Err(error) => Err(format!("Incus doesn't answer on this device: {error}")),
        }
    }
}

/// True for a test host's container.
pub(super) fn is_test_host(instance: &Instance) -> bool {
    instance.key(TEST_HOST_KEY).is_some()
}

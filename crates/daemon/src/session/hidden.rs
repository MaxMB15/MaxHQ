// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The projects you hid from the sidebar. Clients hide a project on the
//! Mac's `maxd`, so the list lives with the rest of the sidebar's state.
//! Every `maxd` publishes its list, even an empty one, which tells a
//! client the device takes `SetProjectHidden`. Clients decide when a
//! hidden project shows anyway, since that follows its agents on every
//! device.

use maxhq_protocol::ids::{DeviceId, ProjectId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::HiddenProjects;
use maxhq_protocol::state::{Change, Entity};
use tracing::warn;

use super::{refused, Manager};

impl Manager {
    /// Shows the list the previous run stored.
    pub(super) fn load_hidden(&self) {
        let projects = self.db.hidden_projects().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the hidden projects");
            Vec::new()
        });
        self.show_hidden(projects);
    }

    /// Answers `SetProjectHidden`: stores the change and shows the list.
    pub(super) fn set_project_hidden(
        &self,
        device: DeviceId,
        project: ProjectId,
        hidden: bool,
    ) -> ActionOutcome {
        if device != self.device {
            return refused("that list is on another device");
        }
        if let Err(err) = self.db.set_project_hidden(project, hidden) {
            return refused(format!("couldn't store the hidden projects: {err}"));
        }
        match self.db.hidden_projects() {
            Ok(projects) => self.show_hidden(projects),
            Err(err) => return refused(format!("couldn't read the hidden projects: {err}")),
        }
        ActionOutcome::Done
    }

    /// Publishes this device's list.
    fn show_hidden(&self, projects: Vec<ProjectId>) {
        self.hub.apply([Change::Upsert {
            value: Entity::HiddenProjects(HiddenProjects {
                device: self.device,
                projects,
            }),
        }]);
    }
}

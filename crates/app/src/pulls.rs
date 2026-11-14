// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pull requests. A project's list goes to a device with a checkout of
//! it, the viewer's reads and `PullsDue` to the device the caller names,
//! a link change to the agent's device, and a merge or an approval to the
//! device holding your GitHub sign-in. Each comes back as the device's
//! outcome, for the window to read.

use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, ProjectId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::CredentialKind;
use maxhq_protocol::pulls::{PullAction, PullListState};
use maxhq_protocol::state::Entity;
use maxhq_protocol::ClientMessage;

use crate::devices::{agent_device, capabilities, device_links, is_local, Connector};
use crate::fleet::{check_can, Needs};
use crate::merge::Merge;
use crate::request::CommandError;
use crate::App;

/// Why a merge or an approval has nowhere to go.
const NO_SIGN_IN: &str = "Sign in to GitHub in Settings to merge and approve here.";

impl<C: Connector> App<C> {
    /// A project's open or closed PRs, from the device that reads them.
    pub async fn list_pulls(
        &self,
        project: ProjectId,
        state: PullListState,
    ) -> Result<ActionOutcome, CommandError> {
        self.act_on_project(project, |key| ClientMessage::ListPulls {
            key,
            project,
            state,
        })
        .await
    }

    /// The branches of a project's repository, from the device that reads
    /// its PRs.
    pub async fn list_branches(&self, project: ProjectId) -> Result<ActionOutcome, CommandError> {
        self.act_on_project(project, |key| ClientMessage::ListBranches { key, project })
            .await
    }

    /// Sends a read of a project's repository to the device that reads its
    /// PRs, under a fresh key.
    async fn act_on_project(
        &self,
        project: ProjectId,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        let device = self.view.read(|merge| {
            let device = project_device(merge, project)?;
            check_can(merge, device, Needs::Pulls)?;
            Ok::<_, CommandError>(device)
        })?;
        self.devices
            .act_on(device, action(IdempotencyKey::generate()))
            .await
    }

    /// Sends a PR read or `PullsDue` to `device` under a fresh key, once
    /// its `maxd` reads PRs.
    pub async fn act_on_pulls(
        &self,
        device: DeviceId,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        self.act_needing(device, Needs::Pulls, action).await
    }

    /// Sends a link change to the device the agent runs on.
    pub async fn act_on_pull_link(
        &self,
        agent: AgentId,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        let device = self.view.read(|merge| {
            let device = agent_device(merge, agent).ok_or_else(|| CommandError::NotFound {
                what: "This agent".to_owned(),
            })?;
            check_can(merge, device, Needs::Pulls)?;
            Ok::<_, CommandError>(device)
        })?;
        self.devices
            .act_on(device, action(IdempotencyKey::generate()))
            .await
    }

    /// Merges or approves a PR as you, on the device holding your GitHub
    /// sign-in, if the head is still `expect_sha`.
    pub async fn pull_action(
        &self,
        repo: String,
        number: u64,
        action: PullAction,
        expect_sha: String,
    ) -> Result<ActionOutcome, CommandError> {
        let holder = self.view.read(|merge| {
            let holder = github_holder(merge).ok_or_else(|| CommandError::Failed {
                message: NO_SIGN_IN.to_owned(),
            })?;
            check_can(merge, holder, Needs::GithubUser)?;
            Ok::<_, CommandError>(holder)
        })?;
        let action = ClientMessage::PullAction {
            key: IdempotencyKey::generate(),
            repo,
            number,
            action,
            expect_sha,
        };
        self.devices.act_on(holder, action).await
    }

    /// Sends a sign-in step to `device`, once its `maxd` holds a GitHub
    /// sign-in.
    pub async fn act_on_github_sign_in(
        &self,
        device: DeviceId,
        action: impl FnOnce(IdempotencyKey) -> ClientMessage,
    ) -> Result<ActionOutcome, CommandError> {
        self.act_needing(device, Needs::GithubUser, action).await
    }
}

/// The device that reads a project's PRs: of those with a checkout of it,
/// a connected one first, then one whose `maxd` reads PRs, then one with a
/// GitHub App, since reads go through the App, then this machine. A device
/// picks its own ID, so that alone never decides over this machine.
fn project_device(merge: &Merge, project: ProjectId) -> Result<DeviceId, CommandError> {
    let mut devices: Vec<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::Workspace(workspace) if workspace.project == Some(project) => {
                Some(workspace.device)
            }
            _ => None,
        })
        .collect();
    devices.sort_by_key(|&device| {
        (
            !connected(merge, device),
            !capabilities(merge, device).is_some_and(|does| does.pulls),
            !holds(merge, device, |kind| {
                matches!(kind, CredentialKind::GithubApp { .. })
            }),
            !is_local(merge, device),
            device,
        )
    });
    devices
        .first()
        .copied()
        .ok_or_else(|| CommandError::NotFound {
            what: "A device with this project".to_owned(),
        })
}

/// The device holding your GitHub sign-in, a connected one first, then this
/// machine.
fn github_holder(merge: &Merge) -> Option<DeviceId> {
    let mut holders: Vec<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::Credential(credential)
                if matches!(credential.kind, CredentialKind::GithubUser { .. }) =>
            {
                Some(credential.device)
            }
            _ => None,
        })
        .collect();
    holders.sort_by_key(|&device| (!connected(merge, device), !is_local(merge, device), device));
    holders.first().copied()
}

/// Whether a link to the device is connected.
fn connected(merge: &Merge, device: DeviceId) -> bool {
    device_links(merge, device).any(|link| matches!(link.state, LinkState::Connected { .. }))
}

/// Whether the device holds a credential of a kind `is` picks.
fn holds(merge: &Merge, device: DeviceId, is: impl Fn(&CredentialKind) -> bool) -> bool {
    merge.store().entities().any(|entity| {
        matches!(entity, Entity::Credential(credential)
            if credential.device == device && is(&credential.kind))
    })
}

#[cfg(test)]
mod tests {
    use maxhq_client::{LinkEvent, LinkSpec};
    use maxhq_protocol::ids::{CredentialId, LinkId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::Credential;
    use maxhq_protocol::pulls::MergeMethod;
    use maxhq_protocol::state::Change;
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::devices::tests::{connect_doing, disconnect, on, two_devices, Call, Fake, Harness};
    use crate::merge::tests::workspace;
    use crate::tests::{block_on, FakeShell};

    /// What a `maxd` with PRs and a GitHub sign-in does.
    fn pulls() -> Capabilities {
        Capabilities {
            pulls: true,
            github_user: true,
            ..Capabilities::default()
        }
    }

    /// Has `link` publish a credential of `kind` on `device`.
    fn credential(h: &Harness, link: &LinkSpec, device: &str, kind: CredentialKind) {
        let value = Entity::Credential(Credential {
            id: CredentialId::derive(device),
            device: DeviceId::derive(device),
            kind,
            stored_at: Timestamp(1),
            problem: None,
        });
        h.report(link.id, LinkEvent::Changed(vec![Change::Upsert { value }]));
    }

    /// Your GitHub sign-in.
    fn user() -> CredentialKind {
        CredentialKind::GithubUser {
            login: "octo".to_owned(),
        }
    }

    /// The app over this machine and devbox, both with project "p" and
    /// both reading PRs, with a GitHub App on devbox, after `setup`.
    fn app(
        setup: impl FnOnce(&Harness, &LinkSpec, &LinkSpec),
    ) -> (App<Fake>, Fake, LinkSpec, LinkSpec) {
        let (h, local, nl) = two_devices();
        connect_doing(&h, &local, "mac", pulls());
        connect_doing(&h, &nl, "nl", pulls());
        let app_kind = CredentialKind::GithubApp {
            app_id: 1,
            slug: None,
            key_sha256: "00".to_owned(),
            repos: vec!["owner/repo".to_owned()],
        };
        credential(&h, &nl, "nl", app_kind);
        setup(&h, &local, &nl);
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        (
            App::new(view, devices, FakeShell::default()),
            fake,
            local,
            nl,
        )
    }

    /// Each message sent, with the link it went through.
    fn sent(fake: &Fake) -> Vec<(LinkId, ClientMessage)> {
        fake.calls()
            .into_iter()
            .filter_map(|(link, call)| match call {
                Call::Send(message) => Some((link, message)),
                _ => None,
            })
            .collect()
    }

    /// A squash merge of owner/repo#7 at head "abc".
    fn merge(app: &App<Fake>) -> Result<ActionOutcome, CommandError> {
        let action = PullAction::Merge {
            method: MergeMethod::Squash,
        };
        block_on(app.pull_action("owner/repo".to_owned(), 7, action, "abc".to_owned()))
    }

    #[test]
    fn this_machine_comes_before_a_device_whose_id_sorts_first() {
        // A remote ID that sorts first, so only the check for this machine
        // keeps its work here.
        let remote = (0..)
            .map(|n| format!("remote-{n}"))
            .find(|name| DeviceId::derive(name) < DeviceId::derive("mac"))
            .unwrap();
        let registry = crate::registry::Registry::in_memory().unwrap();
        let link = registry.add("devbox").unwrap();
        let h = crate::devices::tests::harness(registry, None);
        let local = h.local();
        connect_doing(&h, &local, "mac", pulls());
        connect_doing(&h, &link, &remote, pulls());
        h.report(
            local.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: on("mac", workspace("here", "p")),
            }]),
        );
        h.report(
            link.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: on(&remote, workspace("there", "p")),
            }]),
        );
        credential(&h, &link, &remote, user());
        credential(&h, &local, "mac", user());
        let Harness {
            devices,
            view,
            fake,
            ..
        } = h;
        drop(fake.calls());
        let app = App::new(view, devices, FakeShell::default());

        block_on(app.list_pulls(ProjectId::derive("p"), PullListState::Open)).unwrap();
        merge(&app).unwrap();
        let calls = sent(&fake);
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls.iter().all(|(sent_on, _)| *sent_on == local.id));
    }

    #[test]
    fn a_list_goes_to_the_project_s_device_with_the_app() {
        let (app, fake, _, nl) = app(|_, _, _| {});
        let project = ProjectId::derive("p");
        block_on(app.list_pulls(project, PullListState::Open)).unwrap();
        let calls = sent(&fake);
        let [(link, ClientMessage::ListPulls { project: asked, .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, *asked), (nl.id, project));

        let missing = block_on(app.list_pulls(ProjectId::derive("none"), PullListState::Open));
        assert!(matches!(missing, Err(CommandError::NotFound { .. })));
    }

    #[test]
    fn a_branch_list_goes_where_the_project_s_prs_are_read() {
        let (app, fake, _, nl) = app(|_, _, _| {});
        let project = ProjectId::derive("p");
        block_on(app.list_branches(project)).unwrap();
        let calls = sent(&fake);
        let [(link, ClientMessage::ListBranches { project: asked, .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, *asked), (nl.id, project));

        let missing = block_on(app.list_branches(ProjectId::derive("none")));
        assert!(matches!(missing, Err(CommandError::NotFound { .. })));
    }

    #[test]
    fn a_list_skips_a_device_that_is_offline() {
        let (app, fake, local, _) = app(|h, _, nl| disconnect(h, nl, "nl"));
        block_on(app.list_pulls(ProjectId::derive("p"), PullListState::Closed)).unwrap();
        let calls = sent(&fake);
        let [(link, ClientMessage::ListPulls { .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!(*link, local.id);
    }

    #[test]
    fn reads_go_to_the_device_named_and_links_to_the_agent_s() {
        let (app, fake, local, nl) = app(|_, _, _| {});
        let read = app.act_on_pulls(DeviceId::derive("mac"), |key| ClientMessage::PullChecks {
            key,
            repo: "owner/repo".to_owned(),
            number: 7,
        });
        block_on(read).unwrap();
        let link = app.act_on_pull_link(AgentId::derive("b"), |key| ClientMessage::LinkPull {
            key,
            agent: AgentId::derive("b"),
            number: 7,
        });
        block_on(link).unwrap();
        let calls = sent(&fake);
        let [(read, ClientMessage::PullChecks { .. }), (linked, ClientMessage::LinkPull { .. })] =
            calls.as_slice()
        else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*read, *linked), (local.id, nl.id));

        let gone = app.act_on_pull_link(AgentId::derive("none"), |key| ClientMessage::UnlinkPull {
            key,
            agent: AgentId::derive("none"),
            repo: "owner/repo".to_owned(),
            number: 7,
        });
        assert!(matches!(block_on(gone), Err(CommandError::NotFound { .. })));
    }

    #[test]
    fn a_merge_goes_to_the_sign_in_s_holder() {
        let (unsigned, fake, _, _) = app(|_, _, _| {});
        assert_eq!(merge(&unsigned).unwrap_err().to_string(), NO_SIGN_IN);
        assert!(sent(&fake).is_empty());

        let (app, fake, local, _) = app(|h, local, _| credential(h, local, "mac", user()));
        fake.record()
            .acts
            .push_back(ActionOutcome::PullMerged { queued: false });
        assert_eq!(merge(&app), Ok(ActionOutcome::PullMerged { queued: false }));
        let calls = sent(&fake);
        let [(link, ClientMessage::PullAction { expect_sha, .. })] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!((*link, expect_sha.as_str()), (local.id, "abc"));
    }

    #[test]
    fn an_old_maxd_is_refused_in_words() {
        let (app, fake, _, _) = app(|h, local, _| {
            connect_doing(h, local, "mac", Capabilities::default());
            credential(h, local, "mac", user());
        });
        // The local device's name differs by platform.
        let refused = merge(&app).unwrap_err().to_string();
        assert!(
            refused.ends_with("'s maxd is too old to hold your GitHub sign-in. Update it first")
        );
        let begin = app.act_on_github_sign_in(DeviceId::derive("mac"), |key| {
            ClientMessage::BeginGithubSignIn {
                key,
                client_id: None,
            }
        });
        assert!(block_on(begin).is_err());
        let due = app.act_on_pulls(DeviceId::derive("mac"), |key| ClientMessage::PullsDue {
            key,
            repo: "owner/repo".to_owned(),
        });
        let refused = block_on(due).unwrap_err().to_string();
        assert!(refused.ends_with("'s maxd is too old to read pull requests. Update it first"));
        assert!(sent(&fake).is_empty());
    }
}

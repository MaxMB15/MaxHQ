// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Setting a device up from the app: the client prepares it over your own
//! login, then the app adds it, waits for its hello, keeps its host key
//! under the device's alias and sends it the logins you ticked. Secrets go
//! over the link once it's up, never on a command line.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use maxhq_client::setup::{self as prepare, Confirmations, Setup, SetupBinary, Step, System};
use maxhq_protocol::ids::IdempotencyKey;
use maxhq_protocol::ids::{AccountId, DeviceId, LinkId};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::message::{ActionOutcome, ClientMessage, UpdateKind};
use maxhq_protocol::model::DeviceSettingChange;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::{Entity, EntityKey};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::devices::Connector;
use crate::fleet::{Binaries, BinaryDirs};
use crate::merge::Merge;
use crate::request::{ActionReply, CommandError, GithubAppId, Request, Response};
use crate::{App, Caller};

mod claim;

pub use claim::{ClaimedLink, LinkClaimDetails};

/// How long a newly set-up device has to say hello.
const HELLO_WITHIN: Duration = Duration::from_secs(60);

/// Host keys waiting for your OK, across every setup running.
static CONFIRMS: LazyLock<Confirmations> = LazyLock::new(Confirmations::default);

/// Which question each setup waits on, by the setup's name.
static ASKING: LazyLock<Mutex<BTreeMap<String, u32>>> = LazyLock::new(Default::default);

/// Numbers the setups.
static SETUPS: AtomicU32 = AtomicU32::new(0);

fn asking() -> std::sync::MutexGuard<'static, BTreeMap<String, u32>> {
    ASKING.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Answers the host key question setup `setup` waits on. False when it
/// waits on none.
pub fn confirm_host_key(setup: &str, ok: bool) -> bool {
    let Some(id) = asking().remove(setup) else {
        return false;
    };
    CONFIRMS.answer(id, ok)
}

/// Where a host setup offers came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SetupHostFrom {
    SshConfig,
    Tailnet,
}

/// A host setup offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct SetupHost {
    pub host: String,
    pub from: SetupHostFrom,
    /// Its address, when the app knows it apart from the name.
    pub address: Option<String>,
    /// The link that reaches it already, if any.
    pub link: Option<LinkId>,
}

/// The hosts in your SSH config, then the `tailnet` peers it doesn't
/// name, each with the link that reaches it already.
pub fn list_hosts(merge: &Merge, tailnet: Vec<String>) -> Vec<SetupHost> {
    let config = ssh_config()
        .map(|text| prepare::config_hosts(&text))
        .unwrap_or_default();
    let mut hosts: Vec<SetupHost> = Vec::new();
    let offered = config
        .into_iter()
        .map(|host| (host, SetupHostFrom::SshConfig))
        .chain(
            tailnet
                .into_iter()
                .map(|host| (host, SetupHostFrom::Tailnet)),
        );
    for (host, from) in offered {
        if hosts.iter().any(|known| known.host == host) {
            continue;
        }
        hosts.push(SetupHost {
            link: link_to(merge, &host),
            host,
            from,
            address: None,
        });
    }
    hosts
}

/// The link whose routes include `host`.
fn link_to(merge: &Merge, host: &str) -> Option<LinkId> {
    let is_host = |route: &Route| matches!(route, Route::Ssh { host: at } if at == host);
    merge.store().entities().find_map(|entity| match entity {
        Entity::DeviceLink(link)
            if is_host(&link.route) || link.routes.iter().any(|c| is_host(&c.route)) =>
        {
            Some(link.id)
        }
        _ => None,
    })
}

/// Your SSH config, read only for its `Host` lines. Tests never read it.
fn ssh_config() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let home = directories::BaseDirs::new()?.home_dir().to_owned();
    std::fs::read_to_string(home.join(".ssh").join("config")).ok()
}

/// A login to hand a device once it's set up.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "secret", rename_all = "snake_case")]
pub enum SetupSecret {
    /// A Claude setup-token, as `StoreClaudeToken` takes it. Sending one
    /// turns setup-tokens on for the device first, so the wizard asks you
    /// before it offers one.
    ClaudeToken {
        origin: AccountId,
        label: String,
        token: Secret,
    },
    /// A GitHub App, its private key read from `key_path`.
    GithubApp {
        app_id: GithubAppId,
        key_path: String,
    },
}

/// The stages of a setup, as the wizard shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    Pick,
    HostKey,
    Install,
    Key,
    Handshake,
    Secrets,
}

/// What a setup is doing. `setup` names the run.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SetupProgress {
    /// A stage started.
    Step {
        setup: String,
        step: SetupStep,
        message: String,
    },
    /// A host key ssh hasn't seen waits for your OK. Answer with
    /// `ConfirmHostKey`. Nothing runs on the host before.
    HostKey {
        setup: String,
        host: String,
        key_type: String,
        fingerprint: String,
    },
    /// Something for you to do there yourself.
    Hint {
        setup: String,
        step: SetupStep,
        text: String,
        command: String,
    },
    /// Set up.
    Done {
        setup: String,
        link: LinkId,
        device: DeviceId,
    },
    /// Stopped.
    Failed {
        setup: String,
        step: SetupStep,
        reason: String,
    },
}

/// The key type and fingerprint in an `ssh-keygen -l` line, such as
/// `256 SHA256:abc devbox (ED25519)`.
fn fingerprint_parts(line: &str) -> (String, String) {
    let fingerprint = line
        .split_whitespace()
        .find(|word| word.contains(':'))
        .unwrap_or(line)
        .to_owned();
    let key_type = line
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or("")
        .to_owned();
    (key_type, fingerprint)
}

/// Reports one setup's progress and remembers its stage.
struct Run<'a> {
    setup: String,
    host: String,
    at: Mutex<SetupStep>,
    report: &'a (dyn Fn(SetupProgress) + Sync),
}

impl Run<'_> {
    fn step(&self, step: SetupStep, message: impl Into<String>) {
        *self.at.lock().unwrap_or_else(|p| p.into_inner()) = step;
        (self.report)(SetupProgress::Step {
            setup: self.setup.clone(),
            step,
            message: message.into(),
        });
    }

    fn at(&self) -> SetupStep {
        *self.at.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Shows what the client did.
    fn client(&self, step: Step) {
        match step {
            Step::HostKey => self.step(
                SetupStep::HostKey,
                format!("Reading {}'s host key", self.host),
            ),
            Step::ConfirmHostKey { id, fingerprints } => {
                asking().insert(self.setup.clone(), id);
                let first = fingerprints.first().map_or("", String::as_str);
                let (key_type, fingerprint) = fingerprint_parts(first);
                (self.report)(SetupProgress::HostKey {
                    setup: self.setup.clone(),
                    host: self.host.clone(),
                    key_type,
                    fingerprint,
                });
            }
            Step::Probe => self.step(SetupStep::Install, "Checking the system"),
            Step::Install => self.step(SetupStep::Install, "Copying maxd"),
            Step::Linger { command } => (self.report)(SetupProgress::Hint {
                setup: self.setup.clone(),
                step: SetupStep::Install,
                text: "maxd can't keep running after you log out until lingering is on.".to_owned(),
                command,
            }),
            Step::Key { .. } => self.step(SetupStep::Key, "Added MaxHQ's key"),
        }
    }
}

/// The device a link reached, once it's connected.
fn connected_device(merge: &Merge, link: LinkId) -> Option<DeviceId> {
    match merge.store().get(&EntityKey::DeviceLink(link)) {
        Some(Entity::DeviceLink(found)) if matches!(found.state, LinkState::Connected { .. }) => {
            found.device
        }
        _ => None,
    }
}

/// Why `link` is down, such as a device that reports a different ID.
fn down_reason(merge: &Merge, link: LinkId) -> Option<String> {
    match merge.store().get(&EntityKey::DeviceLink(link)) {
        Some(Entity::DeviceLink(found)) => match &found.state {
            LinkState::Unreachable { reason, .. } => Some(reason.clone()),
            _ => None,
        },
        _ => None,
    }
}

impl<C: Connector> App<C> {
    /// Sets `host` up as a device and hands it `secrets`, and gives the
    /// link it got. `report` hears each step, ending with done or failed.
    pub async fn setup_device(
        &self,
        host: String,
        secrets: Vec<SetupSecret>,
        report: &(dyn Fn(SetupProgress) + Sync),
    ) -> Result<LinkId, CommandError> {
        let setup = format!("setup-{}", SETUPS.fetch_add(1, Ordering::Relaxed) + 1);
        let run = Run {
            setup: setup.clone(),
            host: host.clone(),
            at: Mutex::new(SetupStep::Pick),
            report,
        };
        let result = self.set_up(&run, secrets).await;
        asking().remove(&setup);
        match result {
            Ok((link, device)) => {
                report(SetupProgress::Done {
                    setup,
                    link,
                    device,
                });
                Ok(link)
            }
            Err(reason) => {
                report(SetupProgress::Failed {
                    setup,
                    step: run.at(),
                    reason: reason.clone(),
                });
                Err(CommandError::Failed { message: reason })
            }
        }
    }

    async fn set_up(
        &self,
        run: &Run<'_>,
        secrets: Vec<SetupSecret>,
    ) -> Result<(LinkId, DeviceId), String> {
        let paths = prepare::paths().ok_or("This Mac has no Application Support folder")?;
        let binaries = BinaryDirs::for_app();
        let binary = |target: &str| {
            binaries
                .find(UpdateKind::Maxd, target)
                .map(|found| SetupBinary {
                    path: found.path,
                    sha256: found.sha256,
                })
        };
        let mut setup = Setup::new(&System, &paths, &run.host).map_err(|error| error.0)?;
        // A device that trusts no app key yet takes this Mac's from setup,
        // so the app's first connection needs no card.
        if let Some((public_key, label)) = self.devices.app_key_for_setup() {
            setup = setup
                .with_app_key(&public_key, &label)
                .map_err(|error| error.0)?;
        }
        let prepared = setup
            .prepare(binary, &CONFIRMS, &|done| run.client(done))
            .await
            .map_err(|error| error.0)?;

        run.step(SetupStep::Handshake, "Connecting to maxd");
        let link = self
            .devices
            .add_device(&prepared.host)
            .map_err(|error| error.to_string())?;
        claim::bring(link, &prepared.host_keys);
        let device = self
            .vouched_device(run, &paths.known_hosts, link, &prepared.host_keys)
            .await;
        claim::done(link);
        let device = device?;
        // Keys you vouched for take over from the old ones, so a device with
        // a new key links again. A hello can say any device ID, so one that
        // another link reaches keeps its pin.
        let name = device.to_string();
        let kept =
            if prepared.host_keys_confirmed && !self.devices.another_link_reaches(device, link) {
                prepare::replace_host_keys(&paths.known_hosts, &name, &prepared.host_keys)
            } else {
                prepare::record_host_keys(&paths.known_hosts, &name, &prepared.host_keys)
            };
        if let Err(error) = kept {
            tracing::warn!(%error, "couldn't keep the device's host key");
        }
        self.devices.host_keys_changed();

        if !secrets.is_empty() {
            run.step(SetupStep::Secrets, "Sending the logins you picked");
            let failed = self.hand_over(device, secrets).await;
            if !failed.is_empty() {
                return Err(failed.join(". "));
            }
        }
        Ok((link, device))
    }

    /// Turns setup-tokens on for `device`, which you agreed to by sending
    /// it one. A `maxd` too old to read the change always takes them.
    async fn take_setup_tokens(&self, device: DeviceId) -> Result<(), String> {
        let on = ClientMessage::UpdateDeviceSettings {
            key: IdempotencyKey::generate(),
            device,
            changes: vec![DeviceSettingChange::ClaudeSetupToken { value: true }],
        };
        match self.devices.act_on_unless_old(device, on).await {
            Ok(None | Some(ActionOutcome::Done)) => Ok(()),
            Ok(Some(ActionOutcome::Refused { reason })) => Err(reason),
            Ok(Some(other)) => Err(format!("{other:?}")),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Sends each login to `device` over its link, and says which failed.
    async fn hand_over(&self, device: DeviceId, secrets: Vec<SetupSecret>) -> Vec<String> {
        let caller = Caller {
            window: "setup".into(),
            on_screen: false,
        };
        let mut failed = Vec::new();
        for secret in secrets {
            let (what, request) = match secret {
                SetupSecret::ClaudeToken {
                    origin,
                    label,
                    token,
                } => {
                    if let Err(error) = self.take_setup_tokens(device).await {
                        failed.push(format!("The Claude login didn't reach the device: {error}"));
                        continue;
                    }
                    (
                        "The Claude login",
                        Request::StoreClaudeToken {
                            devices: vec![device],
                            origin,
                            label,
                            token,
                        },
                    )
                }
                SetupSecret::GithubApp { app_id, key_path } => (
                    "The GitHub App",
                    Request::SetGithubApp {
                        devices: vec![device],
                        app_id,
                        key_path,
                    },
                ),
            };
            let replies = match self.request(&caller, request).await {
                Ok(Response::StoreClaudeToken(replies) | Response::SetGithubApp(replies)) => {
                    replies
                }
                Ok(_) => Vec::new(),
                Err(error) => {
                    failed.push(format!("{what} didn't reach the device: {error}"));
                    continue;
                }
            };
            for reply in replies {
                if let ActionReply::Error { error } = reply {
                    failed.push(format!("{what} didn't reach the device: {error}"));
                }
            }
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_that_gives_up_says_why_the_link_is_down() {
        use crate::merge::tests::{connected, link_state, spec};

        let spec = spec("devbox");
        let mut merge = Merge::new();
        merge.add_link(&spec);
        assert_eq!(down_reason(&merge, spec.id), None);

        let refused = LinkState::Unreachable {
            reason: "devbox reports a different device ID".to_owned(),
            last_seen: None,
        };
        merge.link_event(spec.id, link_state(&spec, refused));
        assert_eq!(
            down_reason(&merge, spec.id).as_deref(),
            Some("devbox reports a different device ID")
        );

        merge.link_event(spec.id, link_state(&spec, connected()));
        assert_eq!(down_reason(&merge, spec.id), None);
    }

    #[test]
    fn a_setup_nobody_asked_about_answers_false() {
        assert!(!confirm_host_key("setup-0", true));
    }

    #[test]
    fn a_fingerprint_line_gives_its_type_and_hash() {
        assert_eq!(
            fingerprint_parts("256 SHA256:abcDEF devbox (ED25519)"),
            ("ED25519".to_owned(), "SHA256:abcDEF".to_owned())
        );
    }

    #[test]
    fn a_secret_reads_from_the_wire_and_never_shows_in_a_log_line() {
        let secret: SetupSecret = serde_json::from_value(serde_json::json!({
            "secret": "claude_token",
            "origin": AccountId::derive("a"),
            "label": "Work",
            "token": "test-not-a-real-token",
        }))
        .unwrap();
        assert!(!format!("{secret:?}").contains("test-not-a-real-token"));
    }
}

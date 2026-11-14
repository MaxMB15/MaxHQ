// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! SSH with every choice pinned: MaxHQ's own key, its own `known_hosts`
//! and a host key alias per device, so nothing in your `~/.ssh/config`
//! changes where a login goes or which host key it trusts. `maxd`'s peer
//! links dial through it, and any other login to a set-up device builds
//! the same options from it.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ssh_config::is_safe_destination;

/// One pinned SSH login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedSsh {
    /// The host name or address.
    pub host: String,
    /// The SSH port.
    pub port: u16,
    /// The login.
    pub user: String,
    /// The private key, which is the only one offered.
    pub key_file: PathBuf,
    /// The `known_hosts` that holds the host's key, under `alias`.
    pub known_hosts: PathBuf,
    /// The name the host key is kept under, from
    /// [`crate::setup::host_key_alias`].
    pub alias: String,
}

impl PinnedSsh {
    /// The ssh options, in order. [`PinnedSsh::args`] and
    /// [`PinnedSsh::config`] both come from them, so a command and a
    /// config file never differ.
    pub fn options(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Port", self.port.to_string()),
            ("User", self.user.clone()),
            ("IdentityFile", self.key_file.display().to_string()),
            ("IdentitiesOnly", "yes".into()),
            ("BatchMode", "yes".into()),
            ("StrictHostKeyChecking", "yes".into()),
            ("UserKnownHostsFile", self.known_hosts.display().to_string()),
            ("HostKeyAlias", self.alias.clone()),
        ]
    }

    /// The arguments for `ssh` up to the host, with no config file read.
    /// A remote command goes after them.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["-F".to_owned(), "none".to_owned()];
        for (name, value) in self.options() {
            args.push("-o".into());
            args.push(format!("{name}={value}"));
        }
        args.push("--".into());
        args.push(self.host.clone());
        args
    }

    /// The same options as a config file, for a tool that takes `-F` and
    /// no `-o`, such as a ControlMaster's builder.
    pub fn config(&self) -> String {
        let mut text = String::from("Host *\n");
        for (name, value) in self.options() {
            text.push_str(&format!("  {name} \"{value}\"\n"));
        }
        text
    }

    /// Whether every part is plain enough to go in a command or a config
    /// file unchanged.
    pub fn is_plain(&self) -> bool {
        let plain = |text: &str| {
            !text.is_empty()
                && !text
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\'))
        };
        is_safe_destination(&self.host)
            && plain(&self.host)
            && plain(&self.user)
            && !self.user.starts_with('-')
            && plain(&self.alias)
            && plain(&self.key_file.display().to_string())
            && plain(&self.known_hosts.display().to_string())
    }
}

/// The pinned logins a peer client dials, by the route host that names
/// each. Whoever holds a clone can replace them, and the next dial uses
/// the new ones.
#[derive(Clone, Debug, Default)]
pub struct Pins(Arc<RwLock<HashMap<String, PinnedSsh>>>);

impl Pins {
    /// Replaces every pin.
    pub fn set(&self, pins: HashMap<String, PinnedSsh>) {
        *self.0.write().expect("pins lock") = pins;
    }

    /// The pin for a route host.
    #[cfg(feature = "system-ssh")]
    pub(crate) fn get(&self, host: &str) -> Option<PinnedSsh> {
        self.0.read().expect("pins lock").get(host).cloned()
    }
}

/// A stream to a `maxd`: its reading half and its writing half.
pub type PeerStream = (
    Box<dyn AsyncRead + Send + Unpin>,
    Box<dyn AsyncWrite + Send + Unpin>,
);

/// Opens streams for a peer client in place of pinned SSH, which is how
/// tests link two `maxd` in one process.
pub trait PeerDial: Send + Sync + 'static {
    /// A stream to the `maxd` behind a route host.
    fn dial(&self, host: &str) -> BoxFuture<'static, io::Result<PeerStream>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> PinnedSsh {
        PinnedSsh {
            host: "devbox.example.ts.net".into(),
            port: 22,
            user: "dev".into(),
            key_file: "/home/dev/.config/maxhq/ssh/id_ed25519".into(),
            known_hosts: "/home/dev/.config/maxhq/ssh/known_hosts".into(),
            alias: "maxhq-dev-d1".into(),
        }
    }

    #[test]
    fn the_command_reads_no_config_and_trusts_only_the_pinned_key() {
        assert_eq!(
            pin().args(),
            [
                "-F",
                "none",
                "-o",
                "Port=22",
                "-o",
                "User=dev",
                "-o",
                "IdentityFile=/home/dev/.config/maxhq/ssh/id_ed25519",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "UserKnownHostsFile=/home/dev/.config/maxhq/ssh/known_hosts",
                "-o",
                "HostKeyAlias=maxhq-dev-d1",
                "--",
                "devbox.example.ts.net",
            ]
        );
    }

    #[test]
    fn the_config_file_holds_the_same_options_as_the_command() {
        let pin = pin();
        let config = pin.config();
        let mut lines = config.lines();
        assert_eq!(lines.next(), Some("Host *"));
        let from_config: Vec<String> = lines
            .map(|line| {
                let (name, value) = line.trim().split_once(' ').expect("a name and a value");
                format!("{name}={}", value.trim_matches('"'))
            })
            .collect();
        let from_args: Vec<String> = pin
            .args()
            .chunks(2)
            .filter(|pair| pair[0] == "-o")
            .map(|pair| pair[1].clone())
            .collect();
        assert_eq!(from_config, from_args);
    }

    #[test]
    fn only_plain_parts_pass() {
        assert!(pin().is_plain());
        for bad in [
            PinnedSsh {
                host: "-oProxyCommand=x".into(),
                ..pin()
            },
            PinnedSsh {
                user: "dev x".into(),
                ..pin()
            },
            PinnedSsh {
                known_hosts: "/tmp/a\"b".into(),
                ..pin()
            },
            PinnedSsh {
                alias: "a\nb".into(),
                ..pin()
            },
        ] {
            assert!(!bad.is_plain(), "{bad:?}");
        }
    }
}

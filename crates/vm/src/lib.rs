// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `maxd` and `maxd-vm` say to each other: one JSON object per line,
//! [`Request`]s on the helper's stdin and [`Event`]s on its stdout.
//!
//! `maxd-vm` runs one macOS VM through Virtualization.framework, on its own
//! main-thread run loop, so the `com.apple.security.virtualization`
//! entitlement and AppKit stay out of `maxd`. One process runs one VM, and
//! exits once it stops or its stdin closes. `maxd` waits for that exit
//! before it starts the same bundle again, since a VM whose last run hasn't
//! exited can't lock its auxiliary storage.
//!
//! A bundle is a folder of [`bundle`] files: the disk, the auxiliary
//! storage, the hardware model and machine ID, the [`Shape`] and, once
//! saved, the machine state. Every run gives the VM the same devices, so a
//! saved state restores in a clone of the bundle: one shared folder at the
//! macOS guest's automount tag, and one virtio socket device whose guest
//! port [`GUEST_PORT`] the helper relays to a Unix socket.
//!
//! Only this crate and `maxd` speak it, and they ship together, so it has
//! no version of its own.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The helper's file name, next to `maxd`'s own.
pub const BINARY: &str = "maxd-vm";

/// This build's version, which `version` answers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The vsock port the guest's `maxd guest --vm` listens on.
pub const GUEST_PORT: u32 = 7070;

/// Where the guest finds the shared folder, which macOS mounts by itself
/// for the automount tag.
pub const GUEST_SHARE: &str = "/Volumes/My Shared Files";

/// The files in a bundle.
pub mod bundle {
    /// The boot disk, a sparse raw image.
    pub const DISK: &str = "disk.img";
    /// The Mac's auxiliary storage, which holds the guest's NVRAM.
    pub const AUX: &str = "aux.img";
    /// The hardware model the restore image asked for.
    pub const HARDWARE: &str = "hardware-model";
    /// The machine ID, which stays the same in every clone.
    pub const MACHINE: &str = "machine-id";
    /// The [`Shape`](super::Shape), as JSON.
    pub const SHAPE: &str = "shape.json";
    /// The saved machine state, once there is one.
    pub const STATE: &str = "state.vzvmsave";
}

/// How big a VM is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    /// Its CPUs.
    pub cpus: u32,
    /// Its memory.
    pub memory_bytes: u64,
    /// Its disk, which takes room only as the guest writes.
    pub disk_bytes: u64,
    /// The network device's MAC address, kept so a saved state restores.
    pub mac: Option<String>,
}

/// The folder a run shares with the guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// The folder on the Mac.
    pub path: PathBuf,
    /// Whether the guest may only read it.
    pub read_only: bool,
}

/// One request from `maxd`. Each gets one [`Event::Done`] or
/// [`Event::Failed`] with its `id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Picked by `maxd`, and echoed in the answer.
    pub id: u64,
    /// What to do.
    #[serde(flatten)]
    pub op: Op,
}

/// What a request asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Answers [`VERSION`].
    Version,
    /// Makes a new bundle at `bundle` and installs macOS into it from the
    /// restore image file `restore_image`. It never downloads one.
    /// Progress comes as [`Event::Progress`]. The process exits after.
    Install {
        restore_image: PathBuf,
        bundle: PathBuf,
        shape: Shape,
    },
    /// Runs the bundle. With `restore`, it restores the bundle's saved
    /// state and resumes, and otherwise boots. `window` shows the screen,
    /// with `note` above it, for the one-time setup. The guest's
    /// [`GUEST_PORT`] answers on the Unix socket `socket`, one vsock
    /// connection for each connection there.
    Start {
        bundle: PathBuf,
        share: Share,
        socket: PathBuf,
        restore: bool,
        window: bool,
        note: Option<String>,
    },
    /// Pauses the running VM and saves its state into the bundle. It stays
    /// paused, for a stop.
    Save,
    /// Stops the VM at once. The process exits after its answer.
    Stop,
}

/// What the helper says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Request `id` is done. A `version` request's answer carries it.
    Done { id: u64, version: Option<String> },
    /// Request `id` failed, with why for a person to read.
    Failed { id: u64, error: String },
    /// How far an install has come, from 0 to 1.
    Progress { fraction: f64 },
    /// The VM stopped by itself, as when the guest shut down or failed.
    Stopped { error: Option<String> },
}

impl Request {
    /// The line that sends it, newline included.
    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a request always encodes");
        line.push('\n');
        line
    }
}

impl Event {
    /// The line that sends it, newline included.
    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("an event always encodes");
        line.push('\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_events_go_as_one_line_each() {
        let start = Request {
            id: 3,
            op: Op::Start {
                bundle: "/vm/lease".into(),
                share: Share {
                    path: "/vm/lease/share".into(),
                    read_only: false,
                },
                socket: "/run/guests/vm.sock".into(),
                restore: true,
                window: false,
                note: None,
            },
        };
        let line = start.line();
        assert!(
            line.starts_with(r#"{"id":3,"op":"start","bundle":"/vm/lease""#),
            "{line}"
        );
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), start);
        let version: Request = serde_json::from_str(r#"{"id":1,"op":"version"}"#).unwrap();
        assert_eq!(version.op, Op::Version);

        let done = Event::Done {
            id: 1,
            version: Some(VERSION.to_owned()),
        };
        assert_eq!(serde_json::from_str::<Event>(&done.line()).unwrap(), done);
        assert_eq!(
            Event::Progress { fraction: 0.5 }.line(),
            "{\"event\":\"progress\",\"fraction\":0.5}\n"
        );
    }
}

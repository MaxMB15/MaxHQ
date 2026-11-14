// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The lease tools' calls once checked, and the limits on them. The tools
//! read the arguments into a [`LeaseCall`], and `maxd`'s leases run it for
//! a caller on the device that hosts the kind.

use std::path::PathBuf;

use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::TestKind;

/// How long `lease_open` waits for a busy host when the agent doesn't say,
/// in seconds.
pub const DEFAULT_WAIT_SECONDS: u16 = 120;

/// The longest `lease_open` may wait for a busy host, in seconds.
pub const MAX_WAIT_SECONDS: u16 = 600;

/// How long one `lease_run` command may run when the agent doesn't say, in
/// seconds.
pub const DEFAULT_RUN_SECONDS: u32 = 120;

/// The longest one `lease_run` command may run, in seconds.
pub const MAX_RUN_SECONDS: u32 = 600;

/// The most of stdout and of stderr one command answers with, in bytes.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// The largest app `lease_install` takes, in bytes.
pub const MAX_INSTALL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The longest edge of a screenshot an agent gets, in pixels. Larger ones
/// are scaled down.
pub const MAX_SCREEN_EDGE: u32 = 1280;

/// One checked call to a lease tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseCall {
    /// `list_test_hosts`: this device's hosts, the caller's leases, and
    /// which linked devices host the kinds this one doesn't.
    ListTestHosts,
    /// `lease_open`: asks for a host of one kind.
    Open {
        /// The kind.
        kind: TestKind,
        /// How long the lease lasts, or the device's default.
        minutes: Option<u16>,
        /// How long to wait in the queue before giving up, at most
        /// [`MAX_WAIT_SECONDS`].
        wait_seconds: u16,
    },
    /// `lease_run`: runs one allowed agent-device command.
    Run {
        /// The lease.
        lease: LeaseId,
        /// The subcommand and its arguments, without the flags `maxd`
        /// adds.
        command: Vec<String>,
        /// How long it may run, at most [`MAX_RUN_SECONDS`].
        timeout_seconds: u32,
    },
    /// `lease_screenshot`: a PNG of the machine's screen.
    Screenshot {
        /// The lease.
        lease: LeaseId,
    },
    /// `lease_install`: installs an app from the caller's place.
    Install {
        /// The lease.
        lease: LeaseId,
        /// The `.app`, `.ipa` or `.apk`, resolved inside the caller's
        /// workspace or folder.
        source: PathBuf,
    },
    /// `lease_close`: ends the lease.
    Close {
        /// The lease.
        lease: LeaseId,
    },
}

impl LeaseCall {
    /// The lease the call acts on, if it names one.
    pub fn lease(&self) -> Option<LeaseId> {
        match self {
            LeaseCall::ListTestHosts | LeaseCall::Open { .. } => None,
            LeaseCall::Run { lease, .. }
            | LeaseCall::Screenshot { lease }
            | LeaseCall::Install { lease, .. }
            | LeaseCall::Close { lease } => Some(*lease),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_calls_on_a_lease_name_one() {
        let lease = LeaseId::derive("lease");
        assert_eq!(LeaseCall::ListTestHosts.lease(), None);
        assert_eq!(
            LeaseCall::Open {
                kind: TestKind::IosSimulator,
                minutes: None,
                wait_seconds: DEFAULT_WAIT_SECONDS,
            }
            .lease(),
            None
        );
        assert_eq!(LeaseCall::Screenshot { lease }.lease(), Some(lease));
        assert_eq!(
            LeaseCall::Run {
                lease,
                command: vec!["snapshot".into()],
                timeout_seconds: DEFAULT_RUN_SECONDS,
            }
            .lease(),
            Some(lease)
        );
    }
}

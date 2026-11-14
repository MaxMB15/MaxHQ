// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Puts the static guest `maxd` into a container. It lands at
//! `/usr/local/lib/maxhq/maxd`, and `/usr/local/bin/maxd` links to it.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::guestlink::Upgrader;
use crate::incus::{Incus, IncusError, InstanceName};
use crate::recipe::MAXD;
use crate::session::clock::BoxFuture;

/// The folder the binary goes to in the container.
pub const LIB_DIR: &str = "/usr/local/lib/maxhq";

/// The binary in the container.
pub const LIB_MAXD: &str = "/usr/local/lib/maxhq/maxd";

/// The guest's systemd unit, which the recipe writes.
pub const GUEST_UNIT: &str = "maxhq-guest.service";

/// Where the recipe writes the unit.
pub const GUEST_UNIT_PATH: &str = "/etc/systemd/system/maxhq-guest.service";

/// The upload's name until it moves into place.
const UPLOAD: &str = "/usr/local/lib/maxhq/maxd.new";

/// Moves the upload into place and links it, in one command.
const PLACE: &str = r#"mv -f "$1" "$2" && ln -sfn "$2" "$3""#;

/// Restarts the guest unit when the recipe has written it.
const RESTART: &str = r#"test -e "$1" || exit 0; systemctl restart "$2""#;

/// Why the guest binary couldn't go in.
#[derive(Debug, Error)]
pub enum InstallError {
    /// No guest binary was found on this device.
    #[error("there's no guest maxd at {0}")]
    Missing(String),
    /// The binary couldn't be read.
    #[error("couldn't read the guest maxd at {path}: {error}")]
    Read {
        /// The path.
        path: String,
        /// What went wrong.
        error: std::io::Error,
    },
    /// Incus refused a call.
    #[error(transparent)]
    Incus(#[from] IncusError),
    /// A command in the container failed.
    #[error("`{command}` exited with {status}: {stderr}")]
    Failed {
        /// The command.
        command: String,
        /// Its exit status.
        status: i32,
        /// What it printed to stderr.
        stderr: String,
    },
}

/// Installs guest binaries for the guest links when a guest reports a
/// version other than this `maxd`'s.
pub struct GuestInstall(pub Incus);

impl Upgrader for GuestInstall {
    fn upgrade(&self, instance: InstanceName, binary: PathBuf) -> BoxFuture<Result<(), String>> {
        let incus = self.0.clone();
        Box::pin(async move {
            install(&incus, &instance, &binary)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

/// Pushes the binary at `path` into `instance`, links it from
/// `/usr/local/bin/maxd`, and restarts the guest unit when the recipe has
/// written it.
pub async fn install(
    incus: &Incus,
    instance: &InstanceName,
    path: &Path,
) -> Result<(), InstallError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::Missing(path.display().to_string()));
        }
        Err(error) => {
            return Err(InstallError::Read {
                path: path.display().to_string(),
                error,
            });
        }
    };
    run(incus, instance, &["mkdir", "-p", LIB_DIR]).await?;
    incus.push_file(instance, UPLOAD, bytes, 0o755).await?;
    run(
        incus,
        instance,
        &["sh", "-c", PLACE, "sh", UPLOAD, LIB_MAXD, MAXD],
    )
    .await?;
    run(
        incus,
        instance,
        &["sh", "-c", RESTART, "sh", GUEST_UNIT_PATH, GUEST_UNIT],
    )
    .await
}

/// Runs a command as root and fails unless it exits 0.
async fn run(incus: &Incus, instance: &InstanceName, cmd: &[&str]) -> Result<(), InstallError> {
    let output = incus.exec(instance, cmd, None, None).await?;
    if output.success() {
        return Ok(());
    }
    Err(InstallError::Failed {
        command: cmd.join(" "),
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

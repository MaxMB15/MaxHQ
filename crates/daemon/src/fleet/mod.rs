// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeping a device's MaxHQ binaries and agent CLIs up to date, and
//! reading what it has free. The server hands every step 4 request to
//! [`handle`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use maxhq_protocol::message::{ActionOutcome, UpdateKind};
use maxhq_protocol::state::ToolKind;
use maxhq_protocol::ClientMessage;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::diagnostics::{self, Monitor};
use crate::guestlink::Guests;
use crate::identity;
use crate::tools::Tools;

pub mod release;
pub mod update;

use update::{Installed, Layout, Refusal};

/// How long maxd waits after answering an update before it exits, so the
/// answer reaches the app.
const EXIT_AFTER: Duration = Duration::from_secs(1);

/// What the fleet requests need from the running maxd.
pub struct Fleet {
    /// Where maxd's versions live, or none without a home folder.
    pub layout: Option<Layout>,
    /// The attachments folder, which holds staged uploads.
    pub attachments: PathBuf,
    /// The binary container workspaces run, if this device has one.
    pub guest_binary: Option<PathBuf>,
    /// The guest links, told to recheck builds after a guest update.
    pub guests: Option<Arc<Guests>>,
    /// The diagnostics sampler.
    pub monitor: Option<Arc<Monitor>>,
    /// The agent CLIs.
    pub tools: Option<Arc<Tools>>,
}

struct Running {
    fleet: Fleet,
    exit: Notify,
    healthy: AtomicBool,
}

static FLEET: OnceLock<Running> = OnceLock::new();

/// Makes `fleet` the one requests use, and starts counting this maxd
/// healthy once it has listened for a while.
pub fn install(fleet: Fleet) {
    let first = FLEET
        .set(Running {
            fleet,
            exit: Notify::new(),
            healthy: AtomicBool::new(false),
        })
        .is_ok();
    if first {
        tokio::spawn(async {
            tokio::time::sleep(update::LISTENING_HEALTHY).await;
            served_hello();
        });
    }
}

/// Whether this maxd can install updates of itself.
pub fn updates() -> bool {
    identity::build().is_some()
        && FLEET
            .get()
            .and_then(|running| running.fleet.layout.as_ref())
            .is_some()
}

/// Whether this maxd installs and removes tools: it does once it keeps
/// track of them.
pub fn installs() -> bool {
    FLEET
        .get()
        .is_some_and(|running| running.fleet.tools.is_some())
}

/// Called after each hello this maxd serves. The first one marks an update
/// to this build healthy, so its watcher leaves it be.
pub fn served_hello() {
    let Some(running) = FLEET.get() else {
        return;
    };
    if running.healthy.swap(true, Ordering::SeqCst) {
        return;
    }
    if let (Some(layout), Some(build)) = (&running.fleet.layout, identity::build()) {
        let layout = layout.clone();
        let build = build.to_owned();
        // A small file write, off the connection's task.
        tokio::task::spawn_blocking(move || update::mark_healthy(&layout, &build));
    }
}

/// Resolves when an update asked this maxd to exit so the new build runs.
pub async fn exit_requested() {
    match FLEET.get() {
        Some(running) => running.exit.notified().await,
        None => std::future::pending().await,
    }
}

/// Answers one step 4 request.
pub async fn handle(message: ClientMessage) -> ActionOutcome {
    match message {
        ClientMessage::ReadLoad { .. } => ActionOutcome::Load(diagnostics::load::read().await),
        ClientMessage::ReadDiagnostics {
            window_secs,
            bucket_secs,
            ..
        } => match FLEET
            .get()
            .and_then(|running| running.fleet.monitor.as_ref())
        {
            Some(monitor) => monitor.read(window_secs, bucket_secs).await,
            None => ActionOutcome::Diagnostics {
                buckets: Vec::new(),
                top: Vec::new(),
                containers: Vec::new(),
                error: Some("This maxd doesn't sample its use.".to_owned()),
            },
        },
        ClientMessage::UpdateTools { tools, .. } => {
            match FLEET.get().and_then(|running| running.fleet.tools.as_ref()) {
                Some(owned) => owned.update(tools).await,
                None => refused("This maxd can't update agent CLIs."),
            }
        }
        ClientMessage::InstallTool {
            place,
            tool: ToolKind::Acp { id },
            version,
            ..
        } => crate::acp_registry::update(id, version, place).await,
        ClientMessage::RemoveTool {
            place,
            tool: ToolKind::Acp { id },
            ..
        } => crate::acp_registry::remove(id, place).await,
        ClientMessage::InstallTool {
            place,
            tool,
            version,
            ..
        } => match FLEET.get().and_then(|running| running.fleet.tools.as_ref()) {
            Some(owned) => owned.install(tool, place, version),
            None => refused("This maxd can't install tools."),
        },
        ClientMessage::RemoveTool { place, tool, .. } => {
            match FLEET.get().and_then(|running| running.fleet.tools.as_ref()) {
                Some(owned) => owned.remove(tool, place),
                None => refused("This maxd can't install tools."),
            }
        }
        ClientMessage::AcceptLicence {
            licence,
            text_sha256,
            ..
        } => match FLEET.get().and_then(|running| running.fleet.tools.as_ref()) {
            Some(owned) => owned.accept_licence(&licence, &text_sha256),
            None => refused("This maxd can't install tools."),
        },
        ClientMessage::InstallUpdate {
            staged,
            kind,
            sha256,
            release,
            unsigned,
            ..
        } => {
            let Some(running) = FLEET.get() else {
                return refused("This maxd can't install updates.");
            };
            let Some(path) = crate::sidecar::attachment_in(
                &running
                    .fleet
                    .attachments
                    .join("staged")
                    .join(staged.to_string()),
            ) else {
                return refused(&sentence(&Refusal::Missing.to_string()));
            };
            // Nothing runs the upload, not even `--build`, until it's shown
            // to be a release's, or one you chose to send unsigned.
            let vouched = release::vouch(
                &path,
                &sha256,
                kind,
                release.as_ref(),
                unsigned,
                release::Trust::own(),
            );
            let result = match (vouched, kind) {
                (Err(refusal), _) => Err(refusal),
                (Ok(()), UpdateKind::Maxd) => install_maxd(running, &path, &sha256).await,
                (Ok(()), UpdateKind::Guest) => install_guest(running, &path, &sha256).await,
            };
            let _ = std::fs::remove_file(&path);
            match result {
                Ok(()) => ActionOutcome::Done,
                Err(refusal) => refused(&sentence(&refusal.to_string())),
            }
        }
        other => {
            warn!(?other, "the fleet got a request that isn't its own");
            refused("maxd couldn't finish that.")
        }
    }
}

/// Installs a new maxd, starts the old one watching it, and exits so the
/// service manager starts the new one.
async fn install_maxd(
    running: &'static Running,
    staged: &Path,
    sha256: &str,
) -> Result<(), Refusal> {
    let layout = running.fleet.layout.as_ref().ok_or(Refusal::NotLinked)?;
    let current = std::env::current_exe()?;
    let installed = update::install_maxd(layout, staged, sha256, &current).await?;
    also_as_guest(running, staged, sha256).await;
    match installed {
        Installed::Same => Ok(()),
        Installed::Flipped { build, previous } => {
            if let Err(err) = update::start_watcher(&layout.version(&previous), &build) {
                // Without a watcher there's no rollback, so put the old
                // build back and refuse.
                warn!(%err, "couldn't start the update watcher");
                maxhq_os::link::point(&layout.link(), &layout.link_target(&previous))?;
                return Err(Refusal::Io(format!(
                    "couldn't start the update watcher: {err}"
                )));
            }
            info!(%build, %previous, "installed a new maxd, restarting into it");
            tokio::spawn(async move {
                tokio::time::sleep(EXIT_AFTER).await;
                running.exit.notify_one();
            });
            Ok(())
        }
    }
}

/// Linux builds ship as one static file that is both maxd and the guest.
/// When one arrives as maxd it becomes the guest binary too. A guest sent
/// on its own afterwards replaces it.
async fn also_as_guest(running: &Running, staged: &Path, sha256: &str) {
    if !cfg!(target_os = "linux") || !update::is_static_elf(staged) {
        return;
    }
    let Some(guest) = &running.fleet.guest_binary else {
        return;
    };
    if let Some(dir) = guest.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        warn!(%err, "couldn't make the guest binary's folder");
        return;
    }
    match update::install_guest(staged, sha256, guest).await {
        Ok(build) => {
            info!(%build, "the new maxd is the guest binary too");
            if let Some(guests) = &running.fleet.guests {
                guests.refresh();
            }
        }
        Err(err) => warn!(%err, "couldn't install the new maxd as the guest binary"),
    }
}

/// Replaces the guest binary and has each linked guest recheck its build.
async fn install_guest(running: &Running, staged: &Path, sha256: &str) -> Result<(), Refusal> {
    if !cfg!(target_os = "linux") {
        return Err(Refusal::NoGuests);
    }
    let guest = running
        .fleet
        .guest_binary
        .as_ref()
        .ok_or(Refusal::NoGuests)?;
    update::install_guest(staged, sha256, guest).await?;
    if let Some(guests) = &running.fleet.guests {
        guests.refresh();
    }
    Ok(())
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// `text` with a capital first letter and a full stop.
pub(crate) fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_read_as_sentences() {
        assert_eq!(
            sentence(&Refusal::Checksum.to_string()),
            "The upload doesn't match its checksum, so it wasn't installed."
        );
        assert_eq!(sentence("done."), "Done.");
    }
}

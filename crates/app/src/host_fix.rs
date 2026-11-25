// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a terminal runs for a host item's root fix. The command comes from
//! the device's own host report, never from the window, so a page can't
//! have a terminal run anything else. The report and the route come from
//! the same link, so what a device reports only ever runs on that device,
//! even if it claims another device's ID when it says hello. On this
//! machine it's the command alone. Another device's goes through `ssh -t`
//! along the route the app reaches it by, so sudo can ask for the password
//! there. A WSL distro's goes through `wsl.exe`, which hands it to the
//! distro's shell, on the PC it's on.

use maxhq_core::shell::word;
use maxhq_protocol::hosts::HostFix;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;
use maxhq_protocol::state::{Entity, EntityKey};

use crate::devices::{best_link, Connector};
use crate::request::CommandError;
use crate::App;

/// A root fix to run: its command, and the host `ssh -t` runs it on, or
/// none for this machine. Each terminal starts it its own way: a POSIX one
/// runs [`RootFix::line`], and PowerShell hands `ssh` its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFix {
    /// The SSH host the command runs on, or `None` for this machine.
    pub host: Option<String>,
    /// The command, for the shell where it runs.
    pub command: String,
}

impl RootFix {
    /// `command` on the device reached by `route`.
    fn new(route: &Route, command: &str) -> Self {
        let (host, command) = match route {
            Route::Local => (None, command.to_owned()),
            Route::Ssh { host } => (Some(host.clone()), command.to_owned()),
            Route::Wsl { distro, via } => {
                (via.clone(), format!("wsl.exe -d {distro} -- {command}"))
            }
        };
        Self { host, command }
    }

    /// The line a POSIX shell runs: the command here, or `ssh -t` with
    /// the host and the command.
    pub fn line(&self) -> String {
        match &self.host {
            None => self.command.clone(),
            Some(host) => format!("ssh -t {} {}", word(host), word(&self.command)),
        }
    }
}

impl<C: Connector> App<C> {
    /// The root fix of `item` in `device`'s host report.
    pub fn root_fix(&self, device: DeviceId, item: &str) -> Result<RootFix, CommandError> {
        self.view.read(|merge| {
            let reported = best_link(merge, device).and_then(|link| {
                let held = merge.held_by(link.id, &EntityKey::HostReport(device))?;
                Some((link, held))
            });
            let Some((link, Entity::HostReport(report))) = reported else {
                return Err(CommandError::NotFound {
                    what: "This device's host check".to_owned(),
                });
            };
            match report.items.iter().find(|found| found.id == item) {
                Some(found) => match &found.fix {
                    Some(HostFix::RootCommand { command }) => {
                        Ok(RootFix::new(&link.route, command))
                    }
                    _ => Err(CommandError::Failed {
                        message: "This item has no command to run as root".to_owned(),
                    }),
                },
                None => Err(CommandError::NotFound {
                    what: "This item".to_owned(),
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use maxhq_client::LinkEvent;
    use maxhq_protocol::hosts::{HostItem, HostItemState, HostReport, HostSource};
    use maxhq_protocol::state::Change;
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::devices::tests::two_devices;
    use crate::tests::FakeShell;

    const ROOT: &str = "sudo /home/dev/.local/bin/maxd host apply --system";

    fn report(device: &str) -> Entity {
        let item = |id: &str, fix| HostItem {
            id: id.into(),
            label: id.into(),
            state: HostItemState::Missing,
            fix,
        };
        Entity::HostReport(HostReport {
            device: DeviceId::derive(device),
            source: HostSource::Defaults,
            checked_at: Timestamp(1),
            items: vec![
                item(
                    "caps.parent",
                    Some(HostFix::RootCommand {
                        command: ROOT.into(),
                    }),
                ),
                item("tools.user.gh", Some(HostFix::Now)),
            ],
        })
    }

    /// The line for `command` on the device `route` reaches.
    fn line_for(route: &Route, command: &str) -> String {
        RootFix::new(route, command).line()
    }

    #[test]
    fn a_root_fix_runs_alone_here_and_over_ssh_elsewhere() {
        assert_eq!(line_for(&Route::Local, ROOT), ROOT);
        assert_eq!(
            line_for(
                &Route::Ssh {
                    host: "dev@devbox".into()
                },
                ROOT
            ),
            format!("ssh -t dev@devbox '{ROOT}'")
        );
        let wsl = |via: Option<&str>| Route::Wsl {
            distro: "Ubuntu".into(),
            via: via.map(str::to_owned),
        };
        assert_eq!(
            line_for(&wsl(None), ROOT),
            format!("wsl.exe -d Ubuntu -- {ROOT}")
        );
        assert_eq!(
            line_for(&wsl(Some("dev@pc")), ROOT),
            format!("ssh -t dev@pc 'wsl.exe -d Ubuntu -- {ROOT}'")
        );
        assert_eq!(
            RootFix::new(&wsl(Some("dev@pc")), ROOT),
            RootFix {
                host: Some("dev@pc".into()),
                command: format!("wsl.exe -d Ubuntu -- {ROOT}"),
            }
        );
    }

    #[test]
    fn the_line_comes_from_the_devices_report() {
        let (h, local, nl) = two_devices();
        h.report(local.id, LinkEvent::Synced(vec![report("mac")]));
        h.report(nl.id, LinkEvent::Synced(vec![report("nl")]));
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());

        let line = |device: &str| {
            app.root_fix(DeviceId::derive(device), "caps.parent")
                .map(|fix| fix.line())
        };
        assert_eq!(line("mac"), Ok(ROOT.to_owned()));
        assert_eq!(line("nl"), Ok(format!("ssh -t devbox '{ROOT}'")));
        assert_eq!(
            app.root_fix(DeviceId::derive("nl"), "tools.user.gh"),
            Err(CommandError::Failed {
                message: "This item has no command to run as root".into()
            })
        );
        assert_eq!(
            app.root_fix(DeviceId::derive("nl"), "rm -rf"),
            Err(CommandError::NotFound {
                what: "This item".into()
            })
        );
        assert_eq!(
            app.root_fix(DeviceId::derive("gone"), "caps.parent"),
            Err(CommandError::NotFound {
                what: "This device's host check".into()
            })
        );
    }

    /// A report for the Mac with a command in it that some other device
    /// wrote.
    fn forged() -> Entity {
        let Entity::HostReport(mut report) = report("mac") else {
            panic!("not a report");
        };
        report.checked_at = Timestamp(2);
        report.items[0].fix = Some(HostFix::RootCommand {
            command: "touch /tmp/forged".into(),
        });
        Entity::HostReport(report)
    }

    /// The line for the Mac's root fix, once the reports are in.
    fn mac_line(h: crate::devices::tests::Harness) -> Result<String, CommandError> {
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        app.root_fix(DeviceId::derive("mac"), "caps.parent")
            .map(|fix| fix.line())
    }

    fn no_report() -> Result<String, CommandError> {
        Err(CommandError::NotFound {
            what: "This device's host check".into(),
        })
    }

    #[test]
    fn another_device_cant_write_this_macs_root_fix() {
        // Devbox reports for the Mac, which hasn't reported yet.
        let (h, _, nl) = two_devices();
        h.report(nl.id, LinkEvent::Synced(vec![forged()]));
        assert_eq!(mac_line(h), no_report());

        // Or after the Mac's report, and later than it.
        let (h, local, nl) = two_devices();
        h.report(local.id, LinkEvent::Synced(vec![report("mac")]));
        h.report(
            nl.id,
            LinkEvent::Changed(vec![
                Change::Upsert {
                    value: report("nl"),
                },
                Change::Upsert { value: forged() },
            ]),
        );
        assert_eq!(mac_line(h), Ok(ROOT.to_owned()));
    }

    /// Devbox says hello as the Mac, so its report passes as the Mac's.
    fn claim_to_be_the_mac(h: &crate::devices::tests::Harness, nl: &maxhq_client::LinkSpec) {
        let mut claims = crate::merge::tests::spec("devbox");
        claims.id = nl.id;
        claims.name = "mac".into();
        h.report(
            nl.id,
            crate::merge::tests::link_state(&claims, crate::merge::tests::connected()),
        );
        h.report_quiet(nl.id, LinkEvent::Synced(vec![forged()]));
    }

    #[test]
    fn a_device_that_claims_to_be_this_mac_cant_write_its_root_fix() {
        let (h, _, nl) = two_devices();
        claim_to_be_the_mac(&h, &nl);
        assert_eq!(mac_line(h), no_report());

        // The line for the Mac reads only the Mac's own link.
        let (h, local, nl) = two_devices();
        claim_to_be_the_mac(&h, &nl);
        h.report(local.id, LinkEvent::Synced(vec![report("mac")]));
        assert_eq!(mac_line(h), Ok(ROOT.to_owned()));
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The files the recipe writes whole, and the block it keeps in the agent
//! user's `.bashrc`.

use std::path::Path;

use super::CLAUDE;

/// Where the guest's systemd unit lives.
pub const GUEST_UNIT_PATH: &str = "/etc/systemd/system/maxhq-guest.service";
/// The guest unit's name.
pub const GUEST_UNIT_NAME: &str = "maxhq-guest.service";

/// The guest's systemd unit. `KillMode=process` stops only `maxd guest` on
/// a restart, so the holders it started keep their agents running. No
/// `RuntimeDirectory`, since systemd would delete the sockets in it on
/// every restart.
pub const GUEST_UNIT: &str = "\
# Written by the MaxHQ recipe, which replaces any change made here.
[Unit]
Description=MaxHQ guest
StartLimitIntervalSec=0

[Service]
Type=simple
Environment=MAXHQ_ROLE=guest
ExecStart=/usr/local/bin/maxd guest
Restart=always
RestartSec=1
KillMode=process

[Install]
WantedBy=multi-user.target
";

/// The folder for the guest unit's drop-ins.
pub const GUEST_UNIT_DROP_INS: &str = "/etc/systemd/system/maxhq-guest.service.d";
/// Where the drop-in that makes the guest a test host lives.
pub const TEST_HOST_PATH: &str = "/etc/systemd/system/maxhq-guest.service.d/test-host.conf";

/// Runs the guest as a test host, which answers a lease's requests.
pub const TEST_HOST: &str = "\
# Written by the MaxHQ recipe, which replaces any change made here.
[Service]
ExecStart=
ExecStart=/usr/local/bin/maxd guest --test-host
";

/// Where git's system config lives.
pub const GITCONFIG_PATH: &str = "/etc/gitconfig";

/// Git's system config. The recipe owns the whole file, since Debian ships
/// none. Git adds `get`, `store` or `erase` to the helper's command.
pub const GITCONFIG: &str = "\
# Written by the MaxHQ recipe, which replaces any change made here.
[credential \"https://github.com\"]
\thelper = !/usr/local/bin/maxd guest git-credential
\tuseHttpPath = true
";

/// Where the `gh` wrapper lives. It comes before `/usr/bin` on `PATH`.
pub const GH_WRAPPER_PATH: &str = "/usr/local/bin/gh";

/// Runs the real `gh` with a token for the workspace's repo. `set -e` stops
/// it when `maxd` gives no token, so `gh` never runs without one.
pub const GH_WRAPPER: &str = "\
#!/bin/sh
# Written by the MaxHQ recipe. Asks maxd for a token scoped to this
# workspace's repo, then runs the real gh with it.
set -e
GH_TOKEN=\"$(/usr/local/bin/maxd guest github-token)\"
export GH_TOKEN
exec /usr/bin/gh \"$@\"
";

/// The agent user's `.bashrc`.
pub const BASHRC_PATH: &str = "/home/agent/.bashrc";

/// The first line of the recipe's block in `.bashrc`.
const PATH_START: &str = "# >>> MaxHQ PATH >>>";
/// The last line of the recipe's block in `.bashrc`.
const PATH_END: &str = "# <<< MaxHQ PATH <<<";

/// The folder that holds Claude's launcher, which the agent's `PATH` needs.
pub fn agent_bin() -> &'static str {
    Path::new(CLAUDE)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("/home/agent/.local/bin")
}

/// The recipe's block in `.bashrc`, which puts Claude's folder on `PATH`.
pub fn path_block() -> String {
    let bin = agent_bin();
    format!(
        "{PATH_START}\n\
         # Kept first by the MaxHQ recipe, so shells that stop early still get it.\n\
         case \":$PATH:\" in\n\
         \x20   *\":{bin}:\"*) ;;\n\
         \x20   *) PATH=\"{bin}:$PATH\" ;;\n\
         esac\n\
         export PATH\n\
         {PATH_END}\n"
    )
}

/// `.bashrc` with the recipe's block first and everything else as it was.
/// Debian's `.bashrc` returns early in shells that aren't interactive, so
/// the block has to come before that.
pub fn bashrc(existing: Option<&str>) -> String {
    let mut lines: Vec<&str> = existing.unwrap_or("").split_inclusive('\n').collect();
    let marker = |line: &str| {
        let line = line.trim_end_matches(['\n', '\r']);
        (line == PATH_START || line == PATH_END).then_some(line == PATH_START)
    };
    // Drops each earlier copy of the block. A start line with no end before
    // the next start isn't a block, so it stays with the lines after it.
    let mut at = 0;
    while let Some(found) = lines[at..]
        .iter()
        .position(|line| marker(line) == Some(true))
    {
        let start = at + found;
        let next = lines[start + 1..]
            .iter()
            .position(|line| marker(line).is_some())
            .map(|found| start + 1 + found);
        match next {
            Some(end) if marker(lines[end]) == Some(false) => {
                lines.drain(start..=end);
                at = start;
            }
            _ => at = start + 1,
        }
    }
    let mut text = path_block();
    text.extend(lines);
    text
}

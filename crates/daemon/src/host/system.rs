// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd host apply --system`, the root part on Debian and Ubuntu: Incus
//! and the packages, the user in `incus-admin`, linger, the data folder,
//! the units, the egress table, zram and `MemoryLow`.
//!
//! It plans first, from what's there, and prints the plan. A file it
//! didn't write, or a data folder someone else owns, is a surprise, and a
//! surprise stops it before any change. Each step leaves the host as the
//! step says however often it runs, and a second run plans nothing.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use maxhq_core::host_spec::HostSpec;

use super::check::in_group;
use super::{load, Env, Os, User, INCUS_GROUP};
use crate::service::Runner;

/// The first line of every file `--system` writes, which lets a later run
/// replace it.
pub const MARKER: &str = "# Written by maxd host apply --system. A later run replaces this file.";

/// What `--system` says on NixOS.
pub const NIXOS: &str = "This host is NixOS. Change maxhq-host.toml and rebuild.";

/// The cgroup period `cpu.max` uses.
const PERIOD: u64 = 100_000;

/// The egress table's name, in the `inet` family.
const TABLE: &str = "maxhq-egress";

/// Where the egress table's rules live.
const EGRESS_RULES: &str = "/etc/maxhq/egress.nft";

/// Zabbly's Incus repository, for releases whose archive has no Incus.
const ZABBLY_LIST: &str = "/etc/apt/sources.list.d/zabbly-incus-stable.sources";
const ZABBLY_KEY: &str = "/etc/apt/keyrings/zabbly.asc";
const ZABBLY_KEY_URL: &str = "https://pkgs.zabbly.com/key.asc";
/// Where the key lands before its fingerprint is checked.
const ZABBLY_DOWNLOAD: &str = "/etc/apt/keyrings/zabbly.asc.download";
/// The fingerprint Zabbly's README gives for that key.
pub const ZABBLY_FINGERPRINT: &str = "4EFC590696CB15B87C73A3AD82CC8797C838DCFD";

/// What `--system` says when it can't ask.
pub const NO_TERMINAL: &str = "No terminal to ask on. Run it again with --yes to go ahead.";

const ZRAM_CONF: &str = "/etc/systemd/zram-generator.conf";
const MEMORY_LOW_CONF: &str = "/etc/systemd/system/system.slice.d/50-maxhq-memory-low.conf";
const UNITS: &str = "/etc/systemd/system";

/// `cpu.max` for a cap of `cpus` CPUs.
pub fn cpu_max(cpus: u32) -> String {
    format!("{} {PERIOD}", u64::from(cpus) * PERIOD)
}

/// A group of files the root part writes, which the check reports as one
/// item.
#[derive(Clone, Debug)]
pub struct Want {
    /// The item's id.
    pub id: &'static str,
    /// The item's label.
    pub label: &'static str,
    /// Each file's path and text. The first is the unit the NixOS module
    /// writes too.
    pub files: Vec<(String, String)>,
}

/// The units and the egress table the spec asks for.
pub fn files(env: &Env, spec: &HostSpec) -> Vec<Want> {
    let mut wants = Vec::new();
    if let Some(script) = prepare_script(spec) {
        wants.push(Want {
            id: "units.prepare",
            label: "The workspace cgroup is made at boot",
            files: vec![(unit("maxd-prepare.service"), prepare_unit(&script))],
        });
    }
    wants.push(Want {
        id: "units.shutdown",
        label: "Workspaces stop cleanly when the host shuts down",
        files: vec![(unit("maxd-shutdown.service"), shutdown_unit(env, spec))],
    });
    if let Some(rules) = egress_rules(spec) {
        wants.push(Want {
            id: "egress",
            label: "Workspaces reach nothing private",
            files: vec![
                (unit("maxd-egress.service"), egress_unit()),
                (EGRESS_RULES.to_owned(), format!("{MARKER}\n{rules}")),
                (unit("maxd-egress-check.service"), egress_check_unit()),
                (unit("maxd-egress-check.timer"), egress_timer()),
            ],
        });
    }
    wants
}

fn unit(name: &str) -> String {
    format!("{UNITS}/{name}")
}

/// The script that makes the parent cgroup and sets its limits, when the
/// spec has maxd manage a parent. The NixOS module runs the same script,
/// through `maxd host render prepare`.
pub fn prepare_script(spec: &HostSpec) -> Option<String> {
    let caps = &spec.caps;
    let parent = caps.parent.as_deref().filter(|_| caps.manage_parent)?;
    let mut script = String::from("set -e; ");
    let mut dir = String::from("/sys/fs/cgroup");
    for part in parent.split('/') {
        script.push_str(&format!(
            "echo +cpu +memory > {dir}/cgroup.subtree_control; "
        ));
        dir = format!("{dir}/{part}");
        script.push_str(&format!("mkdir -p {dir}; "));
    }
    let monitor = format!("/sys/fs/cgroup/{parent}.monitor");
    script.push_str(&format!("mkdir -p {monitor}; "));
    if let Some(high) = caps.memory_high {
        script.push_str(&format!("echo {} > {dir}/memory.high; ", high.0));
    }
    if let Some(cpus) = caps.cpu_max {
        script.push_str(&format!("echo {} > {dir}/cpu.max; ", cpu_max(cpus)));
    }
    if let Some(weight) = caps.cpu_weight {
        script.push_str(&format!("echo {weight} > {dir}/cpu.weight; "));
    }
    Some(script.trim_end_matches("; ").to_owned())
}

/// Runs the prepare script before Incus starts any container in the
/// parent.
fn prepare_unit(script: &str) -> String {
    format!(
        "{MARKER}
[Unit]
Description=The cgroup MaxHQ's workspaces share
Before=incus.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c '{script}'

[Install]
WantedBy=multi-user.target
"
    )
}

/// Stops the workspaces cleanly at shutdown, while Incus and the user's
/// maxd still run. Systemd stops it first, since it's ordered after both.
/// It never restarts, so a deploy never runs its stop. The stop runs as
/// root, with `+`, since maxd winds its agents down only for root or a
/// proven app; it still finds the user's socket by the runtime folder.
fn shutdown_unit(env: &Env, spec: &HostSpec) -> String {
    let User { name, uid, .. } = &env.user;
    let window = spec.shutdown.quiesce_window.0;
    let timeout = window + 60;
    let maxd = env.maxd.display();
    format!(
        "{MARKER}
[Unit]
Description=Stop MaxHQ's workspaces cleanly at shutdown
After=incus.service user@{uid}.service
Wants=user@{uid}.service

[Service]
Type=oneshot
RemainAfterExit=yes
User={name}
Environment=XDG_RUNTIME_DIR=/run/user/{uid}
ExecStart=/bin/true
ExecStop=+{maxd} host quiesce --window {window}s
TimeoutStopSec={timeout}

[Install]
WantedBy=multi-user.target
"
    )
}

/// Loads the egress table before Incus starts.
fn egress_unit() -> String {
    format!(
        "{MARKER}
[Unit]
Description=Keep MaxHQ's workspaces from reaching anything private
After=nftables.service
Before=incus.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/sbin/nft -f {EGRESS_RULES}
ExecStop=-/usr/sbin/nft delete table inet {TABLE}

[Install]
WantedBy=multi-user.target
"
    )
}

/// Puts the table back if something flushed it.
fn egress_check_unit() -> String {
    format!(
        "{MARKER}
[Unit]
Description=Check MaxHQ's egress table is loaded

[Service]
Type=oneshot
ExecStart=/bin/sh -c '/usr/sbin/nft list table inet {TABLE} > /dev/null || /usr/sbin/nft -f {EGRESS_RULES}'
"
    )
}

fn egress_timer() -> String {
    format!(
        "{MARKER}
[Unit]
Description=Check MaxHQ's egress table every five minutes

[Timer]
OnBootSec=1min
OnUnitActiveSec=5min

[Install]
WantedBy=timers.target
"
    )
}

/// W-7's rules for traffic from the spec's bridge, when Incus is on and
/// the spec names one: nothing private through the host, and nothing on
/// the host itself but DNS, DHCP and ICMP. Loading them replaces the
/// table whole. The NixOS module loads the same rules, through
/// `maxd host render egress`.
pub fn egress_rules(spec: &HostSpec) -> Option<String> {
    use crate::workspace::egress::BLOCKED;
    let bridge = &spec
        .incus
        .bridge
        .as_ref()
        .filter(|_| spec.incus.enable)?
        .name;
    let (v6, v4): (Vec<&str>, Vec<&str>) = BLOCKED.iter().partition(|range| range.contains(':'));
    let (v4, v6) = (v4.join(", "), v6.join(", "));
    Some(format!(
        "table inet {TABLE} {{}}
delete table inet {TABLE}

table inet {TABLE} {{
\tset private4 {{
\t\ttype ipv4_addr; flags interval
\t\telements = {{ {v4} }}
\t}}

\tset private6 {{
\t\ttype ipv6_addr; flags interval
\t\telements = {{ {v6} }}
\t}}

\tchain forward {{
\t\ttype filter hook forward priority filter - 1; policy accept;
\t\tiifname \"{bridge}\" ct state established,related accept
\t\tiifname \"{bridge}\" ip daddr @private4 drop
\t\tiifname \"{bridge}\" ip6 daddr @private6 drop
\t}}

\tchain input {{
\t\ttype filter hook input priority filter - 1; policy accept;
\t\tiifname \"{bridge}\" ct state established,related accept
\t\tiifname \"{bridge}\" udp dport {{ 53, 67, 547 }} accept
\t\tiifname \"{bridge}\" tcp dport 53 accept
\t\tiifname \"{bridge}\" meta l4proto {{ icmp, ipv6-icmp }} accept
\t\tiifname \"{bridge}\" drop
\t}}
}}
"
    ))
}

/// Swap in compressed memory, half the RAM up to 8 GiB.
fn zram_conf() -> String {
    format!(
        "{MARKER}
[zram0]
zram-size = min(ram / 2, 8192)
compression-algorithm = zstd
"
    )
}

/// The `system.slice` drop-in that keeps memory for the host's own
/// services when workspaces press, when the spec sets how much. The NixOS
/// module installs the same drop-in, through `maxd host render memory-low`.
pub fn memory_low(spec: &HostSpec) -> Option<String> {
    let low = spec.caps.system_memory_low?;
    Some(format!("[Slice]\nMemoryLow={}\n", low.0))
}

/// Zabbly's repository for this release.
fn zabbly_list(codename: &str) -> String {
    format!(
        "{MARKER}
Enabled: yes
Types: deb
URIs: https://pkgs.zabbly.com/incus/stable
Suites: {codename}
Components: main
Signed-By: {ZABBLY_KEY}
"
    )
}

/// One step of the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Writes `text` at `path`, readable by everyone.
    Write {
        /// The file.
        path: String,
        /// Its text.
        text: String,
    },
    /// Makes a folder that belongs to the service user, with mode 0700.
    MakeDir {
        /// The folder.
        path: String,
    },
    /// Runs a command.
    Run(Vec<String>),
    /// Checks the key at `download` has `fingerprint`, then moves it to
    /// `path`, where apt trusts it.
    TrustKey {
        /// Where the key was downloaded.
        download: String,
        /// Where apt reads it.
        path: String,
        /// The one fingerprint it may have.
        fingerprint: String,
    },
}

impl Step {
    fn run(args: &[&str]) -> Self {
        Step::Run(args.iter().map(|arg| (*arg).to_owned()).collect())
    }

    /// The step as the plan shows it.
    pub fn line(&self) -> String {
        match self {
            Step::Write { path, .. } => format!("Write {path}"),
            Step::MakeDir { path } => format!("Make the folder {path}"),
            Step::Run(args) => format!("Run {}", args.join(" ")),
            Step::TrustKey {
                path, fingerprint, ..
            } => format!("Check the key's fingerprint is {fingerprint}, then put it at {path}"),
        }
    }
}

/// What `--system` would do here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The steps, in order.
    pub steps: Vec<Step>,
    /// What stops it from running.
    pub surprises: Vec<String>,
}

/// Works out the plan by reading the host. The commands it runs only ask.
pub fn plan(env: &Env, spec: &HostSpec, runner: &mut dyn Runner) -> Result<Plan, String> {
    let mut plan = Plan::default();
    match &env.os {
        Os::Apt { .. } => {}
        Os::Nix => return Err(NIXOS.to_owned()),
        Os::Mac => return Err("maxd host apply --system is for Debian and Ubuntu".to_owned()),
        Os::Other(id) => {
            return Err(format!(
                "maxd host apply --system is for Debian and Ubuntu, and this host runs {id}. Set it up by hand from maxd host check."
            ))
        }
    }
    if let Some(want) = spec
        .service
        .user
        .as_ref()
        .filter(|want| **want != env.user.name)
    {
        return Err(format!(
            "The spec says {want} runs maxd, so run this through sudo as {want}, not {}",
            env.user.name
        ));
    }

    packages(env, spec, runner, &mut plan);
    let user = env.user.name.as_str();
    if spec.incus.enable && !in_group(env, INCUS_GROUP, user) {
        plan.steps
            .push(Step::run(&["usermod", "-aG", INCUS_GROUP, user]));
    }
    if !env.at(&format!("/var/lib/systemd/linger/{user}")).exists() {
        plan.steps
            .push(Step::run(&["loginctl", "enable-linger", user]));
    }
    if let Some(dir) = &spec.service.data_dir {
        match std::fs::metadata(env.at(dir)) {
            Err(_) => plan.steps.push(Step::MakeDir { path: dir.clone() }),
            Ok(meta) if !meta.is_dir() => {
                plan.surprises
                    .push(format!("{dir} is a file, not a folder"));
            }
            Ok(meta) if meta.uid() != env.user.uid => plan.surprises.push(format!(
                "{dir} belongs to uid {}, not {user}. Give it to {user} or name another folder.",
                meta.uid()
            )),
            Ok(meta) if meta.mode() & 0o077 != 0 => {
                plan.steps.push(Step::run(&["chmod", "0700", dir]));
            }
            Ok(_) => {}
        }
    }

    let wants = files(env, spec);
    let mut written = Vec::new();
    let mut write = |plan: &mut Plan, path: &str, text: String| {
        match env.read(path) {
            Some(have) if have == text => return false,
            Some(have) if !have.starts_with(MARKER) => {
                plan.surprises.push(format!(
                    "{path} is there already and maxd didn't write it. Move it away or merge it by hand, then run this again."
                ));
                return false;
            }
            _ => {}
        }
        written.push(path.to_owned());
        plan.steps.push(Step::Write {
            path: path.to_owned(),
            text,
        });
        true
    };
    for want in &wants {
        for (path, text) in &want.files {
            write(&mut plan, path, text.clone());
        }
    }
    let zram = spec.caps.zram && write(&mut plan, ZRAM_CONF, zram_conf());
    if let Some(conf) = memory_low(spec) {
        write(&mut plan, MEMORY_LOW_CONF, format!("{MARKER}\n{conf}"));
    }
    if written
        .iter()
        .any(|path| path.starts_with(UNITS) || path == ZRAM_CONF)
    {
        plan.steps.push(Step::run(&["systemctl", "daemon-reload"]));
    }

    // Start what's new or stopped. The shutdown unit only starts, since
    // restarting it would run its stop.
    for want in &wants {
        let (path, _) = &want.files[0];
        let name = path.rsplit('/').next().unwrap_or_default();
        let changed = want.files.iter().any(|(path, _)| written.contains(path));
        let up = |runner: &mut dyn Runner, name: &str| {
            asks(runner, &["systemctl", "is-enabled", "--quiet", name])
                && asks(runner, &["systemctl", "is-active", "--quiet", name])
        };
        if changed && want.id != "units.shutdown" && up(runner, name) {
            plan.steps.push(Step::run(&["systemctl", "restart", name]));
        } else if !up(runner, name) {
            plan.steps
                .push(Step::run(&["systemctl", "enable", "--now", name]));
        }
        if want.id == "egress" {
            let timer = "maxd-egress-check.timer";
            if !up(runner, timer) {
                plan.steps
                    .push(Step::run(&["systemctl", "enable", "--now", timer]));
            }
        }
    }
    if spec.caps.zram {
        let swaps = env.read("/proc/swaps").unwrap_or_default();
        let on = swaps.lines().any(|line| line.starts_with("/dev/zram"));
        if zram || !on {
            plan.steps.push(Step::run(&[
                "systemctl",
                "restart",
                "systemd-zram-setup@zram0.service",
            ]));
        }
    }
    if let Some(low) = spec.caps.system_memory_low {
        let have = env.read("/sys/fs/cgroup/system.slice/memory.low");
        if have.as_deref().map(str::trim) != Some(low.0.to_string().as_str()) {
            let property = format!("MemoryLow={}", low.0);
            plan.steps.push(Step::run(&[
                "systemctl",
                "set-property",
                "--runtime",
                "system.slice",
                &property,
            ]));
        }
    }
    Ok(plan)
}

/// True when a command that only asks says yes.
fn asks(runner: &mut dyn Runner, args: &[&str]) -> bool {
    runner.run(args[0], &args[1..]).is_ok_and(|ran| ran.success)
}

/// True when the package is installed.
fn installed(runner: &mut dyn Runner, package: &str) -> bool {
    runner
        .run("dpkg-query", &["-W", "-f=${Status}", package])
        .is_ok_and(|ran| ran.success && ran.stdout.trim() == "install ok installed")
}

/// The packages the spec needs, and Zabbly's repository for Incus where
/// the release has none.
fn packages(env: &Env, spec: &HostSpec, runner: &mut dyn Runner, plan: &mut Plan) {
    let mut wanted: Vec<&str> = Vec::new();
    if spec.incus.enable {
        wanted.push("incus");
    }
    for tool in &spec.tools.system {
        wanted.push(tool.as_str());
    }
    if spec.caps.zram {
        wanted.push("systemd-zram-generator");
    }
    if spec.incus.enable && spec.incus.bridge.is_some() {
        wanted.push("nftables");
    }
    wanted.sort_unstable();
    wanted.dedup();
    wanted.retain(|package| !installed(runner, package));
    if wanted.is_empty() {
        return;
    }
    if wanted.contains(&"incus") && !env.os.packages_incus() {
        let Os::Apt {
            codename: Some(codename),
            ..
        } = &env.os
        else {
            plan.surprises.push(
                "This release's archive has no Incus, and maxd can't tell its codename to add Zabbly's".to_owned(),
            );
            return;
        };
        if !env.at(ZABBLY_KEY).exists() {
            let mut fetch = vec!["curl", "ca-certificates", "gnupg"];
            fetch.retain(|package| !installed(runner, package));
            if !fetch.is_empty() {
                plan.steps.push(apt(&["update"]));
                plan.steps.push(install(&fetch));
            }
            plan.steps
                .push(Step::run(&["mkdir", "-p", "/etc/apt/keyrings"]));
            plan.steps.push(Step::run(&[
                "curl",
                "-fsSL",
                "-o",
                ZABBLY_DOWNLOAD,
                ZABBLY_KEY_URL,
            ]));
            plan.steps.push(Step::TrustKey {
                download: ZABBLY_DOWNLOAD.to_owned(),
                path: ZABBLY_KEY.to_owned(),
                fingerprint: ZABBLY_FINGERPRINT.to_owned(),
            });
        }
        let text = zabbly_list(codename);
        match env.read(ZABBLY_LIST) {
            Some(have) if have == text => {}
            Some(have) if !have.starts_with(MARKER) => plan.surprises.push(format!(
                "{ZABBLY_LIST} is there already and maxd didn't write it. Move it away, then run this again."
            )),
            _ => plan.steps.push(Step::Write {
                path: ZABBLY_LIST.to_owned(),
                text,
            }),
        }
    }
    plan.steps.push(apt(&["update"]));
    plan.steps.push(install(&wanted));
}

/// An apt-get command that never stops to ask.
fn apt(args: &[&str]) -> Step {
    let mut all = vec!["env", "DEBIAN_FRONTEND=noninteractive", "apt-get"];
    all.extend_from_slice(args);
    Step::run(&all)
}

fn install(packages: &[&str]) -> Step {
    let mut args = vec!["install", "-y", "--no-install-recommends"];
    args.extend_from_slice(packages);
    apt(&args)
}

/// Runs one step.
fn run_step(env: &Env, step: &Step, runner: &mut dyn Runner) -> Result<(), String> {
    match step {
        Step::Write { path, text } => {
            write_file(&env.at(path), text).map_err(|err| format!("couldn't write {path}: {err}"))
        }
        Step::MakeDir { path } => {
            let dir = env.at(path);
            std::fs::create_dir_all(&dir)
                .and_then(|()| {
                    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                })
                .and_then(|()| {
                    std::os::unix::fs::chown(&dir, Some(env.user.uid), Some(env.user.gid))
                })
                .map_err(|err| format!("couldn't make {path}: {err}"))
        }
        Step::TrustKey {
            download,
            path,
            fingerprint,
        } => trust_key(env, runner, download, path, fingerprint),
        Step::Run(args) => {
            let words: Vec<&str> = args.iter().map(String::as_str).collect();
            let ran = runner
                .run(words[0], &words[1..])
                .map_err(|err| format!("couldn't run {}: {err}", words[0]))?;
            if ran.success {
                Ok(())
            } else {
                let why = ran.stderr.trim();
                Err(format!("{} failed: {why}", args.join(" ")))
            }
        }
    }
}

/// Moves a downloaded key where apt trusts it, if its one primary key has
/// `fingerprint`. Otherwise it removes the download.
fn trust_key(
    env: &Env,
    runner: &mut dyn Runner,
    download: &str,
    path: &str,
    fingerprint: &str,
) -> Result<(), String> {
    let shown = runner
        .run("gpg", &["--show-keys", "--with-colons", download])
        .map_err(|err| format!("couldn't run gpg: {err}"))?;
    let found = shown.success.then(|| fingerprints(&shown.stdout));
    if found.as_deref() != Some(&[fingerprint.to_owned()][..]) {
        let _ = std::fs::remove_file(env.at(download));
        let have = match found {
            Some(found) if !found.is_empty() => found.join(", "),
            _ => "no key gpg could read".to_owned(),
        };
        return Err(format!(
            "The key from {ZABBLY_KEY_URL} isn't Zabbly's: it should have fingerprint {fingerprint}, and it has {have}. maxd removed it and didn't trust it"
        ));
    }
    let to = env.at(path);
    std::fs::set_permissions(env.at(download), std::fs::Permissions::from_mode(0o644))
        .and_then(|()| std::fs::rename(env.at(download), &to))
        .map_err(|err| format!("couldn't put the key at {path}: {err}"))
}

/// The primary keys' fingerprints in `gpg --with-colons` output: the
/// first `fpr` line after each `pub` line.
fn fingerprints(colons: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut primary = false;
    for line in colons.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        match fields[0] {
            "pub" => primary = true,
            "fpr" if primary => {
                primary = false;
                found.push(fields.get(9).copied().unwrap_or_default().to_owned());
            }
            "sub" => primary = false,
            _ => {}
        }
    }
    found
}

/// How `--system` gets a yes.
pub enum Ask<'a> {
    /// `--yes` was given.
    Yes,
    /// Asks on the terminal.
    Terminal(&'a mut dyn FnMut() -> bool),
    /// There's no terminal and no `--yes`, so it doesn't go ahead.
    NoTerminal,
}

/// Writes a file whole, through a temp file beside it.
fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".maxd-new");
    std::fs::write(&temp, text)?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&temp, path)
}

/// Plans, prints the plan, gets a yes through `ask`, and runs the steps in
/// order, stopping at the first that fails.
pub fn apply(
    env: &Env,
    runner: &mut dyn Runner,
    ask: Ask<'_>,
    out: &mut dyn std::io::Write,
) -> Result<(), String> {
    let loaded = load(env)?;
    let plan = plan(env, &loaded.spec, runner)?;
    let print = |out: &mut dyn std::io::Write, text: &str| {
        let _ = writeln!(out, "{text}");
    };
    print(out, &super::source_line(&loaded.source));
    if !plan.surprises.is_empty() {
        print(out, "maxd changed nothing, because:");
        for surprise in &plan.surprises {
            print(out, &format!("  {surprise}"));
        }
        return Err("Fix what's above, then run this again".to_owned());
    }
    if plan.steps.is_empty() {
        print(out, "Nothing to change.");
        return Ok(());
    }
    print(out, "maxd will:");
    for (n, step) in plan.steps.iter().enumerate() {
        print(out, &format!("  {}. {}", n + 1, step.line()));
    }
    match ask {
        Ask::Yes => {}
        Ask::NoTerminal => return Err(NO_TERMINAL.to_owned()),
        Ask::Terminal(confirm) => {
            if !confirm() {
                return Err("Stopped. Nothing changed.".to_owned());
            }
        }
    }
    for (n, step) in plan.steps.iter().enumerate() {
        print(out, &format!("{}. {}", n + 1, step.line()));
        if let Err(err) = run_step(env, step, runner) {
            return Err(format!("{err}. maxd stopped there, after {n} steps."));
        }
    }
    print(
        out,
        &format!(
            "Done. Next, run maxd host apply as {} for the rest.",
            env.user.name
        ),
    );
    Ok(())
}

/// The service user's view of this host, for `--system` run through sudo.
pub fn service_env(root: &Path) -> Result<Env, String> {
    if !rustix::process::geteuid().is_root() {
        return Err(
            "maxd host apply --system needs root. Run it as sudo maxd host apply --system."
                .to_owned(),
        );
    }
    let os = Os::read(root);
    if os == Os::Nix {
        return Err(NIXOS.to_owned());
    }
    let name = std::env::var("SUDO_USER")
        .ok()
        .filter(|name| !name.is_empty() && name != "root")
        .ok_or("Run this through sudo as the user maxd runs as, not as root itself")?;
    let user = User::lookup(root, &name).ok_or_else(|| format!("There's no user {name} here"))?;
    Ok(Env {
        maxd: super::this_maxd(&user.home),
        root: root.to_owned(),
        os,
        user,
        incus: None,
        service: None,
        find: Box::new(crate::service::find_program),
    })
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One-click setup of a device over your own SSH login: check its host
//! key, install `maxd`, start it as a service and let MaxHQ's own key in
//! through `maxd proxy` only.
//!
//! A Linux device runs each step in `sh`. A Windows PC, found when that
//! fails or runs in a POSIX layer, runs them in PowerShell instead
//! ([`windows`]). A WSL distro runs the Linux steps through `wsl.exe`
//! ([`wsl`]).
//!
//! Nothing runs on the device before you confirm a host key ssh hasn't
//! seen. Secrets never pass through here: the app sends them over the
//! link once the device answers. Every command is built from fixed text
//! and checked values, so a host name can't become an option or a shell
//! word.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use maxhq_core::authorized_keys;
use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::paths::MAXD_LINK_TEMP;
use maxhq_protocol::agents::PeerRoute;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::oneshot;

pub use windows::{shell_refusal, target_for_windows, SSHD_STEPS};
pub use wsl::over_ssh as wsl_over_ssh;

mod windows;
mod wsl;

/// How long a setup waits for you to confirm a host key.
pub const CONFIRM_WITHIN: Duration = Duration::from_secs(10 * 60);

/// How long each command may take. Uploading `maxd` over a slow link is
/// the longest.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The host key alias a set-up device's key is kept under, in MaxHQ's own
/// `known_hosts`.
pub fn host_key_alias(device: &str) -> String {
    format!("maxhq-dev-{device}")
}

/// Where setup finds MaxHQ's SSH key and keeps the host keys it confirms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPaths {
    /// MaxHQ's client key. Setup reads only its `.pub` half.
    pub key: PathBuf,
    /// MaxHQ's own `known_hosts`, never yours.
    pub known_hosts: PathBuf,
    /// Where setup keeps a host key while you look at it.
    pub scratch: PathBuf,
}

/// The one place the paths come from, so they can move in one edit: the
/// `ssh` folder in this machine's `maxd` data folder, which is
/// `~/Library/Application Support/MaxHQ/ssh/` on macOS and
/// `~/.local/share/maxhq/ssh/` on Linux. Taking it from
/// [`maxhq_core::paths::data_dir`] keeps the app reading the key the local
/// `maxd` writes, whatever each OS calls the folder.
pub fn paths() -> Option<SetupPaths> {
    let ssh = maxhq_core::paths::data_dir()?.join("ssh");
    Some(SetupPaths {
        key: ssh.join("id_ed25519"),
        known_hosts: ssh.join("known_hosts"),
        scratch: ssh.join("setup"),
    })
}

/// What a command printed, and whether it succeeded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Whether it exited 0.
    pub ok: bool,
    /// What it wrote to stdout.
    pub stdout: String,
    /// What it wrote to stderr.
    pub stderr: String,
}

/// Runs programs on this machine. Tests give a fake.
pub trait Run: Send + Sync {
    /// Runs `program` with `args`, feeding it `stdin`.
    fn run(
        &self,
        program: &str,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> impl Future<Output = io::Result<Output>> + Send;
}

/// Runs programs for real.
pub struct System;

impl Run for System {
    async fn run(
        &self,
        program: &str,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> io::Result<Output> {
        let mut command = Command::new(program);
        // A console program the app starts would open a window on Windows.
        maxhq_os::launcher::without_window(command.as_std_mut());
        let mut child = command
            .args(&args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(&bytes).await?;
            drop(pipe);
        }
        let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "it took too long"))??;
        Ok(Output {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The `maxd` setup sends, for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupBinary {
    /// Where it is on this machine.
    pub path: PathBuf,
    /// Its sha256, in hex.
    pub sha256: String,
}

/// How far a setup has got, for the wizard to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Checking whether ssh knows the host key.
    HostKey,
    /// Waiting for you to confirm a host key ssh hasn't seen.
    ConfirmHostKey {
        /// What to answer with.
        id: u32,
        /// The key's fingerprints, as `ssh-keygen -l` prints them.
        fingerprints: Vec<String>,
    },
    /// Looking at the device.
    Probe,
    /// Sending and installing `maxd`.
    Install,
    /// The service runs only while you're logged in, until you run this.
    Linger {
        /// The command, for you to run there.
        command: String,
    },
    /// Adding MaxHQ's key to `authorized_keys`.
    Key {
        /// The line, as it now is there.
        line: String,
    },
}

/// What setup learned and did, for the app to finish with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The SSH host, or for a WSL distro its route as one word
    /// ([`crate::wsl::route_word`]).
    pub host: String,
    /// The device's home folder.
    pub home: String,
    /// The Rust target its `maxd` was built for.
    pub target: String,
    /// The host's keys as `type base64` pairs, to keep under its alias
    /// once its hello gives its device ID.
    pub host_keys: Vec<String>,
    /// Whether you vouched for those keys: you confirmed them here, or ssh
    /// asks you before it keeps a host's key. Keys like these can take over
    /// from the ones kept for a device.
    pub host_keys_confirmed: bool,
    /// Whether its service keeps running after you log out.
    pub linger: bool,
}

/// Why a setup stopped, in words a window shows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SetupError(pub String);

fn stop(message: impl Into<String>) -> SetupError {
    SetupError(message.into())
}

/// Host keys waiting for your answer.
#[derive(Default)]
pub struct Confirmations {
    next: AtomicU32,
    waiting: Mutex<BTreeMap<u32, oneshot::Sender<bool>>>,
}

impl Confirmations {
    /// A new question, and where its answer comes.
    fn ask(&self) -> (u32, oneshot::Receiver<bool>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.lock().insert(id, tx);
        (id, rx)
    }

    /// Answers question `id`. False when nothing waits on it.
    pub fn answer(&self, id: u32, ok: bool) -> bool {
        self.lock()
            .remove(&id)
            .is_some_and(|tx| tx.send(ok).is_ok())
    }

    fn forget(&self, id: u32) {
        self.lock().remove(&id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, oneshot::Sender<bool>>> {
        self.waiting
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

/// The `Host` names in an ssh config, leaving out patterns and negations.
pub fn config_hosts(config: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some((key, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        for name in rest.split_whitespace() {
            if name.contains(['*', '?', '!']) || hosts.iter().any(|h| h == name) {
                continue;
            }
            hosts.push(name.to_owned());
        }
    }
    hosts
}

/// The Rust target for what `uname -sm` printed.
pub fn target_for(uname: &str) -> Option<&'static str> {
    let mut parts = uname.split_whitespace();
    let (os, arch) = (parts.next()?, parts.next()?);
    match (os, arch) {
        ("Linux", "x86_64" | "amd64") => Some("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64" | "arm64") => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

/// The `authorized_keys` line that lets MaxHQ's key run only
/// `maxd proxy`, through the stable symlink in `home`.
pub fn authorized_line(home: &str, public_key: &str) -> Result<String, SetupError> {
    authorized_keys::line(home, public_key, None).map_err(stop)
}

/// What the probe prints, one `key=value` a line.
const PROBE: &str = r#"echo "uname=$(uname -sm)"
echo "home=$HOME"
echo "user=$(id -un)"
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then echo systemd=yes; else echo systemd=no; fi
echo "linger=$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || echo unknown)"
if [ -x "$HOME/.local/bin/maxd" ]; then echo "build=$("$HOME/.local/bin/maxd" --build 2>/dev/null || true)"; fi
"#;

/// Installs the `maxd` that comes on stdin into `versions/<build>/`, after
/// checking its sum, and points the stable symlink at it. `$1` is the sum
/// `$2` the build and `$3` the symlink's temporary name. It holds no single
/// quote, so it fits in one.
const INSTALL: &str = r#"set -eu
v="$HOME/.local/lib/maxhq/versions/$2"
mkdir -p "$v" "$HOME/.local/bin"
t="$v/maxd.part"
cat > "$t"
s=$(sha256sum "$t" 2>/dev/null || shasum -a 256 "$t")
s=${s%% *}
if [ "$s" != "$1" ]; then rm -f "$t"; echo "the upload does not match its sha256" >&2; exit 3; fi
chmod 755 "$t"
mv -f "$t" "$v/maxd"
ln -sfn "$v/maxd" "$HOME/.local/bin/$3"
mv -f "$HOME/.local/bin/$3" "$HOME/.local/bin/maxd"
"#;

/// Has the `maxd` just installed add the restricted line for the public
/// key `$1` to `authorized_keys`, unless it's there already, through the
/// same writer it uses for other devices' keys.
const ADD_KEY: &str = r#""$HOME/.local/bin/maxd" authorize-key "$1""#;

/// The same, and then has it trust the app key `$2`, called `$3`, while the
/// device trusts no app key yet.
const ADD_KEYS: &str = r#""$HOME/.local/bin/maxd" authorize-key "$1" --app "$2" "$3""#;

/// A remote command that runs `script` with `args`, as one string for the
/// remote shell. The script and each arg go in single quotes.
fn remote(script: &str, args: &[&str]) -> String {
    debug_assert!(!script.contains('\''));
    let mut command = format!("sh -c '{script}' maxhq");
    for arg in args {
        debug_assert!(!arg.contains('\''));
        command.push_str(&format!(" '{arg}'"));
    }
    command
}

/// One setup round, over your own login.
pub struct Setup<'a, R: Run> {
    run: &'a R,
    paths: &'a SetupPaths,
    /// The SSH host, or for a WSL distro the Windows machine it's on,
    /// empty for this one.
    host: String,
    /// The WSL distro, for a round that sets one up.
    wsl: Option<String>,
    /// The ssh options that trust the host key you confirmed, when your
    /// own `known_hosts` didn't have it.
    trust: Vec<String>,
    /// The app key to hand over with MaxHQ's SSH key, and its label.
    app_key: Option<(String, String)>,
}

impl<'a, R: Run> Setup<'a, R> {
    /// A round for `host`, which must be a plain SSH destination.
    pub fn new(run: &'a R, paths: &'a SetupPaths, host: &str) -> Result<Self, SetupError> {
        let host = host.trim();
        if !crate::ssh_config::is_safe_destination(host) {
            return Err(stop(format!("{host} isn't an SSH host")));
        }
        Ok(Self {
            run,
            paths,
            host: host.to_owned(),
            wsl: None,
            trust: Vec::new(),
            app_key: None,
        })
    }

    /// Hands the app key `public_key`, base64url as the app proves with it,
    /// to the device along with MaxHQ's SSH key. The device trusts it while
    /// it trusts no other, and otherwise the app asks when it connects.
    /// `label` names it, such as "MaxHQ on studio-mac".
    pub fn with_app_key(mut self, public_key: &str, label: &str) -> Result<Self, SetupError> {
        let base64url = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if public_key.is_empty() || !public_key.chars().all(base64url) {
            return Err(stop("The app key isn't base64url"));
        }
        // One line in single quotes, as `remote` needs.
        let label = label
            .chars()
            .map(|c| match c {
                '\'' => '\u{2019}',
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();
        self.app_key = Some((public_key.to_owned(), label));
        Ok(self)
    }

    /// Checks the host key, installs `maxd`, starts its service and adds
    /// MaxHQ's restricted key. `binary` gives the `maxd` for a target.
    pub async fn prepare(
        mut self,
        binary: impl Fn(&str) -> Option<SetupBinary>,
        confirms: &Confirmations,
        step: &(dyn Fn(Step) + Sync),
    ) -> Result<Prepared, SetupError> {
        if self.wsl.is_some() {
            return self.prepare_wsl(binary, confirms, step).await;
        }
        step(Step::HostKey);
        let (host_keys, host_keys_confirmed) = self.host_key(confirms, step).await?;

        step(Step::Probe);
        // Windows has no `sh` unless a POSIX layer brings one, which isn't
        // where maxd runs, so either way the PowerShell probe follows.
        let probe = self.ssh(vec!["sh -s".to_owned()], Some(PROBE.into())).await;
        let posix_layer = probe.as_ref().is_ok_and(|probe| {
            parse_probe(probe)
                .get("uname")
                .is_some_and(|uname| windows::posix_layer(uname))
        });
        if (probe.is_err() || posix_layer)
            && let Some(facts) = self.windows_facts().await
        {
            let keys = (host_keys, host_keys_confirmed);
            return self.prepare_windows(facts, binary, step, keys).await;
        }
        let facts = parse_probe(&probe?);
        self.prepare_linux(facts, binary, step, (host_keys, host_keys_confirmed))
            .await
    }

    /// The rest of [`Setup::prepare`] on Linux, from its probe's facts.
    async fn prepare_linux(
        self,
        facts: BTreeMap<String, String>,
        binary: impl Fn(&str) -> Option<SetupBinary>,
        step: &(dyn Fn(Step) + Sync),
        (host_keys, host_keys_confirmed): (Vec<String>, bool),
    ) -> Result<Prepared, SetupError> {
        let home = facts.get("home").cloned().unwrap_or_default();
        let uname = facts.get("uname").map_or("", String::as_str);
        let Some(target) = target_for(uname) else {
            return Err(stop(format!("MaxHQ can't run on {uname} yet")));
        };
        if facts.get("systemd").map(String::as_str) != Some("yes") {
            return Err(stop(match &self.wsl {
                Some(distro) => wsl::no_systemd(distro),
                None => {
                    "The device has no systemd user session, which maxd's service needs".to_owned()
                }
            }));
        }
        let binary = checked(binary(target), target)?;

        step(Step::Install);
        let build = &binary.sha256[..16];
        if facts.get("build").map(String::as_str) != Some(build) {
            let bytes = std::fs::read(&binary.path)
                .map_err(|error| stop(format!("Reading maxd failed: {error}")))?;
            self.linux(
                INSTALL,
                &[&binary.sha256, build, MAXD_LINK_TEMP],
                Some(bytes),
            )
            .await?;
        }
        self.linux("\"$HOME/.local/bin/maxd\" service install", &[], None)
            .await?;
        let linger = facts.get("linger").map(String::as_str) == Some("yes");
        if !linger {
            let user = facts
                .get("user")
                .map(String::as_str)
                .filter(|user| is_plain_user(user))
                .unwrap_or("$USER");
            let command = match &self.wsl {
                Some(distro) => wsl::linger(distro, user),
                None => format!("sudo loginctl enable-linger {user}"),
            };
            step(Step::Linger { command });
        }

        let public = self.public_key()?;
        let line = authorized_line(&home, &public)?;
        let key = key_of(&public);
        match &self.app_key {
            Some((app_key, label)) => self.linux(ADD_KEYS, &[&key, app_key, label], None).await,
            None => self.linux(ADD_KEY, &[&key], None).await,
        }?;
        step(Step::Key { line });

        let host = match &self.wsl {
            Some(distro) => {
                crate::wsl::route_word(distro, (!self.host.is_empty()).then_some(&*self.host))
            }
            None => self.host,
        };
        Ok(Prepared {
            host,
            home,
            target: target.to_owned(),
            host_keys,
            host_keys_confirmed,
            linger,
        })
    }

    /// The host's keys, and whether you vouched for them. One your
    /// `known_hosts` lacks waits for your OK, and later commands trust only
    /// it. One it has counts when ssh would have asked you before keeping it.
    async fn host_key(
        &mut self,
        confirms: &Confirmations,
        step: &(dyn Fn(Step) + Sync),
    ) -> Result<(Vec<String>, bool), SetupError> {
        let keys = known_host_keys(self.run, &self.host).await;
        if !keys.is_empty() {
            let asked = crate::ssh_config::resolve_with(self.run, &self.host)
                .await
                .is_ok_and(|config| !config.keeps_keys_unasked());
            return Ok((keys, asked));
        }
        std::fs::create_dir_all(&self.paths.scratch)
            .map_err(|error| stop(format!("Making a scratch folder failed: {error}")))?;
        let scratch = self
            .paths
            .scratch
            .join(format!("{}.known_hosts", fastrand::u64(..)));
        let _ = std::fs::remove_file(&scratch);
        let file = format!("UserKnownHostsFile={}", scratch.display());
        // Your config still applies, since it may be what reaches the host,
        // but this only reads the key: it offers no login, so your public
        // keys stay here, and nothing of yours gets forwarded. A jump host on
        // the way still logs in as usual, with its own ssh.
        let args = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            file.clone(),
            "-o".into(),
            "PreferredAuthentications=none".into(),
            "-o".into(),
            "ForwardAgent=no".into(),
            "-o".into(),
            "ForwardX11=no".into(),
            "-o".into(),
            "ClearAllForwardings=yes".into(),
            "-o".into(),
            "PermitLocalCommand=no".into(),
            "--".into(),
            self.host.clone(),
            "true".into(),
        ];
        // It's refused at the login, and that's fine: ssh checks the key first.
        let _ = self.local("ssh", args).await?;
        let text = std::fs::read_to_string(&scratch).unwrap_or_default();
        let Some(shown) = text.lines().find(|line| !key_lines(line).is_empty()) else {
            return Err(stop(format!(
                "{} didn't show a host key. Check that its SSH server is on. {SSHD_STEPS}",
                self.host
            )));
        };
        // You see one key, so that key is the only one that can count.
        let keys = key_lines(shown);
        std::fs::write(&scratch, format!("{shown}\n"))
            .map_err(|error| stop(format!("Keeping the host key failed: {error}")))?;
        let listed = self
            .local(
                "ssh-keygen",
                vec!["-l".into(), "-f".into(), scratch.display().to_string()],
            )
            .await?;
        let fingerprints = listed
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let (id, answer) = confirms.ask();
        step(Step::ConfirmHostKey { id, fingerprints });
        let ok = match tokio::time::timeout(CONFIRM_WITHIN, answer).await {
            Ok(Ok(ok)) => ok,
            _ => {
                confirms.forget(id);
                false
            }
        };
        if !ok {
            let _ = std::fs::remove_file(&scratch);
            return Err(stop(
                "The host key wasn't confirmed. MaxHQ contacted the host to read its key and went no further",
            ));
        }
        self.trust = vec![
            "-o".into(),
            file,
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
        ];
        Ok((keys, true))
    }

    /// Runs `script` in `sh` on the device with `args`, as `$1` onwards:
    /// over your own login, or in the WSL distro.
    async fn linux(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<String, SetupError> {
        match &self.wsl {
            Some(distro) => self.in_distro(distro, script, args, stdin).await,
            None => self.ssh(vec![remote(script, args)], stdin).await,
        }
    }

    /// Runs a remote command over your own login.
    async fn ssh(
        &self,
        command: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> Result<String, SetupError> {
        let mut args = vec!["-o".to_owned(), "BatchMode=yes".to_owned()];
        args.extend(self.trust.iter().cloned());
        args.push("--".into());
        args.push(self.host.clone());
        args.extend(command);
        let output = self
            .run
            .run("ssh", args, stdin)
            .await
            .map_err(|error| stop(format!("ssh didn't run: {error}")))?;
        if !output.ok {
            let why = output.stderr.trim();
            return Err(stop(if why.is_empty() {
                format!("A step on {} failed", self.host)
            } else {
                format!("A step on {} failed: {why}", self.host)
            }));
        }
        Ok(output.stdout)
    }

    /// MaxHQ's public SSH key, as its `.pub` file has it.
    fn public_key(&self) -> Result<String, SetupError> {
        std::fs::read_to_string(self.paths.key.with_extension("pub"))
            .map_err(|_| stop("MaxHQ's SSH key isn't made yet"))
    }

    async fn local(&self, program: &str, args: Vec<String>) -> Result<Output, SetupError> {
        self.run
            .run(program, args, None)
            .await
            .map_err(|error| stop(format!("{program} didn't run: {error}")))
    }
}

/// The `maxd` the app has for `target`, with a sum that can name a build.
fn checked(binary: Option<SetupBinary>, target: &str) -> Result<SetupBinary, SetupError> {
    let Some(binary) = binary else {
        return Err(stop(format!("This app has no maxd for {target}")));
    };
    if !is_hex(&binary.sha256) || binary.sha256.len() != 64 {
        return Err(stop("The maxd this app carries has a bad sha256"));
    }
    Ok(binary)
}

/// A public key line's type and key, without its comment.
fn key_of(public: &str) -> String {
    public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `key=value` lines the probe printed.
fn parse_probe(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// The `type base64` pairs in `known_hosts` lines, skipping comments.
/// Whether a user name the device gave is safe to show in a command you might
/// paste. The device writes this name itself, so it could hold a command.
fn is_plain_user(user: &str) -> bool {
    !user.is_empty()
        && !user.starts_with('-')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn key_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let first = words.next()?;
            let (kind, key) = if first.starts_with('@') {
                // A marker such as @revoked: never trust it.
                return None;
            } else {
                (words.next()?, words.next()?)
            };
            Some(format!("{kind} {key}"))
        })
        .collect()
}

/// The keys in `known_hosts` lines marked `@revoked`, as `key_lines` gives
/// keys.
fn revoked_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "@revoked" {
                return None;
            }
            let (_hosts, kind, key) = (words.next()?, words.next()?, words.next()?);
            Some(format!("{kind} {key}"))
        })
        .collect()
}

fn is_hex(text: &str) -> bool {
    text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The host keys your own `known_hosts` has for `host`, as `type base64`
/// pairs, found the way ssh finds them: under its `HostKeyAlias` or its
/// real name and port, in the files `UserKnownHostsFile` names. None when
/// ssh can't resolve the host or knows no key for it.
pub async fn known_host_keys(run: &impl Run, host: &str) -> Vec<String> {
    let Ok(config) = crate::ssh_config::resolve_with(run, host).await else {
        return Vec::new();
    };
    let name = config.known_hosts_name();
    let mut keys: Vec<String> = Vec::new();
    let mut revoked: Vec<String> = Vec::new();
    let own = config.known_hosts_files.iter().map(|file| (file, true));
    let global = config.global_known_hosts_files.iter();
    for (file, yours) in own.chain(global.map(|file| (file, false))) {
        let args = vec!["-F".into(), name.clone(), "-f".into(), file.clone()];
        let Ok(found) = run.run("ssh-keygen", args, None).await else {
            continue;
        };
        if !found.ok {
            continue;
        }
        revoked.extend(revoked_keys(&found.stdout));
        // The machine's files count only for what they revoke.
        if !yours {
            continue;
        }
        for key in key_lines(&found.stdout) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    // Revoked in one file counts for them all, whatever a plain line says.
    keys.retain(|key| !revoked.contains(key));
    keys
}

/// Where another device reaches `destination` by itself, as `ssh -G` on
/// this machine resolves it, or none when it needs a jump host or a proxy
/// or ssh can't say.
pub async fn direct_route(destination: &str) -> Option<PeerRoute> {
    crate::ssh_config::resolve(destination).await.ok()?.direct()
}

/// The ed25519 host key setup kept for a device, as `ssh-ed25519 <base64>`,
/// or none when setup never confirmed one.
pub fn pinned_host_key(known_hosts: &Path, device: &str) -> Option<String> {
    let alias = host_key_alias(device);
    let text = std::fs::read_to_string(known_hosts).ok()?;
    text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if words.next()? != alias || words.next()? != "ssh-ed25519" {
            return None;
        }
        Some(format!("ssh-ed25519 {}", words.next()?))
    })
}

/// Every host key MaxHQ keeps for a device under its alias, as `type
/// base64` pairs, in file order.
pub fn kept_host_keys(known_hosts: &Path, device: &str) -> Vec<String> {
    let alias = host_key_alias(device);
    let text = std::fs::read_to_string(known_hosts).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != alias {
                return None;
            }
            Some(format!("{} {}", words.next()?, words.next()?))
        })
        .collect()
}

/// Keeps a set-up device's host keys in MaxHQ's own `known_hosts` under its
/// alias, once each.
pub fn record_host_keys(known_hosts: &Path, device: &str, keys: &[String]) -> io::Result<()> {
    let _held = hold_known_hosts();
    write_host_keys(known_hosts, device, keys)
}

/// Makes `keys` the only host keys MaxHQ keeps for a set-up device, after a
/// setup you confirmed, so a key the device no longer has stops being its
/// pin. With no ed25519 key among them there's nothing to pin, so the old
/// pin stays and they're only added.
pub fn replace_host_keys(known_hosts: &Path, device: &str, keys: &[String]) -> io::Result<()> {
    let _held = hold_known_hosts();
    if !keys.iter().any(|key| key.starts_with("ssh-ed25519 ")) {
        return write_host_keys(known_hosts, device, keys);
    }
    let alias = host_key_alias(device);
    let mut text = String::new();
    for line in std::fs::read_to_string(known_hosts)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.split_whitespace().next() != Some(alias.as_str()))
    {
        text.push_str(line);
        text.push('\n');
    }
    for key in keys {
        text.push_str(&format!("{alias} {key}\n"));
    }
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(known_hosts, text.as_bytes(), Atomic::PLAIN)
}

/// What [`pin_known_host`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pinning {
    /// It kept the ed25519 key your `known_hosts` has as the device's pin.
    Pinned,
    /// MaxHQ already had a pin, and your `known_hosts` agrees or has none.
    Kept,
    /// MaxHQ's pin isn't the key your `known_hosts` has, so the pin stays.
    Differs,
    /// Your `known_hosts` has no ed25519 key for the host, so nothing is
    /// pinned.
    Unknown,
}

/// Pins a linked device's host key, when MaxHQ has none for it, from the
/// ed25519 key your own `known_hosts` has for `host`. It never asks the
/// host for a key and never replaces a pin.
pub async fn pin_known_host(
    run: &impl Run,
    known_hosts: &Path,
    device: &str,
    host: &str,
) -> io::Result<Pinning> {
    let found: Vec<String> = known_host_keys(run, host)
        .await
        .into_iter()
        .filter(|key| key.starts_with("ssh-ed25519 "))
        .collect();
    let _held = hold_known_hosts();
    Ok(match pinned_host_key(known_hosts, device) {
        Some(pin) if found.is_empty() || found.contains(&pin) => Pinning::Kept,
        Some(_) => Pinning::Differs,
        None => match found.first() {
            Some(key) => {
                write_host_keys(known_hosts, device, std::slice::from_ref(key))?;
                Pinning::Pinned
            }
            None => Pinning::Unknown,
        },
    })
}

/// Holds MaxHQ's own `known_hosts` while one write reads and rewrites it,
/// so devices that link at once don't drop each other's keys.
fn hold_known_hosts() -> std::sync::MutexGuard<'static, ()> {
    static KNOWN_HOSTS: Mutex<()> = Mutex::new(());
    KNOWN_HOSTS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn write_host_keys(known_hosts: &Path, device: &str, keys: &[String]) -> io::Result<()> {
    let alias = host_key_alias(device);
    let mut text = std::fs::read_to_string(known_hosts).unwrap_or_default();
    let mut changed = false;
    for key in keys {
        let line = format!("{alias} {key}");
        if text.lines().any(|existing| existing.trim() == line) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(known_hosts, text.as_bytes(), Atomic::PLAIN)
}

#[cfg(test)]
mod tests;

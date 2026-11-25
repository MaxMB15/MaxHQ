// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Getting a byte stream to `maxd`: its Unix socket on this machine, or
//! `maxd proxy` over SSH on another, per decision A-8. The system `ssh`
//! sits behind the `system-ssh` feature, and the phone's own SSH behind
//! `russh`.

use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::link::Route;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::task::JoinHandle;

use crate::pinned::PeerDial;
use crate::ssh_config::SshConfig;

#[cfg(any(windows, feature = "russh"))]
mod channel;
#[cfg(feature = "russh")]
pub(crate) mod phone;
#[cfg(any(windows, feature = "russh", feature = "ws"))]
mod reach;
#[cfg(feature = "system-ssh")]
mod system;
#[cfg(feature = "ws")]
pub(crate) mod ws;

#[cfg(feature = "russh")]
pub use phone::PhoneSsh;
#[cfg(all(feature = "russh", feature = "ws"))]
pub(crate) use reach::Both;
#[cfg(all(test, feature = "russh"))]
pub(crate) use reach::Io;
#[cfg(feature = "ws")]
pub use ws::{Ended, Headers, PhoneWs, Posted, WsRoute, SIGN_IN_ENDED};

/// A phone route as the host its link dials: `ssh://user@host:port`, with
/// an IPv6 address in brackets. [`PhoneSsh::trust`] names its routes this
/// way, so the registry's routes are the ones it dials.
pub fn phone_route_host(route: &maxhq_protocol::phones::PhoneRoute) -> String {
    format!(
        "ssh://{}@{}",
        route.user,
        authority(&route.host, route.port)
    )
}

/// A device's network route as the host its link dials: `wss://host:port`,
/// with an IPv6 address in brackets. `PhoneWs::trust` names its routes
/// this way, as [`phone_route_host`] does for SSH.
pub fn ws_route_host(host: &str, port: u16) -> String {
    format!("wss://{}", authority(host, port))
}

/// `host:port`, with an IPv6 address in brackets.
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
#[cfg(feature = "system-ssh")]
pub(crate) use system::{Pinned, System};

/// How long reaching a host's SSH port gets, when its config sets no
/// `ConnectTimeout`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a whole SSH login gets, since a login that waits on a
/// prompt never times out on its own.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of the far side's stderr is kept.
const STDERR_TAIL: usize = 4096;

/// What the far side runs to reach `maxd`. A non-interactive SSH shell
/// may leave `~/.local/bin` off its PATH, as NixOS does, so the command
/// adds it at the end, where a `maxd` installed for the whole system
/// still wins. The script sits in single quotes, which sh, bash, zsh and
/// fish login shells all pass on unchanged.
const PROXY_COMMAND: &str =
    r#"sh -c 'PATH="$PATH:$HOME/.local/bin"; export PATH; exec maxd proxy'"#;

/// What a Windows device runs to reach `maxd`, through cmd or PowerShell,
/// which find it on the PATH that setting the device up adds it to. A
/// transport uses it for a host once [`lacks_sh`] said the host's shell
/// has no `sh`.
#[cfg(feature = "system-ssh")]
const WINDOWS_PROXY_COMMAND: &str = "maxd proxy";

/// Whether `maxd proxy`'s stderr says the far side's shell has no `sh`, as
/// cmd and PowerShell say on Windows. cmd names the program in quotes in
/// every language, so the quotes are what this looks for.
pub(crate) fn lacks_sh(stderr: &str) -> bool {
    stderr.contains("'sh'") || stderr.contains("\"sh\"")
}

/// A boxed reading half.
pub(crate) type Reader = Box<dyn AsyncRead + Send + Unpin>;

/// A boxed writing half.
pub(crate) type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// An open stream to `maxd`.
pub(crate) struct Conn {
    /// Bytes from `maxd`.
    pub reader: Reader,
    /// Bytes to `maxd`.
    pub writer: Writer,
    /// What else the stream needs.
    pub rest: Rest,
}

impl Conn {
    /// A stream with nothing behind it to keep alive.
    pub fn new(reader: Reader, writer: Writer) -> Self {
        Self {
            reader,
            writer,
            rest: Rest {
                stderr: None,
                _keep: None,
            },
        }
    }
}

/// The parts of a stream besides its halves. Dropping it ends the stream.
pub(crate) struct Rest {
    /// What `maxd proxy` wrote to stderr, for SSH.
    stderr: Option<StderrTail>,
    /// Keeps the far side open, such as the local `ssh` process.
    _keep: Option<Box<dyn Send>>,
}

impl Rest {
    /// Whether the far side's shell had no `sh` to run the proxy command,
    /// once [`Rest::closed_reason`] has read its stderr.
    pub fn lacked_sh(&self) -> bool {
        self.stderr
            .as_ref()
            .is_some_and(|tail| lacks_sh(&tail.text_now()))
    }

    /// Why the far side closed before saying hello, read from its stderr.
    pub async fn closed_reason(&mut self) -> String {
        let Some(tail) = self.stderr.as_mut() else {
            return "maxd closed the connection".to_owned();
        };
        proxy_failure(&tail.text().await)
    }
}

/// The last few kilobytes the far side wrote to stderr.
struct StderrTail {
    /// The bytes so far.
    text: Arc<Mutex<Vec<u8>>>,
    /// Ends when stderr closes.
    reading: Option<JoinHandle<()>>,
}

impl StderrTail {
    /// Starts reading a stream.
    fn read(mut stream: impl AsyncRead + Send + Unpin + 'static) -> Self {
        let text = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&text);
        let reading = tokio::spawn(async move {
            let mut buf = [0; 1024];
            while let Ok(n) = stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut text = sink.lock().expect("stderr lock");
                text.extend_from_slice(&buf[..n]);
                let excess = text.len().saturating_sub(STDERR_TAIL);
                text.drain(..excess);
            }
        });
        Self {
            text,
            reading: Some(reading),
        }
    }

    /// Waits a moment for stderr to close, then returns what it held.
    async fn text(&mut self) -> String {
        if let Some(reading) = self.reading.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), reading).await;
        }
        self.text_now()
    }

    /// What stderr held so far.
    fn text_now(&self) -> String {
        let text = self.text.lock().expect("stderr lock");
        String::from_utf8_lossy(&text).into_owned()
    }
}

impl Drop for StderrTail {
    fn drop(&mut self) {
        if let Some(reading) = self.reading.take() {
            reading.abort();
        }
    }
}

/// Why a dial failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DialError {
    /// What the app shows, such as "SSH sign-in was refused".
    pub reason: String,
    /// Whether the device itself didn't answer, as it doesn't while it
    /// restarts. A device that answered and refused is false.
    pub host_down: bool,
}

impl DialError {
    /// A failure on a device that answered.
    pub fn refused(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            host_down: false,
        }
    }

    /// A failure to reach the device at all.
    pub fn down(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            host_down: true,
        }
    }
}

/// How links reach devices. Tests swap in a fake.
pub(crate) trait Transport: Send + Sync + 'static {
    /// What `ssh -G` says about a destination.
    fn resolve(&self, destination: &str) -> BoxFuture<'static, io::Result<SshConfig>>;

    /// The addresses a host name resolves to, or none.
    fn lookup(&self, host: &str) -> BoxFuture<'static, Vec<IpAddr>>;

    /// Opens a stream to the `maxd` behind a route, on a link to `device`
    /// once the link knows it. The phone's logins go by both, since two
    /// networks can have the same private address.
    fn dial<'a>(
        &'a self,
        route: &'a Route,
        config: Option<&'a SshConfig>,
        device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>>;

    /// Drops the SSH master for a host after its connection went quiet,
    /// so the next dial logs in afresh.
    fn forget<'a>(&'a self, host: &'a str) -> BoxFuture<'a, ()>;

    /// Starts `maxd` on `host` the Windows way from now on, after its
    /// shell said it has no `sh`. Only the system's SSH tells hosts apart.
    fn shell_is_windows(&self, _host: &str) {}

    /// Runs one fixed command on an SSH host over its master, logging in
    /// first if there's none. Only the system `ssh` runs commands.
    #[cfg(feature = "system-ssh")]
    fn run<'a>(
        &'a self,
        _host: &'a str,
        _command: &'a str,
    ) -> BoxFuture<'a, io::Result<crate::setup::Output>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this connection can't run commands",
            ))
        })
    }

    /// How long a link waits on one route before it starts the next as
    /// well, or none to try them one at a time.
    fn stagger(&self) -> Option<Duration> {
        None
    }
}

/// Streams from a [`PeerDial`], in place of SSH.
pub(crate) struct Dialed(pub Arc<dyn PeerDial>);

impl Transport for Dialed {
    fn resolve(&self, _destination: &str) -> BoxFuture<'static, io::Result<SshConfig>> {
        Box::pin(async { Err(io::Error::other("a dialed route reads no ssh config")) })
    }

    fn lookup(&self, _host: &str) -> BoxFuture<'static, Vec<IpAddr>> {
        Box::pin(async { Vec::new() })
    }

    fn dial<'a>(
        &'a self,
        route: &'a Route,
        _config: Option<&'a SshConfig>,
        _device: Option<DeviceId>,
    ) -> BoxFuture<'a, Result<Conn, DialError>> {
        Box::pin(async move {
            let Route::Ssh { host } = route else {
                return Err(DialError::refused("a peer link goes over SSH"));
            };
            match self.0.dial(host).await {
                Ok((reader, writer)) => Ok(Conn::new(reader, writer)),
                Err(err) => Err(DialError::down(err.to_string())),
            }
        })
    }

    fn forget<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Reads `maxd proxy`'s stderr for why it ended before the hello.
pub(crate) fn proxy_failure(stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("couldn't reach maxd") {
        "maxd isn't running".to_owned()
    } else if lower.contains("maxd")
        && (lower.contains("not found")
            || lower.contains("unknown command")
            || lower.contains("is not recognized"))
    {
        "maxd isn't installed on that device".to_owned()
    } else if let Some(line) = stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        shorten(line)
    } else {
        "maxd closed the connection".to_owned()
    }
}

/// Cuts a line to a length a status can show.
fn shorten(line: &str) -> String {
    const MAX: usize = 120;
    match line.char_indices().nth(MAX) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn proxy_errors_become_reasons() {
        assert_eq!(
            proxy_failure(
                "maxd proxy: couldn't reach maxd at /run/user/1000/maxhq/maxd.sock: No such file or directory (os error 2). Is it running?"
            ),
            "maxd isn't running"
        );
        for missing in [
            "sh: line 1: exec: maxd: not found",
            "sh: 1: exec: maxd: not found",
            "sh: exec: line 1: maxd: not found",
            "bash: line 1: maxd: command not found\n",
            "zsh:1: command not found: maxd",
        ] {
            assert_eq!(
                proxy_failure(missing),
                "maxd isn't installed on that device",
                "{missing}"
            );
        }
        assert_eq!(
            proxy_failure(
                "'maxd' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n"
            ),
            "maxd isn't installed on that device"
        );
        assert_eq!(proxy_failure(""), "maxd closed the connection");
    }

    #[test]
    fn a_windows_shell_without_sh_is_noticed() {
        for windows in [
            "'sh' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n",
            "sh : The term 'sh' is not recognized as the name of a cmdlet, function, script file, or operable program.",
            "Der Befehl \"sh\" ist entweder falsch geschrieben oder\r\nkonnte nicht gefunden werden.\r\n",
        ] {
            assert!(lacks_sh(windows), "{windows}");
        }
        for unix in [
            "sh: 1: exec: maxd: not found",
            "bash: line 1: maxd: command not found",
            "maxd proxy: couldn't reach maxd at /run/user/1000/maxhq/maxd.sock",
            "",
        ] {
            assert!(!lacks_sh(unix), "{unix}");
        }
    }

    #[test]
    fn the_proxy_command_is_fixed() {
        assert_eq!(
            PROXY_COMMAND,
            r#"sh -c 'PATH="$PATH:$HOME/.local/bin"; export PATH; exec maxd proxy'"#
        );
    }

    /// Where a program is on this machine's PATH.
    fn find(program: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(program))
            .find(|path| path.is_file())
    }

    /// Writes a `maxd` that prints where it was found and its arguments.
    #[cfg(unix)]
    fn fake_maxd(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("maxd");
        std::fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\"\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn login_shells_find_maxd_in_the_home_folder_after_the_system() {
        let root = std::env::temp_dir().join(format!("maxhq-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let system = root.join("system");
        let empty = root.join("empty");
        fake_maxd(&home.join(".local/bin"), "home");
        fake_maxd(&system, "system");
        std::fs::create_dir_all(&empty).unwrap();
        let base = "/usr/bin:/bin";
        let with_system = format!("{}:{base}", system.display());

        // sshd runs the command through the login shell's -c.
        let mut ran = 0;
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Some(shell) = find(shell) else { continue };
            let run = |home: &Path, path: &str| {
                std::process::Command::new(&shell)
                    .arg("-c")
                    .arg(PROXY_COMMAND)
                    .env_clear()
                    .env("HOME", home)
                    .env("PATH", path)
                    .output()
                    .unwrap()
            };
            let out = |output: std::process::Output| String::from_utf8(output.stdout).unwrap();
            let name = shell.display();
            assert_eq!(out(run(&home, base)), "home proxy\n", "{name}");
            assert_eq!(out(run(&home, &with_system)), "system proxy\n", "{name}");
            let missing = run(&empty, base);
            assert!(!missing.status.success(), "{name}");
            assert_eq!(
                proxy_failure(&String::from_utf8_lossy(&missing.stderr)),
                "maxd isn't installed on that device",
                "{name}"
            );
            ran += 1;
        }
        let _ = std::fs::remove_dir_all(&root);
        assert!(ran > 0, "no shell to try");
    }

    #[test]
    fn long_reasons_are_cut() {
        let long = "x".repeat(300);
        assert_eq!(shorten(&long).chars().count(), 121);
        assert_eq!(shorten("short"), "short");
    }
}

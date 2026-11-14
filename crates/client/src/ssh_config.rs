// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `ssh -G <host>` says about a host: who to log in as, where it
//! really is, which hosts the connection goes through first and where ssh
//! looks its key up.
//!
//! Only the `system-ssh` feature runs `ssh -G`. Without it, as on the
//! phone, a link reads no config, and its routes say all there is.

#[cfg(feature = "system-ssh")]
use std::io;
#[cfg(feature = "system-ssh")]
use std::time::Duration;

#[cfg(feature = "system-ssh")]
use maxhq_protocol::agents::PeerRoute;

#[cfg(feature = "system-ssh")]
use crate::setup::{Run, System};

/// How long `ssh -G` gets before the client gives up on it. It only reads
/// config, so it should take milliseconds.
#[cfg(feature = "system-ssh")]
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// The options from `ssh -G` the client uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SshConfig {
    /// The login user.
    pub user: Option<String>,
    /// The name or address ssh connects to, after `HostName` and aliases.
    pub host_name: String,
    /// The port.
    pub port: Option<u16>,
    /// `ProxyJump` hops, nearest first, without any `ssh://` prefix.
    pub proxy_jump: Vec<String>,
    /// The `ProxyCommand`, with its `%` tokens as written.
    pub proxy_command: Option<String>,
    /// `StrictHostKeyChecking`, lowercased.
    pub strict_host_key_checking: Option<String>,
    /// `ConnectTimeout` in seconds.
    pub connect_timeout: Option<u64>,
    /// `HostKeyAlias`, the name ssh looks the host's key up under.
    pub host_key_alias: Option<String>,
    /// The `UserKnownHostsFile`s, as `ssh -G` expands them. Empty for
    /// `none`.
    pub known_hosts_files: Vec<String>,
    /// The machine's own `known_hosts` files, which an admin revokes keys in.
    pub global_known_hosts_files: Vec<String>,
}

impl SshConfig {
    /// Reads the `key value` lines `ssh -G` prints. Keys come out lowercase,
    /// and options left unset are missing or `none`.
    #[cfg(feature = "system-ssh")]
    pub fn parse(output: &str) -> Self {
        let mut config = Self::default();
        for line in output.lines() {
            let Some((key, value)) = line.trim().split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() || value.eq_ignore_ascii_case("none") {
                continue;
            }
            match key.to_ascii_lowercase().as_str() {
                "user" => config.user = Some(value.to_owned()),
                "hostname" => config.host_name = value.to_owned(),
                "port" => config.port = value.parse().ok(),
                "proxyjump" => {
                    config.proxy_jump = value
                        .split(',')
                        .map(|hop| hop.trim().trim_start_matches("ssh://").to_owned())
                        .filter(|hop| !hop.is_empty())
                        .collect();
                }
                "proxycommand" => config.proxy_command = Some(value.to_owned()),
                "stricthostkeychecking" => {
                    config.strict_host_key_checking = Some(value.to_ascii_lowercase());
                }
                "connecttimeout" => config.connect_timeout = value.parse().ok(),
                "hostkeyalias" => config.host_key_alias = Some(value.to_owned()),
                "userknownhostsfile" => config.known_hosts_files = known_hosts_files(value),
                "globalknownhostsfile" => {
                    config.global_known_hosts_files = known_hosts_files(value);
                }
                _ => {}
            }
        }
        config
    }

    /// Whether ssh keeps a new host's key here without asking you. `ssh -G`
    /// prints `false` for `no` and `off`.
    #[cfg(feature = "system-ssh")]
    pub fn keeps_keys_unasked(&self) -> bool {
        matches!(
            self.strict_host_key_checking.as_deref(),
            Some("accept-new" | "false" | "no" | "off")
        )
    }

    /// The hosts the route passes through before this one, nearest first:
    /// the `ProxyJump` hops, or else the hosts a `ProxyCommand` that runs
    /// ssh dials.
    pub fn hops(&self) -> Vec<String> {
        if !self.proxy_jump.is_empty() {
            return self.proxy_jump.clone();
        }
        self.proxy_command
            .as_deref()
            .map(proxy_command_hops)
            .unwrap_or_default()
    }

    /// The name ssh looks the host's key up under in `known_hosts`: its
    /// `HostKeyAlias`, or else its host name, as `[name]:port` on a port
    /// other than 22.
    #[cfg(feature = "system-ssh")]
    pub fn known_hosts_name(&self) -> String {
        if let Some(alias) = &self.host_key_alias {
            return alias.clone();
        }
        match self.port {
            Some(port) if port != 22 => format!("[{}]:{port}", self.host_name),
            _ => self.host_name.clone(),
        }
    }

    /// Where another device reaches this host by itself, with no host in
    /// between, or none when the route needs a jump host or a proxy.
    #[cfg(feature = "system-ssh")]
    pub fn direct(&self) -> Option<PeerRoute> {
        if !self.proxy_jump.is_empty() || self.proxy_command.is_some() {
            return None;
        }
        Some(PeerRoute {
            host: self.host_name.clone(),
            port: self.port.unwrap_or(22),
            user: self.user.clone()?,
        })
    }
}

/// Runs `ssh -G` for a destination, such as `dev@devbox` or a hop like
/// `bastion:2222`.
#[cfg(feature = "system-ssh")]
pub(crate) async fn resolve(destination: &str) -> io::Result<SshConfig> {
    resolve_with(&System, destination).await
}

/// [`resolve`], with `run` running `ssh -G`.
#[cfg(feature = "system-ssh")]
pub(crate) async fn resolve_with(run: &impl Run, destination: &str) -> io::Result<SshConfig> {
    if !is_safe_destination(destination) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an SSH destination",
        ));
    }
    let (host, port) = split_hop(destination);
    let mut args = vec!["-G".to_owned()];
    if let Some(port) = port {
        args.extend(["-p".to_owned(), port.to_string()]);
    }
    args.push("--".to_owned());
    args.push(host);
    let output = tokio::time::timeout(RESOLVE_TIMEOUT, run.run("ssh", args, None))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ssh -G took too long"))??;
    if !output.ok {
        return Err(io::Error::other("ssh -G failed"));
    }
    let config = SshConfig::parse(&output.stdout);
    if config.host_name.is_empty() {
        return Err(io::Error::other("ssh -G printed no host name"));
    }
    Ok(config)
}

/// The files in a `userknownhostsfile` line. `ssh -G` prints each one
/// expanded and apart by spaces, so a word that doesn't start a path goes
/// on the one before, which keeps a path with a space whole.
#[cfg(feature = "system-ssh")]
fn known_hosts_files(value: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for word in value.split(' ') {
        match files.last_mut() {
            Some(file) if !word.starts_with(['/', '~']) => {
                file.push(' ');
                file.push_str(word);
            }
            _ => files.push(word.to_owned()),
        }
    }
    files
}

/// What a shell reads as more than part of a word.
const SHELL_WORDS: &str = "'`\"$\\;&<>|(){}";

/// Whether a destination is safe to hand to ssh as its last argument. A host
/// that starts with `-`, even behind `ssh://`, a user or a bracket, would be
/// read as an option. OpenSSH before 9.6 expands the name in a shell for a
/// `ProxyCommand` with `%h`, so what 9.6 refuses is refused here too.
pub fn is_safe_destination(destination: &str) -> bool {
    let (hop, _) = split_hop(destination);
    let host = hop.rsplit_once('@').map_or(hop.as_str(), |(_, host)| host);
    !hop.is_empty()
        && !hop.starts_with('-')
        && !host.starts_with('-')
        && !destination
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || SHELL_WORDS.contains(c))
}

/// Splits a `ProxyJump` hop like `user@host:22` or `[fd7a::1]:22` into the
/// destination ssh takes and the port.
pub(crate) fn split_hop(hop: &str) -> (String, Option<u16>) {
    let hop = hop.trim_start_matches("ssh://");
    let host_start = hop.rfind('@').map_or(0, |at| at + 1);
    let (user, host) = hop.split_at(host_start);
    if let Some(bracketed) = host.strip_prefix('[') {
        let Some((address, rest)) = bracketed.split_once(']') else {
            return (hop.to_owned(), None);
        };
        let port = rest.strip_prefix(':').and_then(|port| port.parse().ok());
        return (format!("{user}{address}"), port);
    }
    if host.matches(':').count() == 1 {
        let (name, port) = host.split_once(':').expect("there's one colon");
        if let Ok(port) = port.parse() {
            return (format!("{user}{name}"), Some(port));
        }
    }
    (hop.to_owned(), None)
}

/// The ssh options that take an argument, so the token after them isn't
/// the destination.
const OPTIONS_WITH_ARGUMENT: &str = "BbcDEeFIiJLlmOopQRSWw";

/// The hosts a `ProxyCommand` reaches through ssh, nearest first. A command
/// like `ssh -J bastion dev@devbox ws-proxy %h` gives `bastion` and then
/// `dev@devbox`. A command that doesn't run ssh gives none.
pub(crate) fn proxy_command_hops(command: &str) -> Vec<String> {
    let words = shell_words(command);
    let mut words = words.iter().map(String::as_str).peekable();
    if words.peek() == Some(&"exec") {
        words.next();
    }
    let Some(program) = words.next() else {
        return Vec::new();
    };
    if program.rsplit('/').next() != Some("ssh") {
        return Vec::new();
    }

    let mut user = None;
    let mut jumps = Vec::new();
    let mut destination = None;
    while let Some(word) = words.next() {
        if word == "--" {
            destination = words.next();
            break;
        }
        let Some(flags) = word.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            destination = Some(word);
            break;
        };
        for (at, flag) in flags.char_indices() {
            if !OPTIONS_WITH_ARGUMENT.contains(flag) {
                continue;
            }
            let rest = &flags[at + flag.len_utf8()..];
            let argument = if rest.is_empty() {
                words.next().unwrap_or_default()
            } else {
                rest
            };
            match flag {
                'l' => user = Some(argument),
                'J' => jumps.extend(argument.split(',').map(str::to_owned)),
                'o' => {
                    if let Some((key, value)) = argument.split_once(['=', ' ']) {
                        match key.trim().to_ascii_lowercase().as_str() {
                            "user" => user = Some(value.trim()),
                            "proxyjump" => {
                                jumps.extend(value.trim().split(',').map(str::to_owned));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            break;
        }
    }

    let mut hops: Vec<String> = jumps
        .into_iter()
        .map(|hop| hop.trim().trim_start_matches("ssh://").to_owned())
        .filter(|hop| !hop.is_empty() && !hop.eq_ignore_ascii_case("none"))
        .collect();
    if let Some(destination) = destination.filter(|dest| !dest.contains('%')) {
        let destination = destination.trim_start_matches("ssh://");
        hops.push(match user {
            Some(user) if !destination.contains('@') => format!("{user}@{destination}"),
            _ => destination.to_owned(),
        });
    }
    hops
}

/// Splits a command line into words the way a POSIX shell would for simple
/// commands: quotes group, backslashes escape, and the first `;`, `|` or `&`
/// outside quotes ends the command.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                word.push(next);
                            }
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            ';' | '|' | '&' => break,
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

#[cfg(all(test, feature = "system-ssh"))]
mod tests {
    use super::*;

    /// `ssh -G ws-bravo`, trimmed to a few lines around the ones used.
    const WORKSPACE: &str = "\
host ws-bravo
user agent
hostname ws-bravo
port 22
addressfamily any
batchmode no
controlmaster auto
controlpath /Users/max/.ssh/agent-workspaces/%C
controlpersist 600
serveraliveinterval 30
stricthostkeychecking accept-new
connecttimeout none
proxycommand ssh dev@devbox ws-proxy %h
";

    /// `ssh -G dev@devbox`, trimmed the same way.
    const NODE: &str = "\
host devbox
user dev
hostname devbox
port 22
stricthostkeychecking ask
connecttimeout none
";

    #[test]
    fn reads_the_options_the_client_uses() {
        let config = SshConfig::parse(WORKSPACE);
        assert_eq!(config.user.as_deref(), Some("agent"));
        assert_eq!(config.host_name, "ws-bravo");
        assert_eq!(config.port, Some(22));
        assert_eq!(
            config.proxy_command.as_deref(),
            Some("ssh dev@devbox ws-proxy %h")
        );
        assert!(config.proxy_jump.is_empty());
        assert_eq!(
            config.strict_host_key_checking.as_deref(),
            Some("accept-new")
        );
        assert_eq!(config.connect_timeout, None);
        assert_eq!(config.hops(), ["dev@devbox"]);
    }

    #[test]
    fn only_a_config_that_asks_first_vouches_for_a_key() {
        for (value, unasked) in [
            ("accept-new", true),
            ("false", true),
            ("no", true),
            ("off", true),
            ("ask", false),
            ("true", false),
            ("yes", false),
        ] {
            let config = SshConfig::parse(&format!("stricthostkeychecking {value}\n"));
            assert_eq!(config.keeps_keys_unasked(), unasked, "{value}");
        }
        assert!(!SshConfig::parse("hostname devbox\n").keeps_keys_unasked());
    }

    #[test]
    fn hosts_without_a_proxy_have_no_hops() {
        let config = SshConfig::parse(NODE);
        assert_eq!(config.user.as_deref(), Some("dev"));
        assert_eq!(config.host_name, "devbox");
        assert_eq!(config.proxy_command, None);
        assert!(config.hops().is_empty());
    }

    #[test]
    fn proxy_jump_comes_before_proxy_command() {
        let config = SshConfig::parse(
            "hostname 10.0.0.5\nport 2222\nproxyjump ssh://edge,dev@devbox:2200\nproxycommand none\nconnecttimeout 7\n",
        );
        assert_eq!(config.host_name, "10.0.0.5");
        assert_eq!(config.port, Some(2222));
        assert_eq!(config.proxy_jump, ["edge", "dev@devbox:2200"]);
        assert_eq!(config.proxy_command, None);
        assert_eq!(config.connect_timeout, Some(7));
        assert_eq!(config.hops(), ["edge", "dev@devbox:2200"]);
    }

    #[test]
    fn only_hosts_reached_straight_have_a_direct_route() {
        assert_eq!(
            SshConfig::parse(NODE).direct(),
            Some(PeerRoute {
                host: "devbox".into(),
                port: 22,
                user: "dev".into(),
            })
        );
        assert_eq!(SshConfig::parse(WORKSPACE).direct(), None);
        assert_eq!(
            SshConfig::parse("user dev\nhostname 10.0.0.5\nproxyjump edge\n").direct(),
            None
        );
    }

    #[test]
    fn none_values_are_unset() {
        let config = SshConfig::parse(
            "hostname devbox\nproxyjump none\nproxycommand none\nuserknownhostsfile none\n",
        );
        assert!(config.proxy_jump.is_empty());
        assert_eq!(config.proxy_command, None);
        assert!(config.known_hosts_files.is_empty());
    }

    #[test]
    fn reads_where_ssh_looks_the_host_key_up() {
        let config = SshConfig::parse(
            "hostname devbox\nport 22\nhostkeyalias box\n\
             userknownhostsfile /home/dev/.ssh/known_hosts /home/dev/My Keys/hosts\n",
        );
        assert_eq!(config.host_key_alias.as_deref(), Some("box"));
        assert_eq!(
            config.known_hosts_files,
            ["/home/dev/.ssh/known_hosts", "/home/dev/My Keys/hosts"]
        );
    }

    #[test]
    fn host_keys_go_under_the_alias_or_the_host_and_port() {
        assert_eq!(SshConfig::parse(NODE).known_hosts_name(), "devbox");
        assert_eq!(
            SshConfig::parse("hostname devbox\nport 2200\n").known_hosts_name(),
            "[devbox]:2200"
        );
        assert_eq!(
            SshConfig::parse("hostname devbox\nport 2200\nhostkeyalias box\n").known_hosts_name(),
            "box"
        );
    }

    #[test]
    fn proxy_commands_give_the_host_they_dial() {
        assert_eq!(
            proxy_command_hops("ssh dev@devbox ws-proxy %h"),
            ["dev@devbox"]
        );
        assert_eq!(
            proxy_command_hops("/usr/bin/ssh -q -o BatchMode=yes -p 2200 devbox ws-proxy %h"),
            ["devbox"]
        );
        assert_eq!(
            proxy_command_hops("ssh -l dev -i ~/.ssh/key devbox nc %h %p"),
            ["dev@devbox"]
        );
        assert_eq!(
            proxy_command_hops("exec ssh -W %h:%p bastion.example.com"),
            ["bastion.example.com"]
        );
        assert_eq!(
            proxy_command_hops("ssh -J edge,relay -oUser=dev devbox ws-proxy %h"),
            ["edge", "relay", "dev@devbox"]
        );
        assert_eq!(
            proxy_command_hops("ssh -qp2200 'dev@devbox' \"ws-proxy\" %h"),
            ["dev@devbox"]
        );
        assert_eq!(
            proxy_command_hops("ssh -o 'ProxyJump ssh://edge' -- devbox"),
            ["edge", "devbox"]
        );
    }

    #[test]
    fn proxy_commands_that_dont_run_ssh_have_no_hops() {
        assert!(proxy_command_hops("nc %h %p").is_empty());
        assert!(proxy_command_hops("tailscale nc %h %p").is_empty());
        assert!(proxy_command_hops("sshpass ssh devbox").is_empty());
        assert!(proxy_command_hops("").is_empty());
        // The destination is the target itself, so there's no hop.
        assert!(proxy_command_hops("ssh %h").is_empty());
    }

    #[test]
    fn hops_split_into_destination_and_port() {
        assert_eq!(
            split_hop("dev@devbox:2200"),
            ("dev@devbox".into(), Some(2200))
        );
        assert_eq!(split_hop("ssh://edge"), ("edge".into(), None));
        assert_eq!(
            split_hop("max@[fd7a::1]:22"),
            ("max@fd7a::1".into(), Some(22))
        );
        assert_eq!(split_hop("fd7a::1"), ("fd7a::1".into(), None));
        assert_eq!(split_hop("devbox:ssh"), ("devbox:ssh".into(), None));
    }

    #[test]
    fn destinations_that_look_like_options_are_refused() {
        for kept in [
            "dev@devbox",
            "ssh://edge",
            "ssh://dev@edge:2200",
            "[fd7a::1]:22",
            "fe80::1%en0",
            "my_box.internal",
            "box+2",
            "a-b",
        ] {
            assert!(is_safe_destination(kept), "{kept}");
        }
        for refused in [
            "",
            "ssh://",
            "-oProxyCommand=true",
            "ssh://-oProxyCommand=true",
            "ssh://ssh://-oProxyCommand=true",
            "[-oProxyCommand=true]:22",
            "dev@-oProxyCommand=true",
            "ssh://dev@-oProxyCommand=true",
            "dev@[-oProxyCommand=true]:22",
            "devbox true",
            "dev\nbox",
            "dev\u{7f}box",
            "a'b",
            "a`b",
            "a\"b",
            "a$b",
            "a\\b",
            "a;b",
            "a&b",
            "a<b",
            "a>b",
            "a|b",
            "(a)",
            "{a}",
        ] {
            assert!(!is_safe_destination(refused), "{refused}");
        }
    }

    /// Records what it was asked to run, and answers like `ssh -G`.
    #[derive(Default)]
    struct Recording(std::sync::Mutex<Vec<Vec<String>>>);

    impl Run for Recording {
        async fn run(
            &self,
            _program: &str,
            args: Vec<String>,
            _stdin: Option<Vec<u8>>,
        ) -> io::Result<crate::setup::Output> {
            self.0.lock().unwrap().push(args);
            Ok(crate::setup::Output {
                ok: true,
                stdout: "hostname devbox\nport 22\n".to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn ssh_reads_the_host_after_the_end_of_its_options() {
        let run = Recording::default();
        resolve_with(&run, "devbox").await.unwrap();
        resolve_with(&run, "ssh://devbox:2200").await.unwrap();
        assert_eq!(
            *run.0.lock().unwrap(),
            [
                vec!["-G", "--", "devbox"],
                vec!["-G", "-p", "2200", "--", "devbox"]
            ]
        );
        assert!(resolve_with(&run, "ssh://-oProxyCommand=true")
            .await
            .is_err());
        assert_eq!(run.0.lock().unwrap().len(), 2, "that one never ran");
    }
}

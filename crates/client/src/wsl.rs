// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! WSL, the Linux distros a Windows machine runs. Each distro can be a
//! device of its own: `maxd` runs inside it as on any Linux machine, and a
//! client reaches it through `wsl.exe` on that Windows machine instead of
//! SSH. `maxd` on Windows lists them here, and so does setup.
//!
//! `wsl.exe -l -v` prints a table, the default distro marked with `*`,
//! with each one's WSL version. It fails when there are none, and so does
//! a WSL too old to know `-v`, so `-l -q` comes next: the names alone,
//! default first, and nothing at all when there are none. Both print
//! UTF-16 unless `WSL_UTF8` is set, which a WSL from before it ignores, so
//! [`decode`] reads either.
//!
//! `wsl.exe -d <distro> -- <command>` hands the rest of its command line to
//! the distro's shell as it is, quotes and all, so nothing here uses it.
//! `--exec` splits the rest into arguments the way every Windows program
//! does and runs them without a shell, so the arguments [`exec_args`]
//! makes arrive word for word.

use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;

use maxhq_protocol::link::Route;
use maxhq_protocol::model::WslDistro;

use crate::ssh_config::is_safe_destination;

/// The program.
pub const PROGRAM: &str = "wsl.exe";

/// What `sh -c` runs in a distro to start a link: the folder setup puts
/// `maxd` in goes on the `PATH` after the system's, as over SSH.
pub const PROXY_SCRIPT: &str = r#"PATH="$PATH:$HOME/.local/bin"; export PATH; exec maxd proxy"#;

/// What each script run in a distro starts with. WSLg may set
/// `XDG_RUNTIME_DIR` to a folder of its own, so the runtime folder
/// systemd gives the user's services is named again, where `maxd` keeps
/// its socket and `systemctl --user` finds its bus.
const RUNTIME: &str = r#"if [ -d "/run/user/$(id -u)" ]; then XDG_RUNTIME_DIR="/run/user/$(id -u)"; export XDG_RUNTIME_DIR; fi; "#;

/// `script` as a distro runs it, after [`RUNTIME`].
pub fn in_distro(script: &str) -> String {
    format!("{RUNTIME}{script}")
}

/// How long `wsl.exe` gets to list the distros.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);

/// The longest distro name taken.
const MAX_NAME: usize = 64;

/// Docker Desktop's own distros, which run Docker and nothing else.
const DOCKER: [&str; 2] = ["docker-desktop", "docker-desktop-data"];

/// Whether `name` can be a distro's name: letters, digits, `.`, `-` and
/// `_`, as `wsl.exe` allows, and no leading `-`, so it can't pass for a
/// flag.
pub fn is_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The arguments for `wsl.exe` that run `script` with `sh -c` in
/// `distro`, as its default user, followed by `args` as `$0` onwards.
pub fn exec_args(distro: &str, script: &str, args: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = ["-d", distro, "--exec", "sh", "-c", script]
        .into_iter()
        .map(str::to_owned)
        .collect();
    all.extend(args.iter().map(|arg| (*arg).to_owned()));
    all
}

/// The arguments for `wsl.exe` that start `maxd proxy` in `distro`.
pub fn proxy_args(distro: &str) -> Vec<String> {
    exec_args(distro, &in_distro(PROXY_SCRIPT), &[])
}

/// What a client runs over SSH on a Windows machine to reach the `maxd`
/// in its distro `distro`: that machine's own `maxd`, which starts
/// `wsl.exe` with [`proxy_args`]. None for a name that isn't a distro's.
pub fn via_command(distro: &str) -> Option<String> {
    is_distro_name(distro).then(|| format!("maxd proxy --wsl {distro}"))
}

/// What a WSL route written as one word starts with.
pub const WORD: &str = "wsl:";

/// A WSL route as one word, as the app keeps it with the SSH hosts and
/// names it to set one up: `wsl:<distro>` for a distro on this machine,
/// and `wsl:<distro>@<via>` for one on the Windows machine SSH destination
/// `via` reaches. A distro's name has no `@`, so the first one ends it.
pub fn route_word(distro: &str, via: Option<&str>) -> String {
    match via {
        Some(via) => format!("{WORD}{distro}@{via}"),
        None => format!("{WORD}{distro}"),
    }
}

/// The distro and Windows machine a [`route_word`] names, or none for a
/// word that isn't one, such as an SSH host.
pub fn parse_route_word(word: &str) -> Option<(String, Option<String>)> {
    let rest = word.strip_prefix(WORD)?;
    let (distro, via) = match rest.split_once('@') {
        Some((distro, via)) => (distro, Some(via)),
        None => (rest, None),
    };
    if !is_distro_name(distro) || via.is_some_and(|via| !is_safe_destination(via)) {
        return None;
    }
    Some((distro.to_owned(), via.map(str::to_owned)))
}

/// The route a [`route_word`] names.
pub fn route_of_word(word: &str) -> Option<Route> {
    let (distro, via) = parse_route_word(word)?;
    Some(Route::Wsl { distro, via })
}

/// A WSL route as one word, or none for another route.
pub fn word_of_route(route: &Route) -> Option<String> {
    match route {
        Route::Wsl { distro, via } => Some(route_word(distro, via.as_deref())),
        Route::Local | Route::Ssh { .. } => None,
    }
}

/// What listing the distros found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// This isn't Windows, so there's no WSL.
    Unsupported,
    /// `wsl.exe` isn't there, or WSL isn't turned on.
    NotInstalled,
    /// The distros, default first. None is fine: WSL is on, with none.
    Distros(Vec<WslDistro>),
}

impl Listing {
    /// The distros, when WSL is there.
    pub fn distros(self) -> Option<Vec<WslDistro>> {
        match self {
            Self::Distros(distros) => Some(distros),
            Self::Unsupported | Self::NotInstalled => None,
        }
    }
}

/// This machine's distros.
pub async fn list() -> Listing {
    if cfg!(windows) {
        list_with(PROGRAM).await
    } else {
        Listing::Unsupported
    }
}

/// The distros `program`, which stands in for `wsl.exe`, lists.
pub async fn list_with(program: &str) -> Listing {
    match output(program, &["--list", "--verbose"]).await {
        Ok(out) if out.status.success() => {
            if let Some(distros) = parse_verbose(&decode(&out.stdout)) {
                return Listing::Distros(distros);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Listing::NotInstalled,
        _ => {}
    }
    match output(program, &["--list", "--quiet"]).await {
        Ok(out) if out.status.success() => Listing::Distros(parse_quiet(&decode(&out.stdout))),
        _ => Listing::NotInstalled,
    }
}

/// Runs `program` with `args`, with no window and nothing on stdin.
async fn output(program: &str, args: &[&str]) -> io::Result<Output> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .env("WSL_UTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    maxhq_os::launcher::without_window(&mut command);
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    tokio::time::timeout(LIST_TIMEOUT, command.output())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "wsl.exe took too long"))?
}

/// The text `wsl.exe` printed, as UTF-16 with or without its byte order
/// mark, or UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        || (bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).any(|byte| *byte == 0));
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{FEFF}')
        .chars()
        .filter(|c| *c != '\0' && *c != '\r')
        .collect()
}

/// The distros in `wsl.exe -l -v`'s table. None when the table's header
/// isn't there, as when it printed an error instead.
pub fn parse_verbose(text: &str) -> Option<Vec<WslDistro>> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: Vec<&str> = lines.next()?.split_whitespace().collect();
    if header != ["NAME", "STATE", "VERSION"] {
        return None;
    }
    let mut distros: Vec<WslDistro> = lines
        .filter_map(|line| {
            let line = line.trim_start();
            let (default, rest) = match line.strip_prefix('*') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let words: Vec<&str> = rest.split_whitespace().collect();
            let [name, .., version] = words.as_slice() else {
                return None;
            };
            Some(WslDistro {
                name: (*name).to_owned(),
                default,
                version: version.parse().ok(),
            })
        })
        .filter(|distro| listed(&distro.name))
        .collect();
    distros.sort_by_key(|distro| !distro.default);
    Some(distros)
}

/// The distros in `wsl.exe -l -q`'s names, the first the default.
pub fn parse_quiet(text: &str) -> Vec<WslDistro> {
    text.lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .enumerate()
        .map(|(index, name)| WslDistro {
            name: name.to_owned(),
            default: index == 0,
            version: None,
        })
        .filter(|distro| listed(&distro.name))
        .collect()
}

/// Whether a distro is one to offer as a device.
fn listed(name: &str) -> bool {
    is_distro_name(name) && !DOCKER.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = if bom { vec![0xFF, 0xFE] } else { Vec::new() };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    fn distro(name: &str, default: bool, version: Option<u8>) -> WslDistro {
        WslDistro {
            name: name.to_owned(),
            default,
            version,
        }
    }

    const TABLE: &str = "  NAME            STATE           VERSION\r\n\
                         * Ubuntu-24.04    Running         2\r\n  \
                         Debian          Stopped         1\r\n  \
                         docker-desktop  Stopped         2\r\n";

    #[test]
    fn the_table_reads_as_utf16_with_or_without_its_mark() {
        let want = Some(vec![
            distro("Ubuntu-24.04", true, Some(2)),
            distro("Debian", false, Some(1)),
        ]);
        assert_eq!(parse_verbose(&decode(&utf16(TABLE, true))), want);
        assert_eq!(parse_verbose(&decode(&utf16(TABLE, false))), want);
        assert_eq!(parse_verbose(&decode(TABLE.as_bytes())), want);
    }

    #[test]
    fn the_default_distro_comes_first() {
        let table = "  NAME      STATE           VERSION\n  \
                     Debian    Stopped         2\n\
                     * Ubuntu    Stopped         2\n";
        assert_eq!(
            parse_verbose(table),
            Some(vec![
                distro("Ubuntu", true, Some(2)),
                distro("Debian", false, Some(2)),
            ])
        );
    }

    #[test]
    fn a_message_in_place_of_the_table_is_no_table() {
        let message = "Windows Subsystem for Linux has no installed distributions.\r\n";
        assert_eq!(parse_verbose(&decode(&utf16(message, false))), None);
        assert_eq!(parse_verbose(""), None);
    }

    #[test]
    fn quiet_names_list_the_default_first() {
        let names = utf16("Ubuntu\r\nDebian\r\ndocker-desktop-data\r\n", false);
        assert_eq!(
            parse_quiet(&decode(&names)),
            vec![distro("Ubuntu", true, None), distro("Debian", false, None)]
        );
    }

    #[test]
    fn no_distros_list_as_none() {
        assert_eq!(parse_quiet(&decode(b"")), Vec::new());
        assert_eq!(parse_quiet(&decode(&utf16("\r\n", true))), Vec::new());
        let header = "  NAME      STATE           VERSION\r\n";
        assert_eq!(parse_verbose(header), Some(Vec::new()));
    }

    #[test]
    fn names_that_could_be_flags_or_words_are_skipped() {
        assert!(is_distro_name("Ubuntu-22.04"));
        assert!(is_distro_name("my_distro"));
        assert!(!is_distro_name(""));
        assert!(!is_distro_name("-d"));
        assert!(!is_distro_name("two words"));
        assert!(!is_distro_name("a;b"));
        assert!(!is_distro_name(&"x".repeat(65)));
        assert_eq!(
            parse_quiet("Ubuntu\n--help\nbad name\n"),
            vec![distro("Ubuntu", true, None)]
        );
    }

    #[test]
    fn the_link_runs_maxd_proxy_word_for_word() {
        assert_eq!(
            proxy_args("Ubuntu"),
            [
                "-d",
                "Ubuntu",
                "--exec",
                "sh",
                "-c",
                concat!(
                    r#"if [ -d "/run/user/$(id -u)" ]; then XDG_RUNTIME_DIR="/run/user/$(id -u)"; "#,
                    r#"export XDG_RUNTIME_DIR; fi; "#,
                    r#"PATH="$PATH:$HOME/.local/bin"; export PATH; exec maxd proxy"#
                ),
            ]
        );
        assert_eq!(
            exec_args("Debian", "echo \"$1\"", &["maxhq", "a b"]),
            [
                "-d",
                "Debian",
                "--exec",
                "sh",
                "-c",
                "echo \"$1\"",
                "maxhq",
                "a b"
            ]
        );
    }

    /// The runtime folder systemd made wins over one WSLg set, and one
    /// that isn't there leaves it be.
    #[cfg(unix)]
    #[test]
    fn a_script_in_a_distro_finds_the_services_runtime_folder() {
        let run = |runtime: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(in_distro(r#"printf %s "$XDG_RUNTIME_DIR""#))
                .env("XDG_RUNTIME_DIR", runtime)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let uid = String::from_utf8(
            std::process::Command::new("id")
                .arg("-u")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let systemd = format!("/run/user/{}", uid.trim());
        let expected = if std::path::Path::new(&systemd).is_dir() {
            systemd
        } else {
            "/mnt/wslg/runtime-dir".to_owned()
        };
        assert_eq!(run("/mnt/wslg/runtime-dir"), expected);
    }

    #[test]
    fn a_link_through_a_windows_machine_asks_its_maxd() {
        assert_eq!(
            via_command("Ubuntu-24.04").as_deref(),
            Some("maxd proxy --wsl Ubuntu-24.04")
        );
        assert_eq!(via_command("Ubuntu & calc"), None);
    }

    #[test]
    fn a_route_reads_back_from_its_word() {
        for route in [
            Route::Wsl {
                distro: "Ubuntu".into(),
                via: None,
            },
            Route::Wsl {
                distro: "Ubuntu-24.04".into(),
                via: Some("dev@pc".into()),
            },
            Route::Wsl {
                distro: "Debian".into(),
                via: Some("ssh://dev@[fd7a::1]:2222".into()),
            },
        ] {
            let word = word_of_route(&route).unwrap();
            assert_eq!(route_of_word(&word), Some(route), "{word}");
        }
        assert_eq!(route_word("Ubuntu", Some("dev@pc")), "wsl:Ubuntu@dev@pc");
        for not in [
            "dev@pc",
            "wsl:",
            "wsl:-d",
            "wsl:Ubuntu@",
            "wsl:Ubuntu@-oProxyCommand=x",
            "wsl:two words",
        ] {
            assert_eq!(route_of_word(not), None, "{not}");
        }
        assert_eq!(word_of_route(&Route::Local), None);
    }

    #[tokio::test]
    async fn a_missing_wsl_exe_is_wsl_not_installed() {
        assert_eq!(
            list_with("maxhq-test-no-such-wsl").await,
            Listing::NotInstalled
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_wsl_exe_that_fails_is_wsl_not_installed() {
        assert_eq!(list_with("false").await, Listing::NotInstalled);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn only_windows_has_wsl() {
        assert_eq!(list().await, Listing::Unsupported);
    }

    /// On Windows the real `wsl.exe` answers whatever this machine has,
    /// and a runner without WSL is the case that matters.
    #[cfg(windows)]
    #[tokio::test]
    async fn this_machines_wsl_lists_or_says_it_isnt_there() {
        match list().await {
            Listing::Unsupported => panic!("Windows has WSL to ask"),
            Listing::NotInstalled => {}
            Listing::Distros(distros) => {
                assert!(distros.iter().all(|distro| is_distro_name(&distro.name)));
            }
        }
    }
}

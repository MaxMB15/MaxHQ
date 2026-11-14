// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The recipe's tests. The steps run against [`FakeExec`], so nothing here
//! touches the network or a real container.

use std::collections::BTreeSet;

use super::accounts::{
    claude_config, claude_config_path, claude_folder, codex_config, codex_config_path,
    codex_folder, MergeError,
};
use super::fake::{
    Call, Dir, FakeExec, User, CLAUDE_SCRIPT_URL, CLAUDE_VERSION, CODEX_VERSION, SKEL_BASHRC,
};
use super::files::{
    self, BASHRC_PATH, GH_WRAPPER_PATH, GITCONFIG_PATH, GUEST_UNIT_NAME, GUEST_UNIT_PATH,
    TEST_HOST_PATH,
};
use super::steps::{
    codex_version, Version, AGENT_PATH, CODEX_BUILDS, CODEX_MIN, CODEX_RELEASE, CURSOR_MIN,
    CURSOR_VERSION,
};
use super::{
    ensure, linux_steps, run, steps, steps_after, Exec, ExecError, ExecOutput, ExecUser, Step,
    StepError, CLAUDE, CODEX, CODEX_HOST, CURSOR, RECIPE_VERSION,
};

const CLAUDE_FIXTURE: &str = include_str!("fixtures/claude.json");
const CODEX_FIXTURE: &str = include_str!("fixtures/codex-config.toml");

/// The Codex fixture with the work folder trusted. The new table lands
/// after the other projects and everything else stays byte for byte.
fn codex_fixture_trusted() -> String {
    let demo = "[projects.\"/home/agent/work/demo\"]\ntrust_level = \"trusted\"\n";
    let work = "\n[projects.\"/home/agent/work\"]\ntrust_level = \"trusted\"\n";
    let (before, after) = CODEX_FIXTURE.split_once(demo).unwrap();
    format!("{before}{demo}{work}{after}")
}

fn ids() -> Vec<&'static str> {
    steps().iter().map(|step| step.id).collect()
}

fn step(id: &str) -> Step {
    steps()
        .into_iter()
        .find(|step| step.id == id)
        .unwrap_or_else(|| panic!("no step {id}"))
}

/// A container the whole recipe has run on.
async fn done() -> FakeExec {
    let exec = FakeExec::debian();
    run(&exec, &steps()).await.expect("the recipe runs");
    exec
}

/// The commands a call list holds, after any `env` settings.
fn commands(calls: &[Call]) -> Vec<Vec<String>> {
    calls
        .iter()
        .filter_map(Call::command)
        .map(|words| words.iter().map(|word| (*word).to_owned()).collect())
        .collect()
}

/// True when `cmd` starts with the words in `prefix`.
fn starts(cmd: &[String], prefix: &[&str]) -> bool {
    cmd.len() >= prefix.len() && cmd.iter().zip(prefix).all(|(a, b)| a == b)
}

/// True when any command in `calls` starts with `prefix`.
fn ran(calls: &[Call], prefix: &[&str]) -> bool {
    commands(calls).iter().any(|cmd| starts(cmd, prefix))
}

/// The downloads in a call list, as URLs and who wrote them.
fn downloads(calls: &[Call]) -> Vec<(String, ExecUser)> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Download { url, user, .. } => Some((url.clone(), *user)),
            _ => None,
        })
        .collect()
}

fn output(status: i32, stdout: &str) -> ExecOutput {
    ExecOutput {
        status,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

#[test]
fn steps_have_unique_ids_and_known_versions() {
    let all = ids();
    let unique: BTreeSet<_> = all.iter().collect();
    assert_eq!(unique.len(), all.len());
    assert_eq!(
        all,
        [
            "apt",
            "agent-user",
            "agent-path",
            "claude",
            "codex",
            "cursor",
            "gitconfig",
            "gh-wrapper",
            "guest-unit"
        ]
    );
    for step in steps() {
        assert!(
            step.version >= 1 && step.version <= RECIPE_VERSION,
            "{}",
            step.id
        );
    }
    assert_eq!(steps_after(0).len(), steps().len());
    assert!(steps_after(RECIPE_VERSION).is_empty());
}

#[tokio::test]
async fn every_check_is_true_after_its_apply() {
    let exec = FakeExec::debian();
    for step in steps() {
        assert!(
            !(step.check)(&exec).await.unwrap(),
            "{} is done on a fresh container",
            step.id
        );
        (step.apply)(&exec).await.unwrap();
        assert!(
            (step.check)(&exec).await.unwrap(),
            "{} isn't done after it applied",
            step.id
        );
    }
}

#[tokio::test]
async fn a_second_run_applies_nothing() {
    let exec = FakeExec::debian();
    assert_eq!(run(&exec, &steps()).await.unwrap(), ids());
    let files = exec.lock().files.clone();
    let dirs = exec.lock().dirs.clone();
    let from = exec.call_count();

    assert!(run(&exec, &steps()).await.unwrap().is_empty());

    let calls = exec.calls_since(from);
    assert!(calls.iter().all(|call| !matches!(call, Call::Write { .. })));
    for cmd in commands(&calls) {
        let read_only = matches!(
            cmd.first().map(String::as_str),
            Some("dpkg-query" | "getent" | "stat")
        ) || cmd.last().map(String::as_str) == Some("--version")
            || starts(&cmd, &["systemctl", "is-enabled"]);
        assert!(read_only, "the second run ran {cmd:?}");
    }
    assert_eq!(exec.lock().files, files);
    assert_eq!(exec.lock().dirs, dirs);
    assert_eq!(exec.lock().restarts.get(GUEST_UNIT_NAME), Some(&1));
}

#[tokio::test]
async fn a_rerun_fixes_only_what_drifted() {
    let exec = done().await;
    exec.lock().links.remove(CODEX);
    exec.lock()
        .files
        .get_mut(GITCONFIG_PATH)
        .unwrap()
        .bytes
        .extend(b"[user]\n\tname = someone\n");
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["codex", "gitconfig"]);
    assert_eq!(exec.file(GITCONFIG_PATH).unwrap().text(), files::GITCONFIG);
}

#[tokio::test]
async fn claude_cant_install_before_apt() {
    // Claude's installer needs curl, which apt brings.
    let exec = FakeExec::debian();
    ensure(&exec, &step("agent-user")).await.unwrap();
    let err = ensure(&exec, &step("claude")).await.unwrap_err();
    assert!(matches!(
        err,
        StepError::Failed {
            step: "claude",
            error: ExecError::Failed { status: 127, .. }
        }
    ));
}

#[tokio::test]
async fn apt_installs_every_package_without_prompts() {
    let exec = FakeExec::debian();
    ensure(&exec, &step("apt")).await.unwrap();
    let calls = exec.calls_since(0);
    let cmds = commands(&calls);
    let update = cmds
        .iter()
        .position(|cmd| starts(cmd, &["apt-get", "update"]));
    let install = cmds
        .iter()
        .position(|cmd| starts(cmd, &["apt-get", "install"]));
    assert!(update.unwrap() < install.unwrap());
    let installed = exec.lock().packages.clone();
    for package in [
        "git",
        "curl",
        "ca-certificates",
        "poppler-utils",
        "bubblewrap",
        "socat",
        "ripgrep",
        "jq",
        "python3",
        "gh",
    ] {
        assert!(installed.contains(package), "{package}");
    }
}

#[tokio::test]
async fn apt_check_needs_every_package() {
    let exec = done().await;
    exec.lock().packages.remove("socat");
    assert!(!(step("apt").check)(&exec).await.unwrap());
}

#[tokio::test]
async fn the_agent_user_has_uid_1000_and_a_work_folder() {
    let exec = done().await;
    let state = exec.lock();
    let agent = state
        .users
        .iter()
        .find(|user| user.name == "agent")
        .unwrap();
    assert_eq!((agent.uid, agent.gid), (1000, 1000));
    assert_eq!(agent.home, "/home/agent");
    assert_eq!(agent.shell, "/bin/bash");
    assert!(state.groups.contains(&("agent".to_owned(), 1000)));
    let work = &state.dirs["/home/agent/work"];
    assert_eq!((work.mode, work.uid, work.gid), (0o755, 1000, 1000));
}

#[tokio::test]
async fn an_existing_agent_user_is_kept() {
    let exec = FakeExec::debian();
    exec.lock().groups.push(("agent".to_owned(), 1000));
    exec.lock().users.push(User {
        name: "agent".to_owned(),
        uid: 1000,
        gid: 1000,
        home: "/home/agent".to_owned(),
        shell: "/bin/bash".to_owned(),
    });
    exec.lock().dirs.insert(
        "/home/agent".to_owned(),
        Dir {
            mode: 0o700,
            uid: 1000,
            gid: 1000,
        },
    );
    ensure(&exec, &step("agent-user")).await.unwrap();
    let calls = exec.calls_since(0);
    assert!(!ran(&calls, &["groupadd"]));
    assert!(!ran(&calls, &["useradd"]));
    assert!(ran(&calls, &["install", "-d"]));
}

#[tokio::test]
async fn someone_else_on_uid_1000_is_a_conflict() {
    let exec = FakeExec::debian();
    exec.lock().groups.push(("debian".to_owned(), 1000));
    exec.lock().users.push(User {
        name: "debian".to_owned(),
        uid: 1000,
        gid: 1000,
        home: "/home/debian".to_owned(),
        shell: "/bin/bash".to_owned(),
    });
    let err = ensure(&exec, &step("agent-user")).await.unwrap_err();
    assert!(matches!(
        err,
        StepError::Failed {
            step: "agent-user",
            error: ExecError::Conflict(_)
        }
    ));
    assert!(!ran(&exec.calls_since(0), &["useradd"]));
}

#[tokio::test]
async fn bashrc_gets_the_path_block_first() {
    let exec = done().await;
    let bashrc = exec.file(BASHRC_PATH).unwrap();
    assert_eq!((bashrc.mode, bashrc.uid, bashrc.gid), (0o644, 1000, 1000));
    let text = bashrc.text();
    assert!(text.starts_with("# >>> MaxHQ PATH >>>\n"));
    assert!(text.contains("PATH=\"/home/agent/.local/bin:$PATH\""));
    assert!(text.contains("export PATH\n"));
    assert!(text.ends_with(SKEL_BASHRC));
    // The block comes before Debian's early return.
    let block_end = text.find("# <<< MaxHQ PATH <<<").unwrap();
    assert!(block_end < text.find("*) return;;").unwrap());
}

#[tokio::test]
async fn a_bashrc_linked_to_a_file_only_root_reads_doesnt_copy_it() {
    let exec = done().await;
    exec.write("/etc/secret", b"root's secret", 0o600, ExecUser::Root)
        .await
        .unwrap();
    {
        let mut state = exec.lock();
        state.files.remove(BASHRC_PATH);
        state
            .links
            .insert(BASHRC_PATH.to_owned(), "/etc/secret".to_owned());
    }

    let step = step("agent-path");
    assert!((step.check)(&exec).await.is_err());
    assert!((step.apply)(&exec).await.is_err());

    let state = exec.lock();
    assert!(state.files.values().all(
        |file| file.uid == 0 || !String::from_utf8_lossy(&file.bytes).contains("root's secret")
    ));
}

#[test]
fn bashrc_moves_an_old_block_to_the_top() {
    let block = files::path_block();
    let old = format!("one\n{block}two\n");
    assert_eq!(files::bashrc(Some(&old)), format!("{block}one\ntwo\n"));
    let twice = format!("{block}{block}rest\n");
    assert_eq!(files::bashrc(Some(&twice)), format!("{block}rest\n"));
    assert_eq!(files::bashrc(None), block);
    let done = files::bashrc(Some(SKEL_BASHRC));
    assert_eq!(files::bashrc(Some(&done)), done);
}

#[test]
fn bashrc_keeps_a_start_line_with_no_end() {
    let block = files::path_block();
    let text = "# >>> MaxHQ PATH >>>\nkeep me\n";
    assert_eq!(files::bashrc(Some(text)), format!("{block}{text}"));
}

#[test]
fn the_agent_path_starts_with_claudes_folder() {
    assert_eq!(files::agent_bin(), "/home/agent/.local/bin");
    assert!(CLAUDE.starts_with(files::agent_bin()));
    assert!(AGENT_PATH.starts_with(&format!("PATH={}:", files::agent_bin())));
    assert!(AGENT_PATH.contains(":/usr/local/bin:/usr/bin:"));
}

#[tokio::test]
async fn claude_installs_as_agent_with_its_home() {
    let exec = done().await;
    let launcher = exec.file(CLAUDE).unwrap();
    assert_eq!(launcher.uid, 1000);
    assert!(launcher.text().contains(CLAUDE_VERSION));
    let calls = exec.calls_since(0);
    assert!(downloads(&calls).contains(&(CLAUDE_SCRIPT_URL.to_owned(), ExecUser::Agent)));
    let install = calls
        .iter()
        .find_map(|call| match call {
            Call::Run { cmd, user }
                if cmd.iter().any(|word| word.ends_with("/dl/install.sh"))
                    && cmd.contains(&"sh".to_owned()) =>
            {
                Some((cmd.clone(), *user))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(install.1, ExecUser::Agent);
    assert!(install.0.contains(&"HOME=/home/agent".to_owned()));
}

#[test]
fn versions_compare_as_releases() {
    let parse = |text| Version::parse(text).unwrap();
    assert_eq!(codex_version("codex-cli 0.164.0\n"), Some(parse("0.164.0")));
    assert_eq!(codex_version("codex-cli v0.170.2"), Some(parse("0.170.2")));
    assert_eq!(codex_version(""), None);
    assert_eq!(Version::parse("0.164"), None);
    assert_eq!(Version::parse("0.164.0.1"), None);
    assert!(parse("0.164.0").at_least(CODEX_MIN));
    assert!(parse("0.164.1").at_least(CODEX_MIN));
    assert!(parse("1.0.0").at_least(CODEX_MIN));
    assert!(parse("0.165.0-alpha.1").at_least(CODEX_MIN));
    assert!(parse("0.164.0+build.7").at_least(CODEX_MIN));
    assert!(!parse("0.164.0-alpha.3").at_least(CODEX_MIN));
    assert!(!parse("0.163.9").at_least(CODEX_MIN));
}

#[tokio::test]
async fn codex_installs_the_checked_release() {
    let exec = done().await;
    let build = &CODEX_BUILDS[0];
    let fetched = downloads(&exec.calls_since(0));
    let record = exec
        .file("/usr/local/lib/maxhq/tools/codex/current/.install-complete")
        .unwrap()
        .text();
    for (path, name, sha256) in [
        (CODEX, "codex", build.sha256),
        (CODEX_HOST, "codex-code-mode-host", build.host_sha256),
    ] {
        let file = exec.file(path).unwrap();
        assert_eq!((file.mode, file.uid), (0o755, 0), "{path}");
        assert!(file.text().contains(CODEX_VERSION), "{path}");
        let url = format!("{CODEX_RELEASE}/{name}-{}.tar.gz", build.target);
        assert!(fetched.contains(&(url, ExecUser::Root)));
        assert!(record.contains(&format!("{name}-{}.tar.gz {sha256}", build.target)));
    }
}

#[tokio::test]
async fn a_codex_without_its_host_gets_the_host() {
    let exec = done().await;
    let host = exec.file(CODEX_HOST).unwrap();
    exec.lock().links.remove(CODEX_HOST);
    assert!(!(step("codex").check)(&exec).await.unwrap());
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["codex"]);
    assert_eq!(exec.file(CODEX_HOST).unwrap(), host);

    // A host that isn't executable counts as missing.
    let real = exec.resolve(CODEX_HOST);
    exec.lock().files.get_mut(&real).unwrap().mode = 0o644;
    assert!(!(step("codex").check)(&exec).await.unwrap());
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["codex"]);
    assert_eq!(exec.file(CODEX_HOST).unwrap(), host);
}

#[tokio::test]
async fn codex_picks_the_build_for_the_machine() {
    let exec = FakeExec::debian();
    exec.lock().machine = "aarch64".to_owned();
    run(&exec, &steps()).await.unwrap();
    let arm = CODEX_BUILDS
        .iter()
        .find(|build| build.machine == "aarch64")
        .unwrap();
    let fetched = downloads(&exec.calls_since(0));
    let intel = &CODEX_BUILDS[0];
    assert!(fetched.iter().any(|(url, _)| url.contains(arm.target)));
    assert!(!fetched.iter().any(|(url, _)| url.contains(intel.target)));
    assert!(fetched.iter().any(|(url, _)| url.contains("/linux/arm64/")));
}

#[tokio::test]
async fn codex_refuses_a_machine_with_no_build() {
    let exec = FakeExec::debian();
    exec.lock().machine = "riscv64".to_owned();
    let err = run(&exec, &steps()).await.unwrap_err();
    assert_eq!(err.step(), "codex");
    assert!(matches!(
        err,
        StepError::Failed {
            error: ExecError::Conflict(_),
            ..
        }
    ));
}

#[tokio::test]
async fn codex_stops_on_a_checksum_that_doesnt_match() {
    let exec = FakeExec::debian();
    for download in exec.lock().downloads.values_mut() {
        download.sha256 = "0".repeat(64);
    }
    let err = run(&exec, &steps()).await.unwrap_err();
    assert_eq!(err.step(), "codex");
    assert!(exec.file(CODEX).is_none());
    assert!(exec.file(CODEX_HOST).is_none());
    let StepError::Failed {
        error: ExecError::Conflict(reason),
        ..
    } = &err
    else {
        panic!("{err:?}");
    };
    assert!(reason.contains("SHA-256"), "{reason}");
    // Nothing half done is left: no staging folder, no version.
    let state = exec.lock();
    assert!(!state
        .dirs
        .keys()
        .any(|dir| dir.starts_with("/usr/local/lib/maxhq/tools/codex/")));
}

#[tokio::test]
async fn an_old_codex_is_replaced_and_a_newer_one_kept() {
    let exec = done().await;
    exec.put_binary(CODEX, "codex", "0.150.0", 0);
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["codex"]);
    assert!(exec.file(CODEX).unwrap().text().contains(CODEX_VERSION));

    exec.put_binary(CODEX, "codex", "0.170.0", 0);
    assert!(run(&exec, &steps()).await.unwrap().is_empty());
    assert!(exec.file(CODEX).unwrap().text().contains("0.170.0"));
}

#[tokio::test]
async fn cursor_installs_the_pinned_release_as_agent() {
    assert_eq!(Version::parse(CURSOR_VERSION), Some(CURSOR_MIN));
    let exec = done().await;
    let launcher = exec.file(CURSOR).unwrap();
    assert_eq!(launcher.uid, 1000);
    assert!(launcher.text().contains(CURSOR_VERSION));
    assert!(CURSOR.starts_with(files::agent_bin()));
    let fetched = downloads(&exec.calls_since(0));
    let (url, user) = fetched
        .iter()
        .find(|(url, _)| url.contains(CURSOR_VERSION))
        .unwrap();
    assert_eq!(*user, ExecUser::Agent);
    assert!(url.contains("/linux/x64/"), "{url}");
}

#[tokio::test]
async fn an_old_cursor_is_replaced_and_a_newer_one_kept() {
    let exec = done().await;
    exec.put_binary(CURSOR, "cursor-agent", "2026.01.23-916f423", 1000);
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["cursor"]);
    assert!(exec.file(CURSOR).unwrap().text().contains(CURSOR_VERSION));

    exec.put_binary(CURSOR, "cursor-agent", "2026.11.02-0000000", 1000);
    assert!(run(&exec, &steps()).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_unit_file_is_exact() {
    let exec = done().await;
    let unit = exec.file(GUEST_UNIT_PATH).unwrap();
    assert_eq!((unit.mode, unit.uid, unit.gid), (0o644, 0, 0));
    assert_eq!(
        unit.text(),
        "\
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
"
    );
    assert!(exec.lock().enabled.contains(GUEST_UNIT_NAME));
    assert_eq!(exec.lock().restarts.get(GUEST_UNIT_NAME), Some(&1));
}

#[tokio::test]
async fn the_unit_reloads_before_it_enables_and_restarts() {
    let exec = done().await;
    let order: Vec<String> = commands(&exec.calls_since(0))
        .into_iter()
        .filter(|cmd| starts(cmd, &["systemctl"]) && !starts(cmd, &["systemctl", "is-enabled"]))
        .map(|cmd| cmd[1].clone())
        .collect();
    assert_eq!(order, ["daemon-reload", "enable", "restart"]);
}

#[tokio::test]
async fn a_changed_unit_is_rewritten_and_restarted() {
    let exec = done().await;
    exec.lock()
        .files
        .get_mut(GUEST_UNIT_PATH)
        .unwrap()
        .bytes
        .extend(b"# edited\n");
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["guest-unit"]);
    assert_eq!(
        exec.file(GUEST_UNIT_PATH).unwrap().text(),
        files::GUEST_UNIT
    );
    assert_eq!(exec.lock().restarts.get(GUEST_UNIT_NAME), Some(&2));
}

#[tokio::test]
async fn gitconfig_sets_the_credential_helper() {
    let exec = done().await;
    let config = exec.file(GITCONFIG_PATH).unwrap();
    assert_eq!((config.mode, config.uid, config.gid), (0o644, 0, 0));
    assert_eq!(
        config.text(),
        "\
# Written by the MaxHQ recipe, which replaces any change made here.
[credential \"https://github.com\"]
\thelper = !/usr/local/bin/maxd guest git-credential
\tuseHttpPath = true
"
    );
    let lines: Vec<&str> = files::GITCONFIG.lines().map(str::trim).collect();
    let section = lines
        .iter()
        .position(|line| *line == "[credential \"https://github.com\"]")
        .unwrap();
    assert_eq!(
        lines[section + 1..],
        [
            "helper = !/usr/local/bin/maxd guest git-credential",
            "useHttpPath = true"
        ]
    );
}

#[tokio::test]
async fn the_gh_wrapper_runs_the_real_gh_with_a_token() {
    let exec = done().await;
    let wrapper = exec.file(GH_WRAPPER_PATH).unwrap();
    assert_eq!((wrapper.mode, wrapper.uid, wrapper.gid), (0o755, 0, 0));
    assert_eq!(
        wrapper.text(),
        "\
#!/bin/sh
# Written by the MaxHQ recipe. Asks maxd for a token scoped to this
# workspace's repo, then runs the real gh with it.
set -e
GH_TOKEN=\"$(/usr/local/bin/maxd guest github-token)\"
export GH_TOKEN
exec /usr/bin/gh \"$@\"
"
    );
    // The real gh is still where the wrapper looks.
    assert!(exec.file("/usr/bin/gh").is_some());
}

#[tokio::test]
async fn a_wrapper_that_lost_its_mode_is_fixed() {
    let exec = done().await;
    exec.lock().files.get_mut(GH_WRAPPER_PATH).unwrap().mode = 0o644;
    assert_eq!(run(&exec, &steps()).await.unwrap(), ["gh-wrapper"]);
    assert_eq!(exec.file(GH_WRAPPER_PATH).unwrap().mode, 0o755);
}

#[tokio::test]
async fn the_linux_test_host_gets_the_test_user_and_a_test_host_guest() {
    let exec = FakeExec::debian();
    let ids: Vec<&str> = linux_steps().iter().map(|step| step.id).collect();
    assert_eq!(ids, ["apt", "agent-user", "test-host", "guest-unit"]);
    assert_eq!(run(&exec, &linux_steps()).await.unwrap(), ids);

    let drop_in = exec.file(TEST_HOST_PATH).unwrap();
    assert_eq!((drop_in.mode, drop_in.uid, drop_in.gid), (0o644, 0, 0));
    assert_eq!(
        drop_in.text(),
        "\
# Written by the MaxHQ recipe, which replaces any change made here.
[Service]
ExecStart=
ExecStart=/usr/local/bin/maxd guest --test-host
"
    );
    // The unit's reload and restart come after the drop-in, so the guest
    // starts as a test host.
    let calls = exec.calls_since(0);
    let wrote = calls
        .iter()
        .position(|call| matches!(call, Call::Write { path, .. } if path == TEST_HOST_PATH))
        .unwrap();
    assert!(!ran(&calls[..wrote], &["systemctl", "daemon-reload"]));
    assert!(ran(
        &calls[wrote..],
        &["systemctl", "restart", GUEST_UNIT_NAME]
    ));
    assert!(exec.lock().users.iter().any(|user| user.name == "agent"));
    assert!(!ran(&calls, &["bash"]), "no vendor CLI goes in");

    assert!(run(&exec, &linux_steps()).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_check_that_cant_run_stops_the_recipe() {
    let exec = FakeExec::debian();
    exec.cut(&["dpkg-query"]);
    let err = run(&exec, &steps()).await.unwrap_err();
    assert!(matches!(
        err,
        StepError::Failed {
            step: "apt",
            error: ExecError::Transport(_)
        }
    ));
    let calls = exec.calls_since(0);
    assert_eq!(calls.len(), 1);
    assert!(!ran(&calls, &["apt-get"]));
}

#[tokio::test]
async fn a_check_that_exits_badly_stops_the_recipe() {
    let exec = FakeExec::debian();
    exec.answer(&["getent"], output(3, ""));
    let err = run(&exec, &steps()).await.unwrap_err();
    assert!(matches!(
        err,
        StepError::Failed {
            step: "agent-user",
            error: ExecError::Failed { status: 3, .. }
        }
    ));
    assert!(!ran(&exec.calls_since(0), &["useradd"]));
}

#[tokio::test]
async fn an_apply_that_fails_stops_the_recipe() {
    let exec = FakeExec::debian();
    exec.answer(&["apt-get", "update"], output(100, ""));
    let err = run(&exec, &steps()).await.unwrap_err();
    assert_eq!(err.step(), "apt");
    let calls = exec.calls_since(0);
    assert!(!ran(&calls, &["apt-get", "install"]));
    assert!(!ran(&calls, &["getent"]));
}

#[tokio::test]
async fn an_apply_that_changes_nothing_is_not_done() {
    let exec = FakeExec::debian();
    exec.answer(&["systemctl", "enable"], output(0, ""));
    let err = run(&exec, &steps()).await.unwrap_err();
    assert!(matches!(err, StepError::NotDone { step: "guest-unit" }));
}

#[test]
fn failed_keeps_the_start_of_the_command_and_the_end_of_stderr() {
    let long = "x".repeat(500);
    let stderr = format!("{}\nthe real reason\n", "noise ".repeat(1000));
    let err = ExecError::failed(
        &["bash", "-c", &long],
        &ExecOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: stderr.into_bytes(),
        },
    );
    let ExecError::Failed {
        command,
        status,
        stderr,
    } = &err
    else {
        panic!("{err:?}");
    };
    assert_eq!(*status, 1);
    assert!(command.starts_with("bash -c xxx") && command.ends_with("..."));
    assert!(command.len() <= 123);
    assert!(stderr.ends_with("the real reason"));
    assert!(stderr.len() <= 2000);
}

#[test]
fn claude_onboarding_keeps_every_other_key() {
    let folder = claude_folder("acct-1");
    assert_eq!(folder, "/home/agent/.claude-acct-1");
    let text = claude_config(&folder, Some(CLAUDE_FIXTURE)).unwrap();
    let merged: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut want: serde_json::Value = serde_json::from_str(CLAUDE_FIXTURE).unwrap();
    want["hasCompletedOnboarding"] = serde_json::Value::Bool(true);
    assert_eq!(merged, want);
    // Done once, the text stays as it is.
    assert_eq!(claude_config(&folder, Some(&text)).unwrap(), text);
}

#[test]
fn claude_onboarding_starts_a_missing_or_empty_file() {
    let folder = claude_folder("acct-1");
    for existing in [None, Some(""), Some("  \n")] {
        let text = claude_config(&folder, existing).unwrap();
        assert_eq!(text, "{\n  \"hasCompletedOnboarding\": true\n}\n");
    }
}

#[test]
fn claude_onboarding_leaves_a_done_file_untouched() {
    let text = "{\"zeta\":1,\"hasCompletedOnboarding\":true,\"alpha\":2}";
    assert_eq!(
        claude_config(&claude_folder("a"), Some(text)).unwrap(),
        text
    );
}

#[test]
fn claude_onboarding_refuses_what_it_cant_merge() {
    let folder = claude_folder("acct-1");
    let err = claude_config(&folder, Some("[1, 2]")).unwrap_err();
    assert!(matches!(err, MergeError::Shape { .. }));
    let err = claude_config(&folder, Some("{\"note\": \"sk-secret-value\"")).unwrap_err();
    assert!(matches!(err, MergeError::Parse { .. }));
    let message = err.to_string();
    assert!(message.contains("/home/agent/.claude-acct-1/.claude.json"));
    assert!(!message.contains("sk-secret-value"));
}

#[test]
fn merges_refuse_folders_outside_the_agent_home() {
    for folder in [
        "/root/.claude-a",
        "/home/agent",
        "/home/agent/",
        "/home/agent/..",
        "/home/agent/a/b",
        "/home/agentx/.claude-a",
        "relative",
    ] {
        assert!(
            matches!(claude_config(folder, None), Err(MergeError::Folder { .. })),
            "{folder}"
        );
        assert!(
            matches!(codex_config(folder, None), Err(MergeError::Folder { .. })),
            "{folder}"
        );
    }
    assert_eq!(
        claude_config_path("/home/agent/.claude-a").unwrap(),
        "/home/agent/.claude-a/.claude.json"
    );
    assert_eq!(
        codex_config_path("/home/agent/.codex-a").unwrap(),
        "/home/agent/.codex-a/config.toml"
    );
}

#[test]
fn codex_trust_keeps_comments_and_other_keys() {
    let folder = codex_folder("acct-1");
    assert_eq!(folder, "/home/agent/.codex-acct-1");
    let text = codex_config(&folder, Some(CODEX_FIXTURE)).unwrap();
    assert_eq!(text, codex_fixture_trusted());
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(
        doc["projects"]["/home/agent/work"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert_eq!(
        doc["projects"]["/home/agent/work/demo"]["trust_level"].as_str(),
        Some("trusted")
    );
    // Done once, the text stays as it is.
    assert_eq!(codex_config(&folder, Some(&text)).unwrap(), text);
}

#[test]
fn codex_trust_starts_a_missing_file() {
    let folder = codex_folder("acct-1");
    for existing in [None, Some("")] {
        assert_eq!(
            codex_config(&folder, existing).unwrap(),
            "[projects.\"/home/agent/work\"]\ntrust_level = \"trusted\"\n"
        );
    }
}

#[test]
fn codex_trust_overrides_untrusted_and_keeps_its_comment() {
    let text = "\
# top
[projects.\"/home/agent/work\"]
trust_level = \"untrusted\" # set by hand
other = 1
";
    let merged = codex_config(&codex_folder("a"), Some(text)).unwrap();
    assert_eq!(
        merged,
        "\
# top
[projects.\"/home/agent/work\"]
trust_level = \"trusted\" # set by hand
other = 1
"
    );
}

#[test]
fn codex_trust_works_in_inline_and_dotted_tables() {
    let folder = codex_folder("a");
    let inline = "projects = { \"/srv\" = { trust_level = \"trusted\" } }\n";
    let merged = codex_config(&folder, Some(inline)).unwrap();
    let doc: toml_edit::DocumentMut = merged.parse().unwrap();
    assert_eq!(
        doc["projects"]["/home/agent/work"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert_eq!(
        doc["projects"]["/srv"]["trust_level"].as_str(),
        Some("trusted")
    );

    let dotted = "projects.\"/home/agent/work\".trust_level = \"untrusted\"\n";
    let merged = codex_config(&folder, Some(dotted)).unwrap();
    assert_eq!(
        merged,
        "projects.\"/home/agent/work\".trust_level = \"trusted\"\n"
    );
}

#[test]
fn codex_trust_refuses_what_it_cant_merge() {
    let folder = codex_folder("acct-1");
    let err = codex_config(&folder, Some("projects = 3\n")).unwrap_err();
    assert!(matches!(err, MergeError::Shape { ref key, .. } if key == "projects"));
    let err = codex_config(
        &folder,
        Some("[projects]\n\"/home/agent/work\" = \"yes\"\n"),
    )
    .unwrap_err();
    assert!(matches!(err, MergeError::Shape { .. }));
    let err = codex_config(&folder, Some("token = \"sk-secret-value\nmore")).unwrap_err();
    assert!(matches!(err, MergeError::Parse { .. }));
    let message = err.to_string();
    assert!(message.contains("/home/agent/.codex-acct-1/config.toml"));
    assert!(!message.contains("sk-secret-value"), "{message}");
}

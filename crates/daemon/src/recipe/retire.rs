// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The steps that take what the old scripts left out of a moved copy,
//! before the recipe runs in it: the token file, the old state hook in both
//! CLIs' settings, gh's stored token and its git helper, the links into the
//! old shared volumes, and the copied Claude and Codex sign-ins. Each checks before it
//! applies, like the recipe's steps. They only ever run in an `mhq-` copy.

use futures::FutureExt;
use serde_json::Value;

use super::{ok, Exec, ExecError, ExecUser, Step, AGENT_HOME};

/// The old token export.
pub const TOKEN_FILE: &str = "/etc/profile.d/98-agent-token.sh";

/// What every command of the old state hook names.
pub const HOOK_MARK: &str = "agent-state-hook";

/// The settings files the old hook was merged into.
pub const HOOK_FILES: [&str; 2] = [
    "/home/agent/.claude/settings.json",
    "/home/agent/.codex/hooks.json",
];

/// Where gh keeps the token it was given.
pub const GH_HOSTS: &str = "/home/agent/.config/gh/hosts.yml";

/// Where `gh auth setup-git` left its helper.
pub const GIT_CONFIG: &str = "/home/agent/.gitconfig";

/// What every helper `gh auth setup-git` writes runs.
const GH_HELPER: &str = "gh auth git-credential";

/// The sign-ins the old scripts copied or linked in.
pub const LOGIN_FILES: [&str; 2] = [
    "/home/agent/.claude/.credentials.json",
    "/home/agent/.codex/auth.json",
];

/// Prints each path given that's there, a link included, without opening it.
pub const LEFT: &str =
    r#"for p; do if [ -e "$p" ] || [ -L "$p" ]; then printf '%s\n' "$p"; fi; done"#;

/// The links into the old shared volumes, which a copy no longer mounts.
const LINKS: &[&str] = &[
    "find",
    AGENT_HOME,
    "-xdev",
    "-maxdepth",
    "4",
    "-type",
    "l",
    "(",
    "-lname",
    "/opt/agent-creds/*",
    "-o",
    "-lname",
    "/opt/agent-shared/*",
    ")",
];

/// The retire steps, in the order they run.
pub fn steps() -> Vec<Step> {
    vec![
        Step {
            id: "old-token-file",
            version: 1,
            check: |exec| gone(exec, &[TOKEN_FILE], ExecUser::Root).boxed(),
            apply: |exec| remove(exec, &[TOKEN_FILE], ExecUser::Root).boxed(),
        },
        Step {
            id: "old-state-hook",
            version: 1,
            check: |exec| hook_check(exec).boxed(),
            apply: |exec| hook_apply(exec).boxed(),
        },
        Step {
            id: "old-gh-token",
            version: 1,
            check: |exec| gone(exec, &[GH_HOSTS], ExecUser::Agent).boxed(),
            apply: |exec| remove(exec, &[GH_HOSTS], ExecUser::Agent).boxed(),
        },
        Step {
            id: "old-gh-helper",
            version: 1,
            check: |exec| gh_helper_check(exec).boxed(),
            apply: |exec| gh_helper_apply(exec).boxed(),
        },
        Step {
            id: "old-volume-links",
            version: 1,
            check: |exec| links_check(exec).boxed(),
            apply: |exec| links_apply(exec).boxed(),
        },
        Step {
            id: "old-sign-ins",
            version: 1,
            check: |exec| gone(exec, &LOGIN_FILES, ExecUser::Agent).boxed(),
            apply: |exec| remove(exec, &LOGIN_FILES, ExecUser::Agent).boxed(),
        },
    ]
}

/// True when none of `paths` is there, as `user` sees it. Nothing is read, so
/// a pipe at a path can't hold the check up.
async fn gone(exec: &dyn Exec, paths: &[&str], user: ExecUser) -> Result<bool, ExecError> {
    let mut cmd = vec!["sh", "-c", LEFT, "sh"];
    cmd.extend_from_slice(paths);
    let found = ok(exec, &cmd, user).await?;
    Ok(found.stdout.iter().all(u8::is_ascii_whitespace))
}

/// Removes `paths` as `user`. The files in the agent's home go as the agent,
/// since they're its own: a folder on the way that the agent swapped for a
/// link then leads only where the agent could delete anyway. One the agent
/// can't remove stays, and the step reports it.
async fn remove(exec: &dyn Exec, paths: &[&str], user: ExecUser) -> Result<(), ExecError> {
    let mut cmd = vec!["rm", "-rf", "--"];
    cmd.extend_from_slice(paths);
    ok(exec, &cmd, user).await.map(|_| ())
}

/// The settings as they'd be without the old hook, or None when it isn't
/// in them.
fn without_hook(text: &[u8]) -> Result<Option<Vec<u8>>, ExecError> {
    let mut value: Value = serde_json::from_slice(text)
        .map_err(|error| ExecError::Conflict(format!("its hook settings don't read: {error}")))?;
    if !prune(&mut value) {
        return Ok(None);
    }
    let mut out = serde_json::to_vec_pretty(&value)
        .map_err(|error| ExecError::Conflict(error.to_string()))?;
    out.push(b'\n');
    Ok(Some(out))
}

/// Drops every hook whose command is the old hook's, and every group and
/// event that leaves empty. Says whether anything went.
fn prune(value: &mut Value) -> bool {
    match value {
        Value::Array(items) => {
            let before = items.len();
            let mut changed = false;
            items.retain_mut(|item| {
                if is_old_hook(item) {
                    return false;
                }
                let inner = prune(item);
                changed |= inner;
                // A group whose hooks all went goes too. Its emptied list
                // went already, below.
                !(inner
                    && item
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty))
            });
            changed || items.len() != before
        }
        Value::Object(map) => {
            let mut changed = false;
            map.retain(|_, inner| {
                if !prune(inner) {
                    return true;
                }
                changed = true;
                !inner.as_array().is_some_and(Vec::is_empty)
            });
            changed
        }
        _ => false,
    }
}

fn is_old_hook(item: &Value) -> bool {
    item.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(HOOK_MARK))
}

async fn hook_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    for path in HOOK_FILES {
        if let Some(text) = exec.read_as_agent(path).await?
            && without_hook(&text)?.is_some()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn hook_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    for path in HOOK_FILES {
        if let Some(text) = exec.read_as_agent(path).await?
            && let Some(kept) = without_hook(&text)?
        {
            exec.write(path, &kept, 0o600, ExecUser::Agent).await?;
        }
    }
    Ok(())
}

/// `text` without the helpers `gh auth setup-git` wrote, or none when it has
/// none. Each section it wrote starts with an empty helper, which switches
/// off the helper MaxHQ sets in `/etc/gitconfig`, so the empty one goes too.
/// A section with nothing else in it goes whole. A file that isn't UTF-8
/// isn't one gh wrote to, so it stays.
fn without_gh_helper(text: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(text).ok()?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut kept = String::with_capacity(text.len());
    let mut changed = false;
    for section in lines.chunk_by(|_, next| !next.trim_start().starts_with('[')) {
        let Some((head, body)) = section.split_first() else {
            continue;
        };
        let from_gh = head.trim_start().starts_with("[credential")
            && body
                .iter()
                .any(|line| is_helper(line) && line.contains(GH_HELPER));
        if !from_gh {
            kept.extend(section.iter().copied());
            continue;
        }
        changed = true;
        let rest: Vec<&str> = body.iter().copied().filter(|l| !is_helper(l)).collect();
        if rest.iter().any(|line| !line.trim().is_empty()) {
            kept.push_str(head);
            kept.extend(rest);
        }
    }
    changed.then_some(kept)
}

fn is_helper(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("helper")
        .is_some_and(|rest| rest.trim().is_empty() || rest.trim_start().starts_with('='))
}

async fn gh_helper_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let found = exec.read_as_agent(GIT_CONFIG).await?;
    Ok(found.is_none_or(|text| without_gh_helper(&text).is_none()))
}

async fn gh_helper_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    if let Some(text) = exec.read_as_agent(GIT_CONFIG).await?
        && let Some(kept) = without_gh_helper(&text)
    {
        exec.write(GIT_CONFIG, kept.as_bytes(), 0o644, ExecUser::Agent)
            .await?;
    }
    Ok(())
}

async fn links_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let mut cmd = LINKS.to_vec();
    cmd.push("-print");
    let found = ok(exec, &cmd, ExecUser::Root).await?;
    Ok(found.stdout.iter().all(u8::is_ascii_whitespace))
}

async fn links_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    let mut cmd = LINKS.to_vec();
    cmd.push("-delete");
    ok(exec, &cmd, ExecUser::Root).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakeExec;
    use super::super::{run, ExecOutput};
    use super::*;

    const SETTINGS: &str = r#"{
  "permissions": {"defaultMode": "bypassPermissions"},
  "hooks": {
    "Stop": [
      {"matcher": "", "hooks": [{"type": "command", "command": "/usr/local/bin/agent-state-hook.sh Stop"}]},
      {"matcher": "", "hooks": [{"type": "command", "command": "my-own-hook"}]}
    ],
    "SessionStart": [
      {"matcher": "", "hooks": [{"type": "command", "command": "/usr/local/bin/agent-state-hook.sh SessionStart"}]}
    ]
  }
}"#;

    const GIT_CONFIG_TEXT: &str = "[user]\n\tname = Someone\n\
        [credential \"https://github.com\"]\n\thelper = \n\thelper = !/usr/bin/gh auth git-credential\n\
        [credential \"https://gist.github.com\"]\n\thelper = \n\thelper = !/usr/bin/gh auth git-credential\n\
        [credential \"https://example.com\"]\n\thelper = store\n";

    fn container() -> FakeExec {
        let exec = FakeExec::debian();
        {
            let mut state = exec.lock();
            for dir in [
                "/etc/profile.d",
                "/home/agent",
                "/home/agent/.claude",
                "/home/agent/.codex",
                "/home/agent/.config",
                "/home/agent/.config/gh",
            ] {
                state.dirs.insert(
                    dir.to_owned(),
                    super::super::fake::Dir {
                        mode: 0o755,
                        uid: 1000,
                        gid: 1000,
                    },
                );
            }
        }
        exec
    }

    async fn put(exec: &FakeExec, path: &str, text: &str) {
        exec.write(path, text.as_bytes(), 0o600, ExecUser::Agent)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn settings_linked_to_a_file_only_root_reads_stay_out_of_the_agents_reach() {
        let exec = container();
        let secret = SETTINGS.replace("bypassPermissions", "root's secret");
        exec.write("/etc/secret", secret.as_bytes(), 0o600, ExecUser::Root)
            .await
            .unwrap();
        exec.lock()
            .links
            .insert(HOOK_FILES[0].to_owned(), "/etc/secret".to_owned());

        assert!(hook_check(&exec).await.is_err());
        assert!(hook_apply(&exec).await.is_err());

        // Nothing moved the secret anywhere the agent could read it.
        let state = exec.lock();
        assert!(state
            .files
            .values()
            .all(|file| file.uid == 0
                || !String::from_utf8_lossy(&file.bytes).contains("root's secret")));
    }

    #[tokio::test]
    async fn the_old_scripts_leftovers_go_and_your_own_hooks_stay() {
        let exec = container();
        put(
            &exec,
            TOKEN_FILE,
            "export CLAUDE_CODE_OAUTH_TOKEN=placeholder\n",
        )
        .await;
        put(&exec, HOOK_FILES[0], SETTINGS).await;
        put(&exec, GH_HOSTS, "github.com:\n  oauth_token: placeholder\n").await;
        put(&exec, GIT_CONFIG, GIT_CONFIG_TEXT).await;
        put(&exec, LOGIN_FILES[0], "{}").await;
        put(&exec, LOGIN_FILES[1], "{}").await;
        exec.answer(&[LINKS, &["-print"]].concat(), ExecOutput::default());
        let applied = run(&exec, &steps()).await.unwrap();
        assert_eq!(
            applied,
            [
                "old-token-file",
                "old-state-hook",
                "old-gh-token",
                "old-gh-helper",
                "old-sign-ins"
            ]
        );
        assert!(exec.file(TOKEN_FILE).is_none());
        assert!(exec.file(GH_HOSTS).is_none());
        assert_eq!(
            exec.file(GIT_CONFIG).unwrap().text(),
            "[user]\n\tname = Someone\n[credential \"https://example.com\"]\n\thelper = store\n"
        );
        let kept: Value = serde_json::from_str(&exec.file(HOOK_FILES[0]).unwrap().text()).unwrap();
        assert_eq!(kept["permissions"]["defaultMode"], "bypassPermissions");
        let stop = kept["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["hooks"][0]["command"], "my-own-hook");
        assert!(kept["hooks"].get("SessionStart").is_none());
        assert!(hook_check(&exec).await.unwrap());
        assert!(LOGIN_FILES.iter().all(|path| exec.file(path).is_none()));
    }

    #[tokio::test]
    async fn links_into_the_old_volumes_are_deleted() {
        let exec = container();
        exec.answer(
            &[LINKS, &["-print"]].concat(),
            ExecOutput {
                stdout: b"/home/agent/.claude.token\n".to_vec(),
                ..ExecOutput::default()
            },
        );
        exec.answer(&[LINKS, &["-delete"]].concat(), ExecOutput::default());
        assert!(!links_check(&exec).await.unwrap());
        links_apply(&exec).await.unwrap();
        let deleted = exec.lock().calls.iter().any(|call| {
            call.command()
                .is_some_and(|cmd| cmd.last() == Some(&"-delete"))
        });
        assert!(deleted);
    }

    #[tokio::test]
    async fn a_home_folder_the_agent_linked_elsewhere_doesnt_take_roots_files() {
        let exec = container();
        {
            let mut state = exec.lock();
            for dir in ["/home/agent/.claude", "/home/agent/.config/gh"] {
                state.dirs.remove(dir);
                state.links.insert(dir.to_owned(), "/etc/kept".to_owned());
            }
            state.dirs.insert(
                "/etc/kept".to_owned(),
                super::super::fake::Dir {
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                },
            );
        }
        for name in [".credentials.json", "hosts.yml"] {
            let path = format!("/etc/kept/{name}");
            exec.write(&path, b"root's own", 0o600, ExecUser::Root)
                .await
                .unwrap();
        }
        // The agent's own sign-in, which goes.
        put(&exec, LOGIN_FILES[1], "{}").await;

        for id in ["old-gh-token", "old-sign-ins"] {
            let step = steps().into_iter().find(|step| step.id == id).unwrap();
            assert!(!(step.check)(&exec).await.unwrap(), "{id}");
            assert!((step.apply)(&exec).await.is_err(), "{id}");
        }
        assert!(exec.file("/etc/kept/.credentials.json").is_some());
        assert!(exec.file("/etc/kept/hosts.yml").is_some());
        assert!(exec.file(LOGIN_FILES[1]).is_none());
        for call in &exec.lock().calls {
            if let super::super::fake::Call::Run { cmd, user } = call
                && cmd.first().map(String::as_str) == Some("rm")
            {
                assert_eq!(*user, ExecUser::Agent, "{cmd:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_clean_copy_needs_nothing() {
        let exec = container();
        put(&exec, HOOK_FILES[1], r#"{"hooks": {}}"#).await;
        exec.answer(&[LINKS, &["-print"]].concat(), ExecOutput::default());
        let applied = run(&exec, &steps()).await.unwrap();
        assert!(applied.is_empty(), "{applied:?}");
        assert!(exec.lock().calls.iter().all(|call| {
            call.command()
                .is_none_or(|cmd| cmd.first() == Some(&"find") || cmd.get(2) == Some(&LEFT))
        }));
    }

    #[test]
    fn gh_helpers_go_and_the_rest_of_their_section_stays() {
        let text = "[credential \"https://github.com\"]\n\thelper = \n\t\
            helper = !/usr/bin/gh auth git-credential\n\tusername = me\n";
        assert_eq!(
            without_gh_helper(text.as_bytes()).as_deref(),
            Some("[credential \"https://github.com\"]\n\tusername = me\n")
        );
    }

    #[test]
    fn a_git_config_gh_never_wrote_to_stays_as_it_is() {
        for text in [
            "[user]\n\tname = Someone\n",
            "[credential]\n\thelper = store\n",
            // Not a helper, so not what gh wrote.
            "[alias]\n\tsignin = !gh auth git-credential\n",
            "",
        ] {
            assert_eq!(without_gh_helper(text.as_bytes()), None, "{text}");
        }
        assert_eq!(without_gh_helper(b"[credential]\n\thelper = \xff\n"), None);
    }

    #[test]
    fn settings_without_the_hook_stay_as_they_are() {
        assert!(without_hook(br#"{"hooks": {"Stop": []}}"#)
            .unwrap()
            .is_none());
        assert!(without_hook(b"not json").is_err());
    }
}

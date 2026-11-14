// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The recipe's [`Exec`] over Incus exec, and the git commands a workspace
//! runs in its container: the clone, the push preflight and the check for
//! unpushed work before a delete.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::Secret;

use crate::incus::{Incus, InstanceName};
use crate::recipe::{Exec, ExecError, ExecOutput, ExecUser, WORK};

/// Prints a plain file, or exits 3 when nothing is there and 4 when something
/// else is. It stops one byte past what an exec may return, so a bigger file
/// fails rather than coming back cut short.
pub const READ_SCRIPT: &str =
    r#"test -e "$1" || exit 3; test -f "$1" || exit 4; exec head -c 67108865 -- "$1""#;
/// Starts a file from one base64 chunk.
pub const WRITE_FIRST: &str = r#"rm -f "$1" && printf %s "$2" | base64 -d > "$1""#;
/// Adds a base64 chunk to a file.
pub const WRITE_MORE: &str = r#"printf %s "$2" | base64 -d >> "$1""#;
/// Starts an empty file.
pub const WRITE_EMPTY: &str = r#"rm -f "$1" && : > "$1""#;
/// Gives a written file its owner and mode and moves it into place.
pub const WRITE_FINISH: &str = r#"chown "$3:$3" "$1" && chmod "$2" "$1" && mv -f "$1" "$4""#;

/// How much of a file goes in each write command. Base64 makes it a third
/// bigger, which stays well under Linux's limit on one argument.
const CHUNK: usize = 48 << 10;

/// The exit status [`READ_SCRIPT`] uses for a missing file.
const MISSING: i32 = 3;

/// Runs git as the agent with a GitHub token that it reads from stdin, so
/// the token is never on a command line or in a file. Git asks the
/// one-line helper for it, which replaces every other helper.
pub const GIT_SCRIPT: &str = r#"IFS= read -r MAXHQ_GIT_TOKEN || exit 90
export MAXHQ_GIT_TOKEN
exec git -c credential.helper= -c 'credential.helper=!f() { test "$1" = get && printf "username=x-access-token\npassword=%s\n" "$MAXHQ_GIT_TOKEN"; }; f' "$@""#;

/// Lists the worktrees with changes that aren't committed and the
/// branches with commits that no remote has, in the repo at `$1`. Exits 3
/// when the repo isn't there.
pub const UNPUSHED_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 3
trees=$(git worktree list --porcelain) || exit 4
printf '%s\n' "$trees" | sed -n 's/^worktree //p' | while IFS= read -r tree; do
  out=$(git -C "$tree" status --porcelain 2>/dev/null) || continue
  [ -z "$out" ] || printf 'changes\t%s\n' "$tree"
done
branches=$(git for-each-ref --format='%(refname:short)' refs/heads) || exit 4
for branch in $branches; do
  n=$(git rev-list --count "refs/heads/$branch" --not --remotes) || exit 4
  [ "$n" = 0 ] || printf 'commits\t%s\t%s\n' "$n" "$branch"
done"#;

/// The recipe's [`Exec`] for one instance.
#[derive(Clone, Debug)]
pub struct IncusExec {
    incus: Incus,
    name: InstanceName,
}

impl IncusExec {
    /// Runs commands in `name`.
    pub fn new(incus: Incus, name: InstanceName) -> Self {
        Self { incus, name }
    }

    /// The instance.
    pub fn name(&self) -> &InstanceName {
        &self.name
    }

    async fn call(
        &self,
        cmd: &[&str],
        user: ExecUser,
        stdin: Option<&[u8]>,
    ) -> Result<ExecOutput, ExecError> {
        self.incus
            .exec(&self.name, cmd, Some(user.uid()), stdin.map(<[u8]>::to_vec))
            .await
            .map_err(|error| ExecError::Transport(error.to_string()))
    }

    async fn ok(&self, cmd: &[&str], user: ExecUser) -> Result<(), ExecError> {
        let output = self.call(cmd, user, None).await?;
        if output.success() {
            Ok(())
        } else {
            Err(ExecError::failed(cmd, &output))
        }
    }

    async fn read_as(&self, path: &str, user: ExecUser) -> Result<Option<Vec<u8>>, ExecError> {
        let cmd = ["sh", "-c", READ_SCRIPT, "sh", path];
        let output = self.call(&cmd, user, None).await?;
        match output.status {
            0 => Ok(Some(output.stdout)),
            MISSING => Ok(None),
            _ => Err(ExecError::failed(&cmd, &output)),
        }
    }

    /// Every command runs as `owner`. The agent owns the folders its files
    /// live in and can plant a link at the temporary name, so as root the
    /// redirection, the `chown` and the `chmod` would all follow it.
    async fn write_file(
        &self,
        path: &str,
        bytes: &[u8],
        mode: u32,
        owner: ExecUser,
    ) -> Result<(), ExecError> {
        let temp = format!("{path}.maxhq-new");
        if bytes.is_empty() {
            self.ok(&["sh", "-c", WRITE_EMPTY, "sh", &temp], owner)
                .await?;
        }
        for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
            let script = if i == 0 { WRITE_FIRST } else { WRITE_MORE };
            let encoded = STANDARD.encode(chunk);
            self.ok(&["sh", "-c", script, "sh", &temp, &encoded], owner)
                .await?;
        }
        let mode = format!("{mode:o}");
        let uid = owner.uid().to_string();
        self.ok(
            &["sh", "-c", WRITE_FINISH, "sh", &temp, &mode, &uid, path],
            owner,
        )
        .await
    }
}

impl Exec for IncusExec {
    fn run<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        stdin: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        self.call(cmd, user, stdin).boxed()
    }

    fn run_watched<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        stdin: Option<&'a [u8]>,
        stderr: &'a (dyn Fn(&[u8]) + Send + Sync),
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        async move {
            let stdin = stdin.map(<[u8]>::to_vec).unwrap_or_default();
            self.incus
                .exec_watched(&self.name, cmd, Some(user.uid()), stdin, stderr)
                .await
                .map_err(|error| ExecError::Transport(error.to_string()))
        }
        .boxed()
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        self.read_as(path, ExecUser::Root).boxed()
    }

    fn read_as_agent<'a>(
        &'a self,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        self.read_as(path, ExecUser::Agent).boxed()
    }

    /// The bytes travel on exec command lines, so this is never for a
    /// secret.
    fn write<'a>(
        &'a self,
        path: &'a str,
        bytes: &'a [u8],
        mode: u32,
        owner: ExecUser,
    ) -> BoxFuture<'a, Result<(), ExecError>> {
        self.write_file(path, bytes, mode, owner).boxed()
    }
}

/// Runs `git args...` as the agent with `token` for github.com.
pub async fn git_with_token(
    exec: &dyn Exec,
    args: &[&str],
    token: &Secret,
) -> Result<ExecOutput, ExecError> {
    git_with_token_watched(exec, args, token, &|_| {}).await
}

/// Runs `git args...` as [`git_with_token`] does, handing `stderr` what
/// git prints there as it comes.
pub async fn git_with_token_watched(
    exec: &dyn Exec,
    args: &[&str],
    token: &Secret,
    stderr: &(dyn Fn(&[u8]) + Send + Sync),
) -> Result<ExecOutput, ExecError> {
    let mut cmd = vec![
        "env",
        "GIT_CONFIG_NOSYSTEM=1",
        "GIT_TERMINAL_PROMPT=0",
        "sh",
        "-c",
        GIT_SCRIPT,
        "sh",
    ];
    cmd.extend(args);
    let mut stdin = token.expose().as_bytes().to_vec();
    stdin.push(b'\n');
    let output = exec
        .run_watched(&cmd, ExecUser::Agent, Some(&stdin), stderr)
        .await;
    stdin.fill(0);
    output
}

/// Runs a command as the agent and fails unless it exits 0.
pub async fn agent_ok(exec: &dyn Exec, cmd: &[&str]) -> Result<ExecOutput, ExecError> {
    let output = exec.run(cmd, ExecUser::Agent, None).await?;
    if output.success() {
        Ok(output)
    } else {
        Err(ExecError::failed(cmd, &output))
    }
}

/// Work in a container's repo that a delete would lose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unpushed {
    /// Worktrees with changes that aren't committed.
    pub changed: Vec<String>,
    /// Branches with commits no remote has, and how many.
    pub commits: Vec<(u32, String)>,
}

impl Unpushed {
    /// Reads what [`UNPUSHED_SCRIPT`] printed.
    pub fn parse(stdout: &[u8]) -> Self {
        let mut found = Self::default();
        for line in String::from_utf8_lossy(stdout).lines() {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("changes"), Some(tree), None) => found.changed.push(tree.to_owned()),
                (Some("commits"), Some(count), Some(branch)) => {
                    if let Ok(count) = count.parse() {
                        found.commits.push((count, branch.to_owned()));
                    }
                }
                _ => {}
            }
        }
        found
    }

    /// True when nothing would be lost.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.commits.is_empty()
    }

    /// Why a delete was refused, for the person who asked.
    pub fn reason(&self, instance: &str) -> String {
        let mut parts = Vec::new();
        if !self.commits.is_empty() {
            let total: u32 = self.commits.iter().map(|(count, _)| count).sum();
            let noun = if total == 1 { "commit" } else { "commits" };
            let verb = if total == 1 { "isn't" } else { "aren't" };
            let branches: Vec<&str> = self.commits.iter().map(|(_, b)| b.as_str()).collect();
            parts.push(format!(
                "{total} {noun} that {verb} pushed, on {}",
                list(&branches)
            ));
        }
        if !self.changed.is_empty() {
            let trees: Vec<&str> = self
                .changed
                .iter()
                .map(|tree| {
                    tree.strip_prefix(WORK)
                        .and_then(|rest| rest.strip_prefix('/'))
                        .unwrap_or(tree)
                })
                .collect();
            parts.push(format!("changes that aren't committed in {}", list(&trees)));
        }
        format!(
            "{instance} has {}. Push them first, or delete anyway to lose them.",
            parts.join(", and ")
        )
    }
}

/// `a`, `a and b`, or `a, b and c`.
fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::incus::fake::{ExecReply, FakeIncus, State};
    use crate::incus::MANAGED_KEY;
    use crate::recipe::AGENT_UID;

    /// A container where every command works, and what runs in it.
    fn container() -> (FakeIncus, IncusExec) {
        let mut state = State::default();
        state.add("mhq-box", "Running", &[(MANAGED_KEY, "true")]);
        state.exec = Some(Arc::new(|_| ExecReply::ok("")));
        let fake = FakeIncus::start(state);
        let name = InstanceName::new("mhq-box").unwrap();
        let exec = IncusExec::new(Incus::new(&fake.socket), name);
        (fake, exec)
    }

    #[tokio::test]
    async fn the_agents_files_are_written_and_read_as_the_agent() {
        let (fake, exec) = container();
        let path = "/home/agent/.bashrc";
        exec.write(path, b"", 0o644, ExecUser::Agent).await.unwrap();
        exec.write(path, b"new", 0o644, ExecUser::Agent)
            .await
            .unwrap();
        exec.read_as_agent(path).await.unwrap();

        let state = fake.state();
        assert_eq!(state.execs.len(), 5);
        for call in &state.execs {
            assert_eq!(call.user, AGENT_UID, "{:?}", call.command);
        }
    }

    #[tokio::test]
    async fn the_files_root_owns_are_still_written_as_root() {
        let (fake, exec) = container();
        exec.write("/etc/gitconfig", b"new", 0o644, ExecUser::Root)
            .await
            .unwrap();
        exec.read("/etc/gitconfig").await.unwrap();

        let state = fake.state();
        assert_eq!(state.execs.len(), 3);
        assert!(state.execs.iter().all(|call| call.user == 0));
    }

    #[test]
    fn reads_unpushed_work() {
        let stdout = b"changes\t/home/agent/work/MaxHQ\n\
changes\t/home/agent/work/.worktrees/fix-x\n\
commits\t2\tfix-x\n\
commits\t1\tmain\n\
noise\n";
        let found = Unpushed::parse(stdout);
        assert_eq!(
            found.changed,
            [
                "/home/agent/work/MaxHQ",
                "/home/agent/work/.worktrees/fix-x"
            ]
        );
        assert_eq!(
            found.commits,
            [(2, "fix-x".to_owned()), (1, "main".to_owned())]
        );
        assert_eq!(
            found.reason("mhq-maxhq"),
            "mhq-maxhq has 3 commits that aren't pushed, on fix-x and main, and changes that \
             aren't committed in MaxHQ and .worktrees/fix-x. Push them first, or delete anyway \
             to lose them."
        );
        assert!(Unpushed::parse(b"").is_empty());

        let one = Unpushed {
            changed: Vec::new(),
            commits: vec![(1, "main".to_owned())],
        };
        assert_eq!(
            one.reason("mhq-a"),
            "mhq-a has 1 commit that isn't pushed, on main. Push them first, or delete anyway \
             to lose them."
        );
    }

    #[test]
    fn lists_read_as_words() {
        assert_eq!(list(&["a"]), "a");
        assert_eq!(list(&["a", "b"]), "a and b");
        assert_eq!(list(&["a", "b", "c"]), "a, b and c");
    }
}

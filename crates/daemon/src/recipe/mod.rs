// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest recipe: the steps that turn a fresh Debian 13 container into a
//! MaxHQ workspace, per W-3. Each step checks whether it's done before it
//! applies, so a create, an update and a rerun all agree. The steps reach
//! the container through [`Exec`], which XC implements over Incus exec.
//!
//! The per-account config merges live in [`accounts`]. They run per agent
//! account, so they aren't steps.

pub mod accounts;
mod files;
pub mod retire;
pub mod steps;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use futures::future::BoxFuture;
use thiserror::Error;

/// The recipe's version. A container records the version it last finished
/// in `user.maxhq.recipe`. Raise it with the version of any step that
/// changes.
pub const RECIPE_VERSION: u32 = 4;

/// Where Claude's native installer puts its launcher. The launches, the
/// check and the agent's `PATH` all read this one constant.
pub const CLAUDE: &str = "/home/agent/.local/bin/claude";
/// Where the recipe installs Codex.
pub const CODEX: &str = "/usr/local/bin/codex";
/// Where the recipe installs Codex's code-mode host. Codex looks for it
/// next to its own binary.
pub const CODEX_HOST: &str = "/usr/local/bin/codex-code-mode-host";
/// Where the recipe links Cursor's CLI.
pub const CURSOR: &str = "/home/agent/.local/bin/cursor-agent";
/// Where Cursor's CLI keeps its config, since it has no variable for it.
pub const CURSOR_CONFIG: &str = "/home/agent/.cursor";
/// Where the guest binary lives. XC installs it.
pub const MAXD: &str = "/usr/local/bin/maxd";

/// The user the CLIs run as.
pub const AGENT: &str = "agent";
/// The agent user's uid, which is also its gid.
pub const AGENT_UID: u32 = 1000;
/// The agent user's home folder.
pub const AGENT_HOME: &str = "/home/agent";
/// The folder that holds the repo and its worktrees.
pub const WORK: &str = "/home/agent/work";

/// Who a command runs as, or who owns a file it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecUser {
    /// root, uid 0.
    Root,
    /// The agent user, uid 1000.
    Agent,
}

impl ExecUser {
    /// The user's uid, which is also its gid.
    pub fn uid(self) -> u32 {
        match self {
            ExecUser::Root => 0,
            ExecUser::Agent => AGENT_UID,
        }
    }
}

/// What a command printed and how it exited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// The exit status. A command killed by a signal reports 128 plus the
    /// signal, as a shell does.
    pub status: i32,
    /// Everything it printed to stdout.
    pub stdout: Vec<u8>,
    /// Everything it printed to stderr.
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    /// True when the command exited 0.
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

/// Why a step couldn't check or apply.
#[derive(Debug, Error)]
pub enum ExecError {
    /// The call didn't reach the workspace, or the workspace refused it.
    #[error("the workspace call failed: {0}")]
    Transport(String),
    /// A command exited with a status the step doesn't accept.
    #[error("`{command}` exited with {status}: {stderr}")]
    Failed {
        /// The command, shortened.
        command: String,
        /// Its exit status.
        status: i32,
        /// The end of what it printed to stderr.
        stderr: String,
    },
    /// The container holds something the recipe won't overwrite on its own.
    #[error("{0}")]
    Conflict(String),
}

/// How much of a command a [`ExecError::Failed`] keeps.
const COMMAND_LIMIT: usize = 120;
/// How much of a command's stderr a [`ExecError::Failed`] keeps.
const STDERR_LIMIT: usize = 2000;

impl ExecError {
    /// The error for a command that exited with a status the step doesn't
    /// accept. It keeps the start of the command and the end of stderr.
    pub fn failed(cmd: &[&str], output: &ExecOutput) -> Self {
        let command = cmd.join(" ").replace('\n', " ");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        ExecError::Failed {
            command: head(&command, COMMAND_LIMIT),
            status: output.status,
            stderr: tail(stderr, STDERR_LIMIT).to_owned(),
        }
    }
}

/// The first `limit` bytes of `text`, cut at a character boundary, with an
/// ellipsis when anything was cut.
fn head(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// The last `limit` bytes of `text`, cut at a character boundary.
fn tail(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Runs commands in a workspace. XC implements it over Incus exec.
///
/// Every call borrows its arguments for as long as its future runs, so an
/// implementation can hold them without copying.
pub trait Exec: Send + Sync {
    /// Runs `cmd` as `user` with `stdin` and waits for it to exit. A command
    /// that runs and fails is an [`ExecOutput`] with its status, not an
    /// error. The environment is whatever Incus exec gives, so steps that
    /// need `HOME` or `PATH` set them with `env`.
    fn run<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        stdin: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>>;
    /// Runs `cmd` as [`Exec::run`] does, and hands `stderr` what it prints
    /// there, as it comes when the implementation can. By default it comes
    /// all at once at the end.
    fn run_watched<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        stdin: Option<&'a [u8]>,
        stderr: &'a (dyn Fn(&[u8]) + Send + Sync),
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        Box::pin(async move {
            let output = self.run(cmd, user, stdin).await?;
            stderr(&output.stderr);
            Ok(output)
        })
    }
    /// Reads a whole file, or `None` when nothing is there.
    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>>;
    /// Reads a whole file in the agent's own folders, as the agent, so a link
    /// the agent put there reaches only what the agent can read anyway. The
    /// default suits an [`Exec`] that runs everything as one user.
    fn read_as_agent<'a>(
        &'a self,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        self.read(path)
    }
    /// Writes a whole file, replacing any file there, with `mode` and with
    /// `owner` as its user and group. The parent folder must exist, and
    /// `owner` must be able to write in it.
    fn write<'a>(
        &'a self,
        path: &'a str,
        bytes: &'a [u8],
        mode: u32,
        owner: ExecUser,
    ) -> BoxFuture<'a, Result<(), ExecError>>;
    /// Downloads `url` to `path` as `user`, stopping past `most` bytes, and
    /// says what arrived. maxd fetches it and hashes it as it comes, then
    /// writes it here in pieces through [`Exec::run`].
    fn download<'a>(
        &'a self,
        url: &'a str,
        path: &'a str,
        user: ExecUser,
        most: Option<u64>,
        progress: crate::tools::install::fetch::Progress<'a>,
    ) -> BoxFuture<'a, Result<crate::tools::install::fetch::Fetched, ExecError>> {
        Box::pin(crate::tools::install::fetch::download(
            self, url, path, user, most, progress,
        ))
    }
}

/// Runs a command and fails unless it exits 0.
async fn ok(exec: &dyn Exec, cmd: &[&str], user: ExecUser) -> Result<ExecOutput, ExecError> {
    let output = exec.run(cmd, user, None).await?;
    if output.success() {
        Ok(output)
    } else {
        Err(ExecError::failed(cmd, &output))
    }
}

/// One idempotent step of the recipe.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    /// A short name that stays the same across versions.
    pub id: &'static str,
    /// The recipe version that last changed the step. A container whose
    /// recorded recipe is older runs it again.
    pub version: u32,
    /// True when the step is already done, so create, update and a rerun agree.
    pub check: fn(&dyn Exec) -> BoxFuture<'_, Result<bool, ExecError>>,
    /// Does the step. It may assume every step before it is done.
    pub apply: fn(&dyn Exec) -> BoxFuture<'_, Result<(), ExecError>>,
}

/// Every step, in the order they run.
pub fn steps() -> Vec<Step> {
    steps::all()
}

/// The `linux` test host's steps, in order.
pub fn linux_steps() -> Vec<Step> {
    steps::linux()
}

/// The steps a container that last finished recipe `done` still needs.
pub fn steps_after(done: u32) -> Vec<Step> {
    steps()
        .into_iter()
        .filter(|step| step.version > done)
        .collect()
}

/// Why the recipe stopped.
#[derive(Debug, Error)]
pub enum StepError {
    /// A step's check or apply failed.
    #[error("recipe step {step} failed: {error}")]
    Failed {
        /// The step's ID.
        step: &'static str,
        /// What went wrong.
        #[source]
        error: ExecError,
    },
    /// A step applied without an error, but its check still says it isn't
    /// done.
    #[error("recipe step {step} still isn't done after it applied")]
    NotDone {
        /// The step's ID.
        step: &'static str,
    },
}

impl StepError {
    /// The ID of the step that stopped the recipe.
    pub fn step(&self) -> &'static str {
        match self {
            StepError::Failed { step, .. } | StepError::NotDone { step } => step,
        }
    }
}

/// Makes sure one step is done: checks it, applies it when it isn't, and
/// checks again. Returns true when it had to apply.
pub async fn ensure(exec: &dyn Exec, step: &Step) -> Result<bool, StepError> {
    let failed = |error| StepError::Failed {
        step: step.id,
        error,
    };
    if (step.check)(exec).await.map_err(failed)? {
        return Ok(false);
    }
    tracing::info!(step = step.id, "applying a recipe step");
    (step.apply)(exec).await.map_err(failed)?;
    if (step.check)(exec).await.map_err(failed)? {
        Ok(true)
    } else {
        Err(StepError::NotDone { step: step.id })
    }
}

/// Runs `steps` in order and stops at the first that fails. Returns the IDs
/// of the steps that had to apply.
pub async fn run(exec: &dyn Exec, steps: &[Step]) -> Result<Vec<&'static str>, StepError> {
    let mut applied = Vec::new();
    for step in steps {
        if ensure(exec, step).await? {
            applied.push(step.id);
        }
    }
    Ok(applied)
}

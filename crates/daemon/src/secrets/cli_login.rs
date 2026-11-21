// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Signing a Claude Code, Codex or Cursor login in again through the CLI's
//! own sign-in, on the device that holds the account: `claude auth login`,
//! `codex login`, `codex login --device-auth` or `agent login`, under the
//! variables a session on the account gets. The CLI stores the login, so
//! `maxd` never sees a token.
//!
//! Each account has at most one round: the CLI, what it printed and how
//! it ended. A round answers with the link the CLI printed and, for a
//! Codex device code, the code to type there. Claude Code asks for the
//! code its page shows, which [`Rounds::finish`] writes to the CLI. Codex
//! and Cursor end on their own once the page signs you in. The link, the
//! code and the CLI's output never reach the log.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use maxhq_protocol::ids::{AccountId, SignInId};
use maxhq_protocol::message::{ActionOutcome, CliLoginState};
use maxhq_protocol::model::{LoginKind, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::Secret;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::info;
use zeroize::Zeroizing;

use super::{pasted, refused};
use crate::drive::{claude, codex, cursor, Launch, Login, LoginAuth};
use crate::hub::{Command, Hub};
use crate::tools::run::{plain_text, progress_line};

#[cfg(test)]
mod tests;

/// The most output kept from a sign-in's CLI: its last bytes, from the
/// start of a line.
const KEEP_OUTPUT: usize = 64 * 1024;

/// The longest code taken from a paste.
const MAX_CODE: usize = 4096;

/// Why a round that ended, or never was, refuses.
const GONE: &str = "That sign-in ended. Start it again.";

/// Why a round failed once its CLI ran past [`Limits::round`].
const TIMED_OUT: &str = "The sign-in timed out. Start it again.";

/// How long the output may keep coming once the CLI ended, from anything
/// it started that still holds the pipes open.
const DRAIN: Duration = Duration::from_millis(500);

/// How long a replaced round gets to stop its CLI, which frees what that
/// held, such as the port Codex's sign-in listens on.
const REPLACE: Duration = Duration::from_secs(5);

/// The prompt Claude Code prints without a line break, so what it prints
/// next follows on the same line.
const PROMPT: &str = "if prompted >";

/// How long each step of a round may take.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    /// For the CLI to print its link.
    pub link: Duration,
    /// For the CLI to take a pasted code and end.
    pub code: Duration,
    /// For the whole round, before its CLI is stopped.
    pub round: Duration,
    /// For an ended round to stay, so a client can still read how it ended.
    pub keep: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            link: Duration::from_secs(30),
            code: Duration::from_secs(60),
            round: Duration::from_secs(15 * 60),
            keep: Duration::from_secs(5 * 60),
        }
    }
}

/// Which sign-in a CLI runs, which says what it prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flow {
    /// `claude auth login`, which asks for the code its page shows.
    Claude,
    /// `codex login`, whose page comes back to this device's localhost.
    Codex,
    /// `codex login --device-auth`, with a code you type at OpenAI's page.
    CodexDevice,
    /// `agent login`, which waits until Cursor's page signs you in.
    Cursor,
}

/// The link a CLI printed, with the code to type there, if any.
#[derive(Debug, PartialEq, Eq)]
struct Link {
    url: String,
    user_code: Option<String>,
}

impl Flow {
    /// The CLI's name, for a person to read.
    fn cli(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex | Self::CodexDevice => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    /// Whether you paste a code back into the CLI.
    fn paste(self) -> bool {
        self == Self::Claude
    }

    /// The link in the CLI's whole lines so far, once all of it came.
    fn link(self, lines: &[String]) -> Option<Link> {
        let (marker, user_code) = match self {
            Self::Claude => ("visit:", None),
            Self::Codex => ("navigate to this URL", None),
            Self::Cursor => ("navigate to this link:", None),
            Self::CodexDevice => {
                // The code is alone on the line after the one naming it.
                let at = lines
                    .iter()
                    .position(|line| line.contains("one-time code"))?;
                let code = lines
                    .get(at + 1)
                    .filter(|code| code.len() <= 64 && !code.contains(char::is_whitespace))?;
                ("Open this link", Some(code.clone()))
            }
        };
        let url = link_after(lines, marker)?;
        Some(Link { url, user_code })
    }
}

/// The first link at the end of the line holding `marker`, or of a line
/// after it.
fn link_after(lines: &[String], marker: &str) -> Option<String> {
    let at = lines.iter().position(|line| line.contains(marker))?;
    lines[at..].iter().find_map(|line| {
        let last = line.split_whitespace().last()?;
        (last.len() > "https://".len() && last.starts_with("https://")).then(|| last.to_owned())
    })
}

/// `output`'s lines without escape codes or blank ones.
fn plain_lines(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .split(['\n', '\r'])
        .map(|line| plain_text(line).trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// [`plain_lines`] of `output`'s ended lines, since a link is only whole
/// once its line ends.
fn whole_lines(output: &[u8]) -> Vec<String> {
    let end = output
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |at| at + 1);
    plain_lines(&output[..end])
}

/// What a CLI said last, for a person to read: its last `count` lines
/// with words in them, clipped. A link or a code alone on its line isn't
/// words, and neither is Claude Code's prompt.
fn last_words(lines: &[String], count: usize) -> Option<String> {
    let mut said: Vec<&str> = lines
        .iter()
        .rev()
        .map(|line| {
            line.rsplit_once(PROMPT)
                .map_or(line.as_str(), |(_, after)| after)
                .trim()
        })
        .filter(|line| line.contains(' ') && !line.contains("https://"))
        .take(count)
        .collect();
    said.reverse();
    progress_line(&said.join(" "))
}

/// Adds `bytes` to `kept`, then cuts that to its last [`KEEP_OUTPUT`]
/// bytes from the start of a line.
fn push(kept: &mut Vec<u8>, bytes: &[u8]) {
    kept.extend_from_slice(bytes);
    if kept.len() > KEEP_OUTPUT {
        let cut = kept.len() - KEEP_OUTPUT;
        let start = kept[cut..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(cut, |at| cut + at + 1);
        kept.drain(..start);
    }
}

/// Reads `stream` to its end into `output`.
async fn keep(stream: Option<impl AsyncRead + Unpin>, output: &watch::Sender<Vec<u8>>) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buffer = [0u8; 8192];
    while let Ok(read @ 1..) = stream.read(&mut buffer).await {
        output.send_modify(|kept| push(kept, &buffer[..read]));
    }
}

/// The sign-ins running on this device, at most one per account. Clones
/// share them.
#[derive(Clone)]
pub(super) struct Rounds {
    inner: Arc<Shared>,
}

struct Shared {
    hub: Hub,
    limits: Limits,
    rounds: Mutex<HashMap<AccountId, Round>>,
    /// What runs once you signed in to an account again.
    then: OnceLock<Box<dyn Fn(AccountId) + Send + Sync>>,
}

/// One account's sign-in. Dropping it stops its CLI.
struct Round {
    id: SignInId,
    paste: bool,
    /// The CLI's input, for the code you paste.
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    state: watch::Receiver<CliLoginState>,
    /// Stops the round when sent or dropped.
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// What a round's task runs.
struct Run {
    account: AccountId,
    sign_in: SignInId,
    flow: Flow,
    child: Child,
    output: watch::Sender<Vec<u8>>,
    state: watch::Sender<CliLoginState>,
    stopped: oneshot::Receiver<()>,
}

/// How a round's CLI ended.
enum Ended {
    /// On its own, with its exit code.
    Exited(Option<i32>),
    /// Stopped at [`Limits::round`].
    TimedOut,
    /// Stopped by a cancel or a newer round.
    Stopped,
}

impl Rounds {
    pub(super) fn new(hub: Hub, limits: Limits) -> Self {
        Self {
            inner: Arc::new(Shared {
                hub,
                limits,
                rounds: Mutex::default(),
                then: OnceLock::new(),
            }),
        }
    }

    /// Runs `then` with each account you sign in to again, once its CLI
    /// stored the new login. Only the first call counts.
    pub(super) fn on_signed_in(&self, then: Box<dyn Fn(AccountId) + Send + Sync>) {
        let _ = self.inner.then.set(then);
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<AccountId, Round>> {
        self.inner
            .rounds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers `BeginCliLogin`: runs the CLI's own sign-in for `account`,
    /// a Claude Code, Codex or Cursor login in its folder on this device.
    /// `device_code` picks Codex's device code over a sign-in that comes
    /// back to this device's localhost, and the others leave it out.
    pub(super) async fn begin(&self, account: AccountId, device_code: bool) -> ActionOutcome {
        match self.sign_in(account, device_code) {
            Ok((launch, flow)) => self.start(account, &launch, flow).await,
            Err(reason) => refused(reason),
        }
    }

    /// The CLI [`Rounds::begin`] runs for `account`, or why it runs none.
    fn sign_in(
        &self,
        account: AccountId,
        device_code: bool,
    ) -> Result<(Launch, Flow), &'static str> {
        let Some(Entity::Account(found)) = self.inner.hub.get(&EntityKey::Account(account)) else {
            return Err("This device doesn't hold that account.");
        };
        if found.device != self.inner.hub.device().id {
            return Err("That account is on another device. Sign in to it again from there.");
        }
        if found.login != LoginKind::ConfigDir {
            return Err(
                "This account doesn't sign in through its CLI. Change its login in Settings, Accounts.",
            );
        }
        let login = Login {
            account,
            config_dir: PathBuf::from(&found.config_dir),
            auth: LoginAuth::ConfigDir,
        };
        match found.vendor {
            Vendor::Claude => Ok((claude::login_launch(&login), Flow::Claude)),
            Vendor::Codex if device_code => {
                Ok((codex::login_launch(&login, true), Flow::CodexDevice))
            }
            Vendor::Codex => Ok((codex::login_launch(&login, false), Flow::Codex)),
            Vendor::Cursor => Ok((cursor::login_launch(&login), Flow::Cursor)),
            Vendor::Acp => {
                Err("Signing in again works for Claude Code, Codex and Cursor logins only.")
            }
        }
    }

    /// Starts `launch`, the sign-in `flow` for `account`, and answers with
    /// the link it prints. A round the account already has stops first.
    pub(super) async fn start(
        &self,
        account: AccountId,
        launch: &Launch,
        flow: Flow,
    ) -> ActionOutcome {
        let replaced = self.lock().remove(&account);
        if let Some(Round { stop, task, .. }) = replaced {
            drop(stop);
            let _ = tokio::time::timeout(REPLACE, task).await;
        }

        let mut command = launch.command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = match crate::tools::retry_busy(|| std::future::ready(command.spawn())).await
        {
            Ok(child) => child,
            Err(error) => {
                let cli = flow.cli();
                return refused(format!(
                    "This device couldn't start {cli} to sign in: {error}"
                ));
            }
        };
        let sign_in = SignInId::generate();
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take()));
        let (output, mut printed) = watch::channel(Vec::new());
        let (state, watched) = watch::channel(CliLoginState::Waiting);
        let (stop, stopped) = oneshot::channel();
        let run = Run {
            account,
            sign_in,
            flow,
            child,
            output,
            state,
            stopped,
        };
        let round = Round {
            id: sign_in,
            paste: flow.paste(),
            stdin,
            state: watched,
            stop,
            task: tokio::spawn(self.clone().run(run)),
        };

        // The output closes once the CLI ended.
        let waited = tokio::time::timeout(self.inner.limits.link, async {
            loop {
                if let Some(link) = flow.link(&whole_lines(&printed.borrow_and_update())) {
                    return Some(link);
                }
                if printed.changed().await.is_err() {
                    return None;
                }
            }
        })
        .await;
        let link = waited
            .ok()
            .flatten()
            .or_else(|| flow.link(&whole_lines(&printed.borrow())));
        let ended = round.state.borrow().clone();
        match (link, ended) {
            (Some(_), CliLoginState::Failed { message }) => refused(message),
            (Some(Link { url, user_code }), _) => {
                self.lock().insert(account, round);
                info!(%account, %sign_in, "started a sign-in through the CLI");
                ActionOutcome::CliLogin {
                    sign_in,
                    url,
                    user_code,
                    paste: flow.paste(),
                }
            }
            (None, _) => {
                let said = last_words(&plain_lines(&printed.borrow()), 2);
                drop(round);
                let cli = flow.cli();
                refused(match said {
                    Some(said) => format!("{cli} didn't give a sign-in link: {said}"),
                    None => format!("{cli} didn't give a sign-in link."),
                })
            }
        }
    }

    /// Runs a round's CLI to its end and says how it ended, then keeps the
    /// round for [`Limits::keep`].
    async fn run(self, run: Run) {
        let Run {
            account,
            sign_in,
            flow,
            mut child,
            output,
            state,
            mut stopped,
        } = run;
        let pid = child.id();
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let ended = {
            let readers = async { tokio::join!(keep(stdout, &output), keep(stderr, &output)) };
            tokio::pin!(readers);
            let limit = tokio::time::sleep(self.inner.limits.round);
            tokio::pin!(limit);
            let mut read = false;
            let ended = loop {
                tokio::select! {
                    status = child.wait() => {
                        break Ended::Exited(status.ok().and_then(|status| status.code()));
                    }
                    () = &mut limit => break Ended::TimedOut,
                    _ = &mut stopped => break Ended::Stopped,
                    _ = &mut readers, if !read => read = true,
                }
            };
            if !matches!(ended, Ended::Exited(_)) {
                // The group goes while the CLI is still unreaped, so its ID
                // can't be anyone else's yet.
                if let Some(pid) = pid.and_then(|pid| rustix::process::Pid::from_raw(pid as i32)) {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
                let _ = child.kill().await;
            }
            if !read {
                let _ = tokio::time::timeout(DRAIN, &mut readers).await;
            }
            ended
        };

        let (next, how) = match ended {
            Ended::Stopped => {
                info!(%account, %sign_in, "stopped a sign-in through the CLI");
                return;
            }
            Ended::TimedOut => (
                CliLoginState::Failed {
                    message: TIMED_OUT.to_owned(),
                },
                "timed out",
            ),
            Ended::Exited(Some(0)) => (CliLoginState::SignedIn, "signed in"),
            Ended::Exited(_) => {
                let message = last_words(&plain_lines(&output.borrow()), 1)
                    .unwrap_or_else(|| format!("{} couldn't sign in.", flow.cli()));
                (CliLoginState::Failed { message }, "failed")
            }
        };
        let signed_in = next == CliLoginState::SignedIn;
        state.send_replace(next);
        // A Begin still waiting for the link stops waiting.
        drop(output);
        info!(%account, %sign_in, ended = how, "a sign-in through the CLI ended");
        if signed_in {
            self.inner.hub.send(Command::RefreshUsage(Some(account)));
            if let Some(then) = self.inner.then.get() {
                then(account);
            }
        }

        tokio::select! {
            () = tokio::time::sleep(self.inner.limits.keep) => {}
            _ = stopped => return,
        }
        let mut rounds = self.lock();
        if rounds
            .get(&account)
            .is_some_and(|round| round.id == sign_in)
        {
            rounds.remove(&account);
        }
    }

    /// Answers `FinishCliLogin`: writes `code` to the CLI of `sign_in` and
    /// answers how the round stands once the CLI took it, or after
    /// [`Limits::code`] while it still runs.
    pub(super) async fn finish(&self, sign_in: SignInId, code: Secret) -> ActionOutcome {
        let found = self
            .lock()
            .values()
            .find(|round| round.id == sign_in)
            .map(|round| (round.paste, round.stdin.clone(), round.state.clone()));
        let Some((paste, stdin, mut state)) = found else {
            return refused(GONE);
        };
        if !paste {
            return refused("This sign-in ends in the browser, with no code to paste.");
        }
        let Some(code) = pasted(code, MAX_CODE) else {
            return refused("That isn't a sign-in code. Copy the whole code and paste it again.");
        };
        if *state.borrow() == CliLoginState::Waiting {
            let mut line = Zeroizing::new(Vec::with_capacity(code.expose().len() + 1));
            line.extend_from_slice(code.expose().as_bytes());
            line.push(b'\n');
            if let Some(stdin) = stdin.lock().await.as_mut() {
                // A CLI that ended reads no more, and its state says how.
                let _ = async {
                    stdin.write_all(&line).await?;
                    stdin.flush().await
                }
                .await;
            }
            let took = state.wait_for(|state| *state != CliLoginState::Waiting);
            let _ = tokio::time::timeout(self.inner.limits.code, took).await;
        }
        let state = state.borrow().clone();
        ActionOutcome::CliLoginState { state }
    }

    /// Answers `CheckCliLogin`.
    pub(super) fn check(&self, sign_in: SignInId) -> ActionOutcome {
        match self.lock().values().find(|round| round.id == sign_in) {
            Some(round) => ActionOutcome::CliLoginState {
                state: round.state.borrow().clone(),
            },
            None => refused(GONE),
        }
    }

    /// Answers `CancelCliLogin`: stops the CLI of `sign_in`. One that
    /// already ended counts as stopped.
    pub(super) fn cancel(&self, sign_in: SignInId) -> ActionOutcome {
        let mut rounds = self.lock();
        let account = rounds
            .iter()
            .find(|(_, round)| round.id == sign_in)
            .map(|(account, _)| *account);
        let cancelled = account.and_then(|account| rounds.remove(&account));
        drop(rounds);
        drop(cancelled);
        ActionOutcome::Done
    }
}

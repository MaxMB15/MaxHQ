// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: preparing the
// destination, running the clone, discarding a partial one and redacting
// credentials in URLs, translated to Rust without regexes. A finished clone
// isn't kept for a while, since the folder's workspace shows it from then
// on.

//! Runs `git clone` into folders on this device, one clone per folder,
//! and says how each one goes. A clone that fails or is cancelled stays,
//! to be retried or removed. Cancelling and removing stop git and take
//! away the partial checkout, but only one that holds git's `.git`.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_core::clone_progress::{self, Lines};
use maxhq_core::names;
use maxhq_protocol::ids::{DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{CloneProgress, CloneStage, CloneState, WorkspaceLocation};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::warn;

/// How long git may say nothing before the clone shows as stalled. Git
/// prints nothing while a transfer waits on the network, and the kernel
/// can take many minutes to give up on a dead connection.
pub const STALL_AFTER: Duration = Duration::from_secs(20);

/// The most characters a failure's reason keeps.
const REASON_CHARS: usize = 1000;

/// How many of git's last lines a failure's reason keeps.
const TAIL_LINES: usize = 4;

/// What a failure says when git said nothing useful.
const NOT_CLONED: &str = "The repository could not be cloned.";

/// What a request about a folder that isn't being cloned is told.
const UNKNOWN: &str = "That folder isn't being cloned.";

/// How often a partial checkout's removal is tried again, while the git
/// that was stopped still closes its files.
const REMOVE_TRIES: u32 = 5;
const REMOVE_PAUSE: Duration = Duration::from_millis(200);

/// Where a clone's news goes.
pub trait Sink: Send + Sync + 'static {
    /// The clone is at `progress`.
    fn show(&self, clone: &Spec, progress: CloneProgress);
    /// The clone finished, so its folder is a checkout now.
    fn done(&self, clone: &Spec);
    /// The clone was removed, along with its partial checkout.
    fn removed(&self, clone: &Spec);
}

/// One clone, which a retry runs again.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The folder's workspace.
    pub workspace: WorkspaceId,
    /// The project the folder is a checkout of.
    pub project: ProjectId,
    /// The folder git clones into.
    pub folder: PathBuf,
    /// What git clones from. It may carry credentials, so it's never
    /// shown.
    url: String,
    /// Whether the folder was there, empty, before the clone, so taking
    /// a partial checkout away leaves it.
    existed: bool,
}

impl Spec {
    /// A clone into `folder`, for tests of what shows clones.
    #[cfg(test)]
    pub fn testing(workspace: WorkspaceId, project: ProjectId, folder: PathBuf) -> Self {
        Self {
            workspace,
            project,
            folder,
            url: String::new(),
            existed: false,
        }
    }
}

/// The clones on this device.
pub struct Tracker {
    device: DeviceId,
    git: OsString,
    stall_after: Duration,
    sink: Arc<dyn Sink>,
    clones: Mutex<HashMap<WorkspaceId, Entry>>,
    /// Takes starts, cancels, retries and removes one at a time, so a
    /// retry can't race the clone it replaces.
    actions: tokio::sync::Mutex<()>,
}

struct Entry {
    spec: Spec,
    state: CloneState,
    /// The git that runs, while it does.
    run: Option<Run>,
}

struct Run {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// How one run of git ended.
enum Ended {
    Cloned,
    Failed(String),
    Stopped,
}

impl Tracker {
    /// Clones on `device` with the `git` program, telling `sink` how they
    /// go.
    pub fn new(device: DeviceId, git: impl Into<OsString>, sink: Arc<dyn Sink>) -> Arc<Self> {
        Arc::new(Self {
            device,
            git: git.into(),
            stall_after: STALL_AFTER,
            sink,
            clones: Mutex::default(),
            actions: tokio::sync::Mutex::new(()),
        })
    }

    /// The same, with a stall after `stall_after`.
    #[cfg(test)]
    pub fn stalling_after(
        device: DeviceId,
        git: impl Into<OsString>,
        sink: Arc<dyn Sink>,
        stall_after: Duration,
    ) -> Arc<Self> {
        let mut tracker = Self::new(device, git, sink);
        if let Some(tracker) = Arc::get_mut(&mut tracker) {
            tracker.stall_after = stall_after;
        }
        tracker
    }

    /// Starts cloning `url` into a new folder `name` in `parent`, for
    /// `project`. The answer is the folder's workspace, and the clone goes
    /// on after it.
    pub async fn start(
        self: &Arc<Self>,
        project: ProjectId,
        url: String,
        parent: String,
        name: String,
    ) -> Result<WorkspaceId, String> {
        let _turn = self.actions.lock().await;
        let tracker = self.clone();
        let claim = move || {
            let folder = destination(&parent, &name)?;
            let location = WorkspaceLocation::Folder {
                path: folder.to_string_lossy().into_owned(),
            };
            let workspace = names::workspace(tracker.device, &location);
            if tracker.lock().contains_key(&workspace) {
                return Err("A clone into this destination is already in progress.".to_owned());
            }
            let existed = check_destination(&folder)?;
            Ok(Spec {
                workspace,
                project,
                folder,
                url,
                existed,
            })
        };
        let spec = tokio::task::spawn_blocking(claim)
            .await
            .unwrap_or_else(|_| Err("the folder check stopped".to_owned()))?;
        let workspace = spec.workspace;
        let state = connecting();
        self.sink.show(&spec, progress(&state));
        self.lock().insert(
            workspace,
            Entry {
                spec,
                state,
                run: None,
            },
        );
        self.launch(workspace);
        Ok(workspace)
    }

    /// Stops a running clone and takes its partial checkout away.
    pub async fn cancel(self: &Arc<Self>, workspace: WorkspaceId) -> Result<(), String> {
        let _turn = self.actions.lock().await;
        let run = match self.lock().get_mut(&workspace) {
            Some(entry) if matches!(entry.state, CloneState::Cloning { .. }) => entry.run.take(),
            Some(_) => return Err("That clone isn't running.".to_owned()),
            None => return Err(UNKNOWN.to_owned()),
        };
        if let Some(run) = run {
            stop(run).await;
        }
        // Git may have finished while it was being stopped, and then the
        // checkout is yours.
        let Some(spec) = self.spec(workspace) else {
            return Ok(());
        };
        self.update(workspace, |state| {
            matches!(state, CloneState::Cloning { .. }).then_some(CloneState::Cancelled)
        });
        if let Err(reason) = discard_off_loop(spec).await {
            warn!(%workspace, "couldn't take a cancelled clone away: {reason}");
        }
        Ok(())
    }

    /// Starts a failed or cancelled clone again, in a folder cleared of
    /// what the last try left.
    pub async fn retry(self: &Arc<Self>, workspace: WorkspaceId) -> Result<(), String> {
        let _turn = self.actions.lock().await;
        let spec = match self.lock().get(&workspace) {
            Some(entry) if !matches!(entry.state, CloneState::Cloning { .. }) => entry.spec.clone(),
            Some(_) => return Err("That clone is still running.".to_owned()),
            None => return Err(UNKNOWN.to_owned()),
        };
        discard_off_loop(spec).await?;
        self.update(workspace, |_| Some(connecting()));
        self.launch(workspace);
        Ok(())
    }

    /// Stops a clone if it runs, takes its partial checkout away and
    /// forgets it.
    pub async fn remove(self: &Arc<Self>, workspace: WorkspaceId) -> Result<(), String> {
        let _turn = self.actions.lock().await;
        let run = match self.lock().get_mut(&workspace) {
            Some(entry) => entry.run.take(),
            None => return Err(UNKNOWN.to_owned()),
        };
        if let Some(run) = run {
            stop(run).await;
        }
        let Some(entry) = self.lock().remove(&workspace) else {
            return Ok(());
        };
        if let Err(reason) = discard_off_loop(entry.spec.clone()).await {
            warn!(%workspace, "couldn't take a removed clone away: {reason}");
        }
        self.sink.removed(&entry.spec);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<WorkspaceId, Entry>> {
        self.clones.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn spec(&self, workspace: WorkspaceId) -> Option<Spec> {
        self.lock().get(&workspace).map(|entry| entry.spec.clone())
    }

    /// Runs git for a clone. The lock is held until the run is noted, so
    /// a run that ends at once finds it.
    fn launch(self: &Arc<Self>, workspace: WorkspaceId) {
        let mut clones = self.lock();
        let Some(entry) = clones.get_mut(&workspace) else {
            return;
        };
        let (stop, stopped) = oneshot::channel();
        let tracker = self.clone();
        let spec = entry.spec.clone();
        let task = tokio::spawn(async move { tracker.run(spec, stopped).await });
        entry.run = Some(Run { stop, task });
    }

    async fn run(self: Arc<Self>, spec: Spec, stopped: oneshot::Receiver<()>) {
        let workspace = spec.workspace;
        match self.git_clone(&spec, stopped).await {
            Ended::Cloned => {
                // It's no longer a clone before the sink hears, so a late
                // cancel can't take the checkout away.
                if self.lock().remove(&workspace).is_some() {
                    self.sink.done(&spec);
                }
            }
            Ended::Failed(reason) => {
                let reason = clamp(&reason, REASON_CHARS);
                self.update(workspace, |_| Some(CloneState::Failed { reason }));
                if let Some(entry) = self.lock().get_mut(&workspace) {
                    entry.run = None;
                }
            }
            Ended::Stopped => {}
        }
    }

    /// Runs `git clone --progress` in the folder's parent, passing on its
    /// progress, until it ends or `stopped` fires.
    async fn git_clone(&self, spec: &Spec, mut stopped: oneshot::Receiver<()>) -> Ended {
        let (Some(parent), Some(name)) = (spec.folder.parent(), spec.folder.file_name()) else {
            return Ended::Failed(NOT_CLONED.to_owned());
        };
        let mut git = Command::new(&self.git);
        git.current_dir(parent)
            .args(["clone", "--progress", "--"])
            .arg(normalize_url(&spec.url))
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Progress shows at once on a pipe, and a prompt for a
            // password, with no terminal to answer it, is refused.
            .env("GIT_PROGRESS_DELAY", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            // A repository's own config sets this URL, and one like `evil::x`
            // runs `git-remote-evil` from the PATH. A project whose remote
            // is a folder on this device clones over `file`.
            .env("GIT_ALLOW_PROTOCOL", "http:https:ssh:git:file")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // A git hook sets these, and they would point the clone at the
            // repo running the hook.
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE");
        // Tests clone as whoever runs them, so their own config stays out.
        #[cfg(test)]
        git.env("GIT_CONFIG_GLOBAL", "/dev/null");
        let mut child = match crate::tools::retry_busy(|| std::future::ready(git.spawn())).await {
            Ok(child) => child,
            Err(err) => return Ended::Failed(format!("Couldn't run git: {err}")),
        };
        let Some(mut stderr) = child.stderr.take() else {
            return Ended::Failed(NOT_CLONED.to_owned());
        };
        let workspace = spec.workspace;
        let mut lines = Lines::default();
        let mut tail = Tail::default();
        let mut stalled = false;
        let mut chunk = vec![0; 8 * 1024];
        let heard = |line: &str, tail: &mut Tail| match clone_progress::parse(line) {
            Some(heard) => self.update(workspace, |state| {
                let next = CloneState::Cloning {
                    stage: heard.stage,
                    percent: heard.percent,
                    stalled: false,
                };
                (*state != next).then_some(next)
            }),
            None => tail.push(line),
        };
        loop {
            let read = tokio::select! {
                _ = &mut stopped => return Ended::Stopped,
                read = tokio::time::timeout(self.stall_after, stderr.read(&mut chunk)) => read,
            };
            match read {
                Err(_) if !stalled => {
                    stalled = true;
                    self.set_stalled(workspace, true);
                }
                Err(_) => {}
                Ok(Ok(0) | Err(_)) => break,
                Ok(Ok(len)) => {
                    if stalled {
                        stalled = false;
                        self.set_stalled(workspace, false);
                    }
                    lines.push(&chunk[..len], |line| heard(line, &mut tail));
                }
            }
        }
        lines.finish(|line| heard(line, &mut tail));
        let status = tokio::select! {
            _ = &mut stopped => return Ended::Stopped,
            status = child.wait() => status,
        };
        match status {
            Ok(status) if status.success() => Ended::Cloned,
            _ => Ended::Failed(tail.reason()),
        }
    }

    fn set_stalled(&self, workspace: WorkspaceId, now: bool) {
        self.update(workspace, |state| match state {
            CloneState::Cloning {
                stage,
                percent,
                stalled,
            } if *stalled != now => Some(CloneState::Cloning {
                stage: *stage,
                percent: *percent,
                stalled: now,
            }),
            _ => None,
        });
    }

    /// Changes a clone's state to what `change` gives, if anything, and
    /// tells the sink. The sink hears under the lock, so it hears changes
    /// in order.
    fn update(
        &self,
        workspace: WorkspaceId,
        change: impl FnOnce(&CloneState) -> Option<CloneState>,
    ) {
        let mut clones = self.lock();
        let Some(entry) = clones.get_mut(&workspace) else {
            return;
        };
        if let Some(state) = change(&entry.state) {
            entry.state = state;
            self.sink.show(&entry.spec, progress(&entry.state));
        }
    }
}

/// Stops a run and waits for its git to go.
async fn stop(run: Run) {
    let _ = run.stop.send(());
    let _ = run.task.await;
}

fn connecting() -> CloneState {
    CloneState::Cloning {
        stage: CloneStage::Connecting,
        percent: None,
        stalled: false,
    }
}

fn progress(state: &CloneState) -> CloneProgress {
    CloneProgress {
        state: state.clone(),
    }
}

/// The folder to clone into: `name` in `parent`, which is made if it isn't
/// there, with links in `parent` resolved so the folder has the path
/// agents report for it.
fn destination(parent: &str, name: &str) -> Result<PathBuf, String> {
    let parent = Path::new(parent.trim());
    let plain = parent.is_absolute()
        && parent.components().all(|part| {
            matches!(
                part,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        });
    if !plain || parent.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("Choose the folder to clone into by its full path.".to_owned());
    }
    let mut parts = Path::new(name).components();
    let named = matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(part)), None) if part == name
    );
    if !named || name.contains('\0') {
        return Err(format!("{name:?} can't be a folder's name."));
    }
    fs::create_dir_all(parent)
        .map_err(|err| format!("Couldn't make {}: {err}", parent.display()))?;
    let parent = maxhq_core::fs_ext::canonicalize(parent)
        .map_err(|err| format!("Couldn't open {}: {err}", parent.display()))?;
    if parent.to_str().is_none() {
        return Err("The folder to clone into has a path that isn't UTF-8.".to_owned());
    }
    Ok(parent.join(name))
}

/// Whether `folder` is there already, which is fine only for an empty
/// folder.
fn check_destination(folder: &Path) -> Result<bool, String> {
    let not_folder = || "Destination path already exists and is not a directory.".to_owned();
    match fs::symlink_metadata(folder) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("Couldn't check {}: {err}", folder.display())),
        Ok(meta) if !meta.is_dir() => Err(not_folder()),
        Ok(_) => {
            let mut entries = fs::read_dir(folder).map_err(|_| not_folder())?;
            match entries.next() {
                Some(_) => Err("Destination path already exists and is not empty.".to_owned()),
                None => Ok(true),
            }
        }
    }
}

async fn discard_off_loop(spec: Spec) -> Result<(), String> {
    tokio::task::spawn_blocking(move || discard(&spec))
        .await
        .unwrap_or_else(|_| Err("The partial clone could not be removed.".to_owned()))
}

/// Takes a partial checkout away. Only a folder holding `.git` goes, since
/// anything else in it came from somewhere other than the clone. A folder
/// that was there before the clone stays, empty.
fn discard(spec: &Spec) -> Result<(), String> {
    let folder = &spec.folder;
    let names: Vec<OsString> = match fs::read_dir(folder) {
        Ok(entries) => entries.flatten().map(|entry| entry.file_name()).collect(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err("The clone destination could not be inspected.".to_owned()),
    };
    if !names.is_empty() && !names.iter().any(|name| name == ".git") {
        return Err("Destination path contains files that are not from the clone.".to_owned());
    }
    let mut tries = 0;
    loop {
        let removed = match maxhq_transfer::remove_tree(folder) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            removed => removed,
        };
        let made = removed.and_then(|()| match spec.existed {
            true => fs::create_dir(folder),
            false => Ok(()),
        });
        match made {
            Ok(()) => return Ok(()),
            Err(_) if tries < REMOVE_TRIES => {
                tries += 1;
                std::thread::sleep(REMOVE_PAUSE);
            }
            Err(_) => return Err("The partial clone could not be removed.".to_owned()),
        }
    }
}

/// The last lines git said that aren't progress, for a failure's reason.
#[derive(Default)]
struct Tail(VecDeque<String>);

impl Tail {
    fn push(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Cloning into") {
            return;
        }
        // Git echoes the remote in some failures, and this is shown.
        if self.0.len() == TAIL_LINES {
            self.0.pop_front();
        }
        self.0.push_back(redact(line));
    }

    fn reason(&self) -> String {
        if self.0.is_empty() {
            return NOT_CLONED.to_owned();
        }
        Vec::from_iter(self.0.iter().map(String::as_str)).join(" ")
    }
}

/// `text` cut to `max` characters, ending in an ellipsis when it's cut.
fn clamp(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_owned(),
        Some(_) => {
            let keep: String = text.chars().take(max - 1).collect();
            format!("{keep}…")
        }
    }
}

/// Git reads an `@` in the query of a URL with no path as the start of the
/// host, so the URL gets a `/` before its query.
fn normalize_url(url: &str) -> String {
    if let Some(after) = scheme(url, 0) {
        let host = url[after..]
            .find(['/', '?', '#'])
            .map_or(url.len(), |at| after + at);
        if host > after && url[host..].starts_with('?') {
            return format!("{}/{}", &url[..host], &url[host..]);
        }
    }
    url.to_owned()
}

/// Takes credentials out of the URLs in `text`: a query, which a clone URL
/// only has when it's a token, and a user and password before the host.
pub(crate) fn redact(text: &str) -> String {
    let text = replace_all(text, quoted_query);
    let text = replace_all(&text, url_query);
    replace_all(&text, user_info)
}

/// A match found at a place in the text: where it ends, and what replaces
/// it.
type Found = Option<(usize, String)>;

/// Replaces each match `find` finds, from the left, where `find` tries a
/// match at one place.
fn replace_all(text: &str, find: fn(&str, usize) -> Found) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < text.len() {
        if let Some((end, with)) = find(text, at) {
            out.push_str(&text[copied..at]);
            out.push_str(&with);
            copied = end;
            at = end;
        } else {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Where a URL's `scheme://` that starts at `at` ends.
fn scheme(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.get(at)?.is_ascii_alphabetic() {
        return None;
    }
    let end = at
        + bytes[at..]
            .iter()
            .position(|&b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')))
            .unwrap_or(bytes.len() - at);
    text[end..].starts_with("://").then_some(end + 3)
}

/// Whether a word starts at `at`, as a regex's `\b` before a letter means.
fn word_starts(text: &str, at: usize) -> bool {
    at == 0 || {
        let before = text.as_bytes()[at - 1];
        !(before.is_ascii_alphanumeric() || before == b'_')
    }
}

/// Where the part of a URL from `from` up to a space or a `?` ends.
fn path_end(text: &str, from: usize) -> usize {
    text[from..]
        .find(|c: char| c.is_whitespace() || c == '?')
        .map_or(text.len(), |at| from + at)
}

/// A quoted URL with a query: `'scheme://host/path?query'`, where git
/// doesn't escape a quote in the query, so the match runs to the last
/// quote on the line before a space, a `:` or the end.
fn quoted_query(text: &str, at: usize) -> Found {
    let quote = match text.as_bytes()[at] {
        quote @ (b'\'' | b'"') => quote as char,
        _ => return None,
    };
    let path = path_end(text, scheme(text, at + 1)?);
    if !text[path..].starts_with('?') {
        return None;
    }
    let query = path + 1;
    let line = text[query..]
        .find(['\n', '\r', '\u{2028}', '\u{2029}'])
        .map_or(text.len(), |end| query + end);
    let close = text[query..line]
        .rmatch_indices(quote)
        .find_map(|(close, _)| {
            let close = query + close;
            let next = text[close + 1..].chars().next();
            next.is_none_or(|c| c.is_whitespace() || c == ':')
                .then_some(close)
        })?;
    Some((close + 1, format!("{quote}{}{quote}", &text[at + 1..path])))
}

/// A URL with a query, which goes up to the next space.
fn url_query(text: &str, at: usize) -> Found {
    if !word_starts(text, at) {
        return None;
    }
    let path = path_end(text, scheme(text, at)?);
    if !text[path..].starts_with('?') {
        return None;
    }
    let end = text[path..]
        .find(char::is_whitespace)
        .map_or(text.len(), |end| path + end);
    Some((end, text[at..path].to_owned()))
}

/// A URL's user and password, which may hold `@`s, up to the last `@`
/// before the host.
fn user_info(text: &str, at: usize) -> Found {
    if !word_starts(text, at) {
        return None;
    }
    let after = scheme(text, at)?;
    let run = text[after..]
        .find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#'))
        .map_or(text.len(), |end| after + end);
    let last = after + text[after..run].rfind('@')?;
    (last > after).then(|| (last + 1, text[at..after].to_owned()))
}

#[cfg(test)]
mod tests;

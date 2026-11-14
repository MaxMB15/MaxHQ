// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One poller for each repository an enabled task watches, started and
//! stopped as the tasks change. Each poll reads the repository's listings
//! the tasks need, as [`github_activity`] lays out, and each new event
//! goes to every task that watches the repository and lists its type, with
//! what a listing cuts out read in full. An event a repository webhook
//! rang for comes in through [`Pollers::ring`] and goes the same way, and
//! one heard both ways runs once. While the webhook delivers, polls only
//! back it up.
//!
//! [`github_activity`]: maxhq_core::github_activity

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use maxhq_core::github_activity::{
    happened, listed, listings, pull_opened, pull_synchronized, review_submitted, reviews_path,
    time, uncovered, Listing, REVIEWS,
};
use maxhq_core::github_events::{
    event_id, hook_events, hooked, shared_key, slim_parts, webhook_name, Hooked,
};
use maxhq_protocol::tasks::{Task, Trigger};
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::cursors::{Cursor, PullMark};
use super::poll::{backoff, DEFAULT_INTERVAL};
use super::seen::Heard;
use super::{EventSource, HookStates};
use crate::db::Db;
use crate::hooks::{Deliver, Request};
use crate::secrets::{Fetched, GithubError, RepoName};
use crate::session::clock::Clock;
use crate::tasks::Tasks;

/// What a task watching a repository the App isn't installed on shows.
pub const INSTALL_APP: &str = "Install the GitHub App on this repo.";

/// What a task shows when an event's full details couldn't be read.
pub const SHORT_FORM: &str =
    "Couldn't read an event's details from GitHub, so its run got the short form";
/// What a task shows when a pull request's reviews couldn't be read. The
/// next poll reads them again.
pub const NO_REVIEWS: &str = "Couldn't read a PR's reviews from GitHub";

/// How long after a webhook's last delivery polls only back it up.
pub const FRESH: Duration = Duration::from_secs(60 * 60);
/// How often a repository whose webhook delivers is polled.
pub const BACKSTOP: Duration = Duration::from_secs(15 * 60);
/// How long after a nudge polls keep their usual pace.
const NUDGED_FOR: Duration = Duration::from_secs(5 * 60);

/// Why each repository's last poll fell short, or else why its last read
/// of an event's details did, by [`RepoName::key`]. A repository whose
/// poll and reads worked has no entry.
pub type Problems = BTreeMap<String, String>;

/// What the pollers share.
struct Shared {
    tasks: watch::Receiver<Arc<[Task]>>,
    source: Arc<dyn EventSource>,
    deliver: Arc<dyn Deliver>,
    db: Db,
    clock: Arc<dyn Clock>,
    problems: watch::Sender<Problems>,
    /// How each repository's webhook stands, which paces its polls.
    hooks: watch::Sender<HookStates>,
    /// Each running poller, by [`RepoName::key`].
    pollers: Mutex<HashMap<String, Arc<Poller>>>,
}

/// The running pollers, which the webhook doorbell hands events to.
/// Clones share them.
#[derive(Clone)]
pub struct Pollers {
    shared: Arc<Shared>,
}

/// Starts polling for `tasks`' GitHub event tasks, reading from `source`
/// and handing each new event to `deliver`. It stops when `tasks` stops
/// sending. A repository whose webhook in `hooks` delivers is polled less.
pub fn start(
    tasks: &dyn Tasks,
    source: Arc<dyn EventSource>,
    deliver: Arc<dyn Deliver>,
    db: Db,
    clock: Arc<dyn Clock>,
    hooks: watch::Sender<HookStates>,
) -> Pollers {
    let shared = Arc::new(Shared {
        tasks: tasks.watch(),
        source,
        deliver,
        db,
        clock,
        problems: watch::Sender::new(Problems::new()),
        hooks,
        pollers: Mutex::default(),
    });
    tokio::spawn(run(shared.clone(), tasks.watch()));
    Pollers { shared }
}

impl Pollers {
    /// Which repositories can't be polled, and why.
    pub fn problems(&self) -> watch::Receiver<Problems> {
        self.shared.problems.subscribe()
    }

    /// Whether `repo` has a poller running, which a webhook's events go
    /// through.
    pub fn polls(&self, repo: &RepoName) -> bool {
        self.poller(repo).is_some()
    }

    /// Hands `event`, built from a webhook delivery in the events API's
    /// shape, to the tasks that watch `repo`, as a poll would.
    pub async fn ring(&self, repo: &RepoName, event: &Value) {
        let Some(poller) = self.poller(repo) else {
            return;
        };
        let mut lookups = Lookups::default();
        poller.hand(event, Heard::Hooked, &mut lookups).await;
        poller.looked_up(lookups);
    }

    /// Puts `repo`'s polls back to their usual pace for a few minutes, so
    /// an event its webhook rang for that only a poll reads comes soon.
    pub fn nudge(&self, repo: &RepoName) {
        if let Some(poller) = self.poller(repo) {
            *lock(&poller.nudged) = Some(self.shared.clock.now() + NUDGED_FOR);
            poller.wake.notify_one();
        }
    }

    fn poller(&self, repo: &RepoName) -> Option<Arc<Poller>> {
        lock(&self.shared.pollers).get(&repo.key()).cloned()
    }
}

/// Keeps one poller running for each watched repository.
async fn run(shared: Arc<Shared>, mut tasks: watch::Receiver<Arc<[Task]>>) {
    let mut running: HashMap<String, JoinHandle<()>> = HashMap::new();
    loop {
        let repos = watched(&tasks.borrow_and_update());
        let gone: Vec<String> = running
            .keys()
            .filter(|key| !repos.contains_key(*key))
            .cloned()
            .collect();
        for key in gone {
            lock(&shared.pollers).remove(&key);
            if let Some(poller) = running.remove(&key) {
                poller.abort();
                let _ = poller.await;
            }
            // Turning a task on again starts from then, not from here.
            if let Err(err) = shared.db.remove_github_cursor(&key) {
                warn!(repo = key, "couldn't forget a GitHub cursor: {err}");
            }
            shared
                .problems
                .send_if_modified(|problems| problems.remove(&key).is_some());
        }
        for (key, repo) in repos {
            if let Entry::Vacant(entry) = running.entry(key) {
                let poller = Poller::new(shared.clone(), repo);
                lock(&shared.pollers).insert(entry.key().clone(), poller.clone());
                entry.insert(tokio::spawn(poller.poll()));
            }
        }
        if tasks.changed().await.is_err() {
            break;
        }
    }
    lock(&shared.pollers).clear();
    for poller in running.into_values() {
        poller.abort();
    }
}

/// The repositories enabled tasks watch, by [`RepoName::key`].
fn watched(tasks: &[Task]) -> HashMap<String, RepoName> {
    tasks
        .iter()
        .filter(|task| task.enabled)
        .filter_map(|task| match &task.trigger {
            Trigger::GithubEvents { repo, .. } => RepoName::parse(repo),
            _ => None,
        })
        .map(|repo| (repo.key(), repo))
        .collect()
}

/// Whether `task` takes webhook `event`, events API type `kind`, from
/// `repo`.
fn takes(task: &Task, repo: &RepoName, event: &str, kind: &str) -> bool {
    let Trigger::GithubEvents {
        repo: watched,
        events,
        ..
    } = &task.trigger
    else {
        return false;
    };
    task.enabled
        && RepoName::parse(watched).as_ref() == Some(repo)
        && events
            .iter()
            .any(|listed| listed == event || listed == kind)
}

/// What one poll read from GitHub beyond its events, so each path is read
/// once however many events and tasks want it.
#[derive(Default)]
struct Lookups {
    read: HashMap<String, Option<Arc<Value>>>,
    /// Why the last read that failed did, if one did.
    failed: Option<String>,
    /// Whether a read worked.
    worked: bool,
}

/// One repository's poller.
struct Poller {
    shared: Arc<Shared>,
    repo: RepoName,
    key: String,
    /// Where it got to.
    cursor: Mutex<Cursor>,
    /// The repository as GitHub reads it, once it's been read.
    repository: Mutex<Option<Arc<Value>>>,
    /// Why its last poll fell short, if it did.
    polled: Mutex<Option<String>>,
    /// Why its last read of an event's details failed, until one works.
    lookup: Mutex<Option<String>>,
    /// Cuts a wait short after a nudge.
    wake: Notify,
    /// Until when polls keep their usual pace, after a nudge.
    nudged: Mutex<Option<Instant>>,
}

/// An event a listing showed, with when its listing's items are new from.
type Found = (Value, u64);

impl Poller {
    /// A poller for `repo`, from its stored cursor if it has one.
    fn new(shared: Arc<Shared>, repo: RepoName) -> Arc<Self> {
        let key = repo.key();
        let cursor = shared.db.github_cursor(&key).unwrap_or_else(|err| {
            warn!(repo = key, "couldn't read a GitHub cursor: {err}");
            None
        });
        Arc::new(Self {
            shared,
            repo,
            key,
            cursor: Mutex::new(cursor.unwrap_or_default()),
            repository: Mutex::default(),
            polled: Mutex::default(),
            lookup: Mutex::default(),
            wake: Notify::new(),
            nudged: Mutex::default(),
        })
    }

    /// Polls until it's stopped. After polls that read nothing, the wait
    /// doubles up to 15 minutes.
    async fn poll(self: Arc<Self>) {
        let mut failures = 0;
        loop {
            let wait = match self.sweep().await {
                Some(wait) => {
                    failures = 0;
                    wait
                }
                None => {
                    failures += 1;
                    backoff(failures)
                }
            };
            self.pause(wait).await;
        }
    }

    /// Waits `wait`, or longer while the repository's webhook delivers.
    /// A nudge works the wait out again.
    async fn pause(&self, wait: Duration) {
        let clock = &self.shared.clock;
        let start = clock.now();
        loop {
            let until = start + self.paced(wait);
            if clock.now() >= until {
                return;
            }
            tokio::select! {
                () = clock.sleep_until(until) => return,
                () = self.wake.notified() => {}
            }
        }
    }

    /// `wait`, stretched to [`BACKSTOP`] while the webhook delivered within
    /// [`FRESH`], though never past then, unless a nudge asked for the usual
    /// pace. It keeps the webhook's `instant` in step.
    fn paced(&self, wait: Duration) -> Duration {
        let clock = &self.shared.clock;
        let hooks = &self.shared.hooks;
        let delivered = hooks
            .borrow()
            .get(&self.key)
            .and_then(|state| state.delivered);
        let ago = delivered
            .map(|at| Duration::from_millis(clock.wall().0.saturating_sub(at.0)))
            .filter(|ago| *ago < FRESH);
        hooks.send_if_modified(|states| match states.get_mut(&self.key) {
            Some(state) if state.instant != ago.is_some() => {
                state.instant = ago.is_some();
                true
            }
            _ => false,
        });
        let nudged = lock(&self.nudged).is_some_and(|until| clock.now() < until);
        match ago {
            Some(ago) if !nudged => wait.max(BACKSTOP.min(FRESH - ago)),
            _ => wait,
        }
    }

    /// One poll: reads each listing the tasks need, hands on what's new in
    /// them, oldest first, and moves the cursor. A listing's first read
    /// only notes when it began. It gives how long to wait before the
    /// next poll, or none when every read failed.
    async fn sweep(&self) -> Option<Duration> {
        let watched = self.watching();
        let needed = listings(&watched);
        let repository = self.repository().await;
        let name = self.repo.to_string();
        let now = self.shared.clock.wall().0;
        let mut cursor = lock(&self.cursor).clone();
        let mut found = Vec::new();
        let mut lookups = Lookups::default();
        let mut failed = None;
        let mut worked = false;
        let mut poll_after = None;
        let reviews = watched.contains(&"pull_request_review");
        for &listing in &needed {
            let path = listing.path(&name);
            let etag = cursor.etags.get(&path).cloned();
            let listed_page = self.shared.source.list(&self.repo, &path, etag.as_deref());
            let page = match listed_page.await {
                Ok(page) => page,
                Err(err) => {
                    warn!(repo = %self.repo, path, "couldn't poll GitHub: {err}");
                    let installed = !matches!(err, GithubError::NotInstalled(_));
                    failed.get_or_insert(err);
                    if installed {
                        continue;
                    }
                    break;
                }
            };
            worked = true;
            poll_after = poll_after.max(page.poll_after);
            let since = *cursor.since.entry(listing.key().to_owned()).or_insert(now);
            if listing == Listing::Pulls && reviews {
                cursor.since.entry(REVIEWS.to_owned()).or_insert(now);
            }
            let Fetched::Fresh { value: items, etag } = page.items else {
                continue;
            };
            let whole = match listing {
                Listing::Pulls => {
                    let read = Read {
                        repository: &repository,
                        reviews,
                        found: &mut found,
                        lookups: &mut lookups,
                    };
                    self.pulls(&items, &mut cursor, read).await
                }
                listing => {
                    let events = items
                        .iter()
                        .filter_map(|item| listed(listing, item, &repository));
                    found.extend(events.map(|event| (event, since)));
                    true
                }
            };
            match etag.filter(|_| whole) {
                Some(etag) => cursor.etags.insert(path, etag),
                None => cursor.etags.remove(&path),
            };
        }
        self.forget_unneeded(&mut cursor, &needed, &watched);
        self.hand_new(found, &mut cursor, &mut lookups).await;
        cursor.prune();
        if cursor != *lock(&self.cursor) {
            if let Err(err) = self.shared.db.put_github_cursor(&self.key, &cursor) {
                warn!(repo = %self.repo, "couldn't store a GitHub cursor: {err}");
            }
            *lock(&self.cursor) = cursor;
        }
        let problem = match &failed {
            Some(GithubError::NotInstalled(_)) => Some(INSTALL_APP.to_owned()),
            Some(err) => Some(err.to_string()),
            None => self.unheard(&watched, &repository),
        };
        *lock(&self.polled) = problem;
        self.looked_up(lookups);
        self.show();
        (worked || failed.is_none()).then(|| poll_after.unwrap_or(DEFAULT_INTERVAL))
    }

    /// The webhook events the enabled tasks on this repository watch.
    fn watching(&self) -> Vec<&'static str> {
        let tasks = self.shared.tasks.borrow().clone();
        let listed = tasks
            .iter()
            .filter(|task| task.enabled)
            .filter_map(|task| match &task.trigger {
                Trigger::GithubEvents { repo, events, .. }
                    if RepoName::parse(repo).as_ref() == Some(&self.repo) =>
                {
                    Some(events)
                }
                _ => None,
            })
            .flatten()
            .map(String::as_str);
        hook_events(listed)
    }

    /// The repository as GitHub reads it, or only its name until a read
    /// works.
    async fn repository(&self) -> Arc<Value> {
        if let Some(known) = lock(&self.repository).clone() {
            return known;
        }
        let path = format!("/repos/{}", self.repo);
        match self.shared.source.read(&self.repo, &path).await {
            Ok(read) if read["full_name"].is_string() => {
                let read = Arc::new(read);
                *lock(&self.repository) = Some(read.clone());
                read
            }
            Ok(_) => Arc::new(json!({ "full_name": self.repo.to_string() })),
            Err(err) => {
                debug!(repo = %self.repo, "couldn't read a repository from GitHub: {err}");
                Arc::new(json!({ "full_name": self.repo.to_string() }))
            }
        }
    }

    /// What the pull requests list `pulls` says happened, onto `read`'s
    /// events: each pull request opened, each one's head that moved since
    /// the last read, and when it asks for reviews, the reviews of each one
    /// that changed. It notes each one's head in `cursor`, and says whether
    /// every reviews read worked.
    async fn pulls(&self, pulls: &[Value], cursor: &mut Cursor, read: Read<'_>) -> bool {
        let Read {
            repository,
            reviews,
            found,
            lookups,
        } = read;
        let now = self.shared.clock.wall().0;
        let since = cursor
            .since
            .get(Listing::Pulls.key())
            .copied()
            .unwrap_or(now);
        let reviewed_since = reviews.then(|| cursor.since.get(REVIEWS).copied().unwrap_or(now));
        let name = self.repo.to_string();
        let mut whole = true;
        for pull in pulls {
            let (Some(number), Some(sha)) = (pull["number"].as_u64(), pull["head"]["sha"].as_str())
            else {
                continue;
            };
            let changed = pull["updated_at"].as_str();
            let updated = time(&pull["updated_at"]).map_or(0, |at| at.0);
            let mark = cursor.pulls.remove(&number);
            let event = match &mark {
                None => pull_opened(pull, repository),
                Some(mark) if mark.sha != sha && pull["state"] == "open" => {
                    pull_synchronized(pull, &mark.sha, repository)
                }
                Some(_) => None,
            };
            found.extend(event.map(|event| (event, since)));
            let mut reviewed = mark.and_then(|mark| mark.reviewed);
            let due = reviewed_since.filter(|at| updated > *at && reviewed.as_deref() != changed);
            if let Some(reviewed_since) = due {
                let path = reviews_path(&name, number);
                match self.shared.source.read(&self.repo, &path).await {
                    Ok(list) => {
                        lookups.worked = true;
                        let submitted = list
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|review| review_submitted(review, pull, repository));
                        found.extend(submitted.map(|event| (event, reviewed_since)));
                        reviewed = changed.map(str::to_owned);
                    }
                    Err(err) => {
                        warn!(repo = %self.repo, path, "couldn't read from GitHub: {err}");
                        lookups.failed = Some(format!("{NO_REVIEWS}: {err}"));
                        whole = false;
                    }
                }
            }
            let mark = PullMark {
                sha: sha.to_owned(),
                reviewed,
                updated,
            };
            cursor.pulls.insert(number, mark);
        }
        whole
    }

    /// Forgets where the listings the tasks no longer need got to, so one
    /// needed again starts afresh.
    fn forget_unneeded(&self, cursor: &mut Cursor, needed: &[Listing], watched: &[&str]) {
        let name = self.repo.to_string();
        let reviews = needed.contains(&Listing::Pulls) && watched.contains(&"pull_request_review");
        cursor.since.retain(|key, _| {
            needed.iter().any(|listing| listing.key() == key) || (reviews && key == REVIEWS)
        });
        cursor
            .etags
            .retain(|path, _| needed.iter().any(|listing| listing.path(&name) == *path));
        if !needed.contains(&Listing::Pulls) {
            cursor.pulls.clear();
        }
    }

    /// Hands on each event in `found` that's new and a task takes, oldest
    /// first, and notes it in `cursor`.
    async fn hand_new(&self, found: Vec<Found>, cursor: &mut Cursor, lookups: &mut Lookups) {
        let tasks = self.shared.tasks.borrow().clone();
        let mut new: Vec<(u64, String, Value)> = found
            .into_iter()
            .filter_map(|(event, since)| {
                let id = event_id(&event)?;
                let at = happened(&event)?.0;
                let new = at > since && !cursor.seen.contains_key(&id);
                (new && self.wanted(&tasks, &event)).then_some((at, id, event))
            })
            .collect();
        new.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        new.dedup_by(|a, b| a.1 == b.1);
        for (at, id, event) in new {
            self.hand(&event, Heard::Polled, lookups).await;
            cursor.seen.insert(id, at);
        }
    }

    /// Whether one of `tasks` takes `event`.
    fn wanted(&self, tasks: &[Task], event: &Value) -> bool {
        let kind = event["type"].as_str().unwrap_or_default();
        webhook_name(kind)
            .is_some_and(|name| tasks.iter().any(|task| takes(task, &self.repo, name, kind)))
    }

    /// Why the tasks miss the events in `watched` that only a webhook
    /// brings from a private repository, when it has no webhook of this
    /// device's.
    fn unheard(&self, watched: &[&str], repository: &Value) -> Option<String> {
        let unheard = uncovered(watched);
        let private = repository["private"].as_bool() == Some(true);
        let hooked = self.shared.hooks.borrow().contains_key(&self.key);
        (private && !hooked && !unheard.is_empty()).then(|| {
            format!(
                "GitHub only sends {} from a private repo through a webhook, and this device has none there",
                unheard.join(", ")
            )
        })
    }

    /// Hands `event`, heard from `heard`, to each task that takes it,
    /// unless the other source's copy went first. An event nothing takes
    /// isn't noted, so it never pairs.
    async fn hand(&self, event: &Value, heard: Heard, lookups: &mut Lookups) {
        let Some(hooked) = hooked(event) else {
            return;
        };
        let kind = event["type"].as_str().unwrap_or_default();
        let taken = self
            .shared
            .tasks
            .borrow()
            .iter()
            .any(|task| takes(task, &self.repo, hooked.event, kind));
        if !taken {
            return;
        }
        if !self.first(event, &hooked, heard) {
            debug!(repo = %self.repo, event = hooked.id, ?heard, "the other source handed this GitHub event");
            return;
        }
        self.give(hooked, kind, lookups).await;
    }

    /// Hands `hooked`, events API type `kind`, to each task that takes it,
    /// with the parts that came slim read in full. A part that can't be
    /// read stays slim.
    async fn give(&self, mut hooked: Hooked, kind: &str, lookups: &mut Lookups) {
        let tasks = self.shared.tasks.borrow().clone();
        let takers: Vec<&Task> = tasks
            .iter()
            .filter(|task| takes(task, &self.repo, hooked.event, kind))
            .collect();
        if takers.is_empty() {
            return;
        }
        for part in slim_parts(&hooked, &self.repo.to_string()) {
            if let Some(full) = self.look_up(lookups, &part.path, SHORT_FORM).await {
                hooked.body[part.key] = Value::clone(&full);
            }
        }
        for task in takers {
            let request = request(&hooked, self.shared.clock.wall());
            let outcome = self.shared.deliver.deliver(task, request).await;
            debug!(task = %task.id, event = hooked.id, ?outcome, "handed on a GitHub event");
        }
    }

    /// Whether `hooked`, from `event` and heard from `heard`, runs, which
    /// it doesn't when it's the other source's copy of one that ran. A
    /// copy a poll reads while the repository has no webhook of this
    /// device's isn't kept to pair, as no other copy comes.
    fn first(&self, event: &Value, hooked: &Hooked, heard: Heard) -> bool {
        let Some(key) = shared_key(hooked) else {
            return true;
        };
        let db = &self.shared.db;
        let waits = heard == Heard::Hooked || matches!(db.github_hook(&self.key), Ok(Some(_)));
        let now = self.shared.clock.wall();
        let at = happened(event).unwrap_or(now);
        db.pair_github(&self.key, &key, heard, at, now, waits)
            .unwrap_or_else(|err| {
                warn!(repo = %self.repo, "couldn't note a GitHub event: {err}");
                true
            })
    }

    /// `path`, read once a poll. A failure keeps `why` with its reason.
    async fn look_up(&self, lookups: &mut Lookups, path: &str, why: &str) -> Option<Arc<Value>> {
        if let Some(read) = lookups.read.get(path) {
            return read.clone();
        }
        let read = match self.shared.source.read(&self.repo, path).await {
            Ok(value) => {
                lookups.worked = true;
                Some(Arc::new(value))
            }
            Err(err) => {
                warn!(repo = %self.repo, path, "couldn't read from GitHub: {err}");
                lookups.failed = Some(format!("{why}: {err}"));
                None
            }
        };
        lookups.read.insert(path.to_owned(), read.clone());
        read
    }

    /// Keeps why this poll's reads failed where the tasks show it, until a
    /// later read works.
    fn looked_up(&self, lookups: Lookups) {
        if lookups.failed.is_some() || lookups.worked {
            *lock(&self.lookup) = lookups.failed;
            self.show();
        }
    }

    /// Shows why the last poll fell short on the repository's tasks, or
    /// else why its last read failed, or nothing.
    fn show(&self) {
        let problem = lock(&self.polled)
            .clone()
            .or_else(|| lock(&self.lookup).clone());
        self.shared
            .problems
            .send_if_modified(|problems| match problem {
                Some(problem) => {
                    problems.insert(self.key.clone(), problem.clone()) != Some(problem)
                }
                None => problems.remove(&self.key).is_some(),
            });
    }
}

/// Where [`Poller::pulls`] reads into.
struct Read<'a> {
    /// The repository, for the events it makes.
    repository: &'a Value,
    /// Whether to read the reviews of each pull request that changed.
    reviews: bool,
    /// The events it found.
    found: &'a mut Vec<Found>,
    /// What it read, and whether that worked.
    lookups: &'a mut Lookups,
}

/// `hooked` as the request GitHub's webhook would have sent. Its ID is the
/// events API's, or a delivery ID the doorbell checked is a UUID.
fn request(hooked: &Hooked, received: Timestamp) -> Request {
    let headers = [
        ("content-type", "application/json"),
        ("x-github-event", hooked.event),
        ("x-github-delivery", &hooked.id),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    Request {
        delivery: hooked.id.clone(),
        received,
        headers,
        query: String::new(),
        body: serde_json::to_vec(&hooked.body).unwrap_or_default(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

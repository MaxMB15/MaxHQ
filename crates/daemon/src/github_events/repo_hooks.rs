// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeps this device's webhook on each repository its enabled GitHub tasks
//! watch, while the device has a public address: made when a task needs
//! it, its events kept in step with the tasks, and deleted when no task
//! needs it or the address goes back to local. Each hook's URL is the
//! device's path and a token only this device's hooks know, so after a
//! restart a hook is found again by its ID, or by that whole URL, and
//! hooks it didn't make are never touched. A new hook stays off until its
//! secret and ID are kept, so no delivery comes before maxd can check it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_core::github_events::hook_events;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::tasks::{HookAddress, Task, Trigger};
use maxhq_protocol::Secret;
use tokio::sync::watch;
use tracing::{debug, warn};

use super::{HookProblem, HookState, HookStates};
use crate::db::Db;
use crate::hooks::{Reach, Webhooks};
use crate::secrets::{GithubError, RepoName};
use crate::session::clock::Clock;

/// How often each webhook is checked, and pinged when it hasn't delivered
/// since, so one that works keeps polls at the backstop's pace.
pub const RECHECK: Duration = Duration::from_secs(30 * 60);
/// How soon a check that failed is tried again.
pub const RETRY: Duration = Duration::from_secs(5 * 60);

/// What a task shows when its repository's webhook can't be managed.
pub const NO_PERMISSION: &str = "The GitHub App can't manage this repo's webhooks, so this task \
     checks every minute. On the App's settings page, set Repository permissions, Webhooks to \
     Read and write, save, then accept the change where the App is installed.";

/// The webhook [`RepoHooks::keep`] found or made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeptHook {
    /// GitHub's ID for it.
    pub id: u64,
    /// Whether it was made or changed.
    pub changed: bool,
    /// Whether it's on. One with a new secret is off until
    /// [`RepoHooks::activate`] turns it on.
    pub active: bool,
}

/// This device's webhooks on GitHub, managed through its GitHub App.
pub trait RepoHooks: Send + Sync {
    /// Makes this device's webhook on `repo`, `known` or the one already
    /// at `url`, send `events` to `url`, signed with a secret it keeps.
    /// One given a new secret is left off.
    fn keep<'a>(
        &'a self,
        repo: &'a RepoName,
        known: Option<u64>,
        url: &'a str,
        events: &'a [&'a str],
    ) -> BoxFuture<'a, Result<KeptHook, GithubError>>;

    /// Turns `hook` on.
    fn activate<'a>(
        &'a self,
        repo: &'a RepoName,
        hook: u64,
    ) -> BoxFuture<'a, Result<(), GithubError>>;

    /// Asks GitHub to send `hook` a ping.
    fn ping<'a>(&'a self, repo: &'a RepoName, hook: u64) -> BoxFuture<'a, Result<(), GithubError>>;

    /// Deletes `hook` from `repo`, and forgets its secret. On a repository
    /// the App no longer reaches, the hook may still be there, so its
    /// secret stays and this answers [`GithubError::NotInstalled`].
    fn remove<'a>(
        &'a self,
        repo: &'a RepoName,
        hook: u64,
    ) -> BoxFuture<'a, Result<(), GithubError>>;

    /// The secret `hook`'s deliveries are signed with.
    fn secret(&self, hook: u64) -> Option<Secret>;

    /// Where the App's permissions are set, once it's known.
    fn permissions_page(&self) -> Option<String>;

    /// The token that follows the device's path in its webhooks' URLs,
    /// made the first time. None when it can't be kept.
    fn doorbell_token(&self) -> Option<Secret>;
}

/// How GitHub reaches this device's doorbell.
pub trait Doorway: Send + Sync {
    /// The device's address, as it changes.
    fn reach(&self) -> watch::Receiver<Reach>;

    /// Opens `path` at that address, and answers its URL.
    fn open<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, String>>;

    /// Takes `path` off it.
    fn close<'a>(&'a self, path: &'a str) -> BoxFuture<'a, ()>;
}

impl Doorway for Webhooks {
    fn reach(&self) -> watch::Receiver<Reach> {
        Webhooks::reach(self)
    }

    fn open<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(self.open_path(path))
    }

    fn close<'a>(&'a self, path: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.close_path(path))
    }
}

/// The path GitHub's webhooks for `device` ring, which the doorbell's
/// token follows.
pub fn path(device: DeviceId) -> String {
    format!("/maxhq/github/{device}")
}

/// What keeps the webhooks.
pub struct Keeper {
    /// The device's doorbell path, which the Funnel opens.
    pub path: String,
    /// The doorbell's token, which follows the path in each hook's URL.
    pub token: Secret,
    pub tasks: watch::Receiver<Arc<[Task]>>,
    pub door: Arc<dyn Doorway>,
    pub github: Arc<dyn RepoHooks>,
    pub db: Db,
    /// How each repository's webhook stands, which this keeps an entry in
    /// for each one wanted.
    pub states: watch::Sender<HookStates>,
    pub clock: Arc<dyn Clock>,
}

/// The webhooks this device should have.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    address: HookAddress,
    /// Each repository's name and the events its hook sends, by
    /// [`RepoName::key`].
    repos: BTreeMap<String, (String, Vec<&'static str>)>,
}

impl Keeper {
    /// Keeps the webhooks in step with the tasks and the address, and
    /// checks them again now and then, until the tasks stop sending.
    pub async fn run(mut self) {
        let mut reach = self.door.reach();
        let mut done: Option<Wanted> = None;
        let mut due = self.clock.now();
        loop {
            let address = reach.borrow_and_update().address.clone();
            let wanted = wanted(&self.tasks.borrow_and_update(), address);
            if done.as_ref() != Some(&wanted) || self.clock.now() >= due {
                let worked = self.reconcile(&wanted).await;
                due = self.clock.now() + if worked { RECHECK } else { RETRY };
                done = Some(wanted);
            }
            tokio::select! {
                changed = self.tasks.changed() => if changed.is_err() { return },
                changed = reach.changed() => if changed.is_err() { return },
                () = self.clock.sleep_until(due) => {}
            }
        }
    }

    /// Makes GitHub's webhooks what `wanted` says, and says whether all of
    /// it worked.
    async fn reconcile(&self, wanted: &Wanted) -> bool {
        let mut worked = true;
        let made = self.db.github_hook_repos().unwrap_or_else(|err| {
            warn!("couldn't list this device's GitHub webhooks: {err}");
            Vec::new()
        });
        for key in made.iter().filter(|key| !wanted.repos.contains_key(*key)) {
            worked &= self.remove(key).await;
        }
        self.states.send_if_modified(|states| {
            if states.keys().eq(wanted.repos.keys()) {
                return false;
            }
            states.retain(|key, _| wanted.repos.contains_key(key));
            for key in wanted.repos.keys() {
                states.entry(key.clone()).or_insert_with(HookState::default);
            }
            true
        });
        if wanted.repos.is_empty() {
            self.door.close(&self.path).await;
            return worked;
        }
        let url = match self.door.open(&self.path).await {
            Ok(base) => format!("{base}/{}", self.token.expose()),
            Err(err) => {
                warn!("couldn't open the path GitHub's webhooks ring: {err}");
                let problem = HookProblem {
                    reason: format!(
                        "Couldn't open the address GitHub sends to, so this task checks every \
                         minute: {err}"
                    ),
                    link: None,
                };
                for key in wanted.repos.keys() {
                    self.show(key, Some(problem.clone()));
                }
                return false;
            }
        };
        for (key, (name, events)) in &wanted.repos {
            worked &= self.keep(key, name, &url, events).await;
        }
        worked
    }

    /// Deletes the webhook on the repository `key` names, and says whether
    /// that worked or there's nothing to do for now. A hook on a
    /// repository the App no longer reaches stays noted, with its secret,
    /// and is tried again at each check.
    async fn remove(&self, key: &str) -> bool {
        let hook = self.db.github_hook(key).ok().flatten();
        let (Some(repo), Some(hook)) = (RepoName::parse(key), hook) else {
            return self.forget(key);
        };
        match self.github.remove(&repo, hook).await {
            Ok(()) => self.forget(key),
            Err(GithubError::NotInstalled(_)) => {
                debug!(
                    repo = key,
                    "a GitHub webhook's repository is out of the App's reach"
                );
                true
            }
            Err(err) => {
                warn!(repo = key, "couldn't delete a GitHub webhook: {err}");
                false
            }
        }
    }

    fn forget(&self, key: &str) -> bool {
        self.db
            .remove_github_hook(key)
            .map_err(|err| {
                warn!(repo = key, "couldn't forget a GitHub webhook: {err}");
            })
            .is_ok()
    }

    /// Keeps the webhook on repository `name`, notes it, turns it on when
    /// it's off, then pings it when it changed or hasn't delivered lately.
    /// Says whether that worked.
    async fn keep(&self, key: &str, name: &str, url: &str, events: &[&'static str]) -> bool {
        let Some(repo) = RepoName::parse(name) else {
            return true;
        };
        let known = self.db.github_hook(key).unwrap_or_else(|err| {
            warn!(repo = key, "couldn't read a GitHub webhook: {err}");
            None
        });
        let kept = match self.github.keep(&repo, known, url, events).await {
            Ok(kept) => kept,
            Err(err) => {
                warn!(repo = key, "couldn't set up a GitHub webhook: {err}");
                self.show(key, problem(&err, self.github.permissions_page()));
                return false;
            }
        };
        if known != Some(kept.id)
            && let Err(err) = self.db.put_github_hook(key, kept.id)
        {
            warn!(repo = key, "couldn't note a GitHub webhook: {err}");
            return false;
        }
        if !kept.active
            && let Err(err) = self.github.activate(&repo, kept.id).await
        {
            warn!(repo = key, "couldn't turn on a GitHub webhook: {err}");
            self.show(key, problem(&err, self.github.permissions_page()));
            return false;
        }
        self.show(key, None);
        let now = self.clock.wall().0;
        let delivered = self
            .states
            .borrow()
            .get(key)
            .and_then(|state| state.delivered);
        let quiet = delivered.is_none_or(|at| now.saturating_sub(at.0) >= millis(RECHECK));
        if (kept.changed || quiet)
            && let Err(err) = self.github.ping(&repo, kept.id).await
        {
            warn!(repo = key, "couldn't ping a GitHub webhook: {err}");
        }
        true
    }

    /// Shows `problem` on the repository `key` names.
    fn show(&self, key: &str, problem: Option<HookProblem>) {
        self.states.send_if_modified(|states| {
            let Some(state) = states.get_mut(key) else {
                return false;
            };
            let changed = state.problem != problem;
            state.problem = problem;
            changed
        });
    }
}

/// The webhooks `tasks` need at `address`: none unless it's public.
fn wanted(tasks: &[Task], address: HookAddress) -> Wanted {
    let mut listed: BTreeMap<String, (String, Vec<&str>)> = BTreeMap::new();
    if address != HookAddress::Local {
        for task in tasks.iter().filter(|task| task.enabled) {
            let Trigger::GithubEvents { repo, events, .. } = &task.trigger else {
                continue;
            };
            let Some(name) = RepoName::parse(repo) else {
                continue;
            };
            listed
                .entry(name.key())
                .or_insert_with(|| (name.to_string(), Vec::new()))
                .1
                .extend(events.iter().map(String::as_str));
        }
    }
    let repos = listed
        .into_iter()
        .map(|(key, (name, events))| (key, (name, hook_events(events))))
        .filter(|(_, (_, events))| !events.is_empty())
        .collect();
    Wanted { address, repos }
}

/// What a task shows when its repository's webhook can't be set up, if
/// anything: the App's own problems show through polling already. A 404
/// only comes once a token was minted for the repository, so the App is
/// installed there and may not see its webhooks. Anything else, such as
/// a 422 for a URL GitHub won't take, shows GitHub's own words.
fn problem(err: &GithubError, page: Option<String>) -> Option<HookProblem> {
    match err {
        GithubError::NoApp | GithubError::NotInstalled(_) => None,
        GithubError::HooksNotGranted(_)
        | GithubError::Refused {
            status: 403 | 404, ..
        } => Some(HookProblem {
            reason: NO_PERMISSION.to_owned(),
            link: page,
        }),
        err => Some(HookProblem {
            reason: format!("Couldn't set up the webhook, so this task checks every minute: {err}"),
            link: None,
        }),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A [`RepoHooks`] and [`Doorway`] for tests, in memory.
#[cfg(test)]
pub mod fake {
    use std::sync::Mutex;

    use super::*;

    /// A hook as the fake holds it: its ID, URL and events.
    pub type Hook = (u64, String, Vec<String>);

    /// The doorbell token the fake keeps.
    pub const TOKEN: &str = "doorbell-token";

    /// What the fake GitHub holds and was asked.
    #[derive(Default)]
    pub struct Github {
        /// Each repository's hook.
        pub hooks: Mutex<BTreeMap<String, Hook>>,
        /// The hooks that are on.
        pub active: Mutex<std::collections::BTreeSet<u64>>,
        pub pings: Mutex<Vec<u64>>,
        pub secrets: Mutex<BTreeMap<u64, String>>,
        /// What every call answers instead, while it's set.
        pub refuse: Mutex<Option<GithubError>>,
        /// Whether turning a hook on fails.
        pub stuck_off: Mutex<bool>,
        next: Mutex<u64>,
    }

    impl Github {
        fn refused(&self) -> Result<(), GithubError> {
            match &*self.refuse.lock().unwrap() {
                Some(GithubError::Refused { status, message }) => Err(GithubError::Refused {
                    status: *status,
                    message: message.clone(),
                }),
                Some(GithubError::NotInstalled(repo)) => {
                    Err(GithubError::NotInstalled(repo.clone()))
                }
                Some(_) => Err(GithubError::BadReply),
                None => Ok(()),
            }
        }
    }

    impl RepoHooks for Github {
        fn keep<'a>(
            &'a self,
            repo: &'a RepoName,
            known: Option<u64>,
            url: &'a str,
            events: &'a [&'a str],
        ) -> BoxFuture<'a, Result<KeptHook, GithubError>> {
            let kept = self.refused().map(|()| {
                let mut hooks = self.hooks.lock().unwrap();
                let wanted = (
                    url.to_owned(),
                    events.iter().map(|e| (*e).to_owned()).collect(),
                );
                match hooks.get_mut(&repo.key()) {
                    Some((id, at, listed)) if known == Some(*id) || *at == wanted.0 => {
                        let changed = (at.clone(), listed.clone()) != wanted;
                        (*at, *listed) = wanted;
                        self.active.lock().unwrap().insert(*id);
                        KeptHook {
                            id: *id,
                            changed,
                            active: true,
                        }
                    }
                    _ => {
                        let mut next = self.next.lock().unwrap();
                        *next += 1;
                        hooks.insert(repo.key(), (*next, wanted.0, wanted.1));
                        self.secrets
                            .lock()
                            .unwrap()
                            .insert(*next, format!("secret-{next}"));
                        KeptHook {
                            id: *next,
                            changed: true,
                            active: false,
                        }
                    }
                }
            });
            Box::pin(std::future::ready(kept))
        }

        fn activate<'a>(
            &'a self,
            _: &'a RepoName,
            hook: u64,
        ) -> BoxFuture<'a, Result<(), GithubError>> {
            let activated = match *self.stuck_off.lock().unwrap() {
                true => Err(GithubError::Unreachable("timed out".into())),
                false => self.refused().map(|()| {
                    self.active.lock().unwrap().insert(hook);
                }),
            };
            Box::pin(std::future::ready(activated))
        }

        fn ping<'a>(
            &'a self,
            _: &'a RepoName,
            hook: u64,
        ) -> BoxFuture<'a, Result<(), GithubError>> {
            let pinged = self
                .refused()
                .map(|()| self.pings.lock().unwrap().push(hook));
            Box::pin(std::future::ready(pinged))
        }

        fn remove<'a>(
            &'a self,
            repo: &'a RepoName,
            hook: u64,
        ) -> BoxFuture<'a, Result<(), GithubError>> {
            let removed = self.refused().map(|()| {
                let mut hooks = self.hooks.lock().unwrap();
                if hooks.get(&repo.key()).is_some_and(|(id, ..)| *id == hook) {
                    hooks.remove(&repo.key());
                }
                self.secrets.lock().unwrap().remove(&hook);
            });
            Box::pin(std::future::ready(removed))
        }

        fn secret(&self, hook: u64) -> Option<Secret> {
            self.secrets
                .lock()
                .unwrap()
                .get(&hook)
                .cloned()
                .map(Secret::new)
        }

        fn permissions_page(&self) -> Option<String> {
            Some("https://github.com/settings/apps/maxhq-test/permissions".into())
        }

        fn doorbell_token(&self) -> Option<Secret> {
            Some(Secret::new(TOKEN.to_owned()))
        }
    }

    /// A doorway whose address the test sets, opening paths under
    /// `https://devbox.example.ts.net` unless told it can't.
    pub struct Door {
        pub reach: watch::Sender<Reach>,
        pub open: Mutex<std::collections::BTreeSet<String>>,
        pub fail: Mutex<Option<String>>,
    }

    impl Default for Door {
        fn default() -> Self {
            Self {
                reach: watch::Sender::new(Reach::default()),
                open: Mutex::default(),
                fail: Mutex::default(),
            }
        }
    }

    impl Doorway for Door {
        fn reach(&self) -> watch::Receiver<Reach> {
            self.reach.subscribe()
        }

        fn open<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, String>> {
            let opened = match self.fail.lock().unwrap().clone() {
                Some(err) => Err(err),
                None => {
                    self.open.lock().unwrap().insert(path.to_owned());
                    Ok(format!("https://devbox.example.ts.net{path}"))
                }
            };
            Box::pin(std::future::ready(opened))
        }

        fn close<'a>(&'a self, path: &'a str) -> BoxFuture<'a, ()> {
            self.open.lock().unwrap().remove(path);
            Box::pin(std::future::ready(()))
        }
    }
}

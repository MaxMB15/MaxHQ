// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The keeper of this device's repository webhooks, against a fake GitHub
//! and a fake address.

use std::collections::BTreeMap;

use maxhq_protocol::tasks::HookAddress;
use maxhq_protocol::Secret;
use tokio::task::JoinHandle;

use super::*;
use crate::github_events::repo_hooks::fake::{self, Hook};
use crate::github_events::repo_hooks::{Keeper, RepoHooks, NO_PERMISSION, RECHECK, RETRY};
use crate::tasks::Tasks;

const PATH: &str = "/maxhq/github/devbox";
const URL: &str = "https://devbox.example.ts.net/maxhq/github/devbox/doorbell-token";
const PERMISSIONS: &str = "https://github.com/settings/apps/maxhq-test/permissions";

/// A keeper on its own, with what it reads and changes.
struct Keep {
    tasks: watch::Sender<Arc<[Task]>>,
    door: Arc<fake::Door>,
    github: Arc<fake::Github>,
    db: Db,
    states: watch::Sender<HookStates>,
    clock: Arc<FakeClock>,
    running: Option<JoinHandle<()>>,
}

impl Keep {
    fn new(tasks: Vec<Task>) -> Self {
        let mut keep = Self {
            tasks: watch::Sender::new(Arc::from(tasks)),
            door: Arc::default(),
            github: Arc::default(),
            db: Db::open_in_memory().unwrap(),
            states: watch::Sender::new(HookStates::new()),
            clock: Arc::new(FakeClock::new()),
            running: None,
        };
        keep.start();
        keep
    }

    /// Starts a keeper, as maxd does when it starts.
    fn start(&mut self) {
        let keeper = Keeper {
            path: PATH.into(),
            token: Secret::new(fake::TOKEN.to_owned()),
            tasks: self.tasks.subscribe(),
            door: self.door.clone(),
            github: self.github.clone(),
            db: self.db.clone(),
            states: self.states.clone(),
            clock: self.clock.clone(),
        };
        self.running = Some(tokio::spawn(keeper.run()));
    }

    /// Stops the keeper and starts another, which knows only what the
    /// database kept, as when maxd restarts.
    async fn restart(&mut self) {
        if let Some(running) = self.running.take() {
            running.abort();
            let _ = running.await;
        }
        self.states.send_replace(HookStates::new());
        self.start();
    }

    fn set(&self, tasks: Vec<Task>) {
        self.tasks.send_replace(Arc::from(tasks));
    }

    fn address(&self, address: HookAddress) {
        self.door.reach.send_modify(|reach| reach.address = address);
    }

    fn hooks(&self) -> BTreeMap<String, Hook> {
        self.github.hooks.lock().unwrap().clone()
    }

    fn pings(&self) -> Vec<u64> {
        self.github.pings.lock().unwrap().clone()
    }

    fn active(&self) -> Vec<u64> {
        self.github.active.lock().unwrap().iter().copied().collect()
    }

    fn open(&self) -> bool {
        self.door.open.lock().unwrap().contains(PATH)
    }

    fn state(&self) -> Option<HookState> {
        self.states.borrow().get(REPO).cloned()
    }

    async fn tick(&self, by: Duration) {
        self.clock.advance(by);
        settle().await;
    }
}

/// Gives what a change started the chance to run, so a test would see it.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

fn hook(id: u64, events: &[&str]) -> Hook {
    let events = events.iter().map(|event| (*event).to_owned()).collect();
    (id, URL.to_owned(), events)
}

fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

#[tokio::test]
async fn keeps_one_hook_per_repository_in_step_with_the_tasks_and_the_address() {
    let pulls = task("pulls", REPO, &["pull_request"]);
    let mut keep = Keep::new(vec![pulls.clone()]);
    // A local address can't take deliveries, so no hook is made.
    settle().await;
    assert!(keep.hooks().is_empty());
    assert!(!keep.open());
    assert_eq!(keep.state(), None);

    keep.address(HookAddress::Funnel);
    eventually(|| !keep.pings().is_empty()).await;
    assert_eq!(keep.hooks()[REPO], hook(1, &["pull_request"]));
    assert_eq!(keep.db.github_hook(REPO).unwrap(), Some(1));
    assert_eq!(keep.active(), [1]);
    assert_eq!(keep.pings(), [1]);
    assert!(keep.open());
    assert_eq!(keep.state(), Some(HookState::default()));

    // Another task on the repository adds its events to the same hook.
    keep.set(vec![
        pulls.clone(),
        task("issues", "Owner/Repo", &["issues"]),
    ]);
    eventually(|| keep.pings().len() == 2).await;
    assert_eq!(keep.hooks()[REPO], hook(1, &["issues", "pull_request"]));

    // After a restart, the keeper finds the same hook and pings it, since
    // it hasn't heard from it yet.
    keep.restart().await;
    eventually(|| keep.pings().len() == 3).await;
    assert_eq!(keep.hooks()[REPO], hook(1, &["issues", "pull_request"]));
    assert_eq!(keep.pings(), [1, 1, 1]);

    // Back to a local address, the hook goes.
    keep.address(HookAddress::Local);
    eventually(|| !keep.open()).await;
    assert!(keep.hooks().is_empty());
    assert_eq!(keep.db.github_hook(REPO).unwrap(), None);
    assert_eq!(keep.state(), None);

    // At an address of its own a hook is made again, and goes once no
    // enabled task needs it.
    keep.address(HookAddress::Own {
        base: "https://hooks.example.com".into(),
    });
    eventually(|| keep.hooks().len() == 1).await;
    assert_eq!(keep.hooks()[REPO], hook(2, &["issues", "pull_request"]));
    let mut off = pulls;
    off.enabled = false;
    keep.set(vec![off]);
    eventually(|| !keep.open()).await;
    assert!(keep.hooks().is_empty());
    assert_eq!(keep.db.github_hook(REPO).unwrap(), None);
    assert_eq!(keep.state(), None);
}

#[tokio::test]
async fn a_recheck_pings_a_hook_only_once_it_goes_quiet() {
    let keep = Keep::new(vec![task("pulls", REPO, &["pull_request"])]);
    keep.address(HookAddress::Funnel);
    eventually(|| keep.pings() == [1]).await;

    // It delivers twenty minutes on, so the check at thirty leaves it be.
    keep.tick(minutes(20)).await;
    let now = keep.clock.wall();
    keep.states
        .send_modify(|states| states.get_mut(REPO).unwrap().delivered = Some(now));
    keep.tick(RECHECK - minutes(20)).await;
    assert_eq!(keep.pings(), [1]);

    // By the next check it has been quiet that long, so it gets a ping.
    keep.tick(RECHECK).await;
    eventually(|| keep.pings() == [1, 1]).await;
}

#[tokio::test]
async fn a_hook_the_app_may_not_manage_shows_why_while_polls_keep_their_minute() {
    let rig = started(vec![task("pulls", REPO, &["pull_request"])]);
    let door = Arc::new(fake::Door::default());
    door.reach
        .send_modify(|reach| reach.address = HookAddress::Funnel);
    let github = Arc::new(fake::Github::default());
    *github.refuse.lock().unwrap() = Some(GithubError::Refused {
        status: 403,
        message: "Resource not accessible by integration".into(),
    });
    let keeper = Keeper {
        path: PATH.into(),
        token: Secret::new(fake::TOKEN.to_owned()),
        tasks: rig.tasks.watch(),
        door,
        github: github.clone(),
        db: rig.db.clone(),
        states: rig.hooks.clone(),
        clock: rig.clock.clone(),
    };
    tokio::spawn(keeper.run());

    let problem = || {
        rig.hooks
            .borrow()
            .get(REPO)
            .and_then(|state| state.problem.clone())
    };
    let refused = HookProblem {
        reason: NO_PERMISSION.to_owned(),
        link: Some(PERMISSIONS.to_owned()),
    };
    eventually(|| problem() == Some(refused.clone())).await;
    // The poller and the keeper each sleep.
    // Its pull requests and their issue events.
    rig.polled(2, 2).await;
    for n in 2..=4 {
        rig.tick(60).await;
        rig.polled(2 * n, 2).await;
    }
    assert!(github.hooks.lock().unwrap().is_empty());
    assert!(!instant(&rig));

    // Once the App may, the next try sets the hook up and the problem goes.
    *github.refuse.lock().unwrap() = None;
    rig.clock.advance(RETRY - minutes(3));
    eventually(|| github.pings.lock().unwrap().len() == 1).await;
    assert_eq!(problem(), None);
}

#[tokio::test]
async fn an_address_that_wont_open_shows_why() {
    let keep = Keep::new(vec![task("pulls", REPO, &["pull_request"])]);
    *keep.door.fail.lock().unwrap() = Some("Funnel is off on this device".into());
    keep.address(HookAddress::Funnel);
    eventually(|| keep.state().is_some_and(|state| state.problem.is_some())).await;
    let problem = keep.state().unwrap().problem.unwrap();
    assert!(
        problem
            .reason
            .ends_with("checks every minute: Funnel is off on this device"),
        "{}",
        problem.reason
    );
    assert_eq!(problem.link, None);
    assert!(keep.hooks().is_empty());

    // It tries again a little later.
    *keep.door.fail.lock().unwrap() = None;
    keep.tick(RETRY).await;
    eventually(|| keep.pings() == [1]).await;
    assert_eq!(keep.state().unwrap().problem, None);
}

#[tokio::test]
async fn a_hook_that_wont_turn_on_is_noted_and_tried_again() {
    let keep = Keep::new(vec![task("pulls", REPO, &["pull_request"])]);
    *keep.github.stuck_off.lock().unwrap() = true;
    keep.address(HookAddress::Funnel);
    eventually(|| keep.state().is_some_and(|state| state.problem.is_some())).await;
    // It's noted before it's turned on, so a delivery is never from a hook
    // maxd doesn't know.
    assert_eq!(keep.db.github_hook(REPO).unwrap(), Some(1));
    assert!(keep.active().is_empty());
    assert!(keep.pings().is_empty());
    let problem = keep.state().unwrap().problem.unwrap();
    assert!(problem.reason.ends_with("timed out"), "{}", problem.reason);

    *keep.github.stuck_off.lock().unwrap() = false;
    keep.tick(RETRY).await;
    eventually(|| keep.pings() == [1]).await;
    assert_eq!(keep.active(), [1]);
    assert_eq!(keep.state().unwrap().problem, None);
}

#[tokio::test]
async fn a_url_github_wont_take_shows_githubs_words() {
    let keep = Keep::new(vec![task("pulls", REPO, &["pull_request"])]);
    *keep.github.refuse.lock().unwrap() = Some(GithubError::Refused {
        status: 422,
        message: "Validation Failed".into(),
    });
    keep.address(HookAddress::Funnel);
    eventually(|| keep.state().is_some_and(|state| state.problem.is_some())).await;
    let problem = keep.state().unwrap().problem.unwrap();
    assert_eq!(
        problem.reason,
        "Couldn't set up the webhook, so this task checks every minute: GitHub refused with \
         422: Validation Failed"
    );
    assert_eq!(problem.link, None);
}

#[tokio::test]
async fn a_hook_out_of_the_apps_reach_stays_noted_until_it_can_go() {
    let keep = Keep::new(vec![task("pulls", REPO, &["pull_request"])]);
    keep.address(HookAddress::Funnel);
    eventually(|| keep.pings() == [1]).await;

    // The App lost the repository, so its hook can't be deleted yet. It
    // stays noted, with its secret, and is tried at each check.
    *keep.github.refuse.lock().unwrap() = Some(GithubError::NotInstalled(REPO.into()));
    keep.set(Vec::new());
    eventually(|| !keep.open()).await;
    assert_eq!(keep.db.github_hook(REPO).unwrap(), Some(1));
    assert!(keep.github.secret(1).is_some());
    assert_eq!(keep.hooks().len(), 1);

    *keep.github.refuse.lock().unwrap() = None;
    keep.tick(RETRY).await;
    assert_eq!(keep.db.github_hook(REPO).unwrap(), Some(1));
    keep.tick(RECHECK - RETRY).await;
    eventually(|| keep.hooks().is_empty()).await;
    assert_eq!(keep.db.github_hook(REPO).unwrap(), None);
    assert!(keep.github.secret(1).is_none());
}

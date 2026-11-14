// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The pollers against a fake GitHub, a fake clock and fake tasks.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_core::github_activity::{reviews_path, Listing};
use maxhq_protocol::ids::{AgentId, DeviceId, PermissionId, TaskId};
use maxhq_protocol::tasks::{HookRules, Maker, Task, TaskTarget, Trigger};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::cursors::Cursor;
use super::pollers::{start, Pollers, Problems, INSTALL_APP, NO_REVIEWS, SHORT_FORM};
use super::*;
use crate::db::Db;
use crate::hooks::FakeDeliver;
use crate::session::clock::{Clock, FakeClock};
use crate::tasks::FakeTasks;

mod doorbell;
mod keeper;

/// A fake GitHub. Each listing gives its answers in turn, then nothing
/// new, and each path reads as `pages` holds.
#[derive(Default)]
struct Source {
    /// Each listing's answers, by its path in lower case.
    answers: Mutex<HashMap<String, VecDeque<Result<Page, GithubError>>>>,
    /// Each listing asked for, with its ETag, oldest first.
    asked: watch::Sender<Vec<(String, Option<String>)>>,
    /// What each path reads as. Any other is GitHub's 404.
    pages: Mutex<HashMap<String, Result<Value, GithubError>>>,
    /// Each path read, oldest first.
    reads: Mutex<Vec<String>>,
}

impl EventSource for Source {
    fn list<'a>(
        &'a self,
        _: &'a RepoName,
        path: &'a str,
        etag: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Page, GithubError>> {
        self.asked
            .send_modify(|asked| asked.push((path.to_owned(), etag.map(str::to_owned))));
        let mut answers = self.answers.lock().unwrap();
        let answer = answers
            .get_mut(&path.to_ascii_lowercase())
            .and_then(VecDeque::pop_front);
        let answer = answer.unwrap_or_else(|| Ok(unchanged()));
        Box::pin(std::future::ready(answer))
    }

    fn read<'a>(
        &'a self,
        _: &'a RepoName,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Value, GithubError>> {
        self.reads.lock().unwrap().push(path.to_owned());
        let page = self.pages.lock().unwrap().get(path).cloned();
        Box::pin(std::future::ready(page.unwrap_or(Err(
            GithubError::Refused {
                status: 404,
                message: "Not Found".into(),
            },
        ))))
    }
}

struct Rig {
    tasks: FakeTasks,
    source: Arc<Source>,
    deliver: Arc<FakeDeliver>,
    db: Db,
    clock: Arc<FakeClock>,
    /// How each repository's webhook stands, which the pollers read.
    hooks: watch::Sender<HookStates>,
    pollers: Option<Pollers>,
}

impl Rig {
    /// A rig whose repository GitHub reads as a public one.
    fn new() -> Self {
        let rig = Self {
            tasks: FakeTasks::default(),
            source: Arc::default(),
            deliver: Arc::default(),
            db: Db::open_in_memory().unwrap(),
            clock: Arc::new(FakeClock::new()),
            hooks: watch::Sender::new(HookStates::new()),
            pollers: None,
        };
        rig.page(&repository_path(), Ok(repository(false)));
        rig
    }

    /// Starts the pollers on `tasks`.
    fn start(mut self, tasks: Vec<Task>) -> Self {
        self.set(tasks);
        self.pollers = Some(start(
            &self.tasks,
            self.source.clone(),
            self.deliver.clone(),
            self.db.clone(),
            self.clock.clone(),
            self.hooks.clone(),
        ));
        self
    }

    fn set(&self, tasks: Vec<Task>) {
        self.tasks.tasks.send_replace(Arc::from(tasks));
    }

    /// Queues an answer from the events API.
    fn answer(&self, answer: Result<Page, GithubError>) {
        self.answer_at(Listing::Events, answer);
    }

    /// Queues an answer from `listing`.
    fn answer_at(&self, listing: Listing, answer: Result<Page, GithubError>) {
        let path = listing.path(REPO).to_ascii_lowercase();
        let mut answers = self.source.answers.lock().unwrap();
        answers.entry(path).or_default().push_back(answer);
    }

    fn page(&self, path: &str, page: Result<Value, GithubError>) {
        self.source
            .pages
            .lock()
            .unwrap()
            .insert(path.to_owned(), page);
    }

    /// Each path read but the repository's own, oldest first.
    fn reads(&self) -> Vec<String> {
        let reads = self.source.reads.lock().unwrap();
        let own = repository_path();
        reads.iter().filter(|path| **path != own).cloned().collect()
    }

    /// The body each delivery carried.
    fn bodies(&self) -> Vec<Value> {
        let delivered = self.deliver.delivered.lock().unwrap();
        delivered
            .iter()
            .map(|(_, request)| serde_json::from_slice(&request.body).unwrap())
            .collect()
    }

    /// Each listing's path and ETag, oldest first.
    fn asks(&self) -> Vec<(String, Option<String>)> {
        self.source.asked.borrow().clone()
    }

    /// Waits for the `n`th ask, then for every poller to sleep again.
    async fn polled(&self, n: usize, pollers: usize) {
        eventually(|| self.asks().len() >= n).await;
        eventually(|| self.clock.sleepers() == pollers).await;
        assert_eq!(self.asks().len(), n, "asked more than expected");
    }

    async fn tick(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
        // Give a poller that shouldn't wake the chance to, so the test
        // would see it.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    /// Each delivery's task, event ID and webhook event.
    fn delivered(&self) -> Vec<(TaskId, String, String)> {
        let delivered = self.deliver.delivered.lock().unwrap();
        delivered
            .iter()
            .map(|(task, request)| {
                (
                    *task,
                    request.delivery.clone(),
                    request.headers["x-github-event"].clone(),
                )
            })
            .collect()
    }

    fn cursor(&self) -> Cursor {
        self.db.github_cursor(REPO).unwrap().unwrap_or_default()
    }

    fn problems(&self) -> Problems {
        self.pollers().problems().borrow().clone()
    }

    fn pollers(&self) -> &Pollers {
        self.pollers.as_ref().unwrap()
    }
}

async fn eventually(mut probe: impl FnMut() -> bool) {
    for _ in 0..500 {
        if probe() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("it never happened");
}

fn task(name: &str, repo: &str, events: &[&str]) -> Task {
    Task {
        id: TaskId::derive(name),
        name: name.into(),
        prompt: "Look at {{body.issue.title}}".into(),
        trigger: Trigger::GithubEvents {
            repo: repo.into(),
            events: events.iter().map(|event| (*event).to_owned()).collect(),
            rules: HookRules::default(),
        },
        target: TaskTarget::Agent {
            device: DeviceId::derive("d"),
            agent: AgentId::derive("a"),
        },
        enabled: true,
        permission: PermissionId::derive("p"),
        made_by: Maker::User,
        next_run: None,
        last_run: None,
        approval_asked: None,
    }
}

const REPO: &str = "owner/repo";
/// The fake clock's start, in seconds.
const START: i64 = 1_800_000_000;

/// `secs` seconds from the fake clock's start, as GitHub writes it.
fn at(secs: i64) -> String {
    jiff::Timestamp::from_second(START + secs)
        .unwrap()
        .to_string()
}

/// `secs` seconds from the fake clock's start, in milliseconds.
fn millis(secs: i64) -> u64 {
    u64::try_from((START + secs) * 1000).unwrap()
}

fn repository_path() -> String {
    format!("/repos/{REPO}")
}

fn repository(private: bool) -> Value {
    json!({ "id": 9, "full_name": REPO, "private": private })
}

/// An events API event `secs` seconds from the start.
fn event(id: u64, kind: &str, secs: i64) -> Value {
    json!({
        "id": id.to_string(),
        "type": kind,
        "actor": { "id": 7, "login": "octo" },
        "repo": { "id": 9, "name": "owner/repo" },
        "payload": {
            "action": "opened",
            "issue": { "number": id, "user": { "login": "kit" } },
        },
        "created_at": at(secs),
    })
}

/// Pushes, by ID and when each happened.
fn pushes(pushes: &[(u64, i64)]) -> Vec<Value> {
    pushes
        .iter()
        .map(|(id, secs)| event(*id, "PushEvent", *secs))
        .collect()
}

fn fresh(items: Vec<Value>, etag: &str) -> Page {
    Page {
        items: Fetched::Fresh {
            value: items,
            etag: Some(etag.to_owned()),
        },
        poll_after: None,
    }
}

fn unchanged() -> Page {
    Page {
        items: Fetched::Unchanged,
        poll_after: None,
    }
}

fn events_path() -> String {
    Listing::Events.path(REPO)
}

#[tokio::test]
async fn the_first_poll_only_records_then_new_events_go_oldest_first() {
    let rig = Rig::new();
    rig.answer(Ok(fresh(pushes(&[(3, -10), (2, -20)]), "a")));
    // The events API's IDs don't keep to when each happened.
    let newer = pushes(&[(4, 40), (5, 20), (3, -10), (2, -20)]);
    rig.answer(Ok(fresh(newer, "b")));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);

    rig.polled(1, 1).await;
    assert_eq!(rig.asks(), [(events_path(), None)]);
    assert!(rig.delivered().is_empty());
    let cursor = rig.cursor();
    assert_eq!(cursor.since["events"], millis(0));
    assert_eq!(cursor.etags[&events_path()], "a");

    rig.tick(60).await;
    rig.polled(2, 1).await;
    assert_eq!(rig.asks()[1].1.as_deref(), Some("a"));
    let id = TaskId::derive("t");
    assert_eq!(
        rig.delivered(),
        [
            (id, "5".to_owned(), "push".to_owned()),
            (id, "4".to_owned(), "push".to_owned()),
        ]
    );
    let cursor = rig.cursor();
    assert_eq!(cursor.etags[&events_path()], "b");
    assert_eq!(cursor.seen.keys().collect::<Vec<_>>(), ["4", "5"]);

    // Listed again, they don't run again.
    rig.answer(Ok(fresh(pushes(&[(4, 40), (5, 20)]), "c")));
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert_eq!(rig.delivered().len(), 2);
}

#[tokio::test]
async fn unchanged_fires_nothing_and_asks_with_the_etag_again() {
    let rig = Rig::new();
    rig.answer(Ok(fresh(pushes(&[(1, -10)]), "a")));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);
    rig.polled(1, 1).await;
    for n in 2..=3 {
        rig.tick(60).await;
        rig.polled(n, 1).await;
    }
    let etags: Vec<_> = rig.asks().into_iter().map(|(_, etag)| etag).collect();
    assert_eq!(etags, [None, Some("a".into()), Some("a".into())]);
    assert!(rig.delivered().is_empty());
}

#[tokio::test]
async fn never_polls_sooner_than_github_asks() {
    let rig = Rig::new();
    rig.answer(Ok(Page {
        poll_after: Some(Duration::from_secs(120)),
        ..fresh(pushes(&[(1, -10)]), "a")
    }));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);
    rig.polled(1, 1).await;

    rig.tick(60).await;
    assert_eq!(rig.asks().len(), 1);
    rig.tick(60).await;
    rig.polled(2, 1).await;

    // Without the header, it waits the default minute.
    rig.tick(59).await;
    assert_eq!(rig.asks().len(), 2);
    rig.tick(1).await;
    rig.polled(3, 1).await;
}

#[tokio::test]
async fn one_repo_watched_by_two_tasks_polls_once() {
    let rig = Rig::new();
    rig.answer(Ok(fresh(pushes(&[(1, -10)]), "a")));
    rig.answer(Ok(fresh(pushes(&[(2, 10), (1, -10)]), "b")));
    let mut paused = task("paused", "owner/other", &["push"]);
    paused.enabled = false;
    let rig = rig.start(vec![
        task("one", REPO, &["push"]),
        task("two", "Owner/Repo", &["push"]),
        paused,
    ]);
    rig.polled(1, 1).await;
    rig.tick(60).await;
    rig.polled(2, 1).await;

    assert!(rig
        .asks()
        .iter()
        .all(|(path, _)| path.eq_ignore_ascii_case(&events_path())));
    let to: Vec<TaskId> = rig.delivered().into_iter().map(|(task, ..)| task).collect();
    assert_eq!(to, [TaskId::derive("one"), TaskId::derive("two")]);
}

#[tokio::test]
async fn only_the_events_a_task_lists_go_to_it() {
    let rig = Rig::new();
    rig.answer(Ok(fresh(Vec::new(), "a")));
    rig.answer(Ok(fresh(
        vec![
            event(4, "WorkflowRunEvent", 30),
            event(3, "ReleaseEvent", 20),
            event(2, "PushEvent", 10),
        ],
        "b",
    )));
    let rig = rig.start(vec![
        task("pushes", REPO, &["push"]),
        task("releases", REPO, &["ReleaseEvent"]),
    ]);
    rig.polled(1, 1).await;
    rig.tick(60).await;
    rig.polled(2, 1).await;

    assert_eq!(
        rig.delivered(),
        [
            (TaskId::derive("pushes"), "2".to_owned(), "push".to_owned()),
            (
                TaskId::derive("releases"),
                "3".to_owned(),
                "release".to_owned()
            ),
        ]
    );
    let delivered = rig.deliver.delivered.lock().unwrap();
    let request = &delivered[1].1;
    assert_eq!(request.headers["x-github-delivery"], "3");
    assert_eq!(request.headers["content-type"], "application/json");
    assert_eq!(request.received, rig.clock.wall());
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["issue"]["number"], 3);
    assert_eq!(body["repository"]["full_name"], REPO);
    assert_eq!(body["sender"]["login"], "octo");
}

#[tokio::test]
async fn turning_a_task_off_stops_its_poller_and_on_again_doesnt_replay() {
    let rig = Rig::new();
    rig.answer(Ok(fresh(pushes(&[(1, -10)]), "a")));
    let on = task("t", REPO, &["push"]);
    let rig = rig.start(vec![on.clone()]);
    rig.polled(1, 1).await;

    let mut off = on.clone();
    off.enabled = false;
    rig.set(vec![off]);
    eventually(|| rig.clock.sleepers() == 0).await;
    eventually(|| rig.db.github_cursor(REPO).unwrap().is_none()).await;
    rig.tick(600).await;
    assert_eq!(rig.asks().len(), 1);

    // What happened while it was off stays unrun.
    rig.answer(Ok(fresh(pushes(&[(3, 400), (2, 300), (1, -10)]), "b")));
    rig.set(vec![on]);
    rig.polled(2, 1).await;
    assert_eq!(rig.asks()[1].1, None);
    assert!(rig.delivered().is_empty());
    let cursor = rig.cursor();
    assert_eq!(cursor.since["events"], millis(600));
    assert_eq!(cursor.etags[&events_path()], "b");
}

#[tokio::test]
async fn a_restart_picks_up_from_the_stored_cursor() {
    let rig = Rig::new();
    let mut cursor = Cursor::default();
    cursor.since.insert("events".into(), millis(-60));
    cursor.seen.insert("3".into(), millis(-30));
    cursor.etags.insert(events_path(), "a".into());
    rig.db.put_github_cursor(REPO, &cursor).unwrap();
    rig.answer(Ok(fresh(pushes(&[(4, -10), (3, -30), (2, -90)]), "b")));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);
    rig.polled(1, 1).await;

    assert_eq!(rig.asks()[0].1.as_deref(), Some("a"));
    let ids: Vec<String> = rig.delivered().into_iter().map(|(_, id, _)| id).collect();
    assert_eq!(ids, ["4"]);
}

#[tokio::test]
async fn a_repo_without_the_app_says_to_install_it_and_backs_off() {
    let rig = Rig::new();
    for _ in 0..2 {
        rig.answer(Err(GithubError::NotInstalled(REPO.into())));
    }
    rig.answer(Ok(fresh(pushes(&[(1, -10)]), "a")));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);
    rig.polled(1, 1).await;
    assert_eq!(
        rig.problems().get(REPO).map(String::as_str),
        Some(INSTALL_APP)
    );

    rig.tick(60).await;
    rig.polled(2, 1).await;
    // Two failures in a row wait two minutes.
    rig.tick(60).await;
    assert_eq!(rig.asks().len(), 2);
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert!(rig.problems().is_empty());
}

/// A pull request as GitHub lists or reads it, opened `opened` and last
/// changed `changed` seconds from the start, with its head at `head`.
fn pull(number: u64, head: &str, opened: i64, changed: i64) -> Value {
    json!({
        "number": number,
        "state": "open",
        "title": format!("Pull {number}"),
        "user": { "login": "kit" },
        "url": format!("https://api.github.com/repos/{REPO}/pulls/{number}"),
        "html_url": format!("https://github.com/{REPO}/pull/{number}"),
        "draft": false,
        "merged": false,
        "created_at": at(opened),
        "updated_at": at(changed),
        "head": { "ref": format!("branch-{number}"), "sha": head, "repo": { "full_name": REPO } },
        "base": { "ref": "main", "sha": "bbb1", "repo": { "full_name": REPO } },
    })
}

/// A pull request as GitHub reads it in full.
fn full_pull(number: u64) -> Value {
    pull(number, "aaa1", -100, -50)
}

fn pull_path(number: u64) -> String {
    format!("/repos/{REPO}/pulls/{number}")
}

/// Polls once more after the first, with `listing` answering `items`.
async fn poll_new(rig: &Rig, listing: Listing, items: Vec<Value>, asks: usize) {
    rig.answer_at(listing, Ok(fresh(items, "b")));
    rig.tick(60).await;
    rig.polled(asks, 1).await;
}

fn started(tasks: Vec<Task>) -> Rig {
    let rig = Rig::new();
    rig.answer(Ok(fresh(Vec::new(), "a")));
    rig.start(tasks)
}

#[tokio::test]
async fn a_pull_request_opened_or_pushed_to_runs_as_github_names_it() {
    let rig = Rig::new();
    let mut closed = pull(5, "aaa5", -300, -200);
    rig.answer_at(
        Listing::Pulls,
        Ok(fresh(
            vec![pull(3, "aaa1", -100, -50), closed.clone()],
            "p1",
        )),
    );
    let rig = rig.start(vec![task("pulls", REPO, &["pull_request"])]);
    // The pull requests and their issue events.
    rig.polled(2, 1).await;
    assert!(rig.delivered().is_empty());
    assert_eq!(rig.cursor().pulls[&3].sha, "aaa1");

    closed["state"] = json!("closed");
    closed["head"]["sha"] = json!("ddd5");
    let listed = vec![pull(3, "ccc2", -100, 20), pull(4, "aaa4", 10, 10), closed];
    rig.answer_at(Listing::Pulls, Ok(fresh(listed, "p2")));
    rig.tick(60).await;
    rig.polled(4, 1).await;

    assert_eq!(
        rig.asks()[2],
        (Listing::Pulls.path(REPO), Some("p1".to_owned()))
    );
    let id = TaskId::derive("pulls");
    let event = "pull_request".to_owned();
    assert_eq!(
        rig.delivered(),
        [
            (id, "pull-4-opened".to_owned(), event.clone()),
            (id, "pull-3-synchronize-ccc2".to_owned(), event),
        ]
    );
    let bodies = rig.bodies();
    assert_eq!(bodies[0]["action"], "opened");
    assert_eq!(bodies[0]["pull_request"]["title"], "Pull 4");
    assert_eq!(bodies[0]["sender"]["login"], "kit");
    assert_eq!(bodies[0]["repository"]["private"], false);
    assert_eq!(bodies[1]["action"], "synchronize");
    assert_eq!(bodies[1]["before"], "aaa1");
    assert_eq!(bodies[1]["after"], "ccc2");
    assert_eq!(bodies[1]["pull_request"]["head"]["sha"], "ccc2");
    // The list has each one whole.
    assert!(rig.reads().is_empty());
}

/// Issue event `id`, `secs` seconds from the start, on pull request
/// `number`.
fn pull_issue_event(id: u64, event: &str, number: u64, secs: i64) -> Value {
    json!({
        "id": id,
        "event": event,
        "actor": { "id": 7, "login": "octo" },
        "created_at": at(secs),
        "issue": {
            "number": number,
            "title": format!("Pull {number}"),
            "pull_request": { "url": format!("https://api.github.com/repos/{REPO}/pulls/{number}") },
        },
    })
}

#[tokio::test]
async fn a_closed_pull_request_goes_with_the_whole_pull_request() {
    let rig = started(vec![
        task("one", REPO, &["pull_request"]),
        task("two", REPO, &["PullRequestEvent"]),
    ]);
    rig.polled(2, 1).await;
    let mut merged = full_pull(4);
    merged["merged"] = json!(true);
    rig.page(&pull_path(4), Ok(merged));
    let closed = pull_issue_event(77, "closed", 4, 10);
    poll_new(&rig, Listing::IssueEvents, vec![closed], 4).await;

    // Read once for both tasks.
    assert_eq!(rig.reads(), [pull_path(4)]);
    let delivered: Vec<String> = rig.delivered().into_iter().map(|(_, id, _)| id).collect();
    assert_eq!(delivered, ["issue-event-77", "issue-event-77"]);
    let body = &rig.bodies()[0];
    assert_eq!(body["action"], "closed");
    assert_eq!(body["number"], 4);
    assert_eq!(body["pull_request"]["user"]["login"], "kit");
    assert_eq!(body["pull_request"]["merged"], true);
    assert_eq!(body["sender"]["login"], "octo");
    assert!(rig.problems().is_empty());
}

/// Comment `id` on issue `number`, `secs` seconds from the start.
fn comment(id: u64, number: u64, secs: i64) -> Value {
    json!({
        "id": id,
        "user": { "login": "kit" },
        "body": "Looks good",
        "created_at": at(secs),
        "issue_url": format!("https://api.github.com/repos/{REPO}/issues/{number}"),
    })
}

#[tokio::test]
async fn a_failed_read_still_hands_on_the_short_form_and_shows_why() {
    let rig = Rig::new().start(vec![task("comments", REPO, &["issue_comment"])]);
    rig.polled(1, 1).await;
    let issue = format!("/repos/{REPO}/issues/4");
    let down = GithubError::Unreachable("down".into());
    rig.page(&issue, Err(down.clone()));
    poll_new(&rig, Listing::Comments, vec![comment(51, 4, 10)], 2).await;

    let body = &rig.bodies()[0];
    assert_eq!(body["action"], "created");
    assert_eq!(body["comment"]["user"]["login"], "kit");
    assert_eq!(body["issue"]["number"], 4);
    assert_eq!(body["issue"].get("user"), None);
    let problem = format!("{SHORT_FORM}: {down}");
    assert_eq!(rig.problems().get(REPO), Some(&problem));

    // A poll with nothing new keeps it; one whose read works clears it.
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert_eq!(rig.problems().get(REPO), Some(&problem));
    let whole = json!({ "number": 4, "title": "Issue 4", "user": { "login": "kit" } });
    rig.page(&issue, Ok(whole));
    rig.answer_at(Listing::Comments, Ok(fresh(vec![comment(52, 4, 130)], "c")));
    rig.tick(60).await;
    rig.polled(4, 1).await;
    assert_eq!(rig.bodies()[1]["issue"]["title"], "Issue 4");
    assert!(rig.problems().is_empty());
}

#[tokio::test]
async fn a_comment_on_a_pull_requests_changes_goes_with_the_pull_request() {
    let rig = Rig::new().start(vec![task("notes", REPO, &["pull_request_review_comment"])]);
    rig.polled(1, 1).await;
    rig.page(&pull_path(4), Ok(full_pull(4)));
    let note = json!({
        "id": 61,
        "user": { "login": "kit" },
        "body": "Nit",
        "created_at": at(10),
        "pull_request_url": format!("https://api.github.com/repos/{REPO}/pulls/4"),
    });
    poll_new(&rig, Listing::ReviewComments, vec![note], 2).await;

    assert_eq!(
        rig.delivered(),
        [(
            TaskId::derive("notes"),
            "review-comment-61".to_owned(),
            "pull_request_review_comment".to_owned()
        )]
    );
    let body = &rig.bodies()[0];
    assert_eq!(body["action"], "created");
    assert_eq!(body["pull_request"]["title"], "Pull 4");
}

#[tokio::test]
async fn a_failed_listing_hands_on_the_others_and_shows_why() {
    let rig = started(vec![
        task("pulls", REPO, &["pull_request"]),
        task("pushes", REPO, &["push"]),
    ]);
    // The pull requests, their issue events and the events API.
    rig.polled(3, 1).await;
    let refused = GithubError::Refused {
        status: 502,
        message: "Bad Gateway".into(),
    };
    rig.answer_at(Listing::Pulls, Err(refused.clone()));
    poll_new(&rig, Listing::Events, pushes(&[(2, 10)]), 6).await;

    assert_eq!(
        rig.delivered(),
        [(TaskId::derive("pushes"), "2".to_owned(), "push".to_owned())]
    );
    assert_eq!(rig.problems().get(REPO), Some(&refused.to_string()));

    // It doesn't back off, and the next poll that works clears it.
    rig.tick(60).await;
    rig.polled(9, 1).await;
    assert!(rig.problems().is_empty());
}

/// Review `id` of a pull request, in `state`, submitted `secs` seconds
/// from the start.
fn review(id: u64, state: &str, secs: i64) -> Value {
    json!({
        "id": id,
        "state": state,
        "user": { "login": "octo" },
        "body": "Fine",
        "submitted_at": at(secs),
    })
}

#[tokio::test]
async fn reviews_are_read_once_each_time_a_pull_request_changes() {
    let rig = Rig::new();
    let listed = vec![pull(4, "aaa1", -100, -50)];
    rig.answer_at(Listing::Pulls, Ok(fresh(listed, "p1")));
    let rig = rig.start(vec![task("reviews", REPO, &["pull_request_review"])]);
    rig.polled(1, 1).await;
    // Nothing's changed since it began.
    assert!(rig.reads().is_empty());

    let reviews = json!([
        review(899, "COMMENTED", -60),
        review(900, "APPROVED", 5),
        { "id": 901, "state": "PENDING", "user": { "login": "octo" } },
    ]);
    rig.page(&reviews_path(REPO, 4), Ok(reviews));
    poll_new(&rig, Listing::Pulls, vec![pull(4, "aaa1", -100, 10)], 2).await;

    assert_eq!(
        rig.delivered(),
        [(
            TaskId::derive("reviews"),
            "review-900".to_owned(),
            "pull_request_review".to_owned()
        )]
    );
    let body = &rig.bodies()[0];
    assert_eq!(body["action"], "submitted");
    assert_eq!(body["review"]["state"], "approved");
    assert_eq!(body["pull_request"]["title"], "Pull 4");
    assert_eq!(body["sender"]["login"], "octo");

    // Listed again unchanged, its reviews aren't read again.
    let listed = vec![pull(4, "aaa1", -100, 10)];
    rig.answer_at(Listing::Pulls, Ok(fresh(listed, "p3")));
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert_eq!(rig.reads(), [reviews_path(REPO, 4)]);
    assert_eq!(rig.delivered().len(), 1);
}

#[tokio::test]
async fn a_failed_reviews_read_shows_why_and_lists_again_in_full() {
    let rig = Rig::new().start(vec![task("reviews", REPO, &["pull_request_review"])]);
    rig.polled(1, 1).await;
    let down = GithubError::Unreachable("down".into());
    rig.page(&reviews_path(REPO, 4), Err(down.clone()));
    poll_new(&rig, Listing::Pulls, vec![pull(4, "aaa1", -100, 10)], 2).await;
    assert!(rig.delivered().is_empty());
    let problem = format!("{NO_REVIEWS}: {down}");
    assert_eq!(rig.problems().get(REPO), Some(&problem));

    rig.page(
        &reviews_path(REPO, 4),
        Ok(json!([review(900, "APPROVED", 5)])),
    );
    let listed = vec![pull(4, "aaa1", -100, 10)];
    rig.answer_at(Listing::Pulls, Ok(fresh(listed, "p3")));
    rig.tick(60).await;
    rig.polled(3, 1).await;
    // Without the ETag, so GitHub lists the pull request again.
    assert_eq!(rig.asks()[2].1, None);
    assert_eq!(rig.delivered().len(), 1);
    assert!(rig.problems().is_empty());
}

#[tokio::test]
async fn a_listing_needed_again_starts_afresh() {
    let comments = task("t", REPO, &["issue_comment"]);
    let rig = Rig::new();
    rig.answer_at(Listing::Comments, Ok(fresh(Vec::new(), "c1")));
    let rig = rig.start(vec![comments.clone()]);
    rig.polled(1, 1).await;

    rig.set(vec![task("t", REPO, &["push"])]);
    rig.tick(60).await;
    rig.polled(2, 1).await;
    assert_eq!(rig.asks()[1].0, events_path());
    let cursor = rig.cursor();
    assert!(!cursor.since.contains_key("comments"));
    assert!(!cursor.etags.contains_key(&Listing::Comments.path(REPO)));

    // A comment made while no task watched them stays unrun.
    rig.set(vec![comments]);
    rig.answer_at(Listing::Comments, Ok(fresh(vec![comment(51, 4, 90)], "c2")));
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert_eq!(rig.asks()[2], (Listing::Comments.path(REPO), None));
    assert!(rig.delivered().is_empty());
}

#[tokio::test]
async fn a_private_repo_without_a_webhook_says_what_its_tasks_miss() {
    let rig = Rig::new();
    rig.page(&repository_path(), Ok(repository(true)));
    let rig = rig.start(vec![task("t", REPO, &["push", "pull_request"])]);
    // The pull requests, their issue events and the events API.
    rig.polled(3, 1).await;
    let problem = "GitHub only sends push from a private repo through a webhook, and this device has none there";
    assert_eq!(rig.problems().get(REPO).map(String::as_str), Some(problem));

    rig.hooks
        .send_replace([(REPO.to_owned(), HookState::default())].into());
    rig.tick(60).await;
    rig.polled(6, 1).await;
    assert!(rig.problems().is_empty());
}

/// Notes a verified delivery from `REPO`'s webhook now, as the doorbell
/// does.
fn rang(rig: &Rig) {
    rig.hooks.send_modify(|states| {
        let state = states.entry(REPO.to_owned()).or_default();
        state.delivered = Some(rig.clock.wall());
        state.instant = true;
    });
}

fn instant(rig: &Rig) -> bool {
    rig.hooks
        .borrow()
        .get(REPO)
        .is_some_and(|state| state.instant)
}

#[tokio::test]
async fn a_delivering_webhook_slows_polls_to_fifteen_minutes_until_it_goes_quiet() {
    let rig = Rig::new();
    rig.hooks
        .send_replace([(REPO.to_owned(), HookState::default())].into());
    rig.answer(Ok(fresh(Vec::new(), "a")));
    let rig = rig.start(vec![task("t", REPO, &["push"])]);
    rig.polled(1, 1).await;
    rig.tick(60).await;
    rig.polled(2, 1).await;
    assert!(!instant(&rig));

    // A delivery stretches the wait after the one under way.
    rang(&rig);
    rig.tick(60).await;
    rig.polled(3, 1).await;
    assert!(instant(&rig));
    rig.tick(14 * 60).await;
    assert_eq!(rig.asks().len(), 3);
    rig.tick(60).await;
    rig.polled(4, 1).await;
    for n in 5..=6 {
        rig.tick(15 * 60).await;
        rig.polled(n, 1).await;
    }

    // An hour after the last delivery, polls go back to every minute.
    rig.tick(14 * 60).await;
    rig.polled(7, 1).await;
    assert!(!instant(&rig));
    rig.tick(60).await;
    rig.polled(8, 1).await;
}

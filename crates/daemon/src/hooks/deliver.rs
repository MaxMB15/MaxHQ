// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What happens to a request once it's in: the checks a webhook request
//! alone has, then the task's filters, replay key, coalescing and
//! template, and the fire. Each step that stops a request writes it to
//! the task's delivery log, in one of three parts that each keep their
//! newest rows. A hook that holds requests keeps one
//! whose target's device is offline, and tries it again later.

use futures::future::BoxFuture;
use maxhq_core::agents;
use maxhq_core::github_events::author_allowed;
use maxhq_core::hooks::template::{self, Fields, Rendered, MAX_PROMPT_CHARS, PROMPT_TOO_LONG};
use maxhq_core::hooks::{dedupe, filter, hold, verify, Incoming};
use maxhq_protocol::tasks::{DeliveryOutcome, HookRules, Task, TaskDelivery, Trigger};
use ring::rand::{SecureRandom, SystemRandom};

use super::rate::Slot;
use super::service::funnel_path;
use super::store::{Held, Kept, Requests, Row, Window, OPEN};
use super::{Deliver, Request, Webhooks};
use crate::tasks::{Delivered, Fire, FireError};

/// The most requests a task takes a minute.
const PER_MINUTE: u32 = 60;

/// The most requests with a bad signature a task checks a minute. It sits
/// well above [`PER_MINUTE`], and it bounds the work a sender without the
/// secret can cause.
pub(super) const BAD_SIGNATURES_PER_MINUTE: u32 = 600;

/// The most filtered, duplicate, coalesced or badly signed rows a task logs
/// a minute.
const LOG_PER_MINUTE: u32 = 10;

/// The most requests a task holds while its target's device is offline.
pub(super) const MAX_HELD: u32 = 200;

/// What a task's log says of an event from an author it doesn't run for.
const NOT_AN_ALLOWED_AUTHOR: &str = "This event's author isn't someone this task runs for.";

/// The most coalescing windows a task holds open.
const MAX_OPEN: usize = 20;

const MINUTE_MS: u64 = 60 * 1000;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;

/// The rules a task's requests meet, for the triggers that take them.
fn rules(task: &Task) -> Option<&HookRules> {
    match &task.trigger {
        Trigger::Hook(hook) => Some(&hook.rules),
        Trigger::GithubEvents { rules, .. } => Some(rules),
        _ => None,
    }
}

/// What a task's prompt may hold.
fn fields(task: &Task) -> Fields<'_> {
    let github = match &task.trigger {
        Trigger::GithubEvents { repo, .. } => Some(repo.as_str()),
        _ => None,
    };
    Fields::of(
        rules(task).and_then(|rules| rules.fields.as_deref()),
        github,
    )
}

/// Notes on a delivery the fields its prompt lacked or left out.
fn note(delivery: &mut TaskDelivery, rendered: &mut Rendered) {
    delivery.missing_fields = std::mem::take(&mut rendered.missing);
    delivery.refused_fields = std::mem::take(&mut rendered.refused);
}

/// Which part of the log an outcome goes in, and how many rows it keeps.
fn part(outcome: DeliveryOutcome) -> (&'static str, usize) {
    match outcome {
        DeliveryOutcome::Filtered => ("filtered", 20),
        DeliveryOutcome::Duplicate | DeliveryOutcome::Coalesced | DeliveryOutcome::RateLimited => {
            ("dedupe", 20)
        }
        _ => ("main", 50),
    }
}

/// What a fire's answer means for its delivery.
fn fired(result: Result<(), FireError>) -> (DeliveryOutcome, Option<String>) {
    match result {
        Ok(()) => (DeliveryOutcome::Accepted, None),
        Err(FireError::Gone) => (
            DeliveryOutcome::DispatchFailed,
            Some("The task is gone.".into()),
        ),
        Err(FireError::Off) => (DeliveryOutcome::Disabled, None),
        Err(FireError::QueueFull) => (DeliveryOutcome::QueueFull, None),
        Err(FireError::Offline) => (DeliveryOutcome::TargetOffline, None),
        Err(FireError::Failed(reason)) => (DeliveryOutcome::DispatchFailed, Some(reason)),
    }
}

/// Fills a task's template from each of its run's requests.
fn render(task: &Task, requests: &Requests) -> Rendered {
    let path = funnel_path(task.id);
    let incoming: Vec<Incoming> = requests
        .all()
        .map(|kept| Incoming {
            method: "POST",
            path: &path,
            query: &kept.query,
            headers: &kept.headers,
            body: &kept.body,
        })
        .collect();
    let dropped = usize::try_from(requests.dropped).unwrap_or(usize::MAX);
    template::render_all(&task.prompt, &incoming, &frame(), dropped, fields(task))
}

/// A request as a window or a hold keeps it.
fn kept(request: &Request, body: &str) -> Kept {
    Kept {
        headers: request.headers.clone(),
        query: request.query.clone(),
        body: body.to_owned(),
    }
}

/// A random tag for one request's frames.
pub(crate) fn frame() -> String {
    let mut bytes = [0u8; 6];
    // The tag only has to be hard to guess, so a failing source still
    // leaves a frame.
    let _ = SystemRandom::new().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Webhooks {
    /// Runs the checks only a webhook request has, then delivers it.
    pub(super) async fn check(&self, task: &Task, request: Request) -> DeliveryOutcome {
        let Trigger::Hook(hook) = &task.trigger else {
            return DeliveryOutcome::DispatchFailed;
        };
        let record = record(task, &request);
        // The signature comes before the slot, so requests that can't sign
        // don't use up the slots real ones need.
        if let Some(check) = &hook.signature {
            let now = request.received;
            let limit = BAD_SIGNATURES_PER_MINUTE;
            let mut rates = self.rates();
            if rates.bad_signature.spent(task.id, now, limit, MINUTE_MS) {
                let slot = rates.bad_signature.take(task.id, now, limit, MINUTE_MS);
                drop(rates);
                return self.over_limit(record, slot);
            }
            drop(rates);
            let secret = self.inner.secrets.get(task.id).unwrap_or_else(|err| {
                tracing::warn!(%err, "couldn't read a webhook's signing secret");
                None
            });
            let signed = secret.is_some_and(|secret| {
                verify::verify(
                    check,
                    secret.expose().as_bytes(),
                    &request.headers,
                    &request.body,
                )
            });
            if !signed {
                self.rates()
                    .bad_signature
                    .take(task.id, now, limit, MINUTE_MS);
                if self.may_log(task, true) {
                    let record = TaskDelivery {
                        signature_verified: false,
                        ..record
                    };
                    self.log(record, DeliveryOutcome::RejectedSignature, None);
                }
                return DeliveryOutcome::RejectedSignature;
            }
        }
        let slot = self
            .rates()
            .minute
            .take(task.id, request.received, PER_MINUTE, MINUTE_MS);
        if slot != Slot::Allowed {
            return self.over_limit(record, slot);
        }
        if !task.enabled {
            return self.log(record, DeliveryOutcome::Disabled, None);
        }
        if let Some(minutes) = hook.rules.max_age_minutes {
            let age = self.now().0.saturating_sub(request.received.0);
            if age > u64::from(minutes) * MINUTE_MS {
                return self.log(record, DeliveryOutcome::Expired, None);
            }
        }
        self.deliver(task, request).await
    }

    /// Answers a request past a limit, logging the first of a run of them.
    fn over_limit(&self, record: TaskDelivery, slot: Slot) -> DeliveryOutcome {
        if slot == Slot::FirstRejected {
            self.log(record, DeliveryOutcome::RateLimited, None);
        }
        DeliveryOutcome::RateLimited
    }

    /// Delivers a request that passed its checks.
    async fn take(&self, task: &Task, request: Request) -> DeliveryOutcome {
        let Some(rules) = rules(task) else {
            return DeliveryOutcome::DispatchFailed;
        };
        let mut record = record(task, &request);
        let body = String::from_utf8_lossy(&request.body);
        let path = funnel_path(task.id);
        let incoming = Incoming {
            method: "POST",
            path: &path,
            query: &request.query,
            headers: &request.headers,
            body: &body,
        };
        let now = request.received;
        if let Some((index, path)) = filter::failed(&rules.filters, &incoming) {
            if self.may_log(task, true) {
                self.log(
                    record,
                    DeliveryOutcome::Filtered,
                    Some(filter::describe(index, path)),
                );
            }
            return DeliveryOutcome::Filtered;
        }
        if matches!(task.trigger, Trigger::GithubEvents { .. })
            && !author_allowed(&body, &rules.authors)
        {
            if self.may_log(task, true) {
                self.log(
                    record,
                    DeliveryOutcome::Filtered,
                    Some(NOT_AN_ALLOWED_AUTHOR.to_owned()),
                );
            }
            return DeliveryOutcome::Filtered;
        }

        let replay = dedupe::key(rules.replay_key.as_deref(), &incoming);
        let db = &self.inner.db;
        let mut rendered = {
            // One request at a time decides on keys and windows.
            let _turn = self.inner.turn.lock().await;
            if let Some(key) = &replay
                && db.replayed(task.id, key, now).unwrap_or(false)
            {
                if self.may_log(task, false) {
                    self.log(record, DeliveryOutcome::Duplicate, None);
                }
                return DeliveryOutcome::Duplicate;
            }
            if let Some(key) = dedupe::key(rules.coalesce_key.as_deref(), &incoming) {
                return self.coalesce(task, rules, record, key, replay, &request, &body);
            }
            let mut rendered = template::render(&task.prompt, &incoming, &frame(), fields(task));
            if rendered.prompt.trim().chars().count() > MAX_PROMPT_CHARS {
                note(&mut record, &mut rendered);
                let error = Some(PROMPT_TOO_LONG.to_owned());
                return self.log(record, DeliveryOutcome::PromptTooLong, error);
            }
            if let Some(key) = &replay {
                self.or_warn(db.claim_key(task.id, key, now));
            }
            rendered
        };
        note(&mut record, &mut rendered);
        let (mut outcome, error) = self
            .run(task, rules, request.delivery.clone(), rendered.prompt)
            .await;
        if outcome == DeliveryOutcome::TargetOffline
            && self.hold(task, &record, &Requests::one(kept(&request, &body)))
        {
            outcome = DeliveryOutcome::Held;
        }
        if !matches!(outcome, DeliveryOutcome::Accepted | DeliveryOutcome::Held)
            && let Some(key) = &replay
        {
            self.or_warn(db.release_key(task.id, key));
        }
        self.log(record, outcome, error)
    }

    /// Adds a request to the window its coalescing key has open, which may
    /// move when it runs, or opens one. Either way its replay key counts as
    /// seen.
    #[allow(clippy::too_many_arguments)]
    fn coalesce(
        &self,
        task: &Task,
        rules: &HookRules,
        record: TaskDelivery,
        key: String,
        replay: Option<String>,
        request: &Request,
        body: &str,
    ) -> DeliveryOutcome {
        let db = &self.inner.db;
        let now = request.received;
        let open = db.open_window(task.id, &key, now).unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read a webhook's coalescing windows");
            None
        });
        let request = kept(request, body);
        let outcome = match open {
            Some(Row {
                delivery,
                window: Some(mut window),
            }) => {
                // The window opened with the delivery its row shows.
                window.due = dedupe::rejoined(rules, window.due, delivery.received, now);
                window.requests.push(request);
                let row = Row {
                    delivery,
                    window: Some(window),
                };
                self.or_warn(db.put_delivery(&row, OPEN, usize::MAX));
                if self.may_log(task, false) {
                    self.log(record, DeliveryOutcome::Coalesced, None);
                }
                DeliveryOutcome::Coalesced
            }
            _ if db.open_windows(task.id).unwrap_or(0) >= MAX_OPEN => {
                return self.log(record, DeliveryOutcome::QueueFull, None);
            }
            _ => {
                let row = Row {
                    delivery: TaskDelivery {
                        outcome: DeliveryOutcome::Accepted,
                        ..record
                    },
                    window: Some(Window {
                        key,
                        due: dedupe::due(now, rules.coalesce_secs),
                        requests: Requests::one(request),
                    }),
                };
                self.or_warn(db.put_delivery(&row, OPEN, usize::MAX));
                self.inner.wake.notify_one();
                DeliveryOutcome::Accepted
            }
        };
        if let Some(key) = &replay {
            self.or_warn(db.claim_key(task.id, key, now));
        }
        outcome
    }

    /// Runs a window that's due, with each request in it.
    pub(super) async fn close_window(&self, mut row: Row) {
        let Some(window) = row.window.take() else {
            return;
        };
        let task = self.task(row.delivery.task);
        let (outcome, error) = match task.as_ref().and_then(|task| Some((task, rules(task)?))) {
            None => (
                DeliveryOutcome::DispatchFailed,
                Some("The task is gone.".into()),
            ),
            Some((task, rules)) => {
                let mut rendered = render(task, &window.requests);
                note(&mut row.delivery, &mut rendered);
                if rendered.prompt.trim().chars().count() > MAX_PROMPT_CHARS {
                    (
                        DeliveryOutcome::PromptTooLong,
                        Some(PROMPT_TOO_LONG.to_owned()),
                    )
                } else {
                    let delivery = row.delivery.id.clone();
                    match self.run(task, rules, delivery, rendered.prompt).await {
                        (DeliveryOutcome::TargetOffline, _)
                            if self.hold(task, &row.delivery, &window.requests) =>
                        {
                            (DeliveryOutcome::Held, None)
                        }
                        answer => answer,
                    }
                }
            }
        };
        self.log(row.delivery, outcome, error);
    }

    /// Holds the requests of a run whose target's device is offline, if
    /// the task's hook holds requests, and says whether it did.
    fn hold(&self, task: &Task, delivery: &TaskDelivery, requests: &Requests) -> bool {
        let Trigger::Hook(hook) = &task.trigger else {
            return false;
        };
        if !hook.hold {
            return false;
        }
        // A sender that keeps going while the device is away can't fill its
        // disk. A request past the cap answers that the target is offline.
        let db = &self.inner.db;
        if db
            .held_besides(task.id, &delivery.id)
            .is_ok_and(|others| others >= MAX_HELD)
        {
            return false;
        }
        let held = Held {
            delivery: delivery.clone(),
            requests: requests.clone(),
            tries: 0,
        };
        let due = hold::due(delivery.received, 0, hook.rules.max_age_minutes);
        match db.put_hook_held(agents::target_device(&task.target), due, &held) {
            Ok(()) => {
                self.inner.wake.notify_one();
                true
            }
            Err(err) => {
                tracing::warn!(%err, "couldn't hold a webhook request");
                false
            }
        }
    }

    /// Tries a held request again. It goes once its fire is taken, or once
    /// it's past its hold, and is otherwise held until its next try.
    pub(super) async fn retry(&self, mut held: Held) {
        let (id, received) = (held.delivery.id.clone(), held.delivery.received);
        let task = self.task(held.delivery.task);
        let Some((task, rules)) = task.as_ref().and_then(|task| Some((task, rules(task)?))) else {
            self.or_warn(self.inner.db.drop_hook_held(held.delivery.task, &id));
            let error = Some("The task is gone.".into());
            self.log(held.delivery, DeliveryOutcome::DispatchFailed, error);
            return;
        };
        let max_age = rules.max_age_minutes;
        let (outcome, error) = if self.now() >= hold::expires(received, max_age) {
            (DeliveryOutcome::Expired, None)
        } else {
            let mut rendered = render(task, &held.requests);
            note(&mut held.delivery, &mut rendered);
            if rendered.prompt.trim().chars().count() > MAX_PROMPT_CHARS {
                let error = Some(PROMPT_TOO_LONG.to_owned());
                (DeliveryOutcome::PromptTooLong, error)
            } else {
                self.fire(task, id.clone(), rendered.prompt).await
            }
        };
        if outcome == DeliveryOutcome::TargetOffline {
            held.tries += 1;
            let due = hold::due(received, held.tries, max_age);
            self.or_warn(self.inner.db.put_hook_held(
                agents::target_device(&task.target),
                due,
                &held,
            ));
            return;
        }
        self.or_warn(self.inner.db.drop_hook_held(task.id, &id));
        self.log(held.delivery, outcome, error);
    }

    /// Fires a task with a prompt, unless it ran its runs for the day.
    async fn run(
        &self,
        task: &Task,
        rules: &HookRules,
        delivery: String,
        prompt: String,
    ) -> (DeliveryOutcome, Option<String>) {
        if let Some(limit) = rules.daily_runs
            && self.rates().daily.take(task.id, self.now(), limit, DAY_MS) != Slot::Allowed
        {
            let error = format!("The task ran its {limit} runs for today.");
            return (DeliveryOutcome::RateLimited, Some(error));
        }
        self.fire(task, delivery, prompt).await
    }

    /// Fires a task with a prompt.
    async fn fire(
        &self,
        task: &Task,
        delivery: String,
        prompt: String,
    ) -> (DeliveryOutcome, Option<String>) {
        let delivered = Delivered { delivery, prompt };
        let fire = match task.trigger {
            Trigger::GithubEvents { .. } => Fire::Github(delivered),
            _ => Fire::Hook(delivered),
        };
        fired(self.inner.tasks.fire(task.id, fire).await)
    }

    /// Whether a filtered, duplicate or coalesced row may go in the log,
    /// which takes ten of each kind a minute.
    fn may_log(&self, task: &Task, filtered: bool) -> bool {
        let mut rates = self.rates();
        let counter = if filtered {
            &mut rates.filtered_log
        } else {
            &mut rates.dedupe_log
        };
        counter.take(task.id, self.now(), LOG_PER_MINUTE, MINUTE_MS) == Slot::Allowed
    }

    /// Writes a delivery to its task's log with what came of it, and
    /// answers that.
    fn log(
        &self,
        delivery: TaskDelivery,
        outcome: DeliveryOutcome,
        error: Option<String>,
    ) -> DeliveryOutcome {
        let (state, keep) = part(outcome);
        let row = Row {
            delivery: TaskDelivery {
                outcome,
                error,
                ..delivery
            },
            window: None,
        };
        self.or_warn(self.inner.db.put_delivery(&row, state, keep));
        outcome
    }

    /// Logs a database failure, which leaves the request's answer as it
    /// was.
    fn or_warn<E: std::fmt::Display>(&self, result: Result<(), E>) {
        if let Err(err) = result {
            tracing::warn!(%err, "couldn't write a webhook's state");
        }
    }
}

/// A delivery's row before anything came of it.
fn record(task: &Task, request: &Request) -> TaskDelivery {
    let signed = matches!(&task.trigger, Trigger::Hook(hook) if hook.signature.is_some());
    TaskDelivery {
        id: request.delivery.clone(),
        task: task.id,
        received: request.received,
        method: "POST".to_owned(),
        content_type: request.headers.get("content-type").cloned(),
        body_bytes: u32::try_from(request.body.len()).unwrap_or(u32::MAX),
        outcome: DeliveryOutcome::Accepted,
        signature_verified: signed,
        missing_fields: Vec::new(),
        refused_fields: Vec::new(),
        error: None,
    }
}

impl Deliver for Webhooks {
    fn deliver<'a>(&'a self, task: &'a Task, request: Request) -> BoxFuture<'a, DeliveryOutcome> {
        Box::pin(self.take(task, request))
    }
}

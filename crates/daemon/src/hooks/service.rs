// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The webhook service: what it holds, the [`Hooks`] side of saving and
//! deleting a task, the loop that runs due coalescing windows, tries held
//! requests again and drops old replay keys, and the answers to the five
//! webhook requests.

use std::collections::BTreeSet;
use std::future::ready;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use base64::Engine;
use futures::future::BoxFuture;
use maxhq_protocol::ids::{DeviceId, TaskId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::tasks::{Hook, HookAddress, Task, Trigger};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::{watch, Notify};

use super::funnel::{own_base, Funnels};
use super::rate::Counter;
use super::{listener, Doorbell, Funnel, Hooks, Reach};
use crate::agents::hash;
use crate::db::Db;
use crate::session::clock::Clock;
use crate::tasks::{self, Tasks};

/// How often replay keys past their seven days go.
const CLEAN_EVERY: Duration = Duration::from_secs(60 * 60);

/// The answer to a webhook request for a task without one.
const NO_HOOK: &str = "That task has no webhook.";

/// The longest signing secret taken.
const MAX_SECRET: usize = 4096;

/// What changes a hook task's [`Hook`], which the tasks package
/// implements: it saves the task and sends the list again.
pub trait HookEdits: Send + Sync {
    /// Applies `edit` to the hook of `task`.
    fn edit_hook(
        &self,
        task: TaskId,
        edit: Box<dyn FnOnce(&mut Hook) + Send>,
    ) -> BoxFuture<'_, Result<(), String>>;
}

/// Where each hook's signing secret is kept: the device's secret store.
pub trait HookSecrets: Send + Sync {
    fn set(&self, task: TaskId, secret: &Secret) -> io::Result<()>;
    fn get(&self, task: TaskId) -> io::Result<Option<Secret>>;
    fn remove(&self, task: TaskId) -> io::Result<()>;
}

/// What the service is made of.
pub struct Parts {
    pub db: Db,
    pub tasks: Arc<dyn Tasks>,
    pub edits: Arc<dyn HookEdits>,
    pub secrets: Arc<dyn HookSecrets>,
    /// Makes the [`Funnel`] for the address this device picked.
    pub funnels: Funnels,
    /// Where the address and its state go, for `DeviceTasks`.
    pub reach: watch::Sender<Reach>,
    /// The devices a link is up to, so a request held for one goes as
    /// soon as it's back.
    pub links: watch::Receiver<BTreeSet<DeviceId>>,
    pub clock: Arc<dyn Clock>,
}

/// The webhook service. Clones share it.
#[derive(Clone)]
pub struct Webhooks {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub db: Db,
    pub tasks: Arc<dyn Tasks>,
    pub edits: Arc<dyn HookEdits>,
    pub secrets: Arc<dyn HookSecrets>,
    pub funnels: Funnels,
    /// The address this device picked, and its [`Funnel`].
    pub address: Mutex<(HookAddress, Arc<dyn Funnel>)>,
    pub reach: watch::Sender<Reach>,
    /// Held while the address changes or a path opens or closes, so a
    /// path never opens on the address being left.
    pub switch: tokio::sync::Mutex<()>,
    pub clock: Arc<dyn Clock>,
    /// The task list as the tasks package last sent it.
    pub list: watch::Receiver<Arc<[Task]>>,
    pub rates: Mutex<Rates>,
    /// Held while a request decides on its replay key or window.
    pub turn: tokio::sync::Mutex<()>,
    /// Woken when a window opens or a request is held, so the loop sees
    /// its time.
    pub wake: Notify,
    pub links: watch::Receiver<BTreeSet<DeviceId>>,
    /// What answers GitHub's repository webhooks, once `maxd` set it.
    pub doorbell: OnceLock<Arc<dyn Doorbell>>,
    /// Paths opened for something other than a task, such as the
    /// doorbell's, which move with the address too.
    pub paths: Mutex<BTreeSet<String>>,
}

/// The counters each task's requests go through.
#[derive(Default)]
pub(super) struct Rates {
    pub minute: Counter,
    pub bad_signature: Counter,
    pub daily: Counter,
    pub filtered_log: Counter,
    pub dedupe_log: Counter,
}

/// The service the request answers reach, once `maxd` has made it.
static SERVICE: OnceLock<Webhooks> = OnceLock::new();

/// Makes `hooks` the service the request answers reach, and starts it
/// on `port`. Only the first call counts.
pub fn install(hooks: Webhooks, port: u16) {
    if SERVICE.set(hooks.clone()).is_ok() {
        hooks.start(port);
    }
}

/// The installed service, for the session manager, which starts before
/// it. Before [`install`], a new hook can't get a URL and there's
/// nothing to forget.
pub struct Installed;

impl Hooks for Installed {
    fn rotate(&self, task: TaskId) -> BoxFuture<'_, Result<Secret, String>> {
        match SERVICE.get() {
            Some(hooks) => hooks.rotate(task),
            None => Box::pin(ready(Err("Webhooks aren't on in this maxd.".to_owned()))),
        }
    }

    fn forget(&self, task: TaskId) -> BoxFuture<'_, Result<(), String>> {
        match SERVICE.get() {
            Some(hooks) => hooks.forget(task),
            None => Box::pin(ready(Ok(()))),
        }
    }
}

impl Webhooks {
    pub fn new(parts: Parts) -> Self {
        let list = parts.tasks.watch();
        let address = parts.db.hook_address().unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read how webhooks are reached");
            HookAddress::Local
        });
        let funnel = (parts.funnels)(&address);
        Self {
            inner: Arc::new(Inner {
                db: parts.db,
                tasks: parts.tasks,
                edits: parts.edits,
                secrets: parts.secrets,
                funnels: parts.funnels,
                address: Mutex::new((address, funnel)),
                reach: parts.reach,
                switch: tokio::sync::Mutex::new(()),
                clock: parts.clock,
                list,
                rates: Mutex::default(),
                turn: tokio::sync::Mutex::new(()),
                wake: Notify::new(),
                links: parts.links,
                doorbell: OnceLock::new(),
                paths: Mutex::default(),
            }),
        }
    }

    /// Starts the listener on `port` and the loop, and says how hooks
    /// are reached.
    pub fn start(&self, port: u16) {
        tokio::spawn(listener::listen(self.clone(), port));
        tokio::spawn(self.clone().tend());
        let hooks = self.clone();
        tokio::spawn(async move { hooks.publish().await });
    }

    /// The [`Funnel`] for the address this device picked.
    fn funnel(&self) -> Arc<dyn Funnel> {
        self.address().1.clone()
    }

    fn address(&self) -> MutexGuard<'_, (HookAddress, Arc<dyn Funnel>)> {
        self.inner
            .address
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends the address and whether it works now.
    async fn publish(&self) {
        let (address, funnel) = self.address().clone();
        let state = funnel.state().await;
        self.inner.reach.send_modify(|reach| {
            reach.address = address;
            reach.funnel = state;
        });
    }

    /// The paths of the hooks that are on, and the others opened.
    fn open_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .inner
            .list
            .borrow()
            .iter()
            .filter(|task| matches!(&task.trigger, Trigger::Hook(hook) if hook.on))
            .map(|task| funnel_path(task.id))
            .collect();
        paths.extend(self.paths().iter().cloned());
        paths
    }

    fn paths(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.inner
            .paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets what answers GitHub's repository webhooks. Only the first call
    /// counts.
    pub fn set_doorbell(&self, doorbell: Arc<dyn Doorbell>) {
        let _ = self.inner.doorbell.set(doorbell);
    }

    pub(super) fn doorbell(&self) -> Option<Arc<dyn Doorbell>> {
        self.inner.doorbell.get().cloned()
    }

    /// How this device's hooks are reached, as it changes.
    pub fn reach(&self) -> watch::Receiver<Reach> {
        self.inner.reach.subscribe()
    }

    /// Opens `path`, which no task owns, at the address this device picked,
    /// and keeps it open there as the address changes. Answers its URL.
    pub async fn open_path(&self, path: &str) -> Result<String, String> {
        let turn = self.inner.switch.lock().await;
        let funnel = self.funnel();
        let opened = funnel.open(path).await;
        if opened.is_ok() {
            self.paths().insert(path.to_owned());
        }
        drop(turn);
        self.publish().await;
        opened?;
        Ok(format!("{}{path}", funnel.base().await?))
    }

    /// Takes a path [`Webhooks::open_path`] opened off the public internet.
    pub async fn close_path(&self, path: &str) {
        let _turn = self.inner.switch.lock().await;
        if self.paths().remove(path) {
            close_all(self.funnel().as_ref(), &[path.to_owned()]).await;
        }
    }

    /// Picks how this device's hooks are reached. Each hook that's on gets
    /// its path at the new address before the old one closes it. Tokens
    /// stay, so only each URL's base changes.
    pub(super) async fn set_address(&self, address: HookAddress) -> ActionOutcome {
        let address = match address {
            HookAddress::Own { base } => match own_base(&base) {
                Ok(base) => HookAddress::Own { base },
                Err(reason) => return ActionOutcome::Refused { reason },
            },
            other => other,
        };
        let turn = self.inner.switch.lock().await;
        let (old, old_funnel) = self.address().clone();
        let moved = std::mem::discriminant(&old) != std::mem::discriminant(&address);
        let funnel = if old == address {
            old_funnel.clone()
        } else {
            (self.inner.funnels)(&address)
        };
        let paths = self.open_paths();
        // A path that won't open shows in the state, and picking the
        // address again tries it again.
        for path in &paths {
            if let Err(err) = funnel.open(path).await {
                tracing::warn!(%err, "couldn't open a webhook's path at the new address");
            }
        }
        if let Err(err) = self.inner.db.put_hook_address(&address) {
            tracing::warn!(%err, "couldn't store how webhooks are reached");
            if moved {
                close_all(funnel.as_ref(), &paths).await;
            }
            return refused("This device couldn't store the address.");
        }
        *self.address() = (address, funnel);
        if moved {
            close_all(old_funnel.as_ref(), &paths).await;
        }
        drop(turn);
        self.publish().await;
        ActionOutcome::Done
    }

    pub(super) fn now(&self) -> Timestamp {
        self.inner.clock.wall()
    }

    pub(super) fn rates(&self) -> MutexGuard<'_, Rates> {
        self.inner
            .rates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The task with this ID, as the list last had it.
    pub(super) fn task(&self, id: TaskId) -> Option<Task> {
        self.inner
            .list
            .borrow()
            .iter()
            .find(|task| task.id == id)
            .cloned()
    }

    /// The task with this ID when its trigger is a webhook.
    fn hook_task(&self, id: TaskId) -> Option<Task> {
        self.task(id)
            .filter(|task| matches!(task.trigger, Trigger::Hook(_)))
    }

    /// Runs coalescing windows when they're due, tries held requests
    /// again when they're due or their device is back, and drops replay
    /// keys, GitHub's among them, past their seven days every hour.
    pub(super) async fn tend(self) {
        let clock = &self.inner.clock;
        let every = millis(CLEAN_EVERY);
        let mut clean = self.now();
        let mut links = self.inner.links.clone();
        let mut up = links.borrow_and_update().clone();
        let mut linked = true;
        let mut back = Vec::new();
        loop {
            let now = self.now();
            if now.0 >= clean.0 {
                let db = &self.inner.db;
                if let Err(err) = db.clean_seen(now).and_then(|_| db.clean_github_seen(now)) {
                    tracing::warn!(%err, "couldn't drop old webhook replay keys");
                }
                clean = Timestamp(now.0 + every);
            }
            let due = self.inner.db.due_windows(now).unwrap_or_else(|err| {
                tracing::warn!(%err, "couldn't read webhook coalescing windows");
                Vec::new()
            });
            for row in due {
                self.close_window(row).await;
            }
            // One look covers both, so a request due as its device comes
            // back is tried once.
            let held = self.inner.db.hook_held(now, &back).unwrap_or_else(|err| {
                tracing::warn!(%err, "couldn't read held webhook requests");
                Vec::new()
            });
            back.clear();
            for held in held {
                self.retry(held).await;
            }
            let db = &self.inner.db;
            let next = [db.next_due(), db.next_hook_held()]
                .into_iter()
                .filter_map(|due| due.ok().flatten())
                .fold(clean.0, |next, due| next.min(due.0));
            let wait = Duration::from_millis(next.saturating_sub(self.now().0));
            tokio::select! {
                () = clock.sleep_until(clock.now() + wait) => {}
                () = self.inner.wake.notified() => {}
                changed = links.changed(), if linked => match changed {
                    Ok(()) => {
                        let now_up = links.borrow_and_update().clone();
                        back = now_up.difference(&up).copied().collect();
                        up = now_up;
                    }
                    Err(_) => linked = false,
                },
            }
        }
    }

    pub(super) async fn set_on(&self, task: TaskId, on: bool) -> ActionOutcome {
        if self.hook_task(task).is_none() {
            return refused(NO_HOOK);
        }
        let path = funnel_path(task);
        let turn = self.inner.switch.lock().await;
        let funnel = self.funnel();
        // The path opens before the hook says it's on, and closes after it
        // says it's off.
        if on && let Err(reason) = funnel.open(&path).await {
            drop(turn);
            self.publish().await;
            return ActionOutcome::Refused { reason };
        }
        let edit = self
            .inner
            .edits
            .edit_hook(task, Box::new(move |hook| hook.on = on));
        let edited = edit.await;
        if on != edited.is_ok() {
            close_all(funnel.as_ref(), &[path]).await;
        }
        drop(turn);
        self.publish().await;
        match edited {
            Ok(()) => ActionOutcome::Done,
            Err(reason) => ActionOutcome::Refused { reason },
        }
    }

    pub(super) async fn rotate_token(&self, task: TaskId) -> ActionOutcome {
        if self.hook_task(task).is_none() {
            return refused(NO_HOOK);
        }
        match self.rotate(task).await {
            Ok(url) => ActionOutcome::HookUrl { url },
            Err(reason) => ActionOutcome::Refused { reason },
        }
    }

    pub(super) async fn set_secret(&self, task: TaskId, secret: Secret) -> ActionOutcome {
        let Some(hook) = self.hook_task(task) else {
            return refused(NO_HOOK);
        };
        let value = secret.expose().trim();
        if value.is_empty() {
            return refused("The signing secret is empty.");
        }
        if value.len() > MAX_SECRET {
            return refused("The signing secret is too long.");
        }
        let secret = Secret::new(value.to_owned());
        if let Err(err) = self.inner.secrets.set(task, &secret) {
            tracing::warn!(%err, "couldn't store a webhook's signing secret");
            return refused("This device couldn't store the signing secret.");
        }
        let signed = matches!(&hook.trigger, Trigger::Hook(hook) if hook.signature.is_some());
        if signed {
            let edit = Box::new(|hook: &mut Hook| {
                if let Some(check) = &mut hook.signature {
                    check.has_secret = true;
                }
            });
            if let Err(reason) = self.inner.edits.edit_hook(task, edit).await {
                return ActionOutcome::Refused { reason };
            }
        }
        ActionOutcome::Done
    }

    pub(super) fn list_deliveries(&self, task: TaskId) -> ActionOutcome {
        match self.inner.db.deliveries(task) {
            Ok(deliveries) => ActionOutcome::Deliveries { deliveries },
            Err(err) => {
                tracing::warn!(%err, "couldn't read a webhook's deliveries");
                refused("This device couldn't read the deliveries.")
            }
        }
    }
}

/// Takes paths off the public internet, which only logs a failure.
async fn close_all(funnel: &dyn Funnel, paths: &[String]) {
    for path in paths {
        if let Err(err) = funnel.close(path).await {
            tracing::warn!(%err, "couldn't close a webhook's public path");
        }
    }
}

impl Hooks for Webhooks {
    fn rotate(&self, task: TaskId) -> BoxFuture<'_, Result<Secret, String>> {
        Box::pin(async move {
            let base = self.funnel().base().await?;
            let mut bytes = [0u8; 32];
            SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| "This device couldn't make a token.".to_owned())?;
            let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
            self.inner
                .db
                .put_hook_token(task, &hash(&token))
                .map_err(|err| {
                    tracing::warn!(%err, "couldn't store a webhook's token");
                    "This device couldn't store the token.".to_owned()
                })?;
            Ok(Secret::new(format!("{base}{}/{token}", funnel_path(task))))
        })
    }

    fn forget(&self, task: TaskId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let db = self.inner.db.forget_hook(task);
            let secret = self.inner.secrets.remove(task);
            {
                let mut rates = self.rates();
                rates.minute.forget(task);
                rates.bad_signature.forget(task);
                rates.daily.forget(task);
                rates.filtered_log.forget(task);
                rates.dedupe_log.forget(task);
            }
            let _turn = self.inner.switch.lock().await;
            close_all(self.funnel().as_ref(), &[funnel_path(task)]).await;
            db.map_err(|err| err.to_string())?;
            secret.map_err(|err| err.to_string())
        })
    }
}

/// What every path the Funnel opens starts with.
pub const FUNNEL_PREFIX: &str = "/maxhq/";

/// The public path of a task's hook, which the Funnel opens.
pub fn funnel_path(task: TaskId) -> String {
    format!("{FUNNEL_PREFIX}{task}")
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// Answers `SetHookOn`.
pub async fn set_on(task: TaskId, on: bool) -> ActionOutcome {
    match SERVICE.get() {
        Some(hooks) => hooks.set_on(task, on).await,
        None => tasks::not_built(),
    }
}

/// Answers `RotateHookToken`.
pub async fn rotate_token(task: TaskId) -> ActionOutcome {
    match SERVICE.get() {
        Some(hooks) => hooks.rotate_token(task).await,
        None => tasks::not_built(),
    }
}

/// Answers `SetHookSecret`.
pub async fn set_secret(task: TaskId, secret: Secret) -> ActionOutcome {
    match SERVICE.get() {
        Some(hooks) => hooks.set_secret(task, secret).await,
        None => tasks::not_built(),
    }
}

/// Answers `SetHookAddress`.
pub async fn set_address(address: HookAddress) -> ActionOutcome {
    match SERVICE.get() {
        Some(hooks) => hooks.set_address(address).await,
        None => tasks::not_built(),
    }
}

/// Answers `ListDeliveries`.
pub fn list_deliveries(task: TaskId) -> ActionOutcome {
    match SERVICE.get() {
        Some(hooks) => hooks.list_deliveries(task),
        None => tasks::not_built(),
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pushes to phones. The rules for what alerts are the desktop's, in
//! [`maxhq_core::notify`], run on this device's own agents. On top:
//!
//! - **Hold and drop:** a push waits [`HOLD`], and goes nowhere if what it
//!   was about clears first, such as a prompt answered on the Mac.
//! - **Presence:** a phone says which agent it shows, good for
//!   [`PRESENCE`]. Nothing is pushed to it for that agent.
//! - **Reminder:** a prompt still open [`REMIND`] after its push goes
//!   again, once, in place of the first, so it sounds again.
//! - **Switches:** each phone's token says which kinds it wants.
//!
//! A push says only "An agent needs you" in the clear. The rest is sealed
//! to the phone's push key, as [`seal`] lays out. Each token names the
//! service that delivers it: APNs for an iPhone, FCM for an Android phone.
//! Only the transport differs; the rules, the sealed part and its fit are
//! the same. A token the service calls gone is dropped.

mod apns;
mod fcm;
mod seal;
#[cfg(test)]
pub(crate) mod tests;
mod tokens;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::notify::{Alert, Notification, NotificationSender, Notifier, Panes};
use maxhq_core::store::Store;
use maxhq_protocol::attention::{AttentionEvent, AttentionKind, AttentionOption};
use maxhq_protocol::ids::{AgentId, AttentionId, DeviceId, PhoneId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::Activity;
use maxhq_protocol::phones::{PushKind, PushService, PushToken};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

pub use apns::Apns;
pub use fcm::Fcm;

use crate::hub::{Hub, Start};
use crate::secrets::files::Folder;
use crate::secrets::{ApnsKey, FcmKey};
use crate::session::clock::Clock;
use apns::Request;
use tokens::Tokens;

/// How long a push waits, in case what it's about clears first.
pub const HOLD: Duration = Duration::from_secs(5);
/// How long after its push a prompt still open goes again.
pub const REMIND: Duration = Duration::from_secs(180);
/// How long a phone's presence counts. It sends one every 25 seconds.
pub const PRESENCE: Duration = Duration::from_secs(45);
/// The push folder under `maxd`'s data folder.
pub const DIR: &str = "push";
/// The refusal for a phone's own request from a connection that isn't one.
pub const ONLY_PHONES: &str = "Only a phone sends that.";

/// What Apple and the path see. An Android phone shows the same when it
/// can't open the sealed part.
const TITLE: &str = "MaxHQ";
const BODY: &str = "An agent needs you";
/// The most the sealed part takes before base64, which keeps the whole
/// push well under APNs's 4 KB, and FCM's 4 KB of data.
const SEALED_MAX: usize = 2_400;
/// The longest title and subtitle, in bytes, before the fit.
const LINE_MAX: usize = 200;
/// The most options a push carries.
const OPTIONS_MAX: usize = 8;
/// The longest option ID or label a push carries. A prompt with a longer
/// one goes without its options, and the phone opens the app instead.
const OPTION_MAX: usize = 64;

/// Where a service's key comes from.
pub type KeySource<K> = Arc<dyn Fn() -> Option<Arc<K>> + Send + Sync>;

/// The services pushes go through, each with where its key comes from.
pub struct Services {
    pub apns: Apns,
    pub apns_key: KeySource<ApnsKey>,
    pub fcm: Fcm,
    pub fcm_key: KeySource<FcmKey>,
}

/// How a service answered a push.
#[derive(Debug, PartialEq, Eq)]
pub enum Sent {
    /// The service took it.
    Done,
    /// The token isn't the phone's any more, so it goes.
    Gone,
    /// The service refused this device's token for it: APNs's provider
    /// token or FCM's access token. The next push makes a new one.
    Expired,
    /// Anything else, in words.
    Failed(String),
}

/// The push service. Clones share it.
#[derive(Clone)]
pub struct Push {
    inner: Arc<Inner>,
}

struct Inner {
    hub: Hub,
    clock: Arc<dyn Clock>,
    services: Services,
    state: Mutex<State>,
}

struct State {
    tokens: Tokens,
    /// What each phone said it shows last, and when.
    presence: HashMap<PhoneId, (Option<AgentId>, Instant)>,
}

/// The service the free functions reach, once `maxd` has started it.
static PUSH: OnceLock<Push> = OnceLock::new();

impl Push {
    /// Starts with the tokens `folder` holds.
    pub fn open(hub: Hub, folder: Folder, clock: Arc<dyn Clock>, services: Services) -> Self {
        Self {
            inner: Arc::new(Inner {
                hub,
                clock,
                services,
                state: Mutex::new(State {
                    tokens: Tokens::open(folder),
                    presence: HashMap::new(),
                }),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers `SetPushToken` from `phone`'s own connection.
    pub fn set_token(&self, phone: PhoneId, push: PushToken) -> ActionOutcome {
        if let Err(reason) = check(&push) {
            return refused(reason);
        }
        let kinds = push.kinds.len();
        let push_service = push.service;
        if let Err(err) = self.lock().tokens.set(phone, push) {
            warn!(%phone, "couldn't save a phone's push token: {err}");
            return refused(format!("This device couldn't save the token: {err}"));
        }
        info!(%phone, kinds, service = name(push_service), "kept a phone's push token");
        ActionOutcome::Done
    }

    /// Takes a `Presence` from `phone`'s own connection.
    pub fn presence(&self, phone: PhoneId, agent: Option<AgentId>) {
        let now = self.inner.clock.now();
        self.lock().presence.insert(phone, (agent, now));
    }

    /// Forgets `phone`'s token and presence, as when it's removed.
    pub fn forget(&self, phone: PhoneId) {
        let mut state = self.lock();
        state.presence.remove(&phone);
        if state.tokens.remove(phone, None) {
            info!(%phone, "dropped a phone's push token");
        }
    }

    /// Watches the hub and pushes what alerts, until the hub stops.
    pub async fn watch(self) {
        let mut watch = Watch::new(self.clone());
        let mut quiet = true;
        loop {
            let subscription = self.inner.hub.subscribe(None);
            match subscription.start {
                Start::Snapshot(snapshot) => watch.reset(snapshot.entities, quiet),
                Start::Deltas(deltas) => {
                    for delta in deltas {
                        watch.observe(delta.changes);
                    }
                }
            }
            // Only the first state is what was already waiting when `maxd`
            // started, which the phone heard about from the run before.
            quiet = false;
            let mut deltas = subscription.deltas;
            loop {
                let wake = watch.next();
                let clock = self.inner.clock.clone();
                let sleep = async move {
                    match wake {
                        Some(at) => clock.sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    got = deltas.recv() => match got {
                        Ok(delta) => watch.observe(delta.changes),
                        // Missed deltas: start again from the whole state.
                        Err(RecvError::Lagged(_)) => break,
                        Err(RecvError::Closed) => return,
                    },
                    () = sleep => {}
                }
                for out in watch.due() {
                    let push = self.clone();
                    tokio::spawn(async move {
                        push.send(out).await;
                    });
                }
            }
        }
    }

    /// Sends one push, dropping its token if the service says it's gone,
    /// and making a new token for the service once if it refused this one.
    async fn send(&self, out: Outgoing) -> Sent {
        let mut sent = self.send_once(&out).await;
        if sent == Sent::Expired {
            out.via.expire();
            sent = self.send_once(&out).await;
        }
        let service = name(out.token.service);
        match &sent {
            Sent::Done => info!(phone = %out.phone, service, "pushed to a phone"),
            Sent::Gone => {
                if self.lock().tokens.remove(out.phone, Some(&out.token)) {
                    info!(phone = %out.phone, "dropped a phone's push token, since {service} says it's gone");
                }
            }
            Sent::Expired => warn!(phone = %out.phone, "{service} refused a new token"),
            Sent::Failed(reason) => {
                warn!(phone = %out.phone, service, "couldn't push to a phone: {reason}")
            }
        }
        sent
    }

    async fn send_once(&self, out: &Outgoing) -> Sent {
        let now = self.inner.clock.wall();
        let services = &self.inner.services;
        match &out.via {
            Via::Apns {
                key,
                payload,
                collapse,
            } => {
                let request = Request { payload, collapse };
                services.apns.send(key, now, &out.token, request).await
            }
            Via::Fcm { key, data } => services.fcm.send(key, now, &out.token, data).await,
        }
    }

    fn device(&self) -> DeviceId {
        self.inner.hub.device().id
    }

    /// Whether `phone` shows `agent` now.
    fn shows(&self, phone: PhoneId, agent: AgentId, now: Instant) -> bool {
        self.lock().presence.get(&phone).is_some_and(|(shown, at)| {
            *shown == Some(agent) && now.saturating_duration_since(*at) < PRESENCE
        })
    }
}

/// One push to one phone, ready to send.
struct Outgoing {
    phone: PhoneId,
    token: PushToken,
    via: Via,
}

/// The service a push goes through, with its key and what it carries.
enum Via {
    Apns {
        key: Arc<ApnsKey>,
        payload: String,
        collapse: String,
    },
    Fcm {
        key: Arc<FcmKey>,
        data: fcm::Data,
    },
}

impl Via {
    /// Drops the token the key made for the service, so the next send
    /// makes a new one.
    fn expire(&self) {
        match self {
            Self::Apns { key, .. } => key.expire(),
            Self::Fcm { key, .. } => key.expire(),
        }
    }
}

#[cfg(test)]
impl Outgoing {
    /// The request body the service gets.
    fn body(&self) -> String {
        match &self.via {
            Via::Apns { payload, .. } => payload.clone(),
            Via::Fcm { data, .. } => fcm::body(&self.token, data),
        }
    }
}

/// What the notifier said, for [`Watch`] to take.
enum Said {
    Alert(Notification, Alert),
    Remove(Vec<String>),
}

/// Collects what the notifier says.
#[derive(Clone, Default)]
struct Collect(Arc<Mutex<Vec<Said>>>);

impl Collect {
    fn take(&self) -> Vec<Said> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn push(&self, said: Said) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(said);
    }
}

impl NotificationSender for Collect {
    // Only alerts about agents reach phones.
    fn deliver(&self, _: &Notification) {}

    // The kind is worked out again when the push goes, from the prompt as
    // it is then.
    fn alert(&self, notification: &Notification, alert: Alert, _: PushKind) {
        self.push(Said::Alert(notification.clone(), alert));
    }

    fn remove(&self, ids: &[String]) {
        self.push(Said::Remove(ids.to_vec()));
    }

    // A phone keeps what it was sent; there's nothing to take back here.
    fn remove_all(&self) {}
}

/// A push waiting out its hold, or a prompt's reminder waiting its turn.
struct Held {
    notification: Notification,
    alert: Alert,
    due: Instant,
    /// For a reminder, the phones the first push went to.
    reminder: Option<Vec<PhoneId>>,
}

/// The rules, run on a copy of the hub's state.
struct Watch {
    push: Push,
    store: Store,
    notifier: Notifier,
    said: Collect,
    held: Vec<Held>,
}

impl Watch {
    fn new(push: Push) -> Self {
        let said = Collect::default();
        Self {
            push,
            store: Store::new(0),
            notifier: Notifier::new(Box::new(said.clone())),
            said,
            held: Vec::new(),
        }
    }

    /// Takes the whole state. Alerts it raises are dropped when `quiet`.
    fn reset(&mut self, entities: Vec<Entity>, quiet: bool) {
        if let Some(delta) = self.store.replace(|_| true, entities) {
            self.notifier
                .observe(&delta, &self.store, &Panes::default());
        }
        if quiet {
            self.said.take();
        } else {
            self.take();
        }
    }

    /// Takes a change to the state.
    fn observe(&mut self, changes: Vec<Change>) {
        if let Some(delta) = self.store.apply(changes) {
            self.notifier
                .observe(&delta, &self.store, &Panes::default());
        }
        self.take();
    }

    /// Holds new alerts and drops held ones that cleared.
    fn take(&mut self) {
        for said in self.said.take() {
            match said {
                Said::Alert(notification, alert) => {
                    self.held
                        .retain(|held| held.notification.id != notification.id);
                    self.held.push(Held {
                        notification,
                        alert,
                        due: self.push.inner.clock.now() + HOLD,
                        reminder: None,
                    });
                }
                Said::Remove(ids) => self
                    .held
                    .retain(|held| !ids.contains(&held.notification.id)),
            }
        }
    }

    /// When the next held push is due.
    fn next(&self) -> Option<Instant> {
        self.held.iter().map(|held| held.due).min()
    }

    /// The pushes whose hold is over, one for each phone that takes it. A
    /// prompt's first push leaves its reminder held.
    fn due(&mut self) -> Vec<Outgoing> {
        let now = self.push.inner.clock.now();
        let (due, held): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|held| held.due <= now);
        self.held = held;
        let mut all = Vec::new();
        for held in due {
            let out = self.outgoing(&held, now);
            if held.reminder.is_none()
                && matches!(held.alert, Alert::NeedsInput(_))
                && !out.is_empty()
            {
                self.held.push(Held {
                    reminder: Some(out.iter().map(|out| out.phone).collect()),
                    due: now + REMIND,
                    ..held
                });
            }
            all.extend(out);
        }
        all
    }

    /// The pushes for one alert.
    fn outgoing(&self, held: &Held, now: Instant) -> Vec<Outgoing> {
        let notification = &held.notification;
        let agent = notification.agent;
        let device = self.push.device();
        if !self.on(agent, device) {
            return Vec::new();
        }
        let services = &self.push.inner.services;
        let apns = (services.apns_key)();
        let firebase = (services.fcm_key)();
        if apns.is_none() && firebase.is_none() {
            debug!("no push key, so no push");
            return Vec::new();
        }
        let attention = match held.alert {
            Alert::NeedsInput(id) => match self.store.get(&EntityKey::Attention(id)) {
                Some(Entity::Attention(event)) => Some(event),
                _ => None,
            },
            _ => None,
        };
        // A reminder goes only while its prompt is open.
        if held.reminder.is_some() && !self.waits(agent, attention) {
            return Vec::new();
        }
        let kind = held.alert.kind(attention.map(|event| event.kind));
        let prompt = match held.alert {
            Alert::NeedsInput(id) => Some(id),
            _ => None,
        };
        let message = Message {
            title: clip(&notification.title, LINE_MAX),
            subtitle: notification
                .subtitle
                .as_deref()
                .map(|line| clip(line, LINE_MAX)),
            body: notification.body.clone(),
            agent,
            device,
            attention: prompt,
            options: attention.map(options).unwrap_or_default(),
        };
        let plaintext = message.fit();
        // A request only its card may answer comes with no options, and
        // its push with no Deny button.
        let permission = attention.is_some_and(|event| {
            event.kind == AttentionKind::Permission && !event.options.is_empty()
        });

        let tokens: Vec<(PhoneId, PushToken)> = self
            .push
            .lock()
            .tokens
            .iter()
            .filter(|(_, push)| push.kinds.contains(&kind))
            .map(|(phone, push)| (*phone, push.clone()))
            .collect();
        let mut out = Vec::new();
        for (phone, token) in tokens {
            if self.push.shows(phone, agent, now)
                || held
                    .reminder
                    .as_ref()
                    .is_some_and(|phones| !phones.contains(&phone))
            {
                continue;
            }
            let has_key = match token.service {
                PushService::Apns => apns.is_some(),
                PushService::Fcm => firebase.is_some(),
            };
            if !has_key {
                debug!(%phone, service = name(token.service), "no key for the phone's service, so no push");
                continue;
            }
            let Some(public) = self.phone_key(phone, device) else {
                debug!(%phone, "no push key for the phone, so no push");
                continue;
            };
            let Ok(sealed) = seal::seal(&public, &plaintext) else {
                warn!(%phone, "couldn't seal a push");
                continue;
            };
            let sealed = STANDARD.encode(sealed);
            let collapse = notification.id.clone();
            let via = match (token.service, &apns, &firebase) {
                (PushService::Apns, Some(key), _) => Via::Apns {
                    key: key.clone(),
                    payload: payload(agent, prompt, permission, &sealed),
                    collapse,
                },
                (PushService::Fcm, _, Some(key)) => Via::Fcm {
                    key: key.clone(),
                    data: fcm::Data {
                        c: sealed,
                        e: prompt.map(|prompt| prompt.to_string()),
                        t: agent.to_string(),
                        k: kind,
                        a: permission.then_some("permission"),
                        n: collapse,
                    },
                },
                _ => continue,
            };
            out.push(Outgoing { phone, token, via });
        }
        out
    }

    /// Whether `agent` still waits on the prompt `attention`.
    fn waits(&self, agent: AgentId, attention: Option<&AttentionEvent>) -> bool {
        let Some(attention) = attention else {
            return false;
        };
        matches!(
            self.store.get(&EntityKey::Agent(agent)),
            Some(Entity::Agent(agent))
                if matches!(&agent.activity, Activity::NeedsInput { attention: id } if *id == attention.id)
        )
    }

    /// Whether `agent` runs on `device`. A peer's agents are its own to
    /// push.
    fn on(&self, agent: AgentId, device: DeviceId) -> bool {
        let Some(Entity::Agent(agent)) = self.store.get(&EntityKey::Agent(agent)) else {
            return false;
        };
        matches!(
            self.store.get(&EntityKey::Workspace(agent.workspace)),
            Some(Entity::Workspace(workspace)) if workspace.device == device
        )
    }

    /// The push key `phone` paired with, from its entity here.
    fn phone_key(&self, phone: PhoneId, device: DeviceId) -> Option<[u8; seal::KEY_LEN]> {
        let Some(Entity::Phone(phone)) = self.store.get(&EntityKey::Phone { phone, device }) else {
            return None;
        };
        STANDARD.decode(&phone.push_public).ok()?.try_into().ok()
    }
}

/// What the sealed part holds.
#[derive(Serialize)]
struct Message {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<String>,
    body: String,
    agent: AgentId,
    device: DeviceId,
    #[serde(skip_serializing_if = "Option::is_none")]
    attention: Option<AttentionId>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<AttentionOption>,
}

impl Message {
    /// The JSON, shortened until it fits: the body first, then the
    /// options, the subtitle and the title.
    fn fit(mut self) -> Vec<u8> {
        loop {
            let json = serde_json::to_vec(&self).unwrap_or_default();
            if json.len() <= SEALED_MAX {
                return json;
            }
            if !self.body.is_empty() {
                self.body = clip(&self.body, self.body.len() / 2);
            } else if !self.options.is_empty() {
                self.options.clear();
            } else if self.subtitle.take().is_none() {
                self.title = clip(&self.title, self.title.len() / 2);
            }
        }
    }
}

/// A prompt's options, or none when one is too long to carry.
fn options(event: &AttentionEvent) -> Vec<AttentionOption> {
    let fits = event.options.len() <= OPTIONS_MAX
        && event
            .options
            .iter()
            .all(|option| option.id.len() <= OPTION_MAX && option.label.len() <= OPTION_MAX);
    if fits {
        event.options.clone()
    } else {
        Vec::new()
    }
}

/// The push APNs carries. Only the placeholder text is in the clear. A
/// prompt sounds and breaks through Focus, as time-sensitive, and goes to
/// the top of the summary.
fn payload(agent: AgentId, prompt: Option<AttentionId>, permission: bool, sealed: &str) -> String {
    let mut aps = json!({
        "alert": { "title": TITLE, "body": BODY },
        "mutable-content": 1,
        "thread-id": agent.to_string(),
    });
    if prompt.is_some() {
        aps["sound"] = json!("default");
        aps["interruption-level"] = json!("time-sensitive");
        aps["relevance-score"] = json!(1);
    }
    if permission {
        aps["category"] = json!("permission");
    }
    let mut payload = json!({ "aps": aps, "c": sealed });
    if let Some(prompt) = prompt {
        payload["e"] = json!(prompt.to_string());
    }
    payload.to_string()
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Why `push` isn't a token its service delivers to, if it isn't: an
/// APNs device token is hex, an FCM registration token is letters,
/// digits, `_`, `:` and `-`. Neither names the token.
fn check(push: &PushToken) -> Result<(), &'static str> {
    let token = push.token.expose();
    let topic = &push.topic;
    match push.service {
        PushService::Apns => {
            if !(16..=200).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("That isn't an APNs device token.");
            }
            if !name_of(topic, |b| {
                b.is_ascii_alphanumeric() || b == b'.' || b == b'-'
            }) {
                return Err("That isn't an app's bundle ID.");
            }
        }
        PushService::Fcm => {
            let allowed = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-');
            if !(16..=FCM_TOKEN_MAX).contains(&token.len()) || !token.bytes().all(allowed) {
                return Err("That isn't an FCM registration token.");
            }
            if !name_of(topic, |b| {
                b.is_ascii_alphanumeric() || b == b'.' || b == b'_'
            }) {
                return Err("That isn't an Android package name.");
            }
        }
    }
    Ok(())
}

/// The longest FCM registration token taken. Today's are about 160
/// characters; this leaves them room to grow.
const FCM_TOKEN_MAX: usize = 1_024;

/// Whether `text` is an app's ID: up to 255 bytes, each `allowed`.
fn name_of(text: &str, allowed: impl Fn(u8) -> bool) -> bool {
    !text.is_empty() && text.len() <= 255 && text.bytes().all(allowed)
}

/// The service's name, for the log.
fn name(service: PushService) -> &'static str {
    match service {
        PushService::Apns => "APNs",
        PushService::Fcm => "FCM",
    }
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// Makes `push` the service the free functions reach.
pub fn install(push: Push) {
    let _ = PUSH.set(push);
}

/// Answers `SetPushToken` from `phone`'s own connection.
pub fn set_token(phone: PhoneId, push: PushToken) -> ActionOutcome {
    match PUSH.get() {
        Some(service) => service.set_token(phone, push),
        None => refused("This device doesn't push to phones yet."),
    }
}

/// Takes a `Presence` from `phone`'s own connection.
pub fn presence(phone: PhoneId, agent: Option<AgentId>) {
    if let Some(service) = PUSH.get() {
        service.presence(phone, agent);
    }
}

/// Forgets `phone`, as when it's removed.
pub fn forget(phone: PhoneId) {
    if let Some(service) = PUSH.get() {
        service.forget(phone);
    }
}

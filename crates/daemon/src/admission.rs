// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The admission card. When a program that proved no key the device trusts
//! asks for more than such a connection may do, the device asks you whether
//! to let it in.
//!
//! Each connection holds a [`Seat`]. Its first refused request asks for a
//! card, which the device raises after [`Limits::delay`], so a MaxHQ that's
//! only updating has proved its key by then. The card is an
//! [`Admission`] entity: proven desktop apps answer it and phones show it.
//! On a Mac with no proven desktop app connected, the same words show as a
//! macOS alert. A yes lets in that one connection until it disconnects,
//! and nothing is remembered. A card that's still open when its connection
//! closes goes with it, and one nobody answers counts as Don't allow.
//!
//! A program that proved a key and asked the device to trust it gets the
//! same kind of card, and a yes trusts the key.

pub mod alert;
mod facts;
mod words;

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use maxhq_protocol::admission::Admission;
use maxhq_protocol::ids::AdmissionId;
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, oneshot};
use tokio::task::AbortHandle;
use tracing::{info, warn};

use self::alert::{Alert, Cancel, Reply};
use self::facts::{Caller, Facts};
use self::words::{Host, Words};
use crate::app_keys::Decision;
use crate::codesign;
use crate::hub::Hub;

/// How long the device waits and how many cards it keeps open.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long a program waits for its card, so one that proves its key
    /// in that time never gets one.
    pub delay: Duration,
    /// How long a card waits for an answer before it counts as Don't
    /// allow.
    pub expiry: Duration,
    /// How long the macOS alert stays quiet after a Don't allow.
    pub quiet: Duration,
    /// The most cards open at once. Programs past that are refused with no
    /// card.
    pub cap: usize,
}

/// The limits the device keeps.
pub const LIMITS: Limits = Limits {
    delay: Duration::from_secs(10),
    expiry: Duration::from_secs(10 * 60),
    quiet: Duration::from_secs(10 * 60),
    cap: 3,
};

/// What a program asks the device for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// To do what the app does until it disconnects.
    Admit,
    /// To trust `public_key`, which it proved it holds, under `label`.
    Enroll {
        /// The key, in its canonical form.
        public_key: String,
        /// What the program calls it.
        label: String,
    },
}

/// What came of asking for a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// A card is open, or will be once the delay passes.
    Open,
    /// Too many cards are open, so none was raised.
    Full,
    /// This connection's card was answered or timed out already.
    Ended(Ended),
}

/// How a connection's card ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// You allowed it.
    Allowed,
    /// You didn't, or nobody answered in time.
    Denied,
}

/// What came of answering a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The answer took.
    Done,
    /// The connection the card is about tried to answer it.
    Own,
    /// No such card is open.
    Gone,
    /// The app keys file couldn't be written, so the card stays open.
    Unread,
}

/// Trusts a key you allowed: its public key and label.
type Enroll = Box<dyn Fn(&str, &str) -> io::Result<Decision> + Send + Sync>;

/// Every connection's card, shared by the device's connections.
#[derive(Clone)]
pub struct Admissions(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    enroll: Enroll,
}

struct State {
    limits: Limits,
    /// Where cards show when no proven desktop app is connected, or none
    /// off the Mac and in tests.
    alert: Option<Arc<dyn Alert>>,
    /// Where the alert's blocking call runs, from the first seat.
    runtime: Option<Handle>,
    /// The next seat's number.
    next: u64,
    /// The cards, raised or waiting out the delay, oldest first.
    cards: Vec<Card>,
    /// The seats refused a card because too many were open.
    waiting: BTreeSet<u64>,
    /// The seats of proven desktop apps, with their process IDs.
    apps: HashMap<u64, Option<i32>>,
    /// Until when the alert stays quiet.
    quiet_until: Option<Instant>,
    /// The card the alert shows, and how to close it.
    showing: Option<(AdmissionId, Cancel)>,
}

struct Card {
    id: AdmissionId,
    seat: u64,
    hub: Hub,
    ask: Ask,
    link: Arc<Link>,
    /// Where an enrolled key goes, to the connection that asked.
    approve: Option<oneshot::Sender<String>>,
    raised: Option<Raised>,
    /// Whether the alert has shown it.
    alerted: bool,
    task: Option<AbortHandle>,
}

struct Raised {
    facts: Facts,
    at: Timestamp,
    expires: Timestamp,
    deadline: Instant,
}

/// What a seat and its card share.
#[derive(Default)]
struct Link {
    /// Whether you let the connection in.
    admitted: AtomicBool,
    /// How its card ended, once it has.
    ended: OnceLock<Ended>,
}

impl Admissions {
    /// No cards yet. `enroll` trusts a key you allow.
    pub fn new(enroll: Enroll) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                limits: LIMITS,
                alert: alert::platform(),
                runtime: None,
                next: 0,
                cards: Vec::new(),
                waiting: BTreeSet::new(),
                apps: HashMap::new(),
                quiet_until: None,
                showing: None,
            }),
            enroll,
        }))
    }

    /// A seat for a connection from the process `pid`, whose audit token
    /// was `token` as it connected.
    pub fn seat(&self, pid: Option<i32>, token: Option<codesign::Token>) -> Seat {
        let mut state = self.lock();
        if state.runtime.is_none() {
            state.runtime = Handle::try_current().ok();
        }
        let id = state.next;
        state.next += 1;
        Seat {
            id,
            admissions: self.clone(),
            link: Arc::default(),
            pid,
            token,
            says: None,
            approved: None,
            revoked: None,
        }
    }

    /// Short limits and a fake alert, for tests.
    #[cfg(test)]
    pub fn set_for_tests(&self, limits: Limits, alert: Option<Arc<dyn Alert>>) {
        let mut state = self.lock();
        state.limits = limits;
        state.alert = alert;
    }

    /// Answers card `id` from the connection on seat `by`.
    pub async fn answer(&self, id: AdmissionId, allow: bool, by: u64) -> Answer {
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.settle(id, allow, Some(by)))
            .await
            .unwrap_or(Answer::Unread)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ask(&self, seat: &mut Seat, hub: &Hub, ask: Ask, phone: bool) -> Asked {
        if let Some(ended) = seat.link.ended.get() {
            return Asked::Ended(*ended);
        }
        let mut guard = self.lock();
        let state = &mut *guard;
        let waiting = state.waiting.len();
        if let Some(card) = state.cards.iter_mut().find(|card| card.seat == seat.id) {
            if let Ask::Enroll { .. } = ask
                && card.ask != ask
            {
                card.ask = ask;
                let (approve, approved) = oneshot::channel();
                card.approve = Some(approve);
                seat.approved = Some(approved);
                publish(card, waiting);
                // The alert shows what was asked before, so it shows again.
                if card.alerted {
                    card.alerted = false;
                    let id = card.id;
                    if let Some((_, cancel)) = state.showing.take_if(|(shown, _)| *shown == id) {
                        cancel.cancel();
                    }
                }
            }
            return Asked::Open;
        }
        if state.cards.len() >= state.limits.cap {
            if state.waiting.insert(seat.id) {
                republish(state);
            }
            return Asked::Full;
        }
        if state.waiting.remove(&seat.id) {
            republish(state);
        }
        let id = AdmissionId::generate();
        let approve = match ask {
            Ask::Enroll { .. } => {
                let (approve, approved) = oneshot::channel();
                seat.approved = Some(approved);
                Some(approve)
            }
            Ask::Admit => None,
        };
        let caller = Caller {
            pid: seat.pid,
            token: seat.token,
            says: seat.says.clone(),
            phone,
            asked_at: Timestamp::now(),
        };
        info!(%id, says = ?caller.says, "a program asked for more than an unproven connection may do");
        let limits = state.limits;
        let task = state.runtime.as_ref().map(|runtime| {
            runtime
                .spawn(self.clone().wait(id, caller, limits))
                .abort_handle()
        });
        state.cards.push(Card {
            id,
            seat: seat.id,
            hub: hub.clone(),
            ask,
            link: seat.link.clone(),
            approve,
            raised: None,
            alerted: false,
            task,
        });
        Asked::Open
    }

    /// Waits out the delay, raises card `id`, and counts it as Don't allow
    /// once it expires.
    async fn wait(self, id: AdmissionId, caller: Caller, limits: Limits) {
        tokio::time::sleep(limits.delay).await;
        let apps: Vec<i32> = self.lock().apps.values().flatten().copied().collect();
        let Ok(facts) = tokio::task::spawn_blocking(move || Facts::find(&caller, &apps)).await
        else {
            return;
        };
        if !self.raise(id, facts, limits.expiry) {
            return;
        }
        tokio::time::sleep(limits.expiry).await;
        info!(%id, "nobody answered an admission card in time, so it counts as Don't allow");
        self.settle(id, false, None);
    }

    fn raise(&self, id: AdmissionId, facts: Facts, expiry: Duration) -> bool {
        let mut guard = self.lock();
        let state = &mut *guard;
        let waiting = state.waiting.len();
        let Some(card) = state.cards.iter_mut().find(|card| card.id == id) else {
            return false;
        };
        let at = Timestamp::now();
        card.raised = Some(Raised {
            facts,
            at,
            expires: at.after(expiry),
            deadline: Instant::now() + expiry,
        });
        publish(card, waiting);
        self.alert_next(state);
        true
    }

    /// Answers card `id`, from seat `by` or, with none, from the alert or
    /// the clock.
    fn settle(&self, id: AdmissionId, allow: bool, by: Option<u64>) -> Answer {
        let mut guard = self.lock();
        let state = &mut *guard;
        let Some(index) = state
            .cards
            .iter()
            .position(|card| card.id == id && card.raised.is_some())
        else {
            return Answer::Gone;
        };
        if by == Some(state.cards[index].seat) {
            return Answer::Own;
        }
        if allow && let Ask::Enroll { public_key, label } = &state.cards[index].ask {
            match (self.0.enroll)(public_key, label) {
                Ok(decision) => info!(?decision, %public_key, "trusts an app key you allowed"),
                Err(err) => {
                    warn!(%err, "couldn't add an app key you allowed");
                    return Answer::Unread;
                }
            }
        }
        let mut card = state.cards.remove(index);
        let ended = if allow { Ended::Allowed } else { Ended::Denied };
        let _ = card.link.ended.set(ended);
        if allow {
            // In at once, before its connection hears of the key, so
            // nothing it sends after your yes is refused.
            card.link.admitted.store(true, Ordering::SeqCst);
            if let (Ask::Enroll { public_key, .. }, Some(approve)) =
                (&card.ask, card.approve.take())
            {
                let _ = approve.send(public_key.clone());
            }
        } else {
            state.quiet_until = Some(Instant::now() + state.limits.quiet);
        }
        info!(%id, allow, "an admission card was answered");
        close(state, card);
        self.alert_next(state);
        Answer::Done
    }

    /// Shows the oldest raised card the alert hasn't shown, when no proven
    /// desktop app is connected, no alert is up and the alert isn't quiet.
    fn alert_next(&self, state: &mut State) {
        if state.showing.is_some() || !state.apps.is_empty() {
            return;
        }
        let (Some(alert), Some(runtime)) = (state.alert.clone(), state.runtime.clone()) else {
            return;
        };
        if state
            .quiet_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let waiting = state.waiting.len();
        let Some(card) = state
            .cards
            .iter_mut()
            .find(|card| card.raised.is_some() && !card.alerted)
        else {
            return;
        };
        let Some(raised) = &card.raised else {
            return;
        };
        card.alerted = true;
        let id = card.id;
        let words = words_for(card, raised, waiting);
        let wait = raised.deadline.saturating_duration_since(Instant::now());
        let cancel = Cancel::default();
        state.showing = Some((id, cancel.clone()));
        let me = self.clone();
        runtime.spawn_blocking(move || {
            let reply = alert.ask(&words, wait, &cancel);
            me.alert_replied(id, reply);
        });
    }

    fn alert_replied(&self, id: AdmissionId, reply: Reply) {
        let mut state = self.lock();
        let _ = state.showing.take_if(|(shown, _)| *shown == id);
        let allow = match reply {
            Reply::Allow => true,
            Reply::DontAllow => false,
            Reply::Closed => {
                self.alert_next(&mut state);
                return;
            }
        };
        drop(state);
        self.settle(id, allow, None);
    }

    /// The connection on `seat` closed: its card goes, and it no longer
    /// waits or counts as an app.
    fn leave(&self, seat: u64) {
        let mut guard = self.lock();
        let state = &mut *guard;
        state.apps.remove(&seat);
        if let Some(index) = state.cards.iter().position(|card| card.seat == seat) {
            let card = state.cards.remove(index);
            info!(id = %card.id, "dropped an admission card, since its program disconnected");
            close(state, card);
        }
        if state.waiting.remove(&seat) {
            republish(state);
        }
        self.alert_next(state);
    }

    /// The connection on `seat` proved a key the device trusts: its card
    /// goes, and a desktop app's connection keeps the alert away.
    fn proven(&self, seat: u64, pid: Option<i32>, phone: bool) {
        let mut guard = self.lock();
        let state = &mut *guard;
        if !phone {
            state.apps.insert(seat, pid);
            // The app shows the card from now on.
            if let Some((id, cancel)) = state.showing.take() {
                cancel.cancel();
                if let Some(card) = state.cards.iter_mut().find(|card| card.id == id) {
                    card.alerted = false;
                }
            }
        }
        if let Some(index) = state.cards.iter().position(|card| card.seat == seat) {
            let card = state.cards.remove(index);
            close(state, card);
        }
        if state.waiting.remove(&seat) {
            republish(state);
        }
    }
}

/// The words of `card`, raised as `raised`, with `waiting` programs
/// refused a card.
fn words_for(card: &Card, raised: &Raised, waiting: usize) -> Words {
    let host = Host::here(&card.hub.device().name);
    words::card(&host, &card.ask, &raised.facts, waiting)
}

/// Publishes `card` as it is now, once it's raised.
fn publish(card: &Card, waiting: usize) {
    let Some(raised) = &card.raised else {
        return;
    };
    let words = words_for(card, raised, waiting);
    let admission = Admission {
        id: card.id,
        device: card.hub.device().id,
        question: words.question(card.id),
        raised_at: raised.at,
        expires_at: raised.expires,
    };
    card.hub.apply([Change::Upsert {
        value: Entity::Admission(admission),
    }]);
}

/// Publishes every raised card again, since how many wait changed.
fn republish(state: &State) {
    for card in &state.cards {
        publish(card, state.waiting.len());
    }
}

/// Takes down `card`, which is out of the list: it stops waiting, leaves
/// the state, and leaves the screen if the alert shows it.
fn close(state: &mut State, card: Card) {
    if let Some(task) = &card.task {
        task.abort();
    }
    if card.raised.is_some() {
        card.hub.apply([Change::Remove {
            key: EntityKey::Admission(card.id),
        }]);
    }
    if let Some((_, cancel)) = state.showing.take_if(|(shown, _)| *shown == card.id) {
        cancel.cancel();
    }
}

/// One connection's place: its card, and whether it was let in.
pub struct Seat {
    id: u64,
    admissions: Admissions,
    link: Arc<Link>,
    pid: Option<i32>,
    token: Option<codesign::Token>,
    /// The name and version from its hello.
    says: Option<String>,
    /// Where the key you allowed comes, while a card asks to trust one.
    approved: Option<oneshot::Receiver<String>>,
    /// Each app key removed, from when the connection asked to trust one.
    revoked: Option<broadcast::Receiver<String>>,
}

impl Seat {
    /// The seat's number, which says who answers a card.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Notes what the connection's hello says it is.
    pub fn hello(&mut self, name: &str, version: &str) {
        self.says = Some(format!("{name} {version}"));
    }

    /// Whether you let the connection in.
    pub fn admitted(&self) -> bool {
        self.link.admitted.load(Ordering::SeqCst)
    }

    /// Asks for a card about `ask`, for a connection on `hub`, which is a
    /// phone's when `phone`.
    pub fn ask(&mut self, hub: &Hub, ask: Ask, phone: bool) -> Asked {
        let admissions = self.admissions.clone();
        admissions.ask(self, hub, ask, phone)
    }

    /// Keeps `revoked` until a key you allow arrives.
    pub fn keep(&mut self, revoked: broadcast::Receiver<String>) {
        self.revoked = Some(revoked);
    }

    /// What [`Seat::keep`] kept.
    pub fn take_revoked(&mut self) -> Option<broadcast::Receiver<String>> {
        self.revoked.take()
    }

    /// The key you allowed this connection's card to trust. Pending until
    /// then, and safe to drop and call again.
    pub async fn approved(&mut self) -> String {
        loop {
            let Some(approved) = &mut self.approved else {
                return std::future::pending().await;
            };
            match approved.await {
                Ok(key) => {
                    self.approved = None;
                    return key;
                }
                Err(_) => self.approved = None,
            }
        }
    }

    /// The connection proved a key the device trusts. It's a phone's or a
    /// pairing's, not the desktop app's, when `phone`.
    pub fn proven(&self, phone: bool) {
        self.admissions.proven(self.id, self.pid, phone);
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.admissions.leave(self.id);
    }
}

#[cfg(test)]
mod tests;

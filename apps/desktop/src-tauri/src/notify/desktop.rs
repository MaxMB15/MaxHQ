// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Notifications on Linux and Windows.
//!
//! On Linux they go to the desktop's notification server over D-Bus. What
//! the server can do is asked once, when the app starts, and whether one
//! answered at all is what the settings page shows as the permission. Some
//! servers send a click only to the D-Bus connection that showed the
//! notification, so each one gets a thread that shows it on its own
//! connection and waits there until it's clicked or closed. Taking one off
//! screen goes by the id the server gave it.
//!
//! On Windows they're toasts for the app's AppUserModelID, which report
//! clicks while the app runs. What Windows Settings allows can change while
//! the app runs, so the permission asks it each time.
//!
//! The work runs in order on a thread of its own, since each call into the
//! server waits for its answer and the notifier calls the sender from async
//! code.

use std::sync::mpsc;
use std::thread;

use maxhq_app::notify::{Notification, NotificationSender};
use maxhq_protocol::ids::AgentId;
use tokio::sync::watch;

#[cfg(target_os = "linux")]
use self::linux as system;
#[cfg(windows)]
use self::windows as system;
use super::content::{Abilities, Content};
use super::NotificationPermission;

/// What a click calls with the agent it was about.
pub type OnClick = Box<dyn Fn(AgentId) + Send + Sync>;

/// Shows notifications on this desktop.
pub struct Desktop {
    jobs: mpsc::Sender<Job>,
    /// What the desktop can do, once the thread has asked it.
    abilities: watch::Receiver<Option<Abilities>>,
}

/// What the thread does next.
enum Job {
    Show(Notification),
    Remove(Vec<String>),
    RemoveAll,
}

impl Desktop {
    /// Starts the thread, which asks the desktop what it can do first.
    pub fn start(on_click: OnClick) -> Self {
        let (jobs, queue) = mpsc::channel();
        let (found, abilities) = watch::channel(None);
        let started = thread::Builder::new()
            .name("notifications".into())
            .spawn(move || work(queue, found, on_click));
        if let Err(error) = started {
            // The thread's half of the watch went with it, so the
            // permission reads as unavailable.
            tracing::warn!(%error, "couldn't start the notification thread, so none will show");
        }
        Self { jobs, abilities }
    }

    /// Whether notifications show, waiting for the desktop's answer if the
    /// thread hasn't had it yet.
    pub async fn permission(&self) -> NotificationPermission {
        let mut abilities = self.abilities.clone();
        let found = abilities
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|found| *found);
        let permission = found.unwrap_or_default().permission();
        #[cfg(windows)]
        if permission == NotificationPermission::Allowed {
            return system::permission();
        }
        permission
    }

    fn send(&self, job: Job) {
        // This fails only when the thread never started, which `start`
        // logged.
        let _ = self.jobs.send(job);
    }
}

impl NotificationSender for Desktop {
    fn deliver(&self, notification: &Notification) {
        self.send(Job::Show(notification.clone()));
    }

    fn remove(&self, ids: &[String]) {
        self.send(Job::Remove(ids.to_vec()));
    }

    fn remove_all(&self) {
        self.send(Job::RemoveAll);
    }
}

/// The thread: asks what the desktop can do, then does each job in turn.
fn work(queue: mpsc::Receiver<Job>, found: watch::Sender<Option<Abilities>>, on_click: OnClick) {
    let mut screen = system::Screen::new(on_click);
    let abilities = screen.abilities();
    found.send_replace(Some(abilities));
    for job in queue {
        match job {
            Job::Show(notification) if abilities.shows => {
                screen.show(&notification, &Content::new(&notification, abilities));
            }
            Job::Show(notification) => {
                tracing::info!(id = notification.id, "nothing here shows notifications");
            }
            Job::Remove(ids) => screen.remove(&ids),
            Job::RemoveAll => screen.remove_all(),
        }
    }
}

/// The desktop's notification server, over D-Bus.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::Duration;

    use dbus::arg::{PropMap, RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::channel::MatchingReceiver;
    use dbus::message::MatchRule;
    use dbus::Message;
    use maxhq_app::notify::Notification;
    use maxhq_protocol::ids::AgentId;

    use super::{Abilities, Content, OnClick};

    /// The name the server lists the app's notifications under.
    const APP: &str = "MaxHQ";
    /// The sound a prompt plays, from the freedesktop sound theme.
    const SOUND: &str = "message-new-instant";
    /// The action the spec reserves for a click on the notification itself.
    const CLICK: &str = "default";
    /// Where the server answers.
    const SERVER: &str = "org.freedesktop.Notifications";
    const PATH: &str = "/org/freedesktop/Notifications";
    /// The bus itself, which says when the server goes away.
    const BUS: &str = "org.freedesktop.DBus";
    /// How long the server has to answer. The first call can start it,
    /// which takes a moment, but a stuck one mustn't hold up the
    /// notifications after it for long.
    const TIMEOUT: Duration = Duration::from_secs(5);
    /// The longest one wait for a notification's signals lasts. A signal
    /// ends the wait at once, so this only bounds how long the thread
    /// sleeps in one go.
    const QUIET: Duration = Duration::from_secs(3600);

    /// The server's id for each notification on screen, by the app's id.
    type Shown = Arc<Mutex<HashMap<String, u32>>>;
    type Clicked = Arc<dyn Fn(AgentId) + Send + Sync>;

    /// The notifications the app has on screen.
    pub struct Screen {
        on_click: Clicked,
        shown: Shown,
        /// The icon the server shows: the app's own, which the packages
        /// install under the program's name.
        icon: String,
        /// The connection the questions and closes go out on, opened when
        /// first needed. It's dropped after an error, so the next call
        /// connects again.
        bus: Option<Connection>,
    }

    impl Screen {
        pub fn new(on_click: OnClick) -> Self {
            let icon = std::env::current_exe()
                .ok()
                .and_then(|path| Some(path.file_name()?.to_str()?.to_owned()))
                .unwrap_or_default();
            Self {
                on_click: Arc::from(on_click),
                shown: Shown::default(),
                icon,
                bus: None,
            }
        }

        /// Asks the server what it can do. Asking is also what starts an
        /// installed server that isn't running yet, since the bus starts
        /// one on demand.
        pub fn abilities(&mut self) -> Abilities {
            let asked = self.call(|bus| {
                let (capabilities,): (Vec<String>,) = bus
                    .with_proxy(SERVER, PATH, TIMEOUT)
                    .method_call(SERVER, "GetCapabilities", ())?;
                Ok(capabilities)
            });
            match asked {
                Ok(capabilities) => Abilities::server(&capabilities),
                Err(error) => {
                    tracing::info!(%error, "no notification server answered, so notifications only go to the log");
                    Abilities::default()
                }
            }
        }

        /// Shows `notification`, closing the one it replaces first.
        /// Replacing it in place would leave its click with the connection
        /// that showed the first one.
        pub fn show(&mut self, notification: &Notification, content: &Content) {
            let replaced = lock(&self.shown).remove(&notification.id);
            if let Some(server_id) = replaced {
                self.close(server_id);
            }
            let (done, showing) = mpsc::channel();
            let one = One {
                id: notification.id.clone(),
                agent: notification.agent,
                request: Request {
                    summary: content.summary.clone(),
                    body: content.body.clone(),
                    icon: self.icon.clone(),
                    sound: content.sound,
                    click: content.click,
                },
                on_click: self.on_click.clone(),
                shown: self.shown.clone(),
            };
            let started = thread::Builder::new()
                .name("notification".into())
                .spawn(move || one.show_and_wait(done));
            if let Err(error) = started {
                tracing::warn!(%error, "couldn't start a notification's thread");
            }
            // This waits until it's on screen, or failed, so a job after
            // this one can close it.
            let _ = showing.recv();
        }

        pub fn remove(&mut self, ids: &[String]) {
            let closing: Vec<u32> = {
                let mut shown = lock(&self.shown);
                ids.iter().filter_map(|id| shown.remove(id)).collect()
            };
            for server_id in closing {
                self.close(server_id);
            }
        }

        pub fn remove_all(&mut self) {
            let closing: Vec<u32> = lock(&self.shown)
                .drain()
                .map(|(_, server_id)| server_id)
                .collect();
            for server_id in closing {
                self.close(server_id);
            }
        }

        /// Asks the server to close one notification. Its thread then hears
        /// that it closed and ends. The spec lets any connection close any
        /// notification, so this doesn't need the one that showed it.
        fn close(&mut self, server_id: u32) {
            let closed = self.call(|bus| {
                bus.with_proxy(SERVER, PATH, TIMEOUT)
                    .method_call::<(), _, _, _>(SERVER, "CloseNotification", (server_id,))
            });
            if let Err(error) = closed {
                tracing::debug!(%error, server_id, "the notification server didn't close a notification");
            }
        }

        fn call<T>(
            &mut self,
            call: impl FnOnce(&Connection) -> Result<T, dbus::Error>,
        ) -> Result<T, dbus::Error> {
            let bus = match self.bus.take() {
                Some(bus) => bus,
                None => Connection::new_session()?,
            };
            let answer = call(&bus);
            if answer.is_ok() {
                self.bus = Some(bus);
            }
            answer
        }
    }

    /// One notification and what its thread needs.
    struct One {
        id: String,
        agent: AgentId,
        request: Request,
        on_click: Clicked,
        shown: Shown,
    }

    impl One {
        /// Shows the notification on a connection of its own and waits
        /// there until it's clicked or closed, or the server goes away and
        /// takes it along. `done` hears once it's on screen.
        fn show_and_wait(self, done: mpsc::Sender<()>) {
            let over = Arc::new(AtomicBool::new(false));
            // The server's id, which the signals name. It stays 0, which
            // the spec never gives out, until the server answers.
            let mine = Arc::new(AtomicU32::new(0));
            // Listening starts before the notification shows, so nothing
            // the server says about it can arrive before the connection
            // listens.
            let shown = self.listen(&over, &mine).and_then(|bus| {
                let server_id = self.request.send(&bus)?;
                Ok((bus, server_id))
            });
            let (bus, server_id) = match shown {
                Ok(shown) => shown,
                Err(error) => {
                    tracing::warn!(%error, id = self.id, "the notification server didn't show a notification");
                    return;
                }
            };
            mine.store(server_id, Ordering::Release);
            lock(&self.shown).insert(self.id.clone(), server_id);
            let _ = done.send(());
            while !over.load(Ordering::Acquire) {
                if let Err(error) = bus.process(QUIET) {
                    tracing::debug!(%error, id = self.id, "stopped waiting on a notification");
                    break;
                }
            }
            let mut shown = lock(&self.shown);
            if shown.get(&self.id) == Some(&server_id) {
                shown.remove(&self.id);
            }
        }

        /// A connection that hears about this notification: a click, its
        /// closing for any reason, and the server going away.
        fn listen(
            &self,
            over: &Arc<AtomicBool>,
            mine: &Arc<AtomicU32>,
        ) -> Result<Connection, dbus::Error> {
            let bus = Connection::new_session()?;

            let (ended, id_of, on_click, agent) = (
                over.clone(),
                mine.clone(),
                self.on_click.clone(),
                self.agent,
            );
            bus.add_match(
                MatchRule::new_signal(SERVER, "ActionInvoked"),
                move |(id, action): (u32, String), _: &Connection, _: &Message| {
                    if id == id_of.load(Ordering::Acquire) {
                        if action == CLICK {
                            on_click(agent);
                        }
                        // The server takes it off screen after any action.
                        ended.store(true, Ordering::Release);
                    }
                    true
                },
            )?;

            let (ended, id_of) = (over.clone(), mine.clone());
            bus.add_match(
                MatchRule::new_signal(SERVER, "NotificationClosed"),
                move |(id, _reason): (u32, u32), _: &Connection, _: &Message| {
                    if id == id_of.load(Ordering::Acquire) {
                        ended.store(true, Ordering::Release);
                    }
                    true
                },
            )?;

            // The bus sends only the server's name changes, but the name is
            // checked here too, since the rule on this side can't say which
            // name.
            bus.add_match_no_cb(&format!(
                "type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{SERVER}'"
            ))?;
            let ended = over.clone();
            bus.start_receive(
                MatchRule::new_signal(BUS, "NameOwnerChanged"),
                Box::new(move |message: Message, _: &Connection| {
                    if message.read1::<&str>().is_ok_and(|name| name == SERVER) {
                        ended.store(true, Ordering::Release);
                    }
                    true
                }),
            );
            Ok(bus)
        }
    }

    /// What a `Notify` call sends.
    struct Request {
        summary: String,
        body: String,
        icon: String,
        sound: bool,
        click: bool,
    }

    impl Request {
        /// Shows it, giving back the server's id for it. A prompt plays the
        /// sound theme's message sound, and anything else asks the server
        /// to stay quiet, since some play a sound for every notification
        /// otherwise.
        fn send(&self, bus: &Connection) -> Result<u32, dbus::Error> {
            let actions: Vec<&str> = if self.click {
                vec![CLICK, "Open"]
            } else {
                Vec::new()
            };
            let mut hints = PropMap::new();
            if self.sound {
                hints.insert(
                    "sound-name".into(),
                    Variant(Box::new(SOUND.to_owned()) as Box<dyn RefArg>),
                );
            } else {
                hints.insert(
                    "suppress-sound".into(),
                    Variant(Box::new(true) as Box<dyn RefArg>),
                );
            }
            // It replaces nothing, and the server picks when it expires.
            let (server_id,): (u32,) = bus.with_proxy(SERVER, PATH, TIMEOUT).method_call(
                SERVER,
                "Notify",
                (
                    APP,
                    0u32,
                    self.icon.as_str(),
                    self.summary.as_str(),
                    self.body.as_str(),
                    actions,
                    hints,
                    -1i32,
                ),
            )?;
            Ok(server_id)
        }
    }

    fn lock(shown: &Shown) -> MutexGuard<'_, HashMap<String, u32>> {
        shown.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(windows)]
mod windows;

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Notifications on Linux and Windows.
//!
//! On Linux they go to the desktop's notification server over D-Bus. What
//! the server can do is asked once, when the app starts, and whether one
//! answered at all is what the settings page shows as the permission. Some
//! servers send a click only to the D-Bus connection that showed the
//! notification, so one connection shows them all and hears their clicks
//! and closes, and the thread waits on it and on its jobs at once. Taking
//! one off screen goes by the id the server gave it.
//!
//! On Windows they're toasts for the app's AppUserModelID, which report
//! clicks while the app runs. What Windows Settings allows can change while
//! the app runs, so the permission asks it each time.
//!
//! The work runs in order on a thread of its own, since each call into the
//! server waits for its answer and the notifier calls the sender from async
//! code.

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
    jobs: system::Jobs<Job>,
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
        let (jobs, queue) = system::jobs();
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
fn work(queue: system::Queue<Job>, found: watch::Sender<Option<Abilities>>, on_click: OnClick) {
    let mut screen = system::Screen::new(on_click);
    let abilities = screen.abilities();
    found.send_replace(Some(abilities));
    while let Some(job) = screen.next(&queue) {
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
    use std::os::fd::{BorrowedFd, OwnedFd};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use dbus::arg::{PropMap, RefArg, Variant};
    use dbus::blocking::Connection;
    use dbus::channel::{BusType, Channel};
    use dbus::message::MatchRule;
    use dbus::{Message, MessageType};
    use maxhq_app::notify::Notification;
    use maxhq_protocol::ids::AgentId;
    use rustix::event::{EventfdFlags, PollFd, PollFlags};
    use rustix::io::Errno;

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

    /// The thread's queue, whose jobs also wake it from its wait on the
    /// bus.
    pub struct Jobs<T> {
        queue: mpsc::Sender<T>,
        wake: Option<Arc<OwnedFd>>,
    }

    /// The thread's end of the queue.
    pub struct Queue<T> {
        queue: mpsc::Receiver<T>,
        wake: Option<Arc<OwnedFd>>,
    }

    /// The queue, with a counter that each job bumps, which the thread
    /// waits on along with the bus.
    pub fn jobs<T>() -> (Jobs<T>, Queue<T>) {
        let (sender, receiver) = mpsc::channel();
        let wake = rustix::event::eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .inspect_err(|error| {
                tracing::warn!(%error, "couldn't make the notification thread's wake-up, so clicks are heard only when the next notification shows");
            })
            .ok()
            .map(Arc::new);
        let jobs = Jobs {
            queue: sender,
            wake: wake.clone(),
        };
        (
            jobs,
            Queue {
                queue: receiver,
                wake,
            },
        )
    }

    impl<T> Jobs<T> {
        /// Queues `job` and wakes the thread. This fails only when the
        /// thread is gone.
        pub fn send(&self, job: T) -> Result<(), mpsc::SendError<T>> {
            self.queue.send(job)?;
            self.wake();
            Ok(())
        }

        fn wake(&self) {
            if let Some(wake) = &self.wake {
                // This fails only when the counter is full, and a full one
                // wakes the thread anyway.
                let _ = rustix::io::write(wake, &1_u64.to_ne_bytes());
            }
        }
    }

    impl<T> Drop for Jobs<T> {
        fn drop(&mut self) {
            // The queue goes first, so the thread wakes to find it gone and
            // ends, as it did when it waited on the queue alone.
            drop(std::mem::replace(&mut self.queue, mpsc::channel().0));
            self.wake();
        }
    }

    /// The notifications the app has on screen.
    pub struct Screen {
        on_click: OnClick,
        /// What's on screen, by the id the server gave each, which its
        /// signals name.
        shown: HashMap<u32, Shown>,
        /// The icon the server shows: the app's own, which the packages
        /// install under the program's name.
        icon: String,
        /// The one connection the notifications go out on and their clicks
        /// and closes come back on, opened when first needed. It's dropped
        /// once it's lost, so the next call connects again.
        bus: Option<Connection>,
    }

    /// A notification on screen: the app's id for it, and its agent.
    struct Shown {
        id: String,
        agent: AgentId,
    }

    impl Screen {
        pub fn new(on_click: OnClick) -> Self {
            let icon = std::env::current_exe()
                .ok()
                .and_then(|path| Some(path.file_name()?.to_str()?.to_owned()))
                .unwrap_or_default();
            Self {
                on_click,
                shown: HashMap::new(),
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

        /// Shows `notification`, closing the one it replaces first, so it
        /// shows as a new one rather than changing one out of sight.
        pub fn show(&mut self, notification: &Notification, content: &Content) {
            if let Some(server_id) = self.take(&notification.id) {
                self.close(server_id);
            }
            let request = Request {
                summary: content.summary.clone(),
                body: content.body.clone(),
                icon: self.icon.clone(),
                sound: content.sound,
                click: content.click,
            };
            match self.call(|bus| request.send(bus)) {
                Ok(server_id) => {
                    let shown = Shown {
                        id: notification.id.clone(),
                        agent: notification.agent,
                    };
                    self.shown.insert(server_id, shown);
                }
                Err(error) => {
                    tracing::warn!(%error, id = notification.id, "the notification server didn't show a notification");
                }
            }
        }

        pub fn remove(&mut self, ids: &[String]) {
            for id in ids {
                if let Some(server_id) = self.take(id) {
                    self.close(server_id);
                }
            }
        }

        pub fn remove_all(&mut self) {
            let closing: Vec<u32> = self.shown.drain().map(|(server_id, _)| server_id).collect();
            for server_id in closing {
                self.close(server_id);
            }
        }

        /// The next job, hearing the server while it waits for one. None
        /// once the app lets go of the queue.
        pub fn next<T>(&mut self, queue: &Queue<T>) -> Option<T> {
            loop {
                match queue.queue.try_recv() {
                    Ok(job) => return Some(job),
                    Err(mpsc::TryRecvError::Disconnected) => return None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                self.hear();
                let waited = match (&self.bus, &queue.wake) {
                    (Some(bus), Some(wake)) => wait(bus, wake),
                    _ => false,
                };
                if !waited {
                    // With no connection to hear, or no way to wake from
                    // its wait, only a job ends the wait.
                    return queue.queue.recv().ok();
                }
            }
        }

        /// Takes the notification the app knows as `id` off the list,
        /// giving back the server's id for it.
        fn take(&mut self, id: &str) -> Option<u32> {
            let server_id = *self.shown.iter().find(|(_, shown)| shown.id == id)?.0;
            self.shown.remove(&server_id);
            Some(server_id)
        }

        /// Asks the server to close one notification. The spec lets any
        /// connection close any notification, so this works on one shown
        /// before the connection was last lost too.
        fn close(&mut self, server_id: u32) {
            let closed = self.call(|bus| {
                bus.with_proxy(SERVER, PATH, TIMEOUT)
                    .method_call::<(), _, _, _>(SERVER, "CloseNotification", (server_id,))
            });
            if let Err(error) = closed {
                tracing::debug!(%error, server_id, "the notification server didn't close a notification");
            }
        }

        /// Handles all the server has said, without waiting for more.
        fn hear(&mut self) {
            loop {
                let heard = match &self.bus {
                    Some(bus) => bus.channel().blocking_pop_message(Duration::ZERO),
                    None => return,
                };
                match heard {
                    Ok(Some(message)) => self.heard(&message),
                    Ok(None) => {
                        // libdbus closes a lost connection's socket, so
                        // it mustn't be waited on.
                        if self
                            .bus
                            .as_ref()
                            .is_some_and(|bus| !bus.channel().is_connected())
                        {
                            self.lost();
                        }
                        return;
                    }
                    Err(error) => {
                        tracing::debug!(%error, "lost the connection to the notification server");
                        self.lost();
                        return;
                    }
                }
            }
        }

        /// Handles one message: a click, a close, the server going away,
        /// or a call to the app.
        fn heard(&mut self, message: &Message) {
            if message.msg_type() != MessageType::Signal {
                // A ping gets its answer and any other call an error, as
                // libdbus's own dispatch would give, so the caller doesn't
                // wait it out.
                if let (Some(reply), Some(bus)) = (dbus::channel::default_reply(message), &self.bus)
                {
                    let _ = bus.channel().send(reply);
                    bus.channel().flush();
                }
                return;
            }
            let (Some(interface), Some(member)) = (message.interface(), message.member()) else {
                return;
            };
            match (&*interface, &*member) {
                (SERVER, "ActionInvoked") => {
                    // The server takes it off screen after any action.
                    if let Ok((server_id, action)) = message.read2::<u32, &str>()
                        && let Some(shown) = self.shown.remove(&server_id)
                        && action == CLICK
                    {
                        (self.on_click)(shown.agent);
                    }
                }
                (SERVER, "NotificationClosed") => {
                    if let Ok((server_id, _reason)) = message.read2::<u32, u32>() {
                        self.shown.remove(&server_id);
                    }
                }
                (BUS, "NameOwnerChanged") => {
                    // The server that showed them went away and took them
                    // along. One that only just started showed none yet.
                    if let Ok((name, old, _new)) = message.read3::<&str, &str, &str>()
                        && name == SERVER
                        && !old.is_empty()
                    {
                        self.shown.clear();
                    }
                }
                _ => {}
            }
        }

        /// Makes `call` on the connection, connecting first when there's
        /// none. An error the server answered with keeps the connection,
        /// since what it showed still reports back on it.
        fn call<T>(
            &mut self,
            call: impl FnOnce(&Connection) -> Result<T, dbus::Error>,
        ) -> Result<T, dbus::Error> {
            let bus = match self.bus.take() {
                Some(bus) => bus,
                None => connect()?,
            };
            let answer = call(&bus);
            if bus.channel().is_connected() {
                self.bus = Some(bus);
            } else {
                self.lost();
            }
            answer
        }

        /// Drops the connection and forgets what it showed, whose clicks
        /// and closes went with it.
        fn lost(&mut self) {
            self.bus = None;
            self.shown.clear();
        }
    }

    /// A connection to the session bus that hears the server's clicks and
    /// closes, and its going away.
    fn connect() -> Result<Connection, dbus::Error> {
        let mut channel = Channel::get_private(BusType::Session)?;
        // This keeps the connection's socket at hand for the thread's wait.
        channel.set_watch_enabled(true);
        let bus = Connection::from(channel);
        for member in ["ActionInvoked", "NotificationClosed"] {
            bus.add_match_no_cb(&MatchRule::new_signal(SERVER, member).match_str())?;
        }
        // The bus sends only the server's name changes.
        bus.add_match_no_cb(&format!(
            "type='signal',sender='{BUS}',interface='{BUS}',member='NameOwnerChanged',arg0='{SERVER}'"
        ))?;
        Ok(bus)
    }

    /// Waits until the connection has something to read, or room for what
    /// it has to send, or a job comes in. False when it can't wait.
    #[allow(unsafe_code)]
    fn wait(bus: &Connection, wake: &Arc<OwnedFd>) -> bool {
        let watch = bus.channel().watch();
        let mut asked = PollFlags::empty();
        asked.set(PollFlags::IN, watch.read);
        asked.set(PollFlags::OUT, watch.write);
        // SAFETY: the descriptor is the connection's socket, which libdbus
        // keeps open while the connection is, and `hear` drops a lost one
        // before this runs. `bus` outlives the wait.
        let socket = unsafe { BorrowedFd::borrow_raw(watch.fd) };
        let mut waits = [
            PollFd::from_borrowed_fd(socket, asked),
            PollFd::new(wake, PollFlags::IN),
        ];
        match rustix::event::poll(&mut waits, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => {
                tracing::warn!(%error, "couldn't wait on the notification server, so it's heard only between notifications");
                return false;
            }
        }
        if waits[1].revents().contains(PollFlags::IN) {
            // Reading the counter clears it. The jobs it counted are in
            // the queue.
            let _ = rustix::io::read(wake, &mut [0; 8]);
        }
        true
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

    // What the server says is tested against signals made here, since a
    // real one needs a desktop session.
    #[cfg(test)]
    mod tests {
        use std::sync::Mutex;

        use super::*;

        type Clicked = Arc<Mutex<Vec<AgentId>>>;

        /// A screen with no connection, whose clicks land in `clicked`.
        fn screen(clicked: &Clicked) -> Screen {
            let clicked = clicked.clone();
            Screen::new(Box::new(move |agent| {
                clicked.lock().unwrap().push(agent);
            }))
        }

        /// Puts a notification the app knows as `id` on screen as the
        /// server's `server_id`.
        fn shown(screen: &mut Screen, server_id: u32, id: &str) -> AgentId {
            let agent = AgentId::derive(id);
            let shown = Shown {
                id: id.into(),
                agent,
            };
            screen.shown.insert(server_id, shown);
            agent
        }

        fn server(member: &str) -> Message {
            Message::new_signal(PATH, SERVER, member).unwrap()
        }

        fn owner_changed(old: &str, new: &str) -> Message {
            Message::new_signal("/org/freedesktop/DBus", BUS, "NameOwnerChanged")
                .unwrap()
                .append3(SERVER, old, new)
        }

        #[test]
        fn a_click_opens_its_agent_once() {
            let clicked = Clicked::default();
            let mut screen = screen(&clicked);
            let agent = shown(&mut screen, 7, "prompt");
            screen.heard(&server("ActionInvoked").append2(7_u32, CLICK));
            // Another app's notification, and this one again once it's gone.
            screen.heard(&server("ActionInvoked").append2(8_u32, CLICK));
            screen.heard(&server("ActionInvoked").append2(7_u32, CLICK));
            assert_eq!(*clicked.lock().unwrap(), [agent]);
            assert!(screen.shown.is_empty());
        }

        #[test]
        fn another_action_takes_it_off_without_opening() {
            let clicked = Clicked::default();
            let mut screen = screen(&clicked);
            shown(&mut screen, 7, "prompt");
            screen.heard(&server("ActionInvoked").append2(7_u32, "Dismiss"));
            assert!(clicked.lock().unwrap().is_empty());
            assert!(screen.shown.is_empty());
        }

        #[test]
        fn a_close_forgets_only_that_one() {
            let mut screen = screen(&Clicked::default());
            shown(&mut screen, 7, "first");
            shown(&mut screen, 8, "second");
            screen.heard(&server("NotificationClosed").append2(7_u32, 2_u32));
            assert_eq!(screen.take("first"), None);
            assert_eq!(screen.take("second"), Some(8));
        }

        #[test]
        fn the_server_going_away_takes_them_all() {
            let mut screen = screen(&Clicked::default());
            shown(&mut screen, 7, "prompt");
            screen.heard(&owner_changed("", ":1.5"));
            assert_eq!(
                screen.shown.len(),
                1,
                "a server that only just started showed none"
            );
            screen.heard(&owner_changed(":1.5", ""));
            assert!(screen.shown.is_empty());
        }
    }
}

#[cfg(windows)]
mod windows;

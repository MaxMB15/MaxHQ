// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the MaxHQ apps do apart from their windows. [`App`] connects to
//! `maxd` on each device you add, merges what they report into one view
//! and answers each [`Request`] a window makes. A shell, such as the
//! desktop app's Tauri one, carries the requests in and the view's changes
//! out, and does the few things only the platform can, through [`Shell`].
//!
//! The `desktop` feature holds what only the Mac does: SSH config, sign-ins,
//! setup, editors, the service install, the git identity and the bundled
//! binaries. The phone builds without it, gets its devices from the book it
//! paired with, and opens links through [`phone::PhoneShell`].

pub mod agents;
pub mod app_keys;
#[cfg(feature = "desktop")]
pub mod app_update;
pub mod devices;
#[cfg(feature = "desktop")]
pub mod editors;
pub mod fleet;
pub mod forwards;
pub mod hidden;
pub mod host_fix;
pub mod keys;
pub mod media;
pub mod merge;
pub mod notify;
pub mod offered;
pub mod pace;
pub mod pair_code;
pub mod phone;
pub mod pulls;
pub mod registry;
pub mod request;
#[cfg(feature = "desktop")]
pub mod restart;
#[cfg(feature = "desktop")]
pub mod setup;
#[cfg(feature = "desktop")]
pub mod signin;
pub mod skills;
#[cfg(feature = "desktop")]
pub mod ssh_config;
pub mod tasks;
pub mod terminals;
pub mod timeline;
pub mod transfers;
pub mod view;
pub mod viewer;
pub mod workflow;

use std::future::Future;
#[cfg(feature = "desktop")]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use maxhq_client::Client;
use maxhq_protocol::ids::{AgentId, TerminalId};
use maxhq_protocol::workflow::RunKey;

use crate::devices::{Connector, Devices};
#[cfg(feature = "desktop")]
use crate::editors::{Installed, Places};
use crate::offered::Offered;
use crate::request::CommandError;
use crate::terminals::RecordSink;
use crate::timeline::Sink;
use crate::view::View;
use crate::workflow::WorkflowMessage;

pub use crate::request::{Request, Response};

/// A page, link or editor on its way to opening, which fails with why it
/// didn't.
pub type Opening = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// What only the platform can do for the app.
pub trait Shell: Send + Sync + 'static {
    /// Opens `url` in the default browser.
    fn open_browser(&self, url: String) -> Opening;
    /// Opens `url` in the app its scheme belongs to, such as an editor's
    /// link. Only web pages and, on the desktop, the editors' links open.
    fn open_url(&self, url: String) -> Opening;
    /// Runs the editor command line at `program` with `args`, as they are
    /// and without a shell.
    #[cfg(feature = "desktop")]
    fn spawn_editor(&self, program: PathBuf, args: Vec<String>) -> Opening;
    /// The folders that hold the app's own data on this machine, which
    /// go when you uninstall `maxd` and ask for its data to go too.
    #[cfg(feature = "desktop")]
    fn app_data(&self) -> Vec<PathBuf>;
    /// Quits the app once the answer to the request at hand has had time
    /// to reach the window.
    #[cfg(feature = "desktop")]
    fn quit(&self);
}

/// The window a request came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The window's label, which tells its panes and timelines apart.
    pub window: String,
    /// Whether the window is on screen. One that's minimized or hidden
    /// shows nothing, whatever its page says.
    pub on_screen: bool,
}

/// The app: the merged view, the devices that feed it and the paths you
/// handed over. The shell holds one and hands it each request.
pub struct App<C: Connector = Client> {
    view: View,
    devices: Devices<C>,
    offered: Offered,
    shell: Box<dyn Shell>,
    #[cfg(feature = "desktop")]
    editors: Installed,
}

impl<C: Connector> App<C> {
    /// The app over `devices`, which feed `view`.
    pub fn new(view: View, devices: Devices<C>, shell: impl Shell) -> Self {
        Self {
            view,
            devices,
            offered: Offered::default(),
            shell: Box::new(shell),
            #[cfg(feature = "desktop")]
            editors: Installed::new(Places::of_this_machine()),
        }
    }

    /// The devices that feed the view.
    pub fn devices(&self) -> &Devices<C> {
        &self.devices
    }

    /// The paths you handed over, which the shell adds to as you drop,
    /// paste or pick them.
    pub fn offered(&self) -> &Offered {
        &self.offered
    }

    /// Opens an agent's timeline in a window. Updates go to `sink`, the
    /// window's one timeline channel, starting with a reset to the newest
    /// items. Each open needs a close, though a window that reloads or
    /// closes lets go of all its opens.
    pub async fn open_timeline(
        &self,
        window: &str,
        sink: Arc<dyn Sink>,
        agent: AgentId,
    ) -> Result<(), CommandError> {
        self.devices.open_timeline(window, sink, agent).await
    }

    /// Opens a workflow run in a window. Updates go to `sink`, the window's
    /// one workflow channel, starting with a reset. Each open needs a
    /// close, though a window that reloads or closes lets go of all its
    /// opens.
    pub async fn open_workflow(
        &self,
        window: &str,
        sink: Arc<dyn Sink<WorkflowMessage>>,
        run: RunKey,
    ) -> Result<(), CommandError> {
        self.devices.open_workflow(window, sink, run).await
    }

    /// Attaches a terminal to a window from byte `after` of its output on.
    /// The output comes to `sink`, the window's one terminal channel, as
    /// records under the number this gives. Each attach needs a detach,
    /// though a window that reloads or closes lets go of all its attaches.
    pub async fn attach_terminal(
        &self,
        window: &str,
        sink: Arc<RecordSink>,
        terminal: TerminalId,
        after: Option<u64>,
    ) -> Result<u32, CommandError> {
        self.devices
            .attach_terminal(window, sink, terminal, after)
            .await
    }

    /// Takes back a window's attach of a terminal.
    pub async fn detach_terminal(&self, window: &str, terminal: TerminalId) {
        self.devices.detach_terminal(window, terminal).await;
    }

    /// Sends what you typed into a terminal. Nothing keeps it.
    pub async fn terminal_input(
        &self,
        terminal: TerminalId,
        bytes: Vec<u8>,
    ) -> Result<(), CommandError> {
        self.devices.terminal_input(terminal, bytes).await
    }

    /// Lets go of the timelines, runs and terminals a window watched, once
    /// its page starts loading a new one. What the new page opens comes
    /// after.
    pub fn release_window(&self, window: &str) {
        drop(self.devices.release_window(window));
    }

    /// Forgets what a window's panes showed once it closes, so the agents
    /// it showed alert again, and lets go of the timelines it watched.
    pub fn close_window(&self, window: &str) {
        self.view.forget_window(window);
        self.release_window(window);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Mutex, OnceLock};

    use tokio::runtime::{Handle, Runtime};

    use super::*;

    /// The runtime every test shares, as the app shares its shell's.
    fn shared() -> &'static Runtime {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| Runtime::new().expect("the test runtime starts"))
    }

    /// A handle to the shared runtime.
    pub(crate) fn runtime() -> Handle {
        shared().handle().clone()
    }

    /// Runs `future` to the end on the shared runtime.
    pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
        shared().block_on(future)
    }

    /// A shell that keeps what it was asked to open: the browser's pages,
    /// then the links and editors. On the desktop it also names the app's
    /// data folders and counts the times it was asked to quit.
    #[derive(Clone, Default)]
    pub(crate) struct FakeShell(
        pub(crate) Arc<Mutex<Vec<String>>>,
        pub(crate) Arc<Mutex<Vec<Opened>>>,
        #[cfg(feature = "desktop")] pub(crate) Arc<Mutex<Leaving>>,
    );

    /// The app data a [`FakeShell`] names, and how often it quit.
    #[cfg(feature = "desktop")]
    #[derive(Debug, Default)]
    pub(crate) struct Leaving {
        pub(crate) data: Vec<PathBuf>,
        pub(crate) quits: u32,
    }

    /// A link or editor a [`FakeShell`] was asked to open.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Opened {
        Url(String),
        #[cfg(feature = "desktop")]
        Editor(PathBuf, Vec<String>),
    }

    impl Shell for FakeShell {
        fn open_browser(&self, url: String) -> Opening {
            self.0.lock().unwrap().push(url);
            Box::pin(async { Ok(()) })
        }

        fn open_url(&self, url: String) -> Opening {
            self.1.lock().unwrap().push(Opened::Url(url));
            Box::pin(async { Ok(()) })
        }

        #[cfg(feature = "desktop")]
        fn spawn_editor(&self, program: PathBuf, args: Vec<String>) -> Opening {
            self.1.lock().unwrap().push(Opened::Editor(program, args));
            Box::pin(async { Ok(()) })
        }

        #[cfg(feature = "desktop")]
        fn app_data(&self) -> Vec<PathBuf> {
            self.2.lock().unwrap().data.clone()
        }

        #[cfg(feature = "desktop")]
        fn quit(&self) {
            self.2.lock().unwrap().quits += 1;
        }
    }
}

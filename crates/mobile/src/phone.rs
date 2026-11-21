// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The core as the phone runs it: the app over the phone's connector, on a
//! runtime of its own, with the view's changes going to one listener.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use maxhq_app::devices::Devices;
use maxhq_app::notify::LogSender;
use maxhq_app::offered::{NotOffered, Offered};
use maxhq_app::pair_code::PairCode;
use maxhq_app::phone::{self, PhoneConnector, PhoneProver, PhoneShell};
use maxhq_app::registry::Registry;
use maxhq_app::request::{CommandError, Request, Response};
use maxhq_app::timeline::Sink;
use maxhq_app::transfers::{CopiedProject, CopyTarget, Downloaded, MediaFile, TransferProgress};
use maxhq_app::view::{Emit, View};
use maxhq_app::workflow::WorkflowMessage;
use maxhq_app::{App, Caller};
use maxhq_client::{Client, LinkEvent, LinkSpec, PhoneSsh, PhoneWs, Transport};
use maxhq_core::authorized_keys;
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, LinkId, PhoneId, WorkspaceId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::{ActionOutcome, ClientMessage, UploadTarget};
use maxhq_protocol::phones::{PhoneBook, PhoneBookEntry, PushToken};
use maxhq_protocol::workflow::RunKey;
use serde::Serialize;
use tokio::runtime::{Handle, Runtime};

use crate::keys::{self, Key, KeyStore};
use crate::net::{self, SignIns};
use crate::{AppState, Network, ProgressListener, ViewListener};

/// The one window the phone's screens share, as a desktop window's label.
/// Its timelines all go to one listener, as a desktop window's do.
pub(crate) const WINDOW: &str = "phone";

/// How long pairing waits for the device, from the scan to its answer.
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the links stay after the app leaves the screen, so the word
/// that it left reaches the devices first.
const LEAVE_GRACE: Duration = Duration::from_millis(300);

/// The phone's key, for you to let it in by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhoneKey {
    /// The phone, as its book names it.
    pub phone: PhoneId,
    /// The key's public half, as an OpenSSH line.
    pub public_key: String,
    /// The `authorized_keys` line that lets it run only `maxd proxy
    /// --phone <phone>`.
    pub line: String,
    /// The public half of the key devices seal its pushes to, as base64.
    pub push_public: String,
}

/// A Mac the phone paired with, as Settings lists it: the book it keeps
/// and the devices the phone has from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairedMac {
    /// The device that keeps the book, which forgetting the Mac names.
    pub keeper: DeviceId,
    /// The Mac's name, as it gave it or else as its book has it.
    pub name: String,
    /// Whether it's the book of devices you added by hand.
    pub by_hand: bool,
    /// The devices the phone has from this Mac, in its book's order. A
    /// device an earlier book lists too shows there instead.
    pub devices: Vec<PairedDevice>,
}

/// A device the phone has from a Mac.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairedDevice {
    pub device: DeviceId,
    /// The link the phone reaches it on.
    pub link: LinkId,
    pub name: String,
    /// Whether another Mac's book lists it too, so it stays when this Mac
    /// goes.
    pub elsewhere: bool,
}

/// The running core.
pub struct Core {
    shared: Arc<Shared>,
    /// The app's caches folder, where the pickers copy what you pick, and
    /// so the one folder the phone uploads from.
    picks: PathBuf,
    /// Dropped last, once nothing on it holds the app.
    runtime: Runtime,
}

/// Where the view's changes go, once the page subscribes.
type Listener = Arc<RwLock<Option<Arc<dyn ViewListener>>>>;

struct Shared {
    app: App<PhoneConnector>,
    client: Arc<Client>,
    listener: Listener,
    on_screen: AtomicBool,
    sign_ins: Arc<SignIns>,
    key: PhoneKey,
}

impl Core {
    /// Opens the core on `dir`, with the phone's keys and tokens in
    /// `store`. The registry is `maxhq.db` there, and none of it goes into
    /// the phone's backups. The media cache is `media` in `cache` and
    /// downloads land in `downloads` there, which the system may empty
    /// when space runs low. A picture it drops fetches again, and a
    /// download has gone to the share sheet by then. The phone uploads
    /// only from the folder `cache` is in, the app's caches folder, where
    /// the pickers copy what you pick.
    pub fn open(dir: &Path, cache: &Path, store: Arc<dyn KeyStore>) -> Result<Self, CommandError> {
        std::fs::create_dir_all(dir)
            .map_err(|error| failed(format!("Couldn't make the phone's data folder: {error}")))?;
        // A folder that stays in backups still works, so the phone opens anyway.
        if let Err(error) = crate::backup::exclude(dir) {
            tracing::warn!(%error, "couldn't keep the phone's data out of backups");
        }
        let media = cache.join("media");
        move_old_media(dir, &media);
        let registry = Registry::open_phone(&dir.join("maxhq.db"))
            .map_err(|error| failed(format!("Couldn't open the phone's data: {error}")))?;
        let books = registry
            .phone_books()
            .map_err(|error| failed(format!("Couldn't read the phone's books: {error}")))?;
        let seed = keys::seed(&*store, Key::Ssh).map_err(failed)?;
        let push_public = keys::push_public(&*keys::seed(&*store, Key::Push).map_err(failed)?);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("maxhq-core")
            .enable_all()
            .build()
            .map_err(|error| failed(format!("Couldn't start the core: {error}")))?;
        let sign_ins = Arc::new(SignIns::new(
            store,
            seed.clone(),
            net::posting(PhoneWs::new()),
        ));
        let signing = Arc::clone(&sign_ins);
        let connector = PhoneConnector::new(
            runtime.handle(),
            &seed,
            Box::new(move |device, mac, route, code| signing.route(device, mac, route, code)),
            Some(cache.join("downloads")),
            Some(media),
        );
        let public_key = connector.public_key();
        let phone = books
            .first()
            .map_or_else(|| PhoneId::derive(&public_key), |book| book.phone);
        let line = authorized_keys::phone_line(&public_key, &phone.to_string()).map_err(failed)?;
        let client = connector.client();
        let listener: Listener = Arc::default();
        let view = View::new(emit_to(Arc::clone(&listener)), Box::new(LogSender));
        let devices = Devices::start(
            runtime.handle().clone(),
            connector,
            registry,
            view.clone(),
            None,
        );
        let shell = PhoneShell::new(|_| {
            Box::pin(async { Err("The phone can't open links yet".to_owned()) })
        });
        let shared = Arc::new(Shared {
            app: App::new(view, devices, shell),
            client,
            listener,
            on_screen: AtomicBool::new(true),
            sign_ins,
            key: PhoneKey {
                phone,
                public_key,
                line,
                push_public,
            },
        });
        let open = Arc::clone(&shared);
        runtime.spawn(async move { open.app.devices().set_foreground(true).await });
        Ok(Self {
            shared,
            picks: cache.parent().unwrap_or(cache).to_owned(),
            runtime,
        })
    }

    /// Lets go of what the page before a JS reload watched.
    pub fn reloaded(&self) {
        *self.listener() = None;
        let _entered = self.runtime.enter();
        self.shared.app.release_window(WINDOW);
    }

    /// Answers `request` on the core's runtime.
    pub fn request(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move {
            let caller = Caller {
                window: WINDOW.to_owned(),
                on_screen: shared.on_screen.load(Ordering::Relaxed),
            };
            shared.app.request(&caller, request).await
        })
    }

    /// Sends each change to the view to `listener`.
    pub fn subscribe(&self, listener: Arc<dyn ViewListener>) {
        *self.listener() = Some(listener);
    }

    /// Opens `agent`'s timeline, with batches going to `sink`.
    pub fn open_timeline(
        &self,
        agent: AgentId,
        sink: Arc<dyn Sink>,
    ) -> impl Future<Output = Result<(), CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move { shared.app.open_timeline(WINDOW, sink, agent).await })
    }

    /// Fetches a picture or video `agent` can see into the media cache,
    /// with how far it has got going to `listener`. A phone never asks for
    /// a file outside the agent's folders, so Show anyway stays off.
    pub fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u32,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<MediaFile, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move {
            (shared.app)
                .media(agent, path, limit, false, reporter(listener))
                .await
        })
    }

    /// Opens a workflow run, with updates going to `sink`.
    pub fn open_workflow(
        &self,
        run: RunKey,
        sink: Arc<dyn Sink<WorkflowMessage>>,
    ) -> impl Future<Output = Result<(), CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move { shared.app.open_workflow(WINDOW, sink, run).await })
    }

    /// Uploads a file you picked to `target` on `device`, with how far it
    /// has got going to `listener`. Picking it handed it over, so a path
    /// in the app's caches folder counts as offered, and any other is
    /// refused as the desktop refuses one you never dropped.
    pub fn upload(
        &self,
        device: DeviceId,
        target: UploadTarget,
        path: String,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        let picked = hand_over(shared.app.offered(), &self.picks, &path);
        self.run(async move {
            let path = picked?.to_string_lossy().into_owned();
            (shared.app)
                .upload(device, target, path, reporter(listener))
                .await
        })
    }

    /// Uploads pasted bytes, `data` in base64, to `target` on `device` as
    /// one file called `name`, with how far it has got going to `listener`.
    pub fn upload_pasted(
        &self,
        device: DeviceId,
        target: UploadTarget,
        name: String,
        mime: String,
        data: String,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move {
            (shared.app)
                .upload_pasted(device, target, name, mime, data, reporter(listener))
                .await
        })
    }

    /// Fetches a file or folder `agent` can see into the downloads folder,
    /// with how far it has got going to `listener`.
    pub fn download(
        &self,
        agent: AgentId,
        path: String,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<Downloaded, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move { shared.app.download(agent, path, reporter(listener)).await })
    }

    /// Readies a file or folder `agent` can see for an agent on `device`,
    /// with both legs going to `listener`.
    pub fn attach_from_agent(
        &self,
        device: DeviceId,
        target: Option<UploadTarget>,
        agent: AgentId,
        path: String,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move {
            (shared.app)
                .attach_from_agent(device, target, agent, path, reporter(listener))
                .await
        })
    }

    /// Copies the project of workspace `from` to `target` with the ignored
    /// `files` chosen, with the files' transfers going to `listener`.
    pub fn copy_project(
        &self,
        from: WorkspaceId,
        target: CopyTarget,
        files: Vec<String>,
        listener: Arc<dyn ProgressListener>,
    ) -> impl Future<Output = Result<CopiedProject, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        self.run(async move {
            (shared.app)
                .copy_project(from, target, files, reporter(listener))
                .await
        })
    }

    /// Closes every link in the background, once the devices know the
    /// app left, and dials them again on screen.
    pub fn app_state(&self, state: AppState) {
        let active = state == AppState::Active;
        self.shared.on_screen.store(active, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        if active {
            shared.client.wake();
            self.runtime
                .spawn(async move { shared.app.devices().set_foreground(true).await });
        } else {
            self.runtime.spawn(async move {
                shared.app.devices().set_foreground(false).await;
                tokio::time::sleep(LEAVE_GRACE).await;
                // Back on screen by now, the links stay.
                if !shared.on_screen.load(Ordering::Relaxed) {
                    shared.client.suspend();
                }
            });
        }
    }

    /// Keeps the push token and kinds, and gives them to every device the
    /// phone links to, now and each time a link comes back.
    pub fn push_token(&self, push: PushToken) {
        self.shared.app.devices().set_push_token(push);
    }

    /// Says which agent is on screen, if any, so no device pushes about it.
    pub fn presence(&self, agent: Option<AgentId>) {
        self.shared.app.devices().show_agent(agent);
    }

    /// Names the phone's network, so each link tries the route that last
    /// worked on it first.
    pub fn network_changed(&self, kind: Network) {
        let key = match kind {
            Network::Wifi => Some("wifi".to_owned()),
            Network::Cellular => Some("cellular".to_owned()),
            Network::None => None,
        };
        self.shared.client.set_network(key);
    }

    /// The phone's key and its `authorized_keys` line.
    pub fn key(&self) -> PhoneKey {
        self.shared.key.clone()
    }

    /// Adds a device by hand, or replaces the one of that name, in a book
    /// of its own beside the Macs' books.
    pub fn add_device(&self, name: &str, route: &str, host_key: &str) -> Result<(), CommandError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(failed("Give the device a name".to_owned()));
        }
        let route = phone::parse_route(&format!("ssh://{}", route.trim()))
            .ok_or_else(|| failed("Write the route as user@host:port".to_owned()))?;
        let mut parts = host_key.split_whitespace();
        let (Some(kind), Some(key)) = (parts.next(), parts.next()) else {
            return Err(failed("Paste the host key as ssh writes it".to_owned()));
        };
        let entry = PhoneBookEntry {
            device: DeviceId::derive(name),
            name: name.to_owned(),
            routes: vec![route],
            host_key: format!("{kind} {key}"),
            network: None,
        };
        let devices = self.shared.app.devices();
        let mut book = self
            .books()?
            .into_iter()
            .find(|book| book.device == phone::by_hand())
            .unwrap_or_else(|| PhoneBook {
                phone: self.shared.key.phone,
                device: phone::by_hand(),
                devices: Vec::new(),
                mac: None,
            });
        book.devices.retain(|each| each.device != entry.device);
        book.devices.push(entry);
        let _entered = self.runtime.enter();
        devices.put_phone_book(&book)?;
        Ok(())
    }

    /// The Macs the phone paired with, in the order it paired, and the
    /// book of devices added by hand last when there is one.
    pub fn paired_macs(&self) -> Result<Vec<PairedMac>, CommandError> {
        let mut macs = paired(&self.books()?);
        macs.sort_by_key(|mac| mac.by_hand);
        Ok(macs)
    }

    /// Forgets the Mac whose book `keeper` keeps: its book, each device
    /// only it lists with what waited for it, and their sign-ins. The
    /// other Macs stay paired.
    pub fn forget_mac(&self, keeper: DeviceId) -> Result<(), CommandError> {
        if self.books()?.iter().all(|book| book.device != keeper) {
            return Err(failed("This phone isn't paired with that Mac".to_owned()));
        }
        let _entered = self.runtime.enter();
        let gone = self.shared.app.devices().forget_phone_book(keeper)?;
        for device in gone {
            self.shared.sign_ins.forget(device);
        }
        Ok(())
    }

    /// The name of the Mac the phone already has a book from that `code`
    /// pairs through, so pairing replaces that Mac's devices, or `None`
    /// for a new Mac or a code that doesn't read.
    pub fn pair_code_mac(&self, code: &str) -> Result<Option<String>, CommandError> {
        let Ok(code) = PairCode::decode(code) else {
            return Ok(None);
        };
        Ok(self
            .books()?
            .iter()
            .find(|book| {
                book.device == code.device
                    || book
                        .mac
                        .as_ref()
                        .is_some_and(|mac| mac.device == code.device)
            })
            .map(phone::mac_name))
    }

    /// The phone's books, one per Mac.
    fn books(&self) -> Result<Vec<PhoneBook>, CommandError> {
        self.shared
            .app
            .devices()
            .registry()
            .phone_books()
            .map_err(|error| failed(format!("Couldn't read the phone's books: {error}")))
    }

    /// Pairs the phone through the device whose pairing code is `code`,
    /// as the Mac shows it, giving it `name` to show, and keeps the book
    /// the device answers with: beside the other Macs' books for a new
    /// Mac, or in place of the one it had from the same Mac, as
    /// [`phone::may_keep`] allows. A code with the device's network access
    /// pairs through it, and the phone keeps the token it gets. Else the
    /// phone logs in over SSH with the code's one-time key, which the
    /// device swaps for the phone's own.
    pub fn pair(
        &self,
        code: &str,
        name: &str,
    ) -> impl Future<Output = Result<PhoneBook, CommandError>> + Send + 'static {
        let shared = Arc::clone(&self.shared);
        let runtime = self.runtime.handle().clone();
        let code = PairCode::decode(code).map_err(failed);
        let name = match name.trim() {
            "" => "iPhone".to_owned(),
            name => name.to_owned(),
        };
        let books = self.books();
        self.run(async move {
            let code = code?;
            let books = books?;
            let pairing = async {
                match &code.network {
                    Some(network) => (shared.sign_ins)
                        .pair(&code, network, &name, &shared.key, &books)
                        .await
                        .map_err(failed),
                    None => {
                        let book = pair_through(&runtime, &code, &name, &shared.key).await?;
                        phone::may_keep(&books, &code, &book).map_err(failed)?;
                        Ok(book)
                    }
                }
            };
            let book = tokio::time::timeout(PAIR_TIMEOUT, pairing)
            .await
            .map_err(|_| {
                failed(
                    "The device didn't answer in time. Check the phone can reach it, then scan a new code."
                        .to_owned(),
                )
            })??;
            // A device the Mac's new book leaves out goes with its sign-in.
            for device in shared.app.devices().put_phone_book(&book)? {
                shared.sign_ins.forget(device);
            }
            Ok(book)
        })
    }

    /// The books the phone has now.
    #[cfg(test)]
    pub(crate) fn shared_books(&self) -> Vec<PhoneBook> {
        self.books().unwrap()
    }

    /// Keeps `book` as pairing with its Mac does.
    #[cfg(test)]
    pub(crate) fn put_book(&self, book: &PhoneBook) {
        let _entered = self.runtime.enter();
        self.shared.app.devices().put_phone_book(book).unwrap();
    }

    /// Runs `task` on the core's runtime, for a caller on any thread.
    fn run<T: Send + 'static>(
        &self,
        task: impl Future<Output = Result<T, CommandError>> + Send + 'static,
    ) -> impl Future<Output = Result<T, CommandError>> + Send + 'static {
        let handle = self.runtime.spawn(task);
        async move {
            handle
                .await
                .map_err(|error| failed(format!("The core stopped: {error}")))?
        }
    }

    fn listener(&self) -> std::sync::RwLockWriteGuard<'_, Option<Arc<dyn ViewListener>>> {
        self.shared
            .listener
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Logs in to `code`'s device with its one-time key and asks it to pair,
/// answering the phone's book.
async fn pair_through(
    runtime: &Handle,
    code: &PairCode,
    name: &str,
    key: &PhoneKey,
) -> Result<PhoneBook, CommandError> {
    let ssh = PhoneSsh::new(&code.seed);
    let entry = PhoneBookEntry {
        device: code.device,
        name: "the device".to_owned(),
        routes: code.routes.clone(),
        host_key: code.host_key.clone(),
        network: None,
    };
    let mut routes = ssh.trust(&entry).into_iter();
    let route = routes
        .next()
        .ok_or_else(|| failed("The pairing code has no way to reach the device".to_owned()))?;
    // A device that asks has the connection prove it holds the same key.
    let client = maxhq_app::devices::start_client(
        runtime,
        || Client::app(Transport::Russh(ssh)).proving(PhoneProver::pairing(&code.seed)),
        None,
        None,
    );
    let spec = LinkSpec {
        id: phone::book_link(code.device),
        name: entry.name,
        route,
        fallbacks: routes.collect(),
        last_good: BTreeMap::new(),
    };
    // The code names the device, so nothing else can answer in its place.
    let (link, mut events) = client.link_to(spec, code.device);
    loop {
        match events.recv().await {
            Some(LinkEvent::Link(link)) => match &link.state {
                LinkState::Connected { .. } => break,
                LinkState::Unreachable { reason, .. } => {
                    return Err(failed(format!("Couldn't reach the device: {reason}")));
                }
                LinkState::Connecting { .. } | LinkState::Unknown => {}
            },
            Some(_) => {}
            None => return Err(failed("The link to the device closed".to_owned())),
        }
    }
    // The link's events go nowhere from here, so it never waits on them.
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    let outcome = link
        .act(ClientMessage::PairPhone {
            key: IdempotencyKey::generate(),
            name: name.to_owned(),
            ssh_public: key.public_key.clone(),
            push_public: key.push_public.clone(),
        })
        .await
        .map_err(|error| failed(format!("The device didn't pair the phone: {error}")))?;
    match outcome {
        ActionOutcome::PhonePaired { phone, book, mac } if phone == key.phone => Ok(PhoneBook {
            phone,
            device: code.device,
            devices: book,
            mac,
        }),
        ActionOutcome::PhonePaired { .. } => Err(failed(
            "The device knows this phone by another name. Scan a new code.".to_owned(),
        )),
        ActionOutcome::Refused { reason } => Err(failed(reason)),
        _ => Err(failed(
            "The device answered something other than a pairing".to_owned(),
        )),
    }
}

/// The Macs `books` come from, each with the devices the phone has from it.
fn paired(books: &[PhoneBook]) -> Vec<PairedMac> {
    let listed = phone::listed(books);
    books
        .iter()
        .enumerate()
        .map(|(at, book)| PairedMac {
            keeper: book.device,
            name: phone::mac_name(book),
            by_hand: book.device == phone::by_hand(),
            devices: listed
                .iter()
                .filter(|(from, _)| *from == at)
                .map(|(_, entry)| PairedDevice {
                    device: entry.device,
                    link: phone::book_link(entry.device),
                    name: entry.name.clone(),
                    elsewhere: books.iter().enumerate().any(|(other, book)| {
                        other != at && book.devices.iter().any(|each| each.device == entry.device)
                    }),
                })
                .collect(),
        })
        .collect()
}

/// Sends each batch of the view's changes to the listener, as one JSON array.
pub(crate) fn emit_to(listener: Listener) -> Emit {
    Box::new(move |deltas| {
        let Some(listener) = listener
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        match serde_json::to_string(&deltas) {
            Ok(json) => listener.changed(json),
            Err(error) => tracing::warn!(%error, "couldn't write the view's changes"),
        }
    })
}

/// Moves the media cache an older app kept in the data folder to `media`,
/// or drops it when it can't go there, since it all fetches again.
fn move_old_media(dir: &Path, media: &Path) {
    let old = dir.join("media");
    if !old.exists() {
        return;
    }
    let moved = media
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(&old, media));
    if moved.is_err()
        && let Err(error) = std::fs::remove_dir_all(&old)
    {
        tracing::warn!(%error, "couldn't drop the old media cache");
    }
}

/// Sends each step of a transfer to `listener` as JSON.
fn reporter(listener: Arc<dyn ProgressListener>) -> impl Fn(TransferProgress) + Send + Sync {
    move |progress| {
        if let Ok(json) = serde_json::to_string(&progress) {
            listener.progress(json);
        }
    }
}

/// Hands `path` over to the app as a pick, when it's a file or folder
/// inside `picks`, and answers it as the app now has it. Anything else is
/// refused, as a path you never dropped is on the desktop.
pub(crate) fn hand_over(
    offered: &Offered,
    picks: &Path,
    path: &str,
) -> Result<PathBuf, CommandError> {
    let refused = || {
        failed(
            NotOffered {
                path: path.to_owned(),
            }
            .to_string(),
        )
    };
    let (Ok(picks), Ok(found)) = (std::fs::canonicalize(picks), std::fs::canonicalize(path)) else {
        return Err(refused());
    };
    if found == picks || !found.starts_with(&picks) {
        return Err(refused());
    }
    offered.offer([found.clone()]);
    Ok(found)
}

fn failed(message: String) -> CommandError {
    CommandError::Failed { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_picks_in_the_caches_folder_are_handed_over() {
        let root = std::env::temp_dir().join(format!(
            "maxhq-picks-{}",
            maxhq_protocol::ids::InstanceId::generate()
        ));
        let picks = root.join("Library/Caches");
        let picked = picks.join("DocumentPicker/notes.txt");
        let kept = root.join("Documents/maxhq/maxhq.db");
        for file in [&picked, &kept] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "x").unwrap();
        }
        let offered = Offered::default();

        let handed = hand_over(&offered, &picks, &picked.to_string_lossy()).unwrap();
        assert!(handed.ends_with("Library/Caches/DocumentPicker/notes.txt"));
        assert!(offered.contains(&handed));

        let escape = picks.join("../../Documents/maxhq/maxhq.db");
        for path in [
            kept.to_string_lossy().into_owned(),
            escape.to_string_lossy().into_owned(),
            picks.to_string_lossy().into_owned(),
            picks.join("gone.txt").to_string_lossy().into_owned(),
        ] {
            assert_eq!(
                hand_over(&offered, &picks, &path),
                Err(failed(format!("{path} wasn't dropped, pasted or picked"))),
            );
        }
        assert!(!offered.contains(&kept));
        let _ = std::fs::remove_dir_all(&root);
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a device's `maxd` does for local models: it keeps the servers you
//! added and their tokens, downloads catalog models, runs the managed
//! server on a Mac that can, and publishes it all as entities. Each server
//! with models to run shows as an account, so a local agent starts on it
//! like any other.

use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, ModelServerId};
use maxhq_protocol::local_models::{
    managed_model_id, CatalogFile, CatalogModel, CatalogRef, DeviceHardware, KvShape,
    LocalModelSettings, LocalModels, LocalRun, ModelAction, ModelCatalog, ModelFile,
    ModelFileState, ModelLoad, ModelRuntime, ModelServer, ModelServerKind, RuntimeSupport,
    ServerApis, ServerModel, ServerProbe,
};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Activity, LoginKind, ModelChoice, Vendor};
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::{Entity, EntityKey, ToolPhase, ToolState};
use maxhq_protocol::time::Timestamp;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

use super::runner::{self, Backend, Host, LoadProgress, Runner, Spec};
use super::{catalog, download, hardware, probe, runtime};
use crate::db::Db;
use crate::drive::ServerLogin;
use crate::hub::Hub;
use crate::secrets::files::Folder;

/// The managed server's key, in the keys folder. llama-server reads it
/// from the file too.
const MANAGED_KEY: &str = "managed-key";

/// The file that keeps the proxy's port, so local agents find it after a
/// restart.
const PORT_FILE: &str = "proxy-port";

/// What a local agent sends a server that wants no token. Claude Code
/// needs one to skip its sign-in.
const NO_TOKEN: &str = "no-token";

/// How often a download's progress shows, at most.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// How often loaded models are read for speed and swept for idleness.
const TICK: Duration = Duration::from_secs(2);

/// Free disk shows rounded down to this, so a log that grows doesn't
/// publish again and again.
const DISK_STEP: u64 = 100_000_000;

const NO_SERVER: &str = "There's no model server here with that id.";
const NOT_MANAGED: &str =
    "This device can't run models itself. Add a model server by its URL instead.";
const ONLY_MANAGED: &str =
    "Only this Mac's own server loads and unloads models. A server you added loads them itself.";
const OWN_SERVER: &str = "This Mac's own server can't be removed. Delete its models instead.";
const BAD_URL: &str = "That isn't a server URL. Give one like http://host:8080.";
const SAME_URL: &str = "That server is already added.";
const NOT_IN_CATALOG: &str = "That model file isn't in the catalog.";
const NO_RUNTIME: &str = "llama-server isn't installed on this Mac.";

/// A catalog model's ID and a file's name.
type FileKey = (String, String);

/// Where the service keeps things and what the device has.
pub struct Setup {
    /// Its own folder: keys, logs, the kept catalog and each server's
    /// Claude config folder.
    pub dir: PathBuf,
    /// The home folder, which llama-server installs under.
    pub home: PathBuf,
    /// Where model files go.
    pub models_dir: PathBuf,
    /// The device's hardware.
    pub hardware: DeviceHardware,
    /// Whether it can run a managed server.
    pub support: RuntimeSupport,
    /// Where model files come from.
    pub files_from: String,
    /// Whether llama-server may be installed. Tests never install it.
    pub install_runtime: bool,
}

/// A download that runs now.
struct Download {
    /// Which start of it this is, so an old one's end changes nothing.
    run: u64,
    abort: AbortHandle,
    phase: ToolPhase,
    done: u64,
}

/// A whole, checked model file the managed server can load.
#[derive(Clone)]
struct Ready {
    id: String,
    label: String,
    reference: CatalogRef,
    path: PathBuf,
    bytes: u64,
    kv: KvShape,
    native_context: u32,
}

struct State {
    catalog: ModelCatalog,
    runtime: ToolState,
    downloads: HashMap<FileKey, Download>,
    failed: HashMap<FileKey, String>,
    servers: HashMap<ModelServerId, ModelServer>,
    settings: LocalModelSettings,
    ready: Vec<Ready>,
    tested_at: Timestamp,
    shown_at: Option<Instant>,
    runs: u64,
}

/// The local models service of one `maxd`.
pub struct Service {
    hub: Hub,
    db: Db,
    dir: PathBuf,
    home: PathBuf,
    models_dir: PathBuf,
    keys: Folder,
    key_file: PathBuf,
    key: Secret,
    hardware: DeviceHardware,
    support: RuntimeSupport,
    files_from: String,
    install_runtime: bool,
    runner: Arc<Runner>,
    proxy: Mutex<Option<u16>>,
    installing: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

impl Service {
    /// The service, with the servers you added before and a managed key,
    /// made the first time.
    pub fn new(hub: Hub, db: Db, setup: Setup) -> io::Result<Arc<Self>> {
        std::fs::create_dir_all(&setup.dir)?;
        let keys_dir = setup.dir.join("keys");
        let keys = Folder::open(keys_dir.clone())?;
        let key = match keys.read(MANAGED_KEY)? {
            Some(key) if key.expose().trim().len() >= 32 => {
                Secret::new(key.expose().trim().to_owned())
            }
            _ => {
                let key = new_key()?;
                keys.write(MANAGED_KEY, key.expose().as_bytes())?;
                key
            }
        };
        let device = hub.device().id;
        let records = db.model_servers().unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read the model servers you added");
            Vec::new()
        });
        let servers = records
            .iter()
            .filter_map(|record| serde_json::from_str::<ModelServer>(record).ok())
            .filter(|server| matches!(server.kind, ModelServerKind::External { .. }))
            .map(|mut server| {
                server.device = device;
                (server.id, server)
            })
            .collect();
        let state = State {
            catalog: catalog::current(&setup.dir),
            runtime: ToolState::Idle,
            downloads: HashMap::new(),
            failed: HashMap::new(),
            servers,
            settings: settings_of(&hub),
            ready: Vec::new(),
            tested_at: Timestamp::now(),
            shown_at: None,
            runs: 0,
        };
        Ok(Arc::new_cyclic(|weak: &Weak<Self>| Self {
            runner: Runner::new(Arc::new(Link(weak.clone())), key.clone()),
            hub,
            db,
            dir: setup.dir,
            home: setup.home,
            models_dir: setup.models_dir,
            keys,
            key_file: keys_dir.join(MANAGED_KEY),
            key,
            hardware: setup.hardware,
            support: setup.support,
            files_from: setup.files_from,
            install_runtime: setup.install_runtime,
            proxy: Mutex::new(None),
            installing: tokio::sync::Mutex::new(()),
            state: Mutex::new(state),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn managed(&self) -> bool {
        self.support == RuntimeSupport::Managed
    }

    /// The managed server's ID on this device.
    fn managed_id(&self) -> ModelServerId {
        ModelServerId::derive(&format!("model-server:{}:managed", self.hub.device().id))
    }

    /// The Claude config folder of `server`'s local agents.
    fn config_dir(&self, server: ModelServerId) -> PathBuf {
        self.dir.join("claude").join(server.to_string())
    }

    /// The account of `server`'s local agents.
    fn account_of(&self, server: ModelServerId) -> AccountId {
        names::account(
            self.hub.device().id,
            Vendor::Claude,
            &self.config_dir(server).to_string_lossy(),
        )
    }

    fn is_managed_account(&self, account: AccountId) -> bool {
        self.managed() && self.account_of(self.managed_id()) == account
    }

    // Publishing.

    /// Publishes everything now.
    pub fn publish(&self) {
        self.show(false);
    }

    /// Publishes everything, at most once a second when `throttled`.
    fn show(&self, throttled: bool) {
        let settings = settings_of(&self.hub);
        let entities = {
            let mut state = self.lock();
            if throttled
                && state
                    .shown_at
                    .is_some_and(|at| at.elapsed() < PROGRESS_EVERY)
            {
                return;
            }
            state.settings = settings;
            state.ready = self.scan(&state);
            state.shown_at = Some(Instant::now());
            self.entities(&state)
        };
        let device = self.hub.device().id;
        self.hub.replace(
            move |entity| match entity {
                Entity::LocalModels(models) => models.device == device,
                Entity::ModelServer(server) => server.device == device,
                Entity::Account(account) => {
                    account.device == device && account.login == LoginKind::ModelServer
                }
                _ => false,
            },
            entities,
        );
    }

    /// The managed models whose files are whole.
    fn scan(&self, state: &State) -> Vec<Ready> {
        if !self.managed() {
            return Vec::new();
        }
        let mut ready = Vec::new();
        for model in &state.catalog.models {
            let files: Vec<&CatalogFile> = model
                .files
                .iter()
                .filter(|file| {
                    !state
                        .downloads
                        .contains_key(&(model.id.clone(), file.name.clone()))
                        && size(&download::path(&self.models_dir, model, file)) == Some(file.bytes)
                })
                .collect();
            for file in &files {
                let label = if files.len() > 1 {
                    format!("{} {}", model.name, file.quant)
                } else {
                    model.name.clone()
                };
                ready.push(Ready {
                    id: managed_model_id(&model.id, &file.quant),
                    label,
                    reference: CatalogRef {
                        model: model.id.clone(),
                        file: file.name.clone(),
                    },
                    path: download::path(&self.models_dir, model, file),
                    bytes: file.bytes,
                    kv: model.kv,
                    native_context: model.native_context,
                });
            }
        }
        ready
    }

    fn entities(&self, state: &State) -> Vec<Entity> {
        let device = self.hub.device().id;
        let mut entities = vec![Entity::LocalModels(LocalModels {
            device,
            hardware: DeviceHardware {
                gpu: self.runner.budget().or_else(|| self.hardware.gpu.clone()),
                ..self.hardware.clone()
            },
            models_dir: self.models_dir.to_string_lossy().into_owned(),
            disk_free: hardware::disk_free(&self.models_dir) / DISK_STEP * DISK_STEP,
            runtime: ModelRuntime {
                support: self.support,
                version: runtime::installed(&self.home),
                state: state.runtime.clone(),
            },
            catalog: state.catalog.clone(),
            files: self.files(state),
        })];
        if self.managed() {
            let id = self.managed_id();
            let server = ModelServer {
                id,
                device,
                kind: ModelServerKind::Managed,
                label: self.hub.device().name.clone(),
                base_url: None,
                token: false,
                probe: Some(ServerProbe {
                    at: state.tested_at,
                    apis: ServerApis {
                        messages: true,
                        chat: true,
                        responses: true,
                    },
                    context: Some(state.settings.context),
                    error: match &state.runtime {
                        ToolState::Failed { reason } => Some(reason.clone()),
                        _ => None,
                    },
                }),
                models: state
                    .ready
                    .iter()
                    .map(|ready| ServerModel {
                        id: ready.id.clone(),
                        label: ready.label.clone(),
                        catalog: Some(ready.reference.clone()),
                        context: Some(state.settings.context.min(ready.native_context)),
                        load: Some(self.runner.load_of(&ready.id)),
                    })
                    .collect(),
            };
            if !server.models.is_empty() {
                entities.push(Entity::Account(self.account(&server, &state.settings)));
            }
            entities.push(Entity::ModelServer(server));
        }
        let mut servers: Vec<&ModelServer> = state.servers.values().collect();
        servers.sort_by_key(|server| server.id);
        for server in servers {
            entities.push(Entity::Account(self.account(server, &state.settings)));
            entities.push(Entity::ModelServer(server.clone()));
        }
        entities
    }

    /// The account local agents on `server` run as.
    fn account(&self, server: &ModelServer, settings: &LocalModelSettings) -> Account {
        let dir = self.config_dir(server.id);
        if !dir.is_dir() {
            let _ = std::fs::create_dir_all(&dir);
        }
        let picked = pick(settings, server.id, &server.models);
        Account {
            id: self.account_of(server.id),
            device: server.device,
            vendor: Vendor::Claude,
            label: server.label.clone(),
            login: LoginKind::ModelServer,
            origin: None,
            config_dir: dir.to_string_lossy().into_owned(),
            email: None,
            organization: None,
            plan: None,
            models: server
                .models
                .iter()
                .map(|model| ModelChoice {
                    id: model.id.clone(),
                    label: model.label.clone(),
                    default: Some(&model.id) == picked,
                    efforts: Vec::new(),
                    default_effort: None,
                    no_images: true,
                })
                .collect(),
            usage: None,
            usage_failure: None,
            acp: None,
            model_server: Some(server.id),
        }
    }

    /// The catalog files on disk or on the way.
    fn files(&self, state: &State) -> Vec<ModelFile> {
        let mut files = Vec::new();
        for model in &state.catalog.models {
            for file in &model.files {
                let key = (model.id.clone(), file.name.clone());
                let path = download::path(&self.models_dir, model, file);
                let failed = state.failed.get(&key);
                let (state, done) = if let Some(running) = state.downloads.get(&key) {
                    (
                        ModelFileState::Downloading {
                            phase: running.phase,
                        },
                        running.done,
                    )
                } else if size(&path) == Some(file.bytes) {
                    (ModelFileState::Ready, file.bytes)
                } else if let Some(have) = size(&download::part(&path)) {
                    let state =
                        failed.map_or(ModelFileState::Paused, |reason| ModelFileState::Failed {
                            reason: reason.clone(),
                        });
                    (state, have)
                } else if let Some(reason) = failed {
                    (
                        ModelFileState::Failed {
                            reason: reason.clone(),
                        },
                        0,
                    )
                } else {
                    continue;
                };
                files.push(ModelFile {
                    model: model.id.clone(),
                    file: file.name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    bytes_done: done.min(file.bytes),
                    bytes_total: file.bytes,
                    state,
                });
            }
        }
        files
    }

    // What local agents get.

    /// How a local agent on `account` reaches its server, when it's one of
    /// this service's.
    pub fn login(&self, account: AccountId) -> Option<ServerLogin> {
        let state = self.lock();
        if self.is_managed_account(account) {
            let port = (*self.proxy.lock().unwrap_or_else(PoisonError::into_inner))?;
            let ready = self.managed_pick(&state)?;
            return Some(ServerLogin {
                base_url: format!("http://127.0.0.1:{port}"),
                token: self.key.clone(),
                context: Some(state.settings.context.min(ready.native_context)),
                model: Some(ready.id.clone()),
                lean: state.settings.fewer_tools,
            });
        }
        let server = state
            .servers
            .values()
            .find(|server| self.account_of(server.id) == account)?;
        let base_url = server.base_url.clone()?;
        let token = if server.token {
            self.keys.read(&token_name(server.id)).ok().flatten()
        } else {
            None
        };
        let model = pick(&state.settings, server.id, &server.models).cloned();
        let context = model
            .as_deref()
            .and_then(|id| server.models.iter().find(|m| m.id == id))
            .and_then(|m| m.context)
            .or_else(|| server.probe.as_ref().and_then(|probe| probe.context));
        Some(ServerLogin {
            base_url,
            token: token.unwrap_or_else(|| Secret::new(NO_TOKEN.to_owned())),
            context,
            model,
            lean: state.settings.fewer_tools,
        })
    }

    /// What the server of a local agent on `account` does for it now.
    pub fn run_of(
        &self,
        account: AccountId,
        model: Option<&str>,
        used: Option<u32>,
    ) -> Option<LocalRun> {
        if self.is_managed_account(account) {
            let (id, context) = {
                let state = self.lock();
                let ready = model
                    .and_then(|model| state.ready.iter().find(|ready| ready.id == model))
                    .or_else(|| self.managed_pick(&state))?;
                (
                    ready.id.clone(),
                    state.settings.context.min(ready.native_context),
                )
            };
            let load = self.runner.load_of(&id);
            let speed = self.runner.speed_of(&id);
            let context = match load {
                ModelLoad::Loaded { context, .. } => context,
                _ => context,
            };
            return Some(LocalRun {
                loading: matches!(load, ModelLoad::Loading { .. }),
                prompt_per_second: speed.prompt,
                decode_per_second: speed.decode,
                context_used: used,
                context_size: Some(context),
            });
        }
        let state = self.lock();
        let server = state
            .servers
            .values()
            .find(|server| self.account_of(server.id) == account)?;
        let context = model
            .and_then(|model| server.models.iter().find(|m| m.id == model))
            .and_then(|m| m.context)
            .or_else(|| server.probe.as_ref().and_then(|probe| probe.context));
        Some(LocalRun {
            context_used: used,
            context_size: context,
            ..LocalRun::default()
        })
    }

    /// The managed model an agent with none of its own runs.
    fn managed_pick<'a>(&self, state: &'a State) -> Option<&'a Ready> {
        let managed = self.managed_id();
        state
            .settings
            .default_model
            .as_ref()
            .filter(|default| default.server == managed)
            .and_then(|default| state.ready.iter().find(|ready| ready.id == default.model))
            .or_else(|| state.ready.first())
    }

    /// The accounts of every server here.
    fn accounts(&self) -> Vec<AccountId> {
        let state = self.lock();
        let mut accounts: Vec<AccountId> = state
            .servers
            .keys()
            .map(|server| self.account_of(*server))
            .collect();
        if self.managed() {
            accounts.push(self.account_of(self.managed_id()));
        }
        accounts
    }

    fn ran_all(&self) {
        for account in self.accounts() {
            super::ran(account);
        }
    }

    // Actions.

    /// Does `action`.
    pub async fn act(self: &Arc<Self>, action: ModelAction) -> ActionOutcome {
        let done = match action {
            ModelAction::AddServer { label, url, token } => {
                return self.add_server(&label, &url, token).await;
            }
            ModelAction::ProbeServer { url, token } => return self.probe_server(&url, token).await,
            ModelAction::TestServer { server } => self.test_server(server).await,
            ModelAction::RemoveServer { server } => self.remove_server(server),
            ModelAction::Download { model, file } | ModelAction::ResumeDownload { model, file } => {
                self.download(&model, &file)
            }
            ModelAction::PauseDownload { model, file } => {
                self.pause(&model, &file);
                Ok(())
            }
            ModelAction::DeleteModel { model, file } => self.delete(&model, &file),
            ModelAction::Load { server, model } => self.load(server, &model).await,
            ModelAction::Unload { server, model } => self.only_managed(server).map(|()| {
                self.runner.unload(&model);
            }),
        };
        match done {
            Ok(()) => ActionOutcome::Done,
            Err(reason) => ActionOutcome::Refused { reason },
        }
    }

    async fn add_server(&self, label: &str, url: &str, token: Option<Secret>) -> ActionOutcome {
        let Some((base, host)) = base_url(url) else {
            return refused(BAD_URL);
        };
        let catalog = {
            let state = self.lock();
            if state
                .servers
                .values()
                .any(|server| server.base_url.as_deref() == Some(base.as_str()))
            {
                return refused(SAME_URL);
            }
            state.catalog.clone()
        };
        let token = clean(token);
        let id = ModelServerId::generate();
        let found = probe::probe(&base, token.as_ref(), &catalog, false).await;
        if let Some(token) = &token
            && let Err(err) = self.keys.write(&token_name(id), token.expose().as_bytes())
        {
            return refused(&format!("Couldn't keep the server's token: {err}"));
        }
        let label = label.trim();
        let server = ModelServer {
            id,
            device: self.hub.device().id,
            kind: ModelServerKind::External {
                flavor: found.flavor,
            },
            label: if label.is_empty() {
                host
            } else {
                label.to_owned()
            },
            base_url: Some(base),
            token: token.is_some(),
            probe: Some(found.probe),
            models: found.models,
        };
        if let Err(reason) = self.save(&server) {
            let _ = self.keys.remove(&token_name(id));
            return refused(&reason);
        }
        self.lock().servers.insert(id, server);
        self.publish();
        ActionOutcome::ModelServerAdded { server: id }
    }

    /// Probes a URL without adding it or keeping its token.
    async fn probe_server(&self, url: &str, token: Option<Secret>) -> ActionOutcome {
        let Some((base, _)) = base_url(url) else {
            return refused(BAD_URL);
        };
        let catalog = self.lock().catalog.clone();
        let found = probe::probe(&base, clean(token).as_ref(), &catalog, false).await;
        ActionOutcome::ServerProbed {
            flavor: found.flavor,
            probe: found.probe,
            models: found.models,
        }
    }

    async fn test_server(&self, id: ModelServerId) -> Result<(), String> {
        if self.managed() && id == self.managed_id() {
            self.lock().tested_at = Timestamp::now();
            self.publish();
            super::ran(self.account_of(id));
            return Ok(());
        }
        let (base, catalog, has_token) = {
            let state = self.lock();
            let server = state.servers.get(&id).ok_or(NO_SERVER)?;
            (
                server.base_url.clone().ok_or(NO_SERVER)?,
                state.catalog.clone(),
                server.token,
            )
        };
        let token = if has_token {
            self.keys.read(&token_name(id)).ok().flatten()
        } else {
            None
        };
        let found = probe::probe(&base, token.as_ref(), &catalog, false).await;
        let server = {
            let mut state = self.lock();
            let server = state.servers.get_mut(&id).ok_or(NO_SERVER)?;
            server.kind = ModelServerKind::External {
                flavor: found.flavor,
            };
            server.probe = Some(found.probe);
            server.models = found.models;
            server.clone()
        };
        self.save(&server)?;
        self.publish();
        super::ran(self.account_of(id));
        Ok(())
    }

    fn remove_server(&self, id: ModelServerId) -> Result<(), String> {
        if id == self.managed_id() {
            return Err(OWN_SERVER.to_owned());
        }
        if self.lock().servers.remove(&id).is_none() {
            return Err(NO_SERVER.to_owned());
        }
        if let Err(err) = self.db.remove_model_server(id) {
            tracing::warn!(%err, "couldn't forget a model server");
        }
        if let Err(err) = self.keys.remove(&token_name(id)) {
            tracing::warn!(%err, "couldn't delete a model server's token");
        }
        self.publish();
        Ok(())
    }

    /// Keeps a server you added, without its token.
    fn save(&self, server: &ModelServer) -> Result<(), String> {
        let record = serde_json::to_string(server).map_err(|err| err.to_string())?;
        self.db
            .put_model_server(server.id, &record)
            .map_err(|err| format!("Couldn't keep the server: {err}"))
    }

    /// The catalog file `model` and `file` name, as a download.
    fn wanted(&self, model: &str, file: &str) -> Result<(download::Wanted, String), String> {
        let state = self.lock();
        let (entry, found) = find(&state.catalog, model, file).ok_or(NOT_IN_CATALOG)?;
        Ok((
            download::Wanted::of(&self.files_from, &self.models_dir, entry, found),
            managed_model_id(&entry.id, &found.quant),
        ))
    }

    fn download(self: &Arc<Self>, model: &str, file: &str) -> Result<(), String> {
        if !self.managed() {
            return Err(NOT_MANAGED.to_owned());
        }
        let (wanted, _) = self.wanted(model, file)?;
        if size(&wanted.path) == Some(wanted.size) {
            return Ok(());
        }
        let have = download::have(&wanted);
        download::room(hardware::disk_free(&self.models_dir), &wanted, have)?;
        let key = (model.to_owned(), file.to_owned());
        {
            let mut state = self.lock();
            if state.downloads.contains_key(&key) {
                return Ok(());
            }
            state.failed.remove(&key);
            state.runs += 1;
            let run = state.runs;
            let task = tokio::spawn(fetch(Arc::downgrade(self), key.clone(), run, wanted));
            state.downloads.insert(
                key,
                Download {
                    run,
                    abort: task.abort_handle(),
                    phase: if have > 0 {
                        ToolPhase::Verifying
                    } else {
                        ToolPhase::Downloading
                    },
                    done: have,
                },
            );
        }
        self.publish();
        if self.install_runtime
            && runtime::installed(&self.home).as_deref() != Some(runtime::VERSION)
        {
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(err) = service.runtime().await {
                    tracing::warn!(%err, "couldn't install llama-server");
                }
            });
        }
        Ok(())
    }

    fn pause(&self, model: &str, file: &str) {
        let stopped = self
            .lock()
            .downloads
            .remove(&(model.to_owned(), file.to_owned()));
        if let Some(stopped) = stopped {
            stopped.abort.abort();
            self.publish();
        }
    }

    fn delete(&self, model: &str, file: &str) -> Result<(), String> {
        let (wanted, id) = self.wanted(model, file)?;
        let key = (model.to_owned(), file.to_owned());
        if let Some(stopped) = self.lock().downloads.remove(&key) {
            stopped.abort.abort();
        }
        self.runner.unload(&id);
        for path in [wanted.path.clone(), wanted.part()] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(format!("Couldn't delete {}: {err}", path.display())),
            }
        }
        self.lock().failed.remove(&key);
        self.publish();
        self.ran_all();
        Ok(())
    }

    fn only_managed(&self, server: ModelServerId) -> Result<(), String> {
        if self.managed() && server == self.managed_id() {
            Ok(())
        } else if self.lock().servers.contains_key(&server) {
            Err(ONLY_MANAGED.to_owned())
        } else {
            Err(NO_SERVER.to_owned())
        }
    }

    async fn load(&self, server: ModelServerId, model: &str) -> Result<(), String> {
        self.only_managed(server)?;
        if self.spec(model).is_none() {
            return Err(format!("{model} isn't downloaded on this Mac."));
        }
        self.runner.load(model).await
    }

    // The managed server.

    /// How to load `model`.
    fn spec(&self, model: &str) -> Option<Spec> {
        let (ready, settings) = {
            let state = self.lock();
            let ready = state.ready.iter().find(|ready| ready.id == model)?.clone();
            (ready, state.settings.clone())
        };
        if size(&ready.path) != Some(ready.bytes) {
            return None;
        }
        let context = settings.context.min(ready.native_context);
        let budget = self
            .runner
            .budget()
            .or_else(|| self.hardware.gpu.clone())
            .map_or(self.hardware.ram, |gpu| gpu.budget);
        Some(Spec {
            slots: runner::slots_for(ready.bytes, &ready.kv, context, settings.kv_type, budget),
            alias: ready.id,
            path: ready.path,
            context,
            kv: settings.kv_type,
        })
    }

    /// llama-server, installed first when it isn't.
    async fn runtime(&self) -> Result<PathBuf, String> {
        let _one = self.installing.lock().await;
        if runtime::installed(&self.home).as_deref() == Some(runtime::VERSION) {
            return Ok(runtime::program(&self.home));
        }
        if !self.install_runtime {
            return Err(NO_RUNTIME.to_owned());
        }
        let report = |state: ToolState| {
            let throttled = matches!(state, ToolState::Running { .. });
            self.lock().runtime = state;
            self.show(throttled);
        };
        runtime::install(&self.home, &report).await
    }

    async fn start_backend(&self, spec: Spec, progress: LoadProgress) -> Result<Backend, String> {
        let program = self.runtime().await?;
        self.learn_budget(&program).await;
        runner::launch(
            &program,
            &self.key_file,
            &self.dir.join("logs"),
            &spec,
            &self.key,
            progress.as_ref(),
        )
        .await
    }

    /// Asks llama-server once for the GPU budget Metal gives, which sizes
    /// the context and slots better than the estimate.
    async fn learn_budget(&self, program: &Path) {
        if self.runner.budget().is_some() {
            return;
        }
        if let Some(budget) = runner::listed_budget(program).await {
            self.runner.set_budget(budget);
            self.publish();
        }
    }

    /// Stops what an earlier `maxd` left running and takes the proxy's
    /// port, before any local agent asks for it. None when this device
    /// runs no managed server or the port can't be had.
    pub fn prepare(&self) -> Option<std::net::TcpListener> {
        if !self.managed() {
            return None;
        }
        runner::stop_leftovers(&self.dir.join("logs"));
        self.bind()
            .inspect_err(|err| tracing::warn!(%err, "couldn't start the model proxy"))
            .ok()
    }

    /// Runs the service until `maxd` stops: the proxy on `listener`, the
    /// catalog refresh when `online`, and the tick.
    pub async fn run(self: Arc<Self>, online: bool, listener: Option<std::net::TcpListener>) {
        if let Some(listener) = listener {
            match TcpListener::from_std(listener) {
                Ok(listener) => {
                    tokio::spawn(self.runner.clone().serve(listener));
                }
                Err(err) => tracing::warn!(%err, "couldn't start the model proxy"),
            }
            if runtime::installed(&self.home).as_deref() == Some(runtime::VERSION) {
                let service = self.clone();
                tokio::spawn(async move {
                    service.learn_budget(&runtime::program(&service.home)).await;
                });
            }
        }
        if online {
            let service = self.clone();
            tokio::spawn(async move {
                let have = service.lock().catalog.clone();
                if let Some(fresh) = catalog::refresh(&service.dir, &have).await {
                    service.lock().catalog = fresh;
                    service.publish();
                }
            });
        }
        let mut every = tokio::time::interval(TICK);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            self.tick().await;
        }
    }

    /// Binds the proxy on 127.0.0.1, on the port it had before when it can.
    fn bind(&self) -> io::Result<std::net::TcpListener> {
        let file = self.dir.join(PORT_FILE);
        let kept = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| text.trim().parse::<u16>().ok())
            .filter(|port| *port > 0);
        let any = || std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0));
        let listener = match kept {
            Some(port) => match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::warn!(port, %err, "the model proxy's port is taken; it moves");
                    any()?
                }
            },
            None => any()?,
        };
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        if kept != Some(port) {
            std::fs::write(&file, port.to_string())?;
        }
        *self.proxy.lock().unwrap_or_else(PoisonError::into_inner) = Some(port);
        Ok(listener)
    }

    async fn tick(self: &Arc<Self>) {
        if self.managed() {
            self.runner.tick().await;
        }
        let settings = settings_of(&self.hub);
        let changed = self.lock().settings != settings;
        if changed {
            self.publish();
            self.ran_all();
        }
    }
}

/// Downloads one file, then shows how it ended.
async fn fetch(service: Weak<Service>, key: FileKey, run: u64, wanted: download::Wanted) {
    let progress = {
        let service = service.clone();
        let key = key.clone();
        move |done: u64, _total: Option<u64>| {
            let Some(service) = service.upgrade() else {
                return;
            };
            {
                let mut state = service.lock();
                let Some(running) = state.downloads.get_mut(&key).filter(|d| d.run == run) else {
                    return;
                };
                running.done = done;
                running.phase = ToolPhase::Downloading;
            }
            service.show(true);
        }
    };
    let result = download::fetch(&wanted, &progress).await;
    let Some(service) = service.upgrade() else {
        return;
    };
    {
        let mut state = service.lock();
        if state.downloads.get(&key).is_none_or(|d| d.run != run) {
            return;
        }
        state.downloads.remove(&key);
        if let Err(reason) = &result {
            tracing::warn!(model = %key.0, file = %key.1, %reason, "a model download failed");
            state.failed.insert(key, reason.clone());
        }
    }
    service.publish();
    if result.is_ok() {
        service.ran_all();
    }
}

/// What the runner reaches the service through. It holds the service
/// weakly, since the service holds the runner.
struct Link(Weak<Service>);

impl Host for Link {
    fn spec(&self, model: &str) -> Option<Spec> {
        self.0.upgrade()?.spec(model)
    }

    fn models(&self) -> Vec<String> {
        self.0.upgrade().map_or_else(Vec::new, |service| {
            service
                .lock()
                .ready
                .iter()
                .map(|ready| ready.id.clone())
                .collect()
        })
    }

    fn start(
        &self,
        spec: Spec,
        progress: LoadProgress,
    ) -> BoxFuture<'static, Result<Backend, String>> {
        let service = self.0.clone();
        Box::pin(async move {
            let service = service.upgrade().ok_or("maxd is stopping.")?;
            service.start_backend(spec, progress).await
        })
    }

    fn changed(&self) {
        if let Some(service) = self.0.upgrade() {
            service.publish();
            super::ran(service.account_of(service.managed_id()));
        }
    }

    fn busy(&self) -> bool {
        let Some(service) = self.0.upgrade() else {
            return false;
        };
        let account = service.account_of(service.managed_id());
        !service
            .hub
            .pick(|entity| match entity {
                Entity::Agent(agent)
                    if agent.account == Some(account)
                        && matches!(
                            agent.activity,
                            Activity::Starting | Activity::Busy | Activity::BusyInSubagents
                        ) =>
                {
                    Some(())
                }
                _ => None,
            })
            .is_empty()
    }

    fn idle_minutes(&self) -> u16 {
        self.0
            .upgrade()
            .map_or(0, |service| service.lock().settings.idle_unload_minutes)
    }
}

/// The device's local model settings.
fn settings_of(hub: &Hub) -> LocalModelSettings {
    match hub.get(&EntityKey::DeviceSettings(hub.device().id)) {
        Some(Entity::DeviceSettings(settings)) => *settings.local_models,
        _ => LocalModelSettings::default(),
    }
}

/// The model of `models` an agent with none of its own runs on `server`.
fn pick<'a>(
    settings: &LocalModelSettings,
    server: ModelServerId,
    models: &'a [ServerModel],
) -> Option<&'a String> {
    settings
        .default_model
        .as_ref()
        .filter(|default| default.server == server)
        .and_then(|default| models.iter().find(|model| model.id == default.model))
        .or_else(|| models.first())
        .map(|model| &model.id)
}

/// The catalog model and file `model` and `file` name.
fn find<'a>(
    catalog: &'a ModelCatalog,
    model: &str,
    file: &str,
) -> Option<(&'a CatalogModel, &'a CatalogFile)> {
    let entry = catalog.models.iter().find(|m| m.id == model)?;
    let found = entry.files.iter().find(|f| f.name == file)?;
    Some((entry, found))
}

/// A server URL as a base for its APIs, and a name for it: http or https
/// with a host, nothing else, and no `/v1` at the end.
fn base_url(url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?.to_owned();
    let name = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let text = parsed.as_str().trim_end_matches('/');
    let base = text
        .strip_suffix("/v1")
        .unwrap_or(text)
        .trim_end_matches('/');
    Some((base.to_owned(), name))
}

/// The keys file of a server's token.
fn token_name(server: ModelServerId) -> String {
    format!("server-{server}")
}

/// A file's size, when it's a file.
fn size(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.len())
}

/// A new managed key: 32 random bytes, in hex.
fn new_key() -> io::Result<Secret> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("no random bytes"))?;
    Ok(Secret::new(crate::tools::install::fetch::hex(&bytes)))
}

/// A token as typed, trimmed, or None when nothing's left.
fn clean(token: Option<Secret>) -> Option<Secret> {
    token
        .map(|token| token.expose().trim().to_owned())
        .filter(|token| !token.is_empty())
        .map(Secret::new)
}

fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests;

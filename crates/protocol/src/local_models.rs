// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Local models, since [`crate::message::LOCAL_MODELS_SINCE`]: a model a
//! device runs itself, or one on a server URL you gave it, driving the
//! Claude Code loop with no Claude account, quota or sign-in.
//!
//! On the wire a local agent is vendor Claude with an account whose login is
//! [`crate::model::LoginKind::ModelServer`] and whose
//! [`crate::model::Account::model_server`] names its server. Every client
//! shows it as Local, never as Claude. An older client drops the account,
//! since it can't read the login kind, and drops both entities here.
//!
//! The view carries:
//!
//! - a [`ModelServer`] entity per server a device runs or reaches: the
//!   llama-server its `maxd` manages, or a URL you gave it, with what the
//!   last probe found and the models it serves;
//! - a [`LocalModels`] entity per device: its hardware for the fit
//!   estimate, the curated catalog, the model files on disk or on the way,
//!   and whether it can run a managed server at all.
//!
//! You act on them with [`crate::ClientMessage::ModelAction`]. A device's
//! choices (idle unload, KV cache type, context size, fewer tools and the
//! default model) are [`LocalModelSettings`] in its device settings.
//!
//! A server's token never leaves the device. `maxd` keeps it in its secrets
//! store, and the entity says only whether there is one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{DeviceId, ModelServerId, SignInId};
use crate::secret::Secret;
use crate::state::{ToolPhase, ToolState};
use crate::time::Timestamp;

/// A model server a device's `maxd` runs or reaches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelServer {
    /// The server's ID.
    pub id: ModelServerId,
    /// The device whose `maxd` runs or reaches it. Agents on other devices
    /// reach a managed server through this device's `maxd` link.
    pub device: DeviceId,
    /// Whether `maxd` runs it or it's a URL you gave.
    pub kind: ModelServerKind,
    /// Your name for it, or the device's name for a managed one.
    pub label: String,
    /// Where it answers, such as `http://devbox:11434`. None for a managed
    /// server, which listens on a loopback port `maxd` picks each start.
    pub base_url: Option<String>,
    /// Whether `maxd` holds a token for it. The token itself stays in
    /// `maxd`'s secrets store and is never sent.
    #[serde(default)]
    pub token: bool,
    /// Whether it's a hosted API rather than hardware you run: a known
    /// cloud host, or a server added with the Cloud service switch on. A
    /// cloud server's agents get every tool and the model's own output
    /// limit, and show under the server's name rather than Local. Since
    /// [`crate::message::CLOUD_SERVERS_SINCE`]; an older `maxd` sends none,
    /// read as false.
    #[serde(default)]
    pub cloud: bool,
    /// Whether `maxd` holds an admin key for it, which reads the account's
    /// balance on OpenRouter. The key itself stays in `maxd`'s secrets
    /// store and is never sent. Since
    /// [`crate::message::CLOUD_SERVERS_SINCE`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin_key: bool,
    /// What the last probe found, or none before the first.
    pub probe: Option<ServerProbe>,
    /// The models it serves. For a managed server, the model files ready
    /// on its device; for a URL, what its model list returned. One this
    /// build can't read is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<ServerModel>)]
    pub models: Vec<ServerModel>,
    /// What the server's key has spent, for a server that meters it, such
    /// as OpenRouter. None for one that doesn't, or before the first read.
    /// Since [`crate::message::CLOUD_SERVERS_SINCE`]; an older client
    /// reads none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<ServerCredits>,
}

/// What a model server's key has spent and has left, in whole US cents,
/// as the server's own key endpoint reports it. It covers the one key
/// `maxd` holds, plus the account's balance when `maxd` holds an admin key
/// for the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ServerCredits {
    /// When the figures were read.
    pub at: Timestamp,
    /// What the key has spent in all.
    pub used_cents: u32,
    /// What it spent today, when the server says.
    #[serde(default)]
    pub today_cents: Option<u32>,
    /// What it spent this week, when the server says.
    #[serde(default)]
    pub week_cents: Option<u32>,
    /// What it spent this month, when the server says.
    #[serde(default)]
    pub month_cents: Option<u32>,
    /// The key's spending limit. None for a key without one.
    #[serde(default)]
    pub limit_cents: Option<u32>,
    /// What's left under the limit. None for a key without one.
    #[serde(default)]
    pub remaining_cents: Option<u32>,
    /// How often the limit starts over, as the server names it, such as
    /// `monthly`. None for a limit that never resets.
    #[serde(default)]
    pub resets: Option<String>,
    /// What's left on the account: what was bought less what was used,
    /// never below zero. None without an admin key.
    #[serde(default)]
    pub balance_cents: Option<u32>,
    /// What the account bought in all. None without an admin key.
    #[serde(default)]
    pub bought_cents: Option<u32>,
    /// Why the last read failed, for a person to read. The figures are the
    /// last good ones then. None when the last read worked.
    #[serde(default)]
    pub error: Option<String>,
}

/// Whether `maxd` runs a server or reaches it at a URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelServerKind {
    /// llama-server, which `maxd` installs, starts on 127.0.0.1 with a key
    /// of its own, and stops when idle.
    Managed,
    /// A server URL you gave.
    External {
        /// What the probe says answers there.
        flavor: ServerFlavor,
    },
}

/// What kind of server answers at a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerFlavor {
    /// llama.cpp's llama-server.
    LlamaCpp,
    /// Ollama.
    Ollama,
    /// LM Studio.
    LmStudio,
    /// vLLM.
    Vllm,
    /// Something else, or not yet probed.
    Other,
}

/// What a probe of a server found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ServerProbe {
    /// When it ran.
    pub at: Timestamp,
    /// Which APIs answered.
    pub apis: ServerApis,
    /// The context size the server runs with, in tokens, when it says.
    pub context: Option<u32>,
    /// Why the probe failed, for a person to read. None when the server
    /// answered.
    pub error: Option<String>,
}

/// Which APIs a server answered on. Only a server with the Anthropic
/// Messages API can run a local agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ServerApis {
    /// `/v1/messages`, the Anthropic Messages API Claude Code talks.
    #[serde(default)]
    pub messages: bool,
    /// `/v1/chat/completions`.
    #[serde(default)]
    pub chat: bool,
    /// `/v1/responses`.
    #[serde(default)]
    pub responses: bool,
}

/// One model a server serves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ServerModel {
    /// The ID the server takes in a request, such as `qwen3.6:35b`.
    pub id: String,
    /// The name to show.
    pub label: String,
    /// The catalog entry and file it runs, for a managed server's model or
    /// a URL's model `maxd` recognized.
    pub catalog: Option<CatalogRef>,
    /// The context size it runs with, in tokens, when known.
    pub context: Option<u32>,
    /// Whether it's in memory. None when the server doesn't say, as with
    /// most URLs.
    pub load: Option<ModelLoad>,
    /// How fast a managed server ran it in its last turn, so a local agent
    /// on another device shows its speed line too. Since
    /// [`crate::message::LOCAL_MODELS_SINCE`]; an older client reads none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<ModelSpeed>,
    /// The most tokens it writes in one answer, when the server says and
    /// it's below the context. Since
    /// [`crate::message::CLOUD_SERVERS_SINCE`]; an older `maxd` sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
}

/// How fast a model ran, in whole tokens per second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelSpeed {
    /// Prompt tokens read per second.
    #[serde(default)]
    pub prompt_per_second: Option<u32>,
    /// Tokens written per second.
    #[serde(default)]
    pub decode_per_second: Option<u32>,
}

/// A Mac's managed model server, as the devices it links to see it in
/// their peer book, so their local agents can use it over the `maxd`
/// link. Its load and speed stay in the Mac's own entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LinkedModels {
    /// The server.
    pub server: ModelServerId,
    /// Its name, the Mac's.
    pub label: String,
    /// The models it serves, with no load or speed. One this build can't
    /// read is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<ServerModel>)]
    pub models: Vec<ServerModel>,
}

/// A catalog file's model ID on a managed server, `<catalog id>@<quant>`
/// in lowercase, such as `qwen3.6-35b-a3b@ud-q4_k_xl`. `maxd` gives it to
/// llama-server as its alias, so the server reports the same ID. A URL's
/// models keep the IDs their server gives them.
#[must_use]
pub fn managed_model_id(model: &str, quant: &str) -> String {
    format!("{model}@{quant}").to_lowercase()
}

/// One file of one catalog model.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct CatalogRef {
    /// The catalog model's ID.
    pub model: String,
    /// The file's name.
    pub file: String,
}

/// Whether a model is in memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelLoad {
    /// Not loaded. A local agent's turn loads it.
    Unloaded,
    /// Loading into memory.
    Loading {
        /// How much of the model file is in memory, from 0 to 99 percent,
        /// when `maxd` can tell. It stays below 100 until the server
        /// serves. Since [`crate::message::LOCAL_MODELS_SINCE`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
    },
    /// Loaded and serving.
    Loaded {
        /// How many requests it serves at once, each with its own KV cache.
        slots: u8,
        /// The context size of each slot, in tokens.
        context: u32,
        /// When it finished loading.
        since: Timestamp,
    },
    /// The last load failed.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
}

/// What one device knows for local models: its hardware, the catalog and
/// the model files it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LocalModels {
    /// The device.
    pub device: DeviceId,
    /// Its hardware, for the fit estimate.
    pub hardware: DeviceHardware,
    /// Where model files go, as an absolute path on the device, such as
    /// `~/.local/share/maxhq/models` expanded.
    pub models_dir: String,
    /// Free bytes on the disk that holds `models_dir`.
    #[specta(type = specta_typescript::Number)]
    pub disk_free: u64,
    /// Whether it can run a managed server, and that server's install.
    pub runtime: ModelRuntime,
    /// The curated models it offers.
    pub catalog: ModelCatalog,
    /// The model files on disk or on their way. One this build can't read
    /// is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<ModelFile>)]
    pub files: Vec<ModelFile>,
}

/// A device's hardware, as far as it matters for running a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DeviceHardware {
    /// The chip, such as "Apple M4 Max", when known.
    pub chip: Option<String>,
    /// The GPU's core count, when known.
    pub gpu_cores: Option<u16>,
    /// Memory in bytes.
    #[specta(type = specta_typescript::Number)]
    pub ram: u64,
    /// The memory a GPU can use for a model. None for a device without a
    /// GPU `maxd` can use, which is too slow for agents.
    pub gpu: Option<GpuMemory>,
}

/// How much memory a GPU can give a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GpuMemory {
    /// The budget in bytes.
    #[specta(type = specta_typescript::Number)]
    pub budget: u64,
    /// Where the figure came from.
    pub source: GpuBudgetSource,
}

/// Where a GPU memory budget came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GpuBudgetSource {
    /// Metal's recommended working set size.
    Metal,
    /// The `iogpu.wired_limit_mb` limit someone set on the Mac.
    WiredLimit,
    /// macOS's default share of memory, estimated from the memory size.
    Estimate,
}

/// Whether a device can run a managed model server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelRuntime {
    /// What the device supports.
    pub support: RuntimeSupport,
    /// The installed llama-server's version, or none before an install.
    pub version: Option<String>,
    /// What its install or update is doing.
    pub state: ToolState,
}

/// What a device supports for local models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSupport {
    /// A managed server and the catalog, as on an Apple silicon Mac.
    Managed,
    /// No GPU `maxd` can use: too slow for agents, so only server URLs.
    NoGpu,
    /// A GPU, but `maxd` doesn't manage a server here yet: server URLs
    /// only.
    UrlOnly,
}

/// The curated models a device offers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelCatalog {
    /// When the catalog was last refreshed, or when the shipped one was
    /// made.
    pub updated_at: Timestamp,
    /// The models, best pick first. One this build can't read is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<CatalogModel>)]
    pub models: Vec<CatalogModel>,
}

/// One curated model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CatalogModel {
    /// The catalog's ID for it, such as `qwen3.6-35b-a3b`.
    pub id: String,
    /// The name to show, such as "Qwen3.6-35B-A3B".
    pub name: String,
    /// The Hugging Face repo the files come from, as `org/repo`.
    pub repo: String,
    /// The repo commit the files are pinned to.
    pub commit: String,
    /// The licence, such as "Apache-2.0".
    pub license: String,
    /// When the model came out, as `YYYY-MM-DD`.
    pub released: String,
    /// Parameters in billions.
    pub params_b: f32,
    /// Parameters active per token in billions, less than `params_b` for a
    /// mixture of experts.
    pub active_params_b: f32,
    /// The files to pick from, one per quantization.
    pub files: Vec<CatalogFile>,
    /// What the KV cache needs per token.
    pub kv: KvShape,
    /// The longest context the model takes, in tokens.
    pub native_context: u32,
    /// A line or two on what it's good at, in English.
    pub notes: String,
    /// Published benchmark scores.
    pub scores: Vec<ModelScore>,
}

/// One quantized file of a catalog model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CatalogFile {
    /// The file's name in the repo.
    pub name: String,
    /// The quantization, such as "Q4_K_M".
    pub quant: String,
    /// Its size in bytes.
    #[specta(type = specta_typescript::Number)]
    pub bytes: u64,
    /// Its SHA-256, in hex.
    pub sha256: String,
    /// Whether it's the file to pick for this model.
    #[serde(default)]
    pub recommended: bool,
}

/// What a model's KV cache needs, to estimate its size for a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct KvShape {
    /// Layers whose attention covers the whole context.
    pub full_layers: u16,
    /// KV heads per layer.
    pub kv_heads: u16,
    /// Each head's size.
    pub head_dim: u16,
    /// Layers whose attention covers only a sliding window.
    #[serde(default)]
    pub swa_layers: u16,
    /// The sliding window in tokens, for `swa_layers`.
    #[serde(default)]
    pub swa_window: u32,
}

/// A published benchmark score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelScore {
    /// The benchmark, such as "SWE-bench Verified".
    pub benchmark: String,
    /// The score, in percent.
    pub value: f32,
    /// The agent harness it ran in, such as "Claude Code", when known.
    pub harness: Option<String>,
    /// Who reported it.
    pub source: ScoreSource,
    /// Where it was reported.
    pub url: String,
    /// When, as `YYYY-MM-DD`.
    pub date: String,
}

/// Who reported a score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScoreSource {
    /// The model's maker.
    Vendor,
    /// Someone else.
    Independent,
}

/// A model file on a device's disk or on its way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelFile {
    /// The catalog model's ID.
    pub model: String,
    /// The file's name.
    pub file: String,
    /// Where it is, as an absolute path on the device:
    /// `<models_dir>/<org>/<repo>/<file>`.
    pub path: String,
    /// Bytes on disk so far.
    #[specta(type = specta_typescript::Number)]
    pub bytes_done: u64,
    /// The whole file's size in bytes.
    #[specta(type = specta_typescript::Number)]
    pub bytes_total: u64,
    /// Where it stands.
    pub state: ModelFileState,
}

/// Where a model file stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelFileState {
    /// Downloading, or checking what came down.
    Downloading {
        /// Downloading or verifying.
        phase: ToolPhase,
    },
    /// Paused, keeping what came down so far.
    Paused,
    /// On disk and checked, ready to load.
    Ready,
    /// The download failed, keeping what came down when it can resume.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
}

/// A device's choices for local models, in its device settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LocalModelSettings {
    /// Minutes with no local agent turn running before a managed server
    /// unloads its model. 0 keeps it loaded.
    #[serde(default = "default_idle_unload_minutes")]
    pub idle_unload_minutes: u16,
    /// How a managed server stores its KV cache.
    #[serde(default)]
    pub kv_type: KvCacheType,
    /// The context size a managed server loads a model with, in tokens.
    #[serde(default = "default_context")]
    pub context: u32,
    /// Whether local agents get a lean set of MaxHQ tools, which leaves
    /// more of a small context for the work.
    #[serde(default = "crate::model::yes")]
    pub fewer_tools: bool,
    /// The model a new local agent on this device starts with, or none to
    /// use the catalog's default once it's ready.
    #[serde(default)]
    pub default_model: Option<LocalModelPick>,
    /// Whether local agents on the devices this one links to may use its
    /// managed server over the `maxd` link. Only this device changes it.
    /// An older `maxd` reads it as on, and doesn't share either way.
    #[serde(default = "crate::model::yes")]
    pub linked_devices: bool,
}

/// The idle unload default, in minutes.
pub const DEFAULT_IDLE_UNLOAD_MINUTES: u16 = 15;

/// The context size default, in tokens.
pub const DEFAULT_CONTEXT: u32 = 65_536;

fn default_idle_unload_minutes() -> u16 {
    DEFAULT_IDLE_UNLOAD_MINUTES
}

fn default_context() -> u32 {
    DEFAULT_CONTEXT
}

impl Default for LocalModelSettings {
    fn default() -> Self {
        Self {
            idle_unload_minutes: DEFAULT_IDLE_UNLOAD_MINUTES,
            kv_type: KvCacheType::default(),
            context: DEFAULT_CONTEXT,
            fewer_tools: true,
            default_model: None,
            linked_devices: true,
        }
    }
}

/// How a managed server stores its KV cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub enum KvCacheType {
    /// 16-bit floats, the exact cache.
    #[default]
    #[serde(rename = "f16")]
    F16,
    /// 8-bit, about half the memory for a small loss.
    #[serde(rename = "q8_0")]
    Q8_0,
}

/// One model on one server.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct LocalModelPick {
    /// The server.
    pub server: ModelServerId,
    /// The model's ID on that server.
    pub model: String,
}

/// Something to do with a device's local models, in
/// [`crate::ClientMessage::ModelAction`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ModelAction {
    /// Adds a server URL and probes it. Answers
    /// [`crate::message::ActionOutcome::ModelServerAdded`].
    AddServer {
        /// Your name for it.
        label: String,
        /// Its base URL, such as `http://devbox:11434`.
        url: String,
        /// A token it wants, which `maxd` keeps in its secrets store.
        #[serde(default)]
        token: Option<Secret>,
        /// Whether you marked it a cloud service. A known cloud host is one
        /// either way. Since [`crate::message::CLOUD_SERVERS_SINCE`]; an
        /// older client sends none, read as false.
        #[serde(default)]
        cloud: bool,
    },
    /// Probes a URL without adding it, to see what answers there before
    /// it's added. Answers
    /// [`crate::message::ActionOutcome::ServerProbed`].
    ProbeServer {
        /// Its base URL, such as `http://devbox:11434`.
        url: String,
        /// A token it wants. `maxd` keeps nothing of it.
        #[serde(default)]
        token: Option<Secret>,
    },
    /// Probes a server again.
    TestServer {
        /// The server.
        server: ModelServerId,
    },
    /// Removes a server URL, with its account, token and admin key. A
    /// managed server can't be removed.
    RemoveServer {
        /// The server.
        server: ModelServerId,
    },
    /// Keeps an admin key for an OpenRouter server, after one read of the
    /// account's balance with it, or removes the one kept. It answers
    /// [`crate::message::ActionOutcome::Done`], or `Refused` with why,
    /// such as a key OpenRouter turned down, of which `maxd` keeps
    /// nothing. Since [`crate::message::CLOUD_SERVERS_SINCE`].
    SetServerAdminKey {
        /// The server.
        server: ModelServerId,
        /// The admin key, or None to remove the one kept.
        #[serde(default)]
        key: Option<Secret>,
    },
    /// Downloads one file of a catalog model.
    Download {
        /// The catalog model's ID.
        model: String,
        /// The file's name.
        file: String,
    },
    /// Pauses a download.
    PauseDownload {
        /// The catalog model's ID.
        model: String,
        /// The file's name.
        file: String,
    },
    /// Resumes a paused or failed download.
    ResumeDownload {
        /// The catalog model's ID.
        model: String,
        /// The file's name.
        file: String,
    },
    /// Deletes a model file, or stops its download and deletes what came
    /// down.
    DeleteModel {
        /// The catalog model's ID.
        model: String,
        /// The file's name.
        file: String,
    },
    /// Loads a model on a managed server now, rather than at the next
    /// local agent turn.
    Load {
        /// The server.
        server: ModelServerId,
        /// The model's ID on that server.
        model: String,
    },
    /// Unloads a model from a managed server.
    Unload {
        /// The server.
        server: ModelServerId,
        /// The model's ID on that server.
        model: String,
    },
    /// Starts a Sign in with OpenRouter round, which makes an OpenRouter
    /// key for this device. `maxd` keeps the round's PKCE verifier and
    /// answers [`crate::message::ActionOutcome::SignIn`] with the page to
    /// open and the port the app listens on. Since protocol 65
    /// ([`crate::message::CLOUD_SERVERS_SINCE`]).
    BeginOpenRouterSignIn {
        /// The name a new OpenRouter server gets, or `OpenRouter` with none.
        /// One the device has already keeps its name.
        #[serde(default)]
        label: Option<String>,
        /// The key's spending limit in US cents, which OpenRouter enforces,
        /// or none for no limit.
        #[serde(default)]
        limit_cents: Option<u32>,
    },
    /// Finishes a Sign in with OpenRouter round with the code the browser
    /// brought back. `maxd` trades it for a key, keeps the key in its
    /// secrets store, and adds the OpenRouter server, or gives the
    /// device's OpenRouter server the new key, then tests it. Answers
    /// [`crate::message::ActionOutcome::ModelServerAdded`] with that
    /// server. Since protocol 65 ([`crate::message::CLOUD_SERVERS_SINCE`]).
    FinishOpenRouterSignIn {
        /// The round, from its `SignIn` answer.
        sign_in: SignInId,
        /// The code the callback brought.
        code: Secret,
    },
}

/// What a local agent's server is doing for it, in its live state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LocalRun {
    /// Whether its model is loading, so the turn waits.
    #[serde(default)]
    pub loading: bool,
    /// Prompt tokens read per second in the current or last turn.
    #[serde(default)]
    pub prompt_per_second: Option<f32>,
    /// Tokens written per second in the current or last turn.
    #[serde(default)]
    pub decode_per_second: Option<f32>,
    /// Tokens of the server's context in use.
    #[serde(default)]
    pub context_used: Option<u32>,
    /// The server's context size in tokens.
    #[serde(default)]
    pub context_size: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdempotencyKey;
    use crate::message::{ActionOutcome, ClientMessage, CLOUD_SERVERS_SINCE, LOCAL_MODELS_SINCE};
    use crate::state::{Entity, EntityKey};

    fn device() -> DeviceId {
        DeviceId::derive("device:example")
    }

    #[test]
    fn a_managed_model_is_named_by_its_catalog_id_and_quant_in_lowercase() {
        assert_eq!(
            managed_model_id("qwen3.6-35b-a3b", "UD-Q4_K_XL"),
            "qwen3.6-35b-a3b@ud-q4_k_xl"
        );
        assert_eq!(
            managed_model_id("gpt-oss-20b", "MXFP4"),
            "gpt-oss-20b@mxfp4"
        );
    }

    fn server() -> ModelServer {
        ModelServer {
            id: ModelServerId::derive("server:example"),
            device: device(),
            kind: ModelServerKind::External {
                flavor: ServerFlavor::Ollama,
            },
            label: "Ollama on devbox".into(),
            base_url: Some("http://devbox:11434".into()),
            token: true,
            cloud: false,
            admin_key: true,
            probe: Some(ServerProbe {
                at: Timestamp(1_000),
                apis: ServerApis {
                    messages: true,
                    chat: true,
                    responses: false,
                },
                context: Some(32_768),
                error: None,
            }),
            models: vec![ServerModel {
                id: "qwen3.6:35b".into(),
                label: "qwen3.6:35b".into(),
                catalog: None,
                context: Some(32_768),
                load: Some(ModelLoad::Loaded {
                    slots: 1,
                    context: 32_768,
                    since: Timestamp(900),
                }),
                speed: Some(ModelSpeed {
                    prompt_per_second: Some(1_180),
                    decode_per_second: Some(74),
                }),
                max_output: None,
            }],
            credits: Some(ServerCredits {
                at: Timestamp(1_100),
                used_cents: 258,
                today_cents: Some(31),
                week_cents: Some(120),
                month_cents: Some(258),
                limit_cents: Some(1_000),
                remaining_cents: Some(742),
                resets: Some("monthly".into()),
                balance_cents: Some(742),
                bought_cents: Some(1_000),
                error: None,
            }),
        }
    }

    fn local_models() -> LocalModels {
        LocalModels {
            device: device(),
            hardware: DeviceHardware {
                chip: Some("Apple M4 Max".into()),
                gpu_cores: Some(32),
                ram: 36 * (1 << 30),
                gpu: Some(GpuMemory {
                    budget: 27 * (1 << 30),
                    source: GpuBudgetSource::Metal,
                }),
            },
            models_dir: "/Users/example/.local/share/maxhq/models".into(),
            disk_free: 200_000_000_000,
            runtime: ModelRuntime {
                support: RuntimeSupport::Managed,
                version: Some("b7000".into()),
                state: ToolState::Idle,
            },
            catalog: ModelCatalog {
                updated_at: Timestamp(500),
                models: vec![CatalogModel {
                    id: "qwen3.6-35b-a3b".into(),
                    name: "Qwen3.6-35B-A3B".into(),
                    repo: "example-org/Qwen3.6-35B-A3B-GGUF".into(),
                    commit: "0123456789abcdef0123456789abcdef01234567".into(),
                    license: "Apache-2.0".into(),
                    released: "2026-04-16".into(),
                    params_b: 35.0,
                    active_params_b: 3.0,
                    files: vec![CatalogFile {
                        name: "model-Q4.gguf".into(),
                        quant: "Q4_K_M".into(),
                        bytes: 22_400_000_000,
                        sha256: "ab".repeat(32),
                        recommended: true,
                    }],
                    kv: KvShape {
                        full_layers: 10,
                        kv_heads: 2,
                        head_dim: 256,
                        swa_layers: 0,
                        swa_window: 0,
                    },
                    native_context: 262_144,
                    notes: "The default.".into(),
                    scores: vec![ModelScore {
                        benchmark: "SWE-bench Verified".into(),
                        value: 73.4,
                        harness: None,
                        source: ScoreSource::Vendor,
                        url: "https://example.com/model".into(),
                        date: "2026-04-16".into(),
                    }],
                }],
            },
            files: vec![ModelFile {
                model: "qwen3.6-35b-a3b".into(),
                file: "model-Q4.gguf".into(),
                path: "/Users/example/.local/share/maxhq/models/example-org/Qwen3.6-35B-A3B-GGUF/model-Q4.gguf".into(),
                bytes_done: 1_000,
                bytes_total: 22_400_000_000,
                state: ModelFileState::Downloading {
                    phase: ToolPhase::Downloading,
                },
            }],
        }
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn entities_round_trip_with_their_keys() {
        let server = Entity::ModelServer(server());
        assert_eq!(round_trip(&server), server);
        assert_eq!(
            server.key(),
            EntityKey::ModelServer(ModelServerId::derive("server:example"))
        );
        let models = Entity::LocalModels(local_models());
        assert_eq!(round_trip(&models), models);
        assert_eq!(models.key(), EntityKey::LocalModels(device()));
        let json = serde_json::to_value(&models).unwrap();
        assert_eq!(json["entity"], "local_models");
        assert_eq!(json["files"][0]["state"]["state"], "downloading");
    }

    #[test]
    fn a_server_entry_this_build_cant_read_is_left_out() {
        let mut json = serde_json::to_value(server()).unwrap();
        json["models"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "id": "x", "load": { "state": "newer" } }));
        let read: ModelServer = serde_json::from_value(json).unwrap();
        assert_eq!(read.models.len(), 1);
    }

    #[test]
    fn a_server_without_credits_writes_none_and_reads_as_before() {
        let json = serde_json::to_value(ModelServer {
            admin_key: false,
            credits: None,
            ..server()
        })
        .unwrap();
        assert!(json.get("credits").is_none());
        assert!(json.get("admin_key").is_none());
        // A server from before cloud servers reads with no credits.
        let read: ModelServer = serde_json::from_value(json).unwrap();
        assert_eq!(read.credits, None);
        assert!(!read.admin_key);
        // Figures left out read as unknown.
        let credits: ServerCredits =
            serde_json::from_value(serde_json::json!({"at": 5, "used_cents": 12})).unwrap();
        assert_eq!(credits.used_cents, 12);
        assert_eq!(credits.limit_cents, None);
        assert_eq!(credits.balance_cents, None);
        assert_eq!(credits.error, None);
    }

    #[test]
    fn an_admin_key_is_set_since_cloud_servers_and_stays_out_of_logs() {
        assert_eq!(CLOUD_SERVERS_SINCE, 65);
        let id = ModelServerId::derive("server:example");
        for key in [Some(Secret::new("sk-or-v1-admin".into())), None] {
            let message = ClientMessage::ModelAction {
                key: IdempotencyKey::derive("admin"),
                action: ModelAction::SetServerAdminKey { server: id, key },
            };
            assert_eq!(round_trip(&message), message);
            assert_eq!(message.since(), CLOUD_SERVERS_SINCE);
            assert!(!format!("{message:?}").contains("sk-or-v1-admin"));
        }
        let json = serde_json::to_value(server()).unwrap();
        assert_eq!(json["admin_key"], true);
        assert_eq!(json["credits"]["balance_cents"], 742);
    }

    #[test]
    fn settings_default_to_the_owner_picks() {
        let read: LocalModelSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(read, LocalModelSettings::default());
        assert_eq!(read.idle_unload_minutes, 15);
        assert_eq!(read.context, 65_536);
        assert_eq!(read.kv_type, KvCacheType::F16);
        assert!(read.fewer_tools);
        assert_eq!(
            serde_json::to_string(&KvCacheType::Q8_0).unwrap(),
            "\"q8_0\""
        );
    }

    #[test]
    fn model_actions_round_trip_since_local_models() {
        assert_eq!(LOCAL_MODELS_SINCE, 54);
        let server = ModelServerId::derive("server:example");
        let actions = [
            ModelAction::AddServer {
                label: "Ollama on devbox".into(),
                url: "http://devbox:11434".into(),
                token: Some(Secret::new("example-token".into())),
                cloud: false,
            },
            ModelAction::AddServer {
                label: "OpenRouter".into(),
                url: "https://openrouter.ai/api".into(),
                token: None,
                cloud: true,
            },
            ModelAction::ProbeServer {
                url: "http://devbox:11434".into(),
                token: None,
            },
            ModelAction::TestServer { server },
            ModelAction::RemoveServer { server },
            ModelAction::Download {
                model: "qwen3.6-35b-a3b".into(),
                file: "model-Q4.gguf".into(),
            },
            ModelAction::PauseDownload {
                model: "qwen3.6-35b-a3b".into(),
                file: "model-Q4.gguf".into(),
            },
            ModelAction::ResumeDownload {
                model: "qwen3.6-35b-a3b".into(),
                file: "model-Q4.gguf".into(),
            },
            ModelAction::DeleteModel {
                model: "qwen3.6-35b-a3b".into(),
                file: "model-Q4.gguf".into(),
            },
            ModelAction::Load {
                server,
                model: "qwen3.6-35b-a3b".into(),
            },
            ModelAction::Unload {
                server,
                model: "qwen3.6-35b-a3b".into(),
            },
        ];
        for action in actions {
            let key = IdempotencyKey::generate();
            let message = ClientMessage::ModelAction {
                key,
                action: action.clone(),
            };
            assert_eq!(round_trip(&message), message);
            assert_eq!(message.since(), LOCAL_MODELS_SINCE);
            assert_eq!(message.action_key(), Some(key));
        }
        let added = ActionOutcome::ModelServerAdded { server };
        assert_eq!(round_trip(&added), added);
        let probed = ActionOutcome::ServerProbed {
            flavor: ServerFlavor::Ollama,
            probe: ServerProbe {
                at: Timestamp(1_000),
                apis: ServerApis {
                    messages: true,
                    chat: true,
                    responses: false,
                },
                context: Some(32_768),
                error: None,
            },
            models: vec![ServerModel {
                id: "qwen3.6:35b-a3b".into(),
                label: "qwen3.6:35b-a3b".into(),
                catalog: None,
                context: Some(32_768),
                load: None,
                speed: None,
                max_output: Some(8_192),
            }],
            cloud: true,
        };
        assert_eq!(round_trip(&probed), probed);
    }

    #[test]
    fn a_server_from_before_cloud_servers_reads_as_local() {
        assert_eq!(CLOUD_SERVERS_SINCE, 65);
        // A server, a model and a probe from an older `maxd`, and an add
        // from an older app, carry none of the cloud fields.
        let mut json = serde_json::to_value(server()).unwrap();
        json.as_object_mut().unwrap().remove("cloud");
        let read: ModelServer = serde_json::from_value(json).unwrap();
        assert!(!read.cloud);
        assert_eq!(read.models[0].max_output, None);
        let add: ModelAction = serde_json::from_value(serde_json::json!({
            "action": "add_server",
            "label": "devbox",
            "url": "http://devbox:11434",
        }))
        .unwrap();
        assert!(matches!(add, ModelAction::AddServer { cloud: false, .. }));
        let probed: ActionOutcome = serde_json::from_value(serde_json::json!({
            "status": "server_probed",
            "flavor": "other",
            "probe": { "at": 1, "apis": {}, "context": null, "error": null },
            "models": [],
        }))
        .unwrap();
        assert!(matches!(
            probed,
            ActionOutcome::ServerProbed { cloud: false, .. }
        ));
        // A model with no output limit writes none, and one with it reads back.
        let json = serde_json::to_value(&server().models[0]).unwrap();
        assert!(json.get("max_output").is_none());
        let capped = ServerModel {
            max_output: Some(32_000),
            ..server().models.remove(0)
        };
        assert_eq!(round_trip(&capped), capped);
    }

    #[test]
    fn open_router_sign_in_rounds_wait_for_cloud_servers() {
        assert_eq!(CLOUD_SERVERS_SINCE, 65);
        let sign_in = SignInId::generate();
        let actions = [
            ModelAction::BeginOpenRouterSignIn {
                label: Some("OpenRouter".into()),
                limit_cents: Some(2_500),
            },
            ModelAction::BeginOpenRouterSignIn {
                label: None,
                limit_cents: None,
            },
            ModelAction::FinishOpenRouterSignIn {
                sign_in,
                code: Secret::new("example-code".into()),
            },
        ];
        for action in actions {
            let message = ClientMessage::ModelAction {
                key: IdempotencyKey::generate(),
                action: action.clone(),
            };
            assert_eq!(round_trip(&message), message);
            assert_eq!(message.since(), CLOUD_SERVERS_SINCE);
            assert!(!message.reaches(Some(CLOUD_SERVERS_SINCE - 1)));
            assert!(message.reaches(Some(CLOUD_SERVERS_SINCE)));
        }
        // A begin with nothing chosen reads without its optional fields.
        let read: ModelAction =
            serde_json::from_value(serde_json::json!({"action": "begin_open_router_sign_in"}))
                .unwrap();
        assert_eq!(
            read,
            ModelAction::BeginOpenRouterSignIn {
                label: None,
                limit_cents: None,
            }
        );
    }

    #[test]
    fn a_load_says_how_far_it_got_only_when_it_knows() {
        let quiet = serde_json::to_value(ModelLoad::Loading { percent: None }).unwrap();
        assert_eq!(quiet, serde_json::json!({"state": "loading"}));
        let read: ModelLoad = serde_json::from_value(quiet).unwrap();
        assert_eq!(read, ModelLoad::Loading { percent: None });
        let half = ModelLoad::Loading { percent: Some(50) };
        assert_eq!(
            serde_json::to_value(&half).unwrap(),
            serde_json::json!({"state": "loading", "percent": 50})
        );
        assert_eq!(round_trip(&half), half);
    }

    #[test]
    fn a_server_token_stays_out_of_logs() {
        let message = ClientMessage::ModelAction {
            key: IdempotencyKey::derive("key"),
            action: ModelAction::AddServer {
                label: "devbox".into(),
                url: "http://devbox:11434".into(),
                token: Some(Secret::new("example-token".into())),
                cloud: false,
            },
        };
        assert!(!format!("{message:?}").contains("example-token"));
        let json = serde_json::to_value(server()).unwrap();
        assert_eq!(json["token"], true);
    }

    #[test]
    fn a_local_run_reads_with_fields_left_out() {
        let read: LocalRun = serde_json::from_str("{}").unwrap();
        assert_eq!(read, LocalRun::default());
    }

    #[test]
    fn linked_devices_may_use_a_mac_s_models_unless_it_says_no() {
        use crate::agents::{PeerEntry, PeerRequest};
        use crate::ids::StreamId;
        // A device saved before the switch reads it as on.
        assert!(LocalModelSettings::default().linked_devices);
        let read: LocalModelSettings = serde_json::from_str(r#"{"fewer_tools":false}"#).unwrap();
        assert!(read.linked_devices);
        // A peer entry from before reads with no shared server.
        let entry: PeerEntry = serde_json::from_value(serde_json::json!({
            "device": device(),
            "name": "Home Mac",
            "routes": [],
            "host_key": "",
        }))
        .unwrap();
        assert_eq!(entry.model_server, None);
        let shared = PeerEntry {
            model_server: Some(LinkedModels {
                server: ModelServerId::derive("server:example"),
                label: "Home Mac".into(),
                models: server().models,
            }),
            ..entry
        };
        assert_eq!(round_trip(&shared), shared);
        let requests = [
            PeerRequest::ModelStream {
                stream: StreamId::derive("stream:example"),
                server: ModelServerId::derive("server:example"),
            },
            PeerRequest::ModelHold {
                server: ModelServerId::derive("server:example"),
                models: vec!["qwen3.6:35b".into()],
            },
        ];
        for request in requests {
            assert_eq!(round_trip(&request), request);
        }
        // A model that never said how fast it ran writes no speed.
        let json = serde_json::to_value(ServerModel {
            speed: None,
            ..server().models.remove(0)
        })
        .unwrap();
        assert!(json.get("speed").is_none());
    }
}

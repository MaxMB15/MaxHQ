// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The descriptor: one file that tells MaxHQ where a workflow's runs live,
//! how its run file maps onto nodes, and how to draw them. These types are
//! the standard's source of truth; the checked-in JSON Schema is generated
//! from them, so their doc comments are the field reference.

use std::borrow::Cow;
use std::fmt;

use maxhq_protocol::workflow::{NodeStatus, Tone};
use regex::Regex;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use serde_json_path::JsonPath;

use crate::inputs::AgentState;

/// The format name a descriptor starts its `format` with.
pub const FORMAT: &str = "maxhq.workflow";
/// The format name of an overlay.
pub const OVERLAY_FORMAT: &str = "maxhq.workflow-overlay";
/// The major version this build reads.
pub const MAJOR: u32 = 1;
/// The newest minor version this build knows.
pub const MINOR: u32 = 0;
/// The id of the built-in descriptor that draws a lead and its sub-agents.
pub const AGENTS_ID: &str = "maxhq.agents";

/// A workflow descriptor, `maxhq.workflow/1.x`. Keys that start with `x-`
/// are extensions and are ignored.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields, extend("patternProperties" = { "^x-": {} }))]
pub struct Descriptor {
    /// `maxhq.workflow/<major>.<minor>`, such as `maxhq.workflow/1.0`.
    #[schemars(regex(pattern = r"^maxhq\.workflow/\d+\.\d+$"))]
    pub format: String,
    /// The workflow's id: lower case letters, digits, `.`, `_` and `-`.
    #[schemars(regex(pattern = r"^[a-z0-9][a-z0-9._-]*$"))]
    pub id: String,
    /// What MaxHQ calls it. The id when left out.
    #[serde(default)]
    pub name: Option<String>,
    /// The descriptor's own revision. The workflow bumps it; MaxHQ only
    /// shows it.
    #[serde(default)]
    pub revision: Option<u64>,
    /// Where runs live and what each run says about itself.
    pub runs: Runs,
    /// How the run file lists its nodes.
    #[serde(default)]
    pub nodes: Nodes,
    /// A label and folded state per group id.
    #[serde(default)]
    pub groups: Ordered<GroupInfo>,
    /// Display rules, applied after the standard status rules. For each
    /// node the first rule whose `when` holds wins.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Short labels on nodes, in order.
    #[serde(default)]
    pub badges: Vec<BadgeRule>,
    /// Loops between nodes, counted from the run's history.
    #[serde(default)]
    pub loops: Vec<LoopRule>,
    /// Links a node offers, only to `127.0.0.1` or `localhost`.
    #[serde(default)]
    pub links: Vec<LinkRule>,
    /// Patterns that bind agents to nodes and runs. When left out, the
    /// standard's default applies: a prompt line `maxhq-node: <run> <node>`.
    #[serde(default)]
    pub bind: Option<Bind>,
    /// Lists for a node's expanded row.
    #[serde(default)]
    pub detail: Vec<DetailRule>,
    /// Lines for the run's header.
    #[serde(default)]
    pub header: Vec<Label>,
    /// The minor version it was written for, filled in on load.
    #[serde(skip)]
    #[schemars(skip)]
    pub minor: u32,
}

impl Descriptor {
    /// The built-in descriptor: each lead with sub-agents is a run, and
    /// each sub-agent a node.
    pub fn agents() -> Self {
        Self {
            format: format!("{FORMAT}/{MAJOR}.{MINOR}"),
            id: AGENTS_ID.to_owned(),
            name: Some("Agents".to_owned()),
            // Nodes name their agents, so no pattern is needed.
            bind: Some(Bind::default()),
            ..Self::default()
        }
    }

    /// What MaxHQ calls the workflow.
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Whether it was written for a newer minor than this build knows.
    pub fn newer(&self) -> bool {
        self.minor > MINOR
    }

    /// The prefix every `runs.env` name must start with: the upper-cased id
    /// and an underscore.
    pub fn env_prefix(&self) -> String {
        let mut prefix: String = self
            .id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        prefix.push('_');
        prefix
    }
}

/// Variable families that let a workflow's variable do more than hand a
/// value to its scripts, each the name itself or the name and `_`, with what
/// a variable in one can do. They hold an agent's logins, change what it and
/// its tools run, or send its traffic somewhere else, and an id can spell
/// any of them: `cargo-home` gives the prefix `CARGO_HOME_`. A variable in
/// one still loads, and the card that asks for the variables marks it.
#[rustfmt::skip]
const RISKY_ENV: &[(&str, &[&str])] = &[
    ("can send your Claude sign-in elsewhere", &["CLAUDE", "ANTHROPIC"]),
    ("can change how MaxHQ or the agent's CLI works", &["MAXHQ", "MCP", "DISABLE"]),
    // The other agents' CLIs and the clouds and services they log in to.
    ("can change which account a tool signs in with, or where it sends your sign-in", &[
        "OPENAI", "CODEX", "CURSOR", "GEMINI", "AWS", "GOOGLE", "AZURE", "VERTEX", "BEDROCK",
        "CLOUD_ML", "CLOUDSDK", "GCLOUD", "GCP", "BOTO", "RCLONE", "VAULT", "HEROKU", "FLY",
        "VERCEL", "NETLIFY", "CLOUDFLARE", "DIGITALOCEAN", "HF", "HUGGINGFACE", "OLLAMA", "MYSQL",
        "KRB5", "NSS", "PKCS11", "PULUMI", "OP_CONNECT", "OP_SERVICE_ACCOUNT", "OP_ACCOUNT",
        "OP_SESSION", "APP_STORE_CONNECT",
    ]),
    ("can run code when a tool starts", &[
        // Loaders, shells and where they look.
        "LD", "DYLD", "GLIBC", "GCONV", "GETCONF", "NIS_PATH", "MALLOC", "BASH", "ZSH", "KSH",
        "FISH", "SHELL", "PROMPT_COMMAND", "IFS", "PATH", "HOME", "TMP", "TEMP", "TERM", "XDG",
        "DBUS", "GIO", "GI_TYPELIB_PATH", "GTK", "QT", "GST", "XPC",
        // Compilers, what they find and the macOS tools that pick them.
        "COMPILER_PATH", "LIBRARY_PATH", "SHLIB_PATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH",
        "OBJC", "OBJCPLUS", "GCC", "CLANG", "LLVM", "ASAN", "UBSAN", "LSAN", "MSAN", "TSAN",
        "DEVELOPER_DIR", "DT_TOOLCHAIN_DIR", "XCODE", "XCRUN", "MESON", "NINJA", "BAZEL",
        // Language tools and package managers.
        "NODE", "NPM", "YARN", "PNPM", "BUN", "DENO", "COREPACK", "NVM", "FNM", "VOLTA",
        "TS_NODE", "TSX", "ESBUILD", "SWC", "TURBO", "PYTHON", "PIP", "PIPX", "PIPENV", "UV",
        "POETRY", "HATCH", "PDM", "RYE", "NOX", "CONDA", "VIRTUAL", "PYENV", "PYTEST", "TOX",
        "COVERAGE_PROCESS_START", "COVERAGE_RCFILE", "JUPYTER", "DJANGO", "FLASK", "CELERY",
        "CARGO", "RUSTC", "RUSTUP", "GO", "GOROOT", "JAVA", "_JAVA", "JDK", "MAVEN", "GRADLE",
        "ANT", "SBT", "KOTLIN", "GROOVY", "SCALA", "RUBY", "GEM", "BUNDLE", "BUNDLER", "RBENV",
        "PERL", "PHP", "COMPOSER", "LUA", "DOTNET", "CORECLR", "MISE", "ASDF", "NIX", "HOMEBREW",
        "NODENV", "GOENV", "JENV", "EM_CONFIG", "ANDROID",
        "PKG_CONFIG", "CMAKE", "CCACHE", "ERL", "ELIXIR", "MIX_HOME", "MIX_ARCHIVES", "JULIA",
        "TCL", "TK", "R_PROFILE", "R_ENVIRON", "R_LIBS", "R_HOME", "GHC", "CABAL", "STACK_ROOT",
        "STACK_YAML", "OPAM", "SWIFT", "DART", "FLUTTER", "PUB_CACHE", "PUB_HOSTED_URL",
        "DOCKER", "COMPOSE", "PODMAN", "BUILDKIT", "CONTAINER_HOST", "CONTAINERS_CONF",
        "CONTAINERS_REGISTRIES_CONF", "CONTAINERS_STORAGE_CONF", "KUBE", "HELM", "TF",
        "TERRAGRUNT", "TG_TF_PATH", "ANSIBLE",
        // Programs that read their options, or the program to run, from a variable.
        "TAR", "XZ", "GREP", "RIPGREP", "FZF", "BAT", "DELTA_PAGER", "SYSTEMD", "VIM", "NVIM",
        "EMACS", "GS_LIB", "GS_OPTIONS", "MAGICK", "PARALLEL_SHELL", "PARALLEL_SSH",
        // Browsers and what drives them.
        "CHROME", "CHROMIUM", "FIREFOX", "ELECTRON", "PUPPETEER", "PLAYWRIGHT", "CYPRESS",
        "SELENIUM",
    ]),
    // Git, other version control and other ways to reach another machine.
    ("can run code or change the account when git or ssh runs", &[
        "GIT", "GH", "GITHUB", "GITLAB", "GLAB", "GITEA", "BITBUCKET", "HG", "JJ", "SVN", "CVS",
        "SSH", "AUTOSSH", "RSYNC", "GPG", "SUDO", "PRE_COMMIT", "LEFTHOOK", "DIRENV",
    ]),
    // Proxies, certificates, name lookups and HTTP clients.
    ("can send the agent's traffic somewhere else", &[
        "HTTP", "HTTPS", "ALL_PROXY", "NO_PROXY", "WS_PROXY", "WSS_PROXY", "GLOBAL_AGENT", "FTP",
        "SOCKS", "SSL", "OPENSSL", "LIBRESSL", "CURL", "REQUESTS", "GRPC", "WEBSOCKET",
        "HTTPLIB2", "RES_OPTIONS", "RESOLV", "OTEL",
    ]),
];

/// Whether `name` has the shape a workflow's variable must have: a `_`, and
/// only upper case letters, digits and `_`. A name that loaded always has
/// it, so the shape only stops one that skipped loading, such as
/// `PYTHONPATH` or `http_proxy`.
pub fn env_shaped(name: &str) -> bool {
    name.contains('_')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// What a workflow's variable named `name` can do beyond handing a value to
/// its scripts, when it's in a family that can do more.
pub fn env_risk(name: &str) -> Option<&'static str> {
    RISKY_ENV.iter().find_map(|(risk, families)| {
        families
            .iter()
            .any(|family| {
                name.strip_prefix(family)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
            })
            .then_some(*risk)
    })
}

/// Where a workflow's runs live and what each says about itself. Values
/// left out read the reference shape: `$.run.id`, `$.run.label`,
/// `$.run.agent` and `$.run.status`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Runs {
    /// A glob, from the project's root, that matches each run's file. It may
    /// start with `{project}/`. `*` stays inside a folder and `**` crosses
    /// folders.
    pub glob: String,
    /// The run's id. The run folder's name when it reads nothing.
    #[serde(default)]
    pub id: Option<Expr>,
    /// What the run is called.
    #[serde(default)]
    pub label: Option<Expr>,
    /// The agent that leads the run, by id, such as the `MAXHQ_AGENT` the
    /// lead was started with.
    #[serde(default)]
    pub agent: Option<Expr>,
    /// The run's own status, in the workflow's words.
    #[serde(default)]
    pub status: Option<Expr>,
    /// Named facts about the run, for rules, badges and templates as
    /// `{run.<name>}`.
    #[serde(default)]
    pub facts: Ordered<Expr>,
    /// Event logs, in order of preference. The first that exists is read.
    #[serde(default)]
    pub events: Vec<EventSource>,
    /// Variables MaxHQ sets on agents it starts in a project the descriptor
    /// applies to. Each name starts with the upper-cased id and `_`.
    #[serde(default)]
    pub env: Ordered<String>,
}

/// An event log a run writes.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EventSource {
    /// JSON Lines, one event object per line, read by offset.
    Jsonl {
        /// The file, relative to the run folder.
        jsonl: String,
    },
    /// Any line log, read by offset, with a pattern whose named groups `t`,
    /// `node` and `status` (and optionally `note`) give each event.
    Lines {
        /// The file, relative to the run folder.
        lines: String,
        /// The pattern each event line matches.
        #[serde(rename = "match")]
        pattern: Pattern,
    },
}

/// How the run file lists its nodes. Field names are keys of each node
/// object. Everything left out reads the reference shape.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Nodes {
    /// Where the list of nodes is, as JSONPath.
    #[serde(default = "Nodes::default_list")]
    pub list: Query,
    /// The node's id.
    #[serde(default = "Nodes::default_id")]
    pub id: String,
    /// The ids of the nodes it needs.
    #[serde(default = "Nodes::default_needs")]
    pub needs: String,
    /// The group it sits in.
    #[serde(default = "Nodes::default_group")]
    pub group: String,
    /// Its status, in the workflow's words.
    #[serde(default = "Nodes::default_status")]
    pub status: String,
    /// How many attempts it took.
    #[serde(default = "Nodes::default_attempts")]
    pub attempts: String,
    /// What it's called, when `info` names no label.
    #[serde(default = "Nodes::default_label")]
    pub label: String,
    /// The agent working on it, by id.
    #[serde(default = "Nodes::default_agent")]
    pub agent: String,
    /// More fields rules, badges and templates may read, as `{<field>}`.
    #[serde(default)]
    pub fields: Vec<String>,
    /// The workflow's status words, mapped to the standard's. A word missing
    /// here that is a standard status reads as itself; any other reads as
    /// `unknown`.
    #[serde(default)]
    pub status_map: Ordered<NodeStatus>,
    /// Per node id: its label, lane, host flag and variables. A node named
    /// here that a run doesn't list is `omitted` in that run.
    #[serde(default)]
    pub info: Ordered<NodeInfo>,
}

impl Nodes {
    fn default_list() -> Query {
        Query::parse("$.nodes").expect("a valid path")
    }
    fn default_id() -> String {
        "id".into()
    }
    fn default_needs() -> String {
        "needs".into()
    }
    fn default_group() -> String {
        "group".into()
    }
    fn default_status() -> String {
        "status".into()
    }
    fn default_attempts() -> String {
        "attempts".into()
    }
    fn default_label() -> String {
        "label".into()
    }
    fn default_agent() -> String {
        "agent".into()
    }
}

impl Default for Nodes {
    fn default() -> Self {
        Self {
            list: Self::default_list(),
            id: Self::default_id(),
            needs: Self::default_needs(),
            group: Self::default_group(),
            status: Self::default_status(),
            attempts: Self::default_attempts(),
            label: Self::default_label(),
            agent: Self::default_agent(),
            fields: Vec::new(),
            status_map: Ordered::default(),
            info: Ordered::default(),
        }
    }
}

/// What the descriptor says about one node.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct NodeInfo {
    /// What MaxHQ calls it.
    #[serde(default)]
    pub label: Option<String>,
    /// A lane other than the main one, such as `side`.
    #[serde(default)]
    pub lane: Option<String>,
    /// Whether the run's lead works on it itself, rather than a sub-agent.
    #[serde(default)]
    pub host: bool,
    /// Variables for templates on this node, as `{vars.<name>}`.
    #[serde(default)]
    pub vars: Ordered<String>,
}

/// What the descriptor says about one group.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct GroupInfo {
    /// What MaxHQ calls it. Its id when left out.
    #[serde(default)]
    pub label: Option<String>,
    /// Whether it starts folded.
    #[serde(default)]
    pub collapsed: bool,
}

/// A display rule.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Rule {
    /// When it applies to a node.
    pub when: Predicate,
    /// A value source the `value` predicate in `when` tests.
    #[serde(default)]
    pub value: Option<Expr>,
    /// The status it gives the node.
    #[serde(default)]
    pub is: Option<NodeStatus>,
    /// A note for the node, as a template.
    #[serde(default)]
    pub note: Option<String>,
    /// Whether the status is a guess: `true`, or a predicate.
    #[serde(default)]
    pub provisional: Option<Provisional>,
}

/// Whether a rule's status is a guess.
#[derive(Clone, Debug, JsonSchema)]
#[serde(untagged)]
pub enum Provisional {
    /// Always or never.
    Always(bool),
    /// While this holds.
    When(Box<Predicate>),
}

// By hand rather than untagged, which buffers the predicate and so hides
// keys the standard lacks from the loader's check.
impl<'de> Deserialize<'de> for Provisional {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Either;
        impl<'de> Visitor<'de> for Either {
            type Value = Provisional;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("`true`, `false` or a predicate")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<Self::Value, E> {
                Ok(Provisional::Always(b))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Predicate::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(|p| Provisional::When(Box::new(p)))
            }
        }
        deserializer.deserialize_any(Either)
    }
}

/// A line of text with an optional value, shown when `when` holds, as in a
/// badge or a header line.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Label {
    /// The text, as a template.
    pub text: String,
    /// How it reads. Neutral when left out.
    #[serde(default)]
    pub tone: Option<Tone>,
    /// A value source, for `{value}` and the `value` predicate.
    #[serde(default)]
    pub value: Option<Expr>,
    /// More value sources, for `{vars.<name>}` in the text.
    #[serde(default)]
    pub vars: Ordered<Expr>,
    /// When it shows. Always when left out.
    #[serde(default)]
    pub when: Option<Predicate>,
}

/// A badge on nodes.
#[derive(Clone, Debug, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct BadgeRule {
    /// The nodes it may show on. Every node when left out.
    #[serde(default)]
    pub node: Option<OneOrMany<String>>,
    /// The badge.
    #[serde(flatten)]
    pub label: Label,
}

// By hand rather than flattened, which buffers the label and so hides keys
// the standard lacks from the loader's check.
impl<'de> Deserialize<'de> for BadgeRule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Flat {
            #[serde(default)]
            node: Option<OneOrMany<String>>,
            text: String,
            #[serde(default)]
            tone: Option<Tone>,
            #[serde(default)]
            value: Option<Expr>,
            #[serde(default)]
            vars: Ordered<Expr>,
            #[serde(default)]
            when: Option<Predicate>,
        }
        let f = Flat::deserialize(deserializer)?;
        Ok(Self {
            node: f.node,
            label: Label {
                text: f.text,
                tone: f.tone,
                value: f.value,
                vars: f.vars,
                when: f.when,
            },
        })
    }
}

/// A loop between nodes. Its count is how often a `to` node, or a member of
/// a `to` group, went busy again after `from` went busy.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LoopRule {
    /// The node work goes back from.
    pub from: String,
    /// The nodes or groups work goes back to.
    pub to: OneOrMany<String>,
    /// What it's called.
    #[serde(default)]
    pub label: Option<String>,
    /// How many times it may go round.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// A link a node offers while `when` holds.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LinkRule {
    /// The nodes it shows on. Every node when left out.
    #[serde(default)]
    pub node: Option<OneOrMany<String>>,
    /// What it says.
    pub label: String,
    /// Where it goes, as a template. Only `http://127.0.0.1` and
    /// `http://localhost` URLs are kept.
    pub url: String,
    /// A value source, for `{value}`.
    #[serde(default)]
    pub value: Option<Expr>,
    /// When it shows. Always when left out.
    #[serde(default)]
    pub when: Option<Predicate>,
}

/// Patterns that bind agents to nodes and runs. Named groups `run` and
/// `node` say what a match binds to.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Bind {
    /// Patterns that bind an agent to a node.
    #[serde(default)]
    pub node: Vec<NodePattern>,
    /// Patterns that bind an agent as a run's lead.
    #[serde(default)]
    pub run: Vec<RunPattern>,
    /// Pids, read per node, that bind the agent with that pid to the node.
    #[serde(default)]
    pub process: Vec<ProcessPattern>,
}

/// A pattern over an agent's title or the head of its first prompt.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(
    deny_unknown_fields,
    extend("oneOf" = [{ "required": ["title"] }, { "required": ["prompt"] }])
)]
pub struct NodePattern {
    /// Matched against the agent's title.
    #[serde(default)]
    pub title: Option<Pattern>,
    /// Matched against the first 4 KB of the agent's first prompt.
    #[serde(default)]
    pub prompt: Option<Pattern>,
    /// A second pattern the same prompt must also match. Its groups count
    /// too.
    #[serde(default)]
    pub and: Option<Pattern>,
}

/// A pattern over the commands an agent ran.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RunPattern {
    /// Matched against each finished command. Checked only on commands that
    /// contain the pattern's literal start.
    pub command: Pattern,
}

/// A pid file per node.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProcessPattern {
    /// The pid, as a value source evaluated per node.
    pub pid: Expr,
}

/// A list for a node's expanded row.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DetailRule {
    /// What the list is.
    pub label: String,
    /// The items, as a value source.
    pub list: Expr,
    /// The most items to show, the last ones.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A value: a literal, or a source read from the run.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Expr {
    /// Read from a file.
    Source(Box<Source>),
    /// A plain value.
    Literal(Json),
}

/// A value read from the run file, or the file named by `in`. Exactly one
/// of `path`, `count`, `sum`, `match` and `alive_pid`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("oneOf" = [
    { "required": ["path"] },
    { "required": ["count"] },
    { "required": ["sum"] },
    { "required": ["match"] },
    { "required": ["alive_pid"] },
]))]
pub struct Source {
    /// The value at a JSONPath (RFC 9535). Several matches read as a list.
    #[serde(default)]
    pub path: Option<Query>,
    /// How many values a JSONPath selects. A path that selects one list
    /// counts its items.
    #[serde(default)]
    pub count: Option<Query>,
    /// The sum of the numbers a JSONPath selects.
    #[serde(default)]
    pub sum: Option<Query>,
    /// What `sum` does to each number first.
    #[serde(default)]
    pub each: Option<Each>,
    /// A pattern over the lines of the file, as a template.
    #[serde(default, rename = "match")]
    pub pattern: Option<String>,
    /// Which matching lines `match` gives. The first when left out.
    #[serde(default)]
    pub pick: Option<Pick>,
    /// The group of the match to give, by number or name. The whole match
    /// when left out.
    #[serde(default)]
    pub group: Option<GroupRef>,
    /// A pid file, relative to the run folder: true while that process
    /// runs.
    #[serde(default)]
    pub alive_pid: Option<String>,
    /// The file to read, as a template: relative to the run folder, or
    /// starting with `{project}` or `{workflow}`. The run file when left out.
    #[serde(default, rename = "in")]
    pub file: Option<String>,
    /// The value when the source reads nothing.
    #[serde(default)]
    pub default: Option<Json>,
}

/// What `sum` does to each number before adding it.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Each {
    /// Added to the number.
    #[serde(default)]
    pub add: Option<f64>,
    /// The least it may be after `add`.
    #[serde(default)]
    pub min: Option<f64>,
    /// The most it may be after `add`.
    #[serde(default)]
    pub max: Option<f64>,
}

/// Which lines a `match` gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    /// The first matching line.
    First,
    /// The last matching line.
    Last,
    /// How many lines match.
    Count,
    /// Every matching line, as a list.
    All,
}

/// A group of a pattern, by number or name.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GroupRef {
    /// By number, 0 for the whole match.
    Index(usize),
    /// By name.
    Name(String),
}

/// A test on nodes and the run. Every key given must hold.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields, transform = node_alias)]
pub struct Predicate {
    /// The node's status is one of these.
    #[serde(default)]
    pub status: Option<OneOrMany<NodeStatus>>,
    /// The node is one of these, by id.
    #[serde(default, alias = "id")]
    pub node: Option<OneOrMany<String>>,
    /// Each named field of the node matches.
    #[serde(default)]
    pub field: Option<Ordered<Matcher>>,
    /// The run's facts match: a map from fact to test, or one fact's name
    /// with `exists`.
    #[serde(default)]
    pub fact: Option<FactTest>,
    /// With `fact` as a name: whether the run has that fact.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The rule's or badge's value matches.
    #[serde(default)]
    pub value: Option<Matcher>,
    /// Some node of the run passes this test.
    #[serde(default)]
    pub any_node: Option<Box<Predicate>>,
    /// The run's lead stands as one of these.
    #[serde(default)]
    pub orchestrator: Option<OneOrMany<AgentState>>,
    /// Every one of these holds.
    #[serde(default)]
    pub all: Option<Vec<Predicate>>,
    /// At least one of these holds.
    #[serde(default)]
    pub any: Option<Vec<Predicate>>,
    /// This doesn't hold.
    #[serde(default)]
    pub not: Option<Box<Predicate>>,
}

impl Predicate {
    /// How many levels deep it nests.
    pub(crate) fn depth(&self) -> usize {
        let one = self.any_node.iter().chain(&self.not).map(|p| p.as_ref());
        let many = self.all.iter().chain(&self.any).flatten();
        1 + one.chain(many).map(Predicate::depth).max().unwrap_or(0)
    }
}

/// Lets the schema take `id` as [`Predicate::node`] does, but not both.
fn node_alias(schema: &mut Schema) {
    if let Some(node) = schema
        .get("properties")
        .and_then(|p| p.get("node"))
        .cloned()
    {
        if let Some(props) = schema.get_mut("properties").and_then(Json::as_object_mut) {
            props.insert("id".to_owned(), node);
        }
        schema.insert(
            "not".to_owned(),
            serde_json::json!({ "required": ["node", "id"] }),
        );
    }
}

/// A test on the run's facts.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FactTest {
    /// One fact, by name, tested with `exists`.
    Name(String),
    /// Each named fact matches.
    Each(Ordered<Matcher>),
}

/// A test on one value.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Matcher {
    /// Comparisons, all of which must hold.
    Compare(Compare),
    /// Equal to one of these.
    OneOf(Vec<Json>),
    /// Equal to this.
    Is(Json),
}

/// Comparisons on one value.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Compare {
    /// Whether it has a value at all.
    #[serde(default)]
    pub exists: Option<bool>,
    /// Equal to.
    #[serde(default)]
    pub eq: Option<Operand>,
    /// Not equal to.
    #[serde(default)]
    pub ne: Option<Operand>,
    /// Greater than.
    #[serde(default)]
    pub gt: Option<Operand>,
    /// Greater than or equal to.
    #[serde(default)]
    pub gte: Option<Operand>,
    /// Less than.
    #[serde(default)]
    pub lt: Option<Operand>,
    /// Less than or equal to.
    #[serde(default)]
    pub lte: Option<Operand>,
}

/// The other side of a comparison.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Operand {
    /// A run fact, by name.
    Fact {
        /// The fact.
        fact: String,
    },
    /// A plain value.
    Literal(Json),
}

/// One value or a list of them.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// One.
    One(T),
    /// Several.
    Many(Vec<T>),
}

impl<T: PartialEq> OneOrMany<T> {
    /// The values, in order.
    pub fn as_slice(&self) -> &[T] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }

    /// Whether `value` is among them.
    pub fn contains(&self, value: &T) -> bool {
        self.as_slice().contains(value)
    }
}

/// A map that keeps the order it was written in.
#[derive(Clone, Debug, PartialEq)]
pub struct Ordered<V>(pub Vec<(String, V)>);

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<V> Ordered<V> {
    /// The value under `key`.
    pub fn get(&self, key: &str) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The entries, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for Entries<V> {
            type Value = Ordered<V>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, V)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, V>()? {
                    if entries.iter().any(|(k, _)| *k == key) {
                        return Err(de::Error::custom(format!("`{key}` appears twice")));
                    }
                    entries.push((key, value));
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Entries(std::marker::PhantomData))
    }
}

impl<V: JsonSchema> JsonSchema for Ordered<V> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Map_of_{}", V::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "additionalProperties": generator.subschema_for::<V>(),
        })
    }
}

/// A regular expression, in the syntax of Rust's `regex` crate. Named
/// groups are written `(?<name>...)`.
#[derive(Clone, Debug)]
pub struct Pattern(pub Regex);

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Regex::new(&text).map(Self).map_err(de::Error::custom)
    }
}

impl JsonSchema for Pattern {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Pattern".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "format": "regex" })
    }
}

/// A JSONPath query, RFC 9535.
#[derive(Clone, Debug)]
pub struct Query(pub JsonPath);

impl Query {
    /// Parses a query.
    pub fn parse(text: &str) -> Result<Self, String> {
        JsonPath::parse(text).map(Self).map_err(|e| e.to_string())
    }
}

impl<'de> Deserialize<'de> for Query {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

impl JsonSchema for Query {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Query".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "pattern": "^\\$" })
    }
}

/// An overlay, `maxhq.workflow-overlay/1.x`: changes to one workflow's
/// descriptor, merged over it key by key. A list replaces the list under
/// it, and `null` removes a key.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Overlay {
    /// `maxhq.workflow-overlay/<major>.<minor>`.
    pub format: String,
    /// The id of the workflow it changes.
    pub workflow: String,
}

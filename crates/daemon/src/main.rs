// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd`, the daemon on every device. `maxd run` watches the agents here
//! and serves what it sees on a Unix socket, `maxd proxy` carries that
//! socket over SSH, and `maxd service` keeps `maxd run` going in your login
//! session. `maxd hold` keeps one agent running for `maxd`, which starts it.
//! `maxd guest` runs inside a container workspace, and `maxd host quiesce`
//! winds agents down before the device restarts.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, Permissions};
use std::future::Future;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use maxhq_core::db::OpenError;
use maxhq_core::paths;
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{ClientId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, ClientHello, ClientInfo};
use maxhq_protocol::{
    ClientMessage, ProtocolRange, ServerMessage, OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::runtime::Runtime;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::db::Db;
use crate::hub::Hub;
use crate::identity::IdentityError;
use crate::server::BindError;
use crate::service::{Action, Clis, ServiceError, Setup, System, SystemHolders, Uninstall};

mod acp_agents;
mod acp_registry;
mod admission;
mod app_keys;
// The lease tools and test hosts call it once they're wired in.
#[allow(dead_code)]
mod agent_device;
mod agents;
mod clone;
mod codesign;
mod db;
mod diagnostics;
mod doctext;
// The session manager doesn't call the Claude driver yet.
#[allow(dead_code)]
mod drive;
mod find;
mod fleet;
mod folder_trust;
mod folders;
mod github_events;
mod guest;
mod guestlink;
mod hooks;
mod host;
mod hosts;
mod hub;
mod identity;
mod incus;
mod later;
mod leases;
mod local_models;
mod moves;
mod netaccess;
mod notes;
mod notices;
mod observe;
mod peers;
mod phones;
mod ports;
mod power;
mod proxy;
// The PR requests and tools refuse until the watcher and handlers read
// through it.
#[allow(dead_code)]
mod pulls;
mod push;
// XC runs the recipe. Sessions in containers don't prepare accounts
// through it yet.
#[allow(dead_code)]
mod recipe;
// XH's and XI's secret store. Nothing reads ChatGPT sign-ins from it
// before XI's Codex broker.
#[allow(dead_code)]
mod secrets;
mod server;
mod service;
mod session;
mod sidecar;
mod skills;
mod sshkey;
mod tasks;
mod terminals;
mod thumbnail;
mod tools;
mod transfer;
mod tunnel;
mod usage;
mod workflow;
mod workspace;

/// Set to anything to run `maxd` without its observers and usage checks,
/// so tests of the binary see only what they make.
const NO_OBSERVERS_ENV: &str = "MAXD_NO_OBSERVERS";

/// How long stopping waits for tasks that are writing to the database.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// The exit code `maxd service status` gives when `maxd` isn't running, as
/// init scripts do.
const NOT_RUNNING: u8 = 3;

/// What `maxd --help` prints.
const USAGE: &str = "usage:
  maxd run                 watch the agents here and serve them on the socket
  maxd proxy               join stdin and stdout to the socket, for ssh
  maxd proxy --guest-ssh <workspace>
                           join stdin and stdout to a container's SSH, for
                           an editor's Remote-SSH
  maxd proxy --peer <device>
                           join stdin and stdout to the socket for another
                           device's maxd; that device's key runs this
  maxd proxy --phone <phone>
                           the same for a paired phone; its key runs this
  maxd proxy --pair <pair> the same for a phone that pairs; the one-time
                           key runs this
  maxd authorize-key <public key> [--app <app key> <label>]
                           let MaxHQ's key run maxd proxy here and, while
                           this device trusts no app key, trust the app's;
                           setup runs this
  maxd service install     run maxd now and at every login
  maxd service uninstall [--delete-data]
                           stop maxd, end the agents it ran and what else
                           it started, and remove the service; with
                           --delete-data, delete its data and logs too
  maxd service status      say whether maxd is installed and running
  maxd mcp --key-file <path>
                           serve the agent tools on stdin and stdout, for
                           a tool outside MaxHQ, with the key in that file
  maxd hold <folder>       keep one agent running; maxd starts these
  maxd guest               run inside a container workspace, as root
  maxd guest --test-host   run inside a test container, answering leases
  maxd guest token codex   print a Codex access token, for Codex
  maxd guest codex <program> [args...]
                           fetch Codex's model catalog, then run Codex
  maxd guest git-credential get
                           git's credential helper, for github.com
  maxd guest github-token  print a token for the workspace's repo, for gh
  maxd guest probe <targets.json>
                           try each target and print what answered
  maxd host check          compare this host with its maxhq-host.toml
  maxd host apply          fix what needs no root, as the service user
  maxd host apply --system [--yes]
                           the root part on Debian and Ubuntu, through sudo;
                           --yes goes ahead without asking
  maxd host render <item> --spec <file>
                           print a file the root part makes from a spec:
                           json, prepare, egress or memory-low
  maxd host quiesce --window <20m>
                           wind agents down, then wait for them; only root
                           may, as the shutdown unit runs it
  maxd update watch --build <build>
                           put the old maxd back if <build> never comes up;
                           an update starts this
  maxd --build             print this binary's build
  maxd --version

MAXD_SOCKET sets the socket path, and RUST_LOG how much maxd run logs.";

/// Picks the subcommand.
fn main() -> ExitCode {
    // `hold` takes a folder, which needn't be UTF-8, so it's checked before
    // the arguments become strings.
    let args_os: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args_os.first().is_some_and(|first| first == "hold") {
        return maxhq_hold::run_holder(&args_os[1..]);
    }
    // The child that reads a document's text, which isn't for people to run.
    if args_os
        .first()
        .is_some_and(|first| first == doctext::SUBCOMMAND)
    {
        return doctext::run(&args_os[1..]);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["run"] => run(),
        ["proxy"] => proxy(),
        ["proxy", "--guest-ssh", workspace] => match proxy::workspace(workspace) {
            Some(workspace) => proxy_with(proxy::Mode::GuestSsh(workspace)),
            None => usage_error(),
        },
        ["proxy", "--peer", device] => match proxy::device(device) {
            Some(device) => proxy_as(&ClientMessage::Peer { device }),
            None => usage_error(),
        },
        ["proxy", "--phone", phone] => match proxy::phone(phone) {
            Some(phone) => proxy_as(&ClientMessage::Phone { phone }),
            None => usage_error(),
        },
        ["proxy", "--pair", pair] => match proxy::pair(pair) {
            Some(pair) => proxy_as(&ClientMessage::Pair { pair }),
            None => usage_error(),
        },
        ["authorize-key", kind, key, "--app", app, label] => {
            authorize_key(&format!("{kind} {key}"), Some((app, label)))
        }
        ["authorize-key", key, "--app", app, label] => authorize_key(key, Some((app, label))),
        ["authorize-key", kind, key] => authorize_key(&format!("{kind} {key}"), None),
        ["authorize-key", key] => authorize_key(key, None),
        ["service", "uninstall", "--delete-data"] => {
            service(Action::Uninstall { delete_data: true })
        }
        ["service", action] => match Action::parse(action) {
            Some(action) => service(action),
            None => usage_error(),
        },
        ["mcp", "--key-file", path] => agents::mcp::run(Path::new(path)),
        ["guest"] => guest::run(false),
        ["guest", "--test-host"] => guest::run(true),
        #[cfg(target_os = "macos")]
        ["guest", "--vm"] => guest::vm::run(),
        ["guest", "token", "codex"] => guest::token_codex(),
        ["guest", "codex", command @ ..] if !command.is_empty() => guest::codex(command),
        ["guest", "git-credential", action] => guest::git_credential(action),
        ["guest", "github-token"] => guest::github_token(),
        ["guest", "probe", targets] => workspace::probe(Path::new(targets)),
        ["host", "check"] => host_check(),
        ["host", "apply"] => host_apply(),
        ["host", "apply", "--system"] => host_apply_system(false),
        ["host", "apply", "--system", "--yes"] => host_apply_system(true),
        ["host", "render", item, "--spec", spec] => match host::render(item, Path::new(spec)) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("maxd host render: {err}");
                ExitCode::FAILURE
            }
        },
        ["host", "quiesce", "--window", window] => match parse_window(window) {
            Some(window) => quiesce(window),
            None => usage_error(),
        },
        ["update", "watch", "--build", build] => update_watch(build),
        ["--build"] => match identity::build() {
            Some(build) => {
                println!("{build}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("maxd --build: couldn't read this binary");
                ExitCode::FAILURE
            }
        },
        ["--version" | "-V"] => {
            println!(
                "maxd {} (protocol {PROTOCOL_VERSION}, oldest {OLDEST_PROTOCOL_VERSION})",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => usage_error(),
    }
}

/// `maxd update watch`: waits for the new build to say it's healthy, and
/// puts this one back if it never does.
fn update_watch(build: &str) -> ExitCode {
    let Some(layout) = fleet::update::Layout::from_env() else {
        eprintln!("maxd update watch: HOME isn't set");
        return ExitCode::FAILURE;
    };
    let setup = match Setup::from_env() {
        Ok(setup) => setup,
        Err(err) => return service_failed(&err),
    };
    let mut services = fleet::update::SystemServices {
        manager: setup.manager,
        uid: setup.uid,
        runner: System,
    };
    match fleet::update::watch(
        &layout,
        build,
        fleet::update::HEALTHY_WAIT,
        fleet::update::WATCH_POLL,
        &mut services,
    ) {
        Ok(watched) => {
            info!(?watched, "the update watch is over");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("maxd update watch: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the usage for arguments `maxd` doesn't know.
fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// Why `maxd run` couldn't start or had to stop.
#[derive(Debug, Error)]
enum RunError {
    /// Neither `MAXD_SOCKET` nor the usual folder gives a socket path.
    #[error("couldn't work out where the socket goes. Set MAXD_SOCKET to a path")]
    NoSocket,
    /// There's no home folder to keep the database in.
    #[error("couldn't find a folder for maxd's data. Is HOME set?")]
    NoDataDir,
    /// The async runtime couldn't start.
    #[error("couldn't start: {0}")]
    Runtime(io::Error),
    /// Another `maxd` holds the socket, or it couldn't be made.
    #[error(transparent)]
    Bind(#[from] BindError),
    /// The data folder couldn't be made.
    #[error("couldn't make {}: {source}", path.display())]
    DataDir {
        /// The data folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The database couldn't be opened.
    #[error("couldn't open {}: {source}", path.display())]
    Db {
        /// The database file.
        path: PathBuf,
        /// What went wrong.
        source: OpenError,
    },
    /// The database couldn't be read.
    #[error("couldn't read {}: {source}", path.display())]
    Seen {
        /// The database file.
        path: PathBuf,
        /// What went wrong.
        source: rusqlite::Error,
    },
    /// The device ID couldn't be read or made.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// The secrets folder couldn't be made or read.
    #[error("couldn't open {}: {source}", path.display())]
    Secrets {
        /// The secrets folder.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The stop signals couldn't be caught.
    #[error("couldn't listen for stop signals: {0}")]
    Signals(io::Error),
}

/// `maxd run`. Serves until SIGINT or SIGTERM, then removes the socket. On
/// macOS it also stops once the app it came in has been deleted, and then
/// uninstalls itself.
fn run() -> ExitCode {
    init_logging();
    rotate_log();
    raise_open_files();
    // The build is read from the program's file, so read it before an app
    // update can put a newer `maxd` there. Read later, a `maxd` still
    // running the old program would report the new build, and the app
    // couldn't tell it needs a restart.
    if identity::build().is_none() {
        warn!("couldn't read this program, so maxd reports no build");
    }
    let runtime = match multi_thread_runtime() {
        Ok(runtime) => runtime,
        Err(err) => return fail(&RunError::Runtime(err)),
    };
    let result = runtime.block_on(serve());
    runtime.shutdown_timeout(SHUTDOWN_WAIT);
    match result {
        Ok(Stopped::Asked) => ExitCode::SUCCESS,
        Ok(Stopped::AppDeleted) => {
            remove_self();
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

/// How far macOS lets the soft limit on open files go, whatever the hard limit
/// says.
const OPEN_MAX: u64 = 10_240;

/// Raises the soft limit on open files as far as the system lets it. launchd
/// and systemd start `maxd` low, and every agent, link and client counts
/// against it.
fn raise_open_files() {
    use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

    let Rlimit { current, maximum } = getrlimit(Resource::Nofile);
    if current == maximum {
        return;
    }
    let raise = |to| {
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: to,
                maximum,
            },
        )
    };
    if raise(maximum).is_ok() {
        return;
    }
    let capped = maximum.map_or(OPEN_MAX, |hard| hard.min(OPEN_MAX));
    if current.is_some_and(|soft| soft < capped)
        && let Err(err) = raise(Some(capped))
    {
        warn!(%err, "couldn't raise the limit on open files");
    }
}

/// Moves launchd's log of `maxd` aside once it's past
/// [`service::LOG_LIMIT`], and sends what `maxd` writes to a fresh one.
/// systemd's journal trims itself, and its output is no file.
fn rotate_log() {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return;
    };
    let Ok(writing) = io::stderr().as_fd().try_clone_to_owned() else {
        return;
    };
    let log = service::log_file(Path::new(&home));
    match service::rotate_log(&log, &File::from(writing), service::LOG_LIMIT) {
        Ok(Some(fresh)) => {
            let _ = rustix::stdio::dup2_stdout(&fresh);
            let _ = rustix::stdio::dup2_stderr(&fresh);
            info!("started a new log, with the last one beside it");
        }
        Ok(None) => {}
        Err(err) => warn!(%err, "couldn't move the log aside"),
    }
}

/// Logs why `maxd run` stopped, for the service log.
fn fail(err: &RunError) -> ExitCode {
    error!("{err}");
    ExitCode::FAILURE
}

/// The runtime `maxd run` serves on, with a thread per core.
fn multi_thread_runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

/// Logs to stderr, which launchd writes to the log file and systemd to the
/// journal.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .init();
}

/// Sets up the hub and serves until asked to stop.
/// Makes the data folder, or closes one that's open to others. It holds the
/// database and the sessions' records, so only its owner may read it. One left
/// by an older maxd or made by another program can be wider, and `create` leaves
/// an existing folder as it is. Failing to close it warns and carries on, as
/// maxd can still run in a folder it doesn't own.
fn private_data_dir(data: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(data)?;
    let mode = std::fs::metadata(data)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        warn!(
            path = %data.display(),
            mode = %format!("{mode:o}"),
            "the data folder was open to others, so maxd closed it"
        );
        if let Err(err) = std::fs::set_permissions(data, Permissions::from_mode(0o700)) {
            warn!(path = %data.display(), %err, "couldn't close the data folder");
        }
    }
    Ok(())
}

async fn serve() -> Result<Stopped, RunError> {
    // Take the socket before anything else, so a second maxd stops here
    // and leaves the first one's files alone.
    let socket = paths::socket().ok_or(RunError::NoSocket)?;
    let listener = server::bind(&socket)?;

    let data = paths::data_dir().ok_or(RunError::NoDataDir)?;
    private_data_dir(&data).map_err(|source| RunError::DataDir {
        path: data.clone(),
        source,
    })?;
    let db_path = data.join(db::FILE);
    let db = Db::open(&db_path).map_err(|source| RunError::Db {
        path: db_path.clone(),
        source,
    })?;
    let seen = db.seen().map_err(|source| RunError::Seen {
        path: db_path,
        source,
    })?;
    let device = identity::device(&data)?;
    info!(
        device = %device.id,
        name = %device.name,
        boot = %device.boot_id.0,
        socket = %listener.path().display(),
        "maxd {} serving",
        env!("CARGO_PKG_VERSION")
    );
    let hub = Hub::new(device, seen);
    hub.remember_models(db.clone());
    hub.remember_deleted(&db);
    hub.remember_ended(db.clone());

    let secrets_path = data.join(secrets::DIR);
    let secrets = secrets::Secrets::open(secrets_path.clone(), hub.clone()).map_err(|source| {
        RunError::Secrets {
            path: secrets_path,
            source,
        }
    })?;
    secrets::install(secrets.clone());
    spawn_task("the GitHub App check", secrets.clone().run());
    start_push(&hub, &data);
    spawn_task(
        "the notification log",
        notices::watch(hub.clone(), db.clone()),
    );
    start_phones(&hub, &db, &data);
    start_network_access(&hub, &db, &data, &socket);
    let acp_store = acp_agents::AcpAgents::open(hub.clone(), db.clone(), secrets.clone());
    acp_agents::install(acp_store.clone());
    let skill_store = Arc::new(skills::store::SkillStore::under(&data));
    let library = skills::Library::open(
        hub.clone(),
        db.clone(),
        skill_store.clone(),
        &data,
        Arc::new(session::clock::SystemClock),
    );
    skills::install(library.clone());
    skills::globals::wire(skill_store.clone(), Arc::new(library.clone()));
    spawn_task("the skill library", library.watch());

    leases::detect::install(leases::detect::Detector::new(
        leases::detect::HostSystem,
        leases::detect::Paths::under(&data),
    ));
    let clock: Arc<dyn session::clock::Clock> = Arc::new(session::clock::SystemClock);
    // The Linux test host and the macOS test VM reach their guests once
    // the links exist.
    let lease_guests = later::Later::new();
    let runtime = paths::runtime_dir().unwrap_or_else(|| data.clone());
    let mut test_backends: Vec<Arc<dyn leases::backend::TestBackend>> = vec![
        #[cfg(target_os = "macos")]
        Arc::new(leases::mac_host::MacHost::on_this_mac(&data)),
        Arc::new(leases::emulator::Emulator::on_host(
            data.clone(),
            clock.clone(),
        )),
    ];
    #[cfg(target_os = "macos")]
    for vm in leases::mac_vm::MacVm::on_this_mac(
        &data,
        runtime.clone(),
        Arc::new(lease_guests.clone()),
        Arc::new(lease_guests.clone()),
    ) {
        test_backends.push(Arc::new(vm));
    }
    if cfg!(target_os = "linux") {
        test_backends.push(Arc::new(leases::incus::IncusTest::wired(Arc::new(
            lease_guests.clone(),
        ))));
    }
    let test_hosts = leases::TestHosts::open(hub.clone(), db.clone(), clock, test_backends);
    leases::install(test_hosts.clone());
    spawn_task("the test hosts", test_hosts.watch());

    let incus = incus::Incus::new(incus::SOCKET);
    let holds = Arc::new(maxhq_hold::Holds::from_env().ok_or(RunError::NoDataDir)?);
    // A holder's row goes in before its folder, so a folder with no row is
    // one maxd dropped.
    let (sweep, kept) = (holds.clone(), db.clone());
    tokio::task::spawn_blocking(move || {
        match sweep.sweep(|session| kept.has_holder(session).unwrap_or(true)) {
            Ok(0) => {}
            Ok(removed) => info!(removed, "removed the files of holders maxd dropped"),
            Err(err) => warn!(%err, "couldn't sweep the holders' folders"),
        }
    });
    let owners = session::Owners::default();
    let linked = link_guests(
        &hub,
        holds.clone(),
        &owners,
        data.join("attachments"),
        GuestSetup {
            runtime: runtime.clone(),
            codex: Arc::new(guestlink::ChatGptSignIn),
            github: Arc::new(guestlink::GithubApp),
            upgrader: Arc::new(workspace::GuestInstall(incus.clone())),
            guest_binary: guestlink::guest_binary(),
            keepalive: guestlink::Config::KEEPALIVE,
            skills: Some(skill_store.clone()),
        },
    );
    lease_guests.set(Arc::downgrade(&linked.guests));
    spawn_task("the port and tunnel links", linked.links.clone().run());

    let containers = cfg!(target_os = "linux").then(|| incus.clone());
    let watched = std::env::var_os(NO_OBSERVERS_ENV).is_none();
    let monitor = watched
        .then(|| diagnostics::Monitor::start(hub.clone(), holds.clone(), containers.clone()));
    let agent_clis = watched
        .then(|| tools::Tools::start(hub.clone(), containers.clone(), Arc::new(tools::Npm::new())));
    let registry = acp_registry::Registry::open(
        hub.clone(),
        db.clone(),
        acp_store,
        agent_clis.as_ref().map_or_else(
            || Arc::new(tools::install::Unbuilt) as Arc<dyn tools::install::Installer>,
            tools::Tools::installer,
        ),
        &data,
    );
    acp_registry::install(registry.clone());
    if watched {
        spawn_task("the ACP registry", registry.watch());
        local_models::start(hub.clone(), db.clone(), &data, true);
    }
    skills::deliver::install(skills::deliver::Delivery::open(
        hub.clone(),
        db.clone(),
        skill_store.clone(),
        containers,
        data.join("skills/workspaces"),
        Arc::new(skills::globals::always_locked),
        Some(linked.guests.clone()),
    ));

    let mut config = workspace::Config::new(incus, hub.clone(), db.clone(), runtime, data.clone());
    config.links = linked.guests.clone();
    config.caps = workspace::Caps::read();
    let workspaces = workspace::Manager::new(config);
    workspace::install(workspaces.clone());
    clone::install(hub.clone(), db.clone());
    spawn_task(
        "the cloned folders' restore",
        clone::restore(hub.clone(), db.clone()),
    );

    if std::env::var_os(NO_OBSERVERS_ENV).is_some() {
        info!("{NO_OBSERVERS_ENV} is set, so nothing is watched");
    } else {
        spawn_task("the workspace manager", workspace::run(workspaces.clone()));
        // Each observer says when it has first reported everything, so the
        // session manager knows an agent it doesn't see is gone. All are
        // counted before any starts.
        let claude_pass = hub.first_pass();
        let codex_pass = hub.first_pass();
        let incus_pass = hub.first_pass();
        spawn_task(
            "the Claude Code observer",
            observe::claude::run(hub.clone(), claude_pass),
        );
        spawn_task(
            "the Codex observer",
            observe::codex::run(hub.clone(), codex_pass),
        );
        spawn_task(
            "the Incus observer",
            observe::incus::run(hub.clone(), incus_pass),
        );
        spawn_task("the host check", host::publish(hub.clone()));
        spawn_task("the Claude usage check", usage::claude::run(hub.clone()));
        spawn_task("the Codex usage check", usage::codex::run(hub.clone()));
        spawn_task("the Cursor login check", usage::cursor::run(hub.clone()));
    }

    let hooks_port = hooks::port(&data).map_err(|source| RunError::DataDir {
        path: data.join(hooks::PORT_FILE),
        source,
    })?;
    let peers = peers::Files::system().map(|files| {
        let pins = maxhq_client::pinned::Pins::default();
        let client = maxhq_client::Client::peers(pins.clone(), paths::runtime_dir());
        peers::Peers::new(
            hub.device().id,
            files,
            db.clone(),
            Arc::new(session::clock::SystemClock),
            client,
            pins,
        )
    });
    let peer_calls: Arc<dyn agents::PeerCalls> = match &peers {
        Some(peers) => Arc::new(peers.clone()),
        None => Arc::new(agents::NoPeers),
    };
    let reach = tokio::sync::watch::Sender::new(hooks::Reach {
        port: Some(hooks_port),
        ..hooks::Reach::default()
    });
    let workflows = workflow::Shared::default();
    workflows.envs.set_library(data.join("workflows"));
    let notes = notes::Notes::new(db.clone(), hub.clone());
    notes::install(notes.clone());
    tokio::spawn({
        let notes = notes.clone();
        async move { notes.show_all().await }
    });
    let terminals = terminals::Terminals::wire(
        maxhq_hold::Terminals::from_env().ok_or(RunError::NoDataDir)?,
        hub.clone(),
    );
    // Terminal requests wait for the return from here, since the server
    // takes connections before it runs.
    let restore = terminals
        .terminals
        .as_ref()
        .map(terminals::Terminals::restoring);
    let sessions = session::start(session::Config {
        hub: hub.clone(),
        db: db.clone(),
        holders: linked.guests.clone(),
        vendors: Arc::new(session::vendors::Wired {
            pictures: Some(data.join("attachments")),
        }),
        clock: Arc::new(session::clock::SystemClock),
        attachments: data.join("attachments"),
        resume: data.join("resume"),
        timelines: data.join("timelines"),
        power: power::system(),
        owners,
        ports: linked.holders.clone(),
        unsaved: Arc::new(workspaces.clone()),
        workspaces: Arc::new(workspaces.clone()),
        credentials: Arc::new(secrets.clone()),
        files: Arc::new(linked.transfers.clone()),
        peers: peer_calls,
        leases: leases::installed(),
        keys: data.join(agents::KEYS_DIR),
        hooks: Arc::new(hooks::Installed),
        funnel: reach.subscribe(),
        workflows: workflows.clone(),
        notes: notes.clone(),
        dispatcher: cfg!(target_os = "macos")
            .then(|| session::dispatcher::home(&hub, &data))
            .flatten(),
        terminals,
    });
    // The terminals a previous maxd left come back once the manager runs,
    // so it hears of the runs that ended meanwhile.
    if let Some(restore) = restore {
        spawn_task("the terminals' return", restore);
    }
    workflow::install(
        hub.clone(),
        workflows,
        Arc::new(session::clock::SystemClock),
    );
    // Moves run where container workspaces do.
    if cfg!(target_os = "linux") {
        moves::install(workspace::mover(workspaces, sessions.clone(), db.clone()));
    }
    sessions.watch_pulls(Arc::new(secrets.clone()));
    skills::deliver::watch(Arc::new(sessions.clone()));
    // Proposals found before a restart come back. A container's wait for
    // its guest, so they come back on its next turn or scan.
    tokio::spawn(skills::deliver::look_at_start());
    let webhooks = hooks::Webhooks::new(hooks::Parts {
        db: db.clone(),
        tasks: Arc::new(sessions.clone()),
        edits: Arc::new(sessions.clone()),
        secrets: Arc::new(secrets.clone()),
        funnels: hooks::funnels(hooks_port),
        reach,
        links: peers.as_ref().map_or_else(
            || tokio::sync::watch::channel(Default::default()).1,
            peers::Peers::links,
        ),
        clock: Arc::new(session::clock::SystemClock),
    });
    hooks::install(webhooks.clone(), hooks_port);
    if let Some(peers) = peers {
        peers.serve_with(Arc::new(sessions.clone()));
        peers.notes_with(notes);
        peers.models_with(Arc::new(local_models::Shares));
        tasks::targets::tell(
            Arc::new(peers.clone()),
            tasks::Tasks::watch(&sessions),
            hub.device().id,
            Arc::new(session::clock::SystemClock),
        );
        peers::install(peers);
    }
    {
        let sessions = sessions.clone();
        secrets.on_signed_in(move |account| sessions.signed_in(account));
    }
    {
        let sessions = sessions.clone();
        local_models::on_run(move |account| sessions.local_run(account));
    }
    github_events::install(github_events::Wiring {
        sessions: &sessions,
        webhooks,
        github: Arc::new(secrets),
        db: db.clone(),
        device: hub.device().id,
    });

    if let Some(agent_clis) = &agent_clis {
        let sessions = sessions.clone();
        agent_clis.on_codex_update(move || sessions.recycle_codex());
    }
    fleet::install(fleet::Fleet {
        layout: fleet::update::Layout::from_env(),
        attachments: data.join("attachments"),
        guest_binary: guestlink::guest_binary(),
        guests: Some(linked.guests.clone()),
        monitor,
        tools: agent_clis,
    });

    let keys = app_keys::AppKeys::open(data.join(phones::DIR), hub.device().id);
    keys.publish_to(hub.clone());
    let mut terminate = signal(SignalKind::terminate()).map_err(RunError::Signals)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(RunError::Signals)?;
    let (transfers, tunnels) = (linked.transfers, linked.tunnels);
    let stopped = tokio::select! {
        () = listener.serve(hub, db, sessions.clone(), transfers, tunnels, keys) => Stopped::Asked,
        _ = terminate.recv() => {
            info!("asked to stop");
            Stopped::Asked
        }
        _ = interrupt.recv() => {
            info!("interrupted");
            Stopped::Asked
        }
        () = fleet::exit_requested() => {
            info!("restarting into an update");
            Stopped::Asked
        }
        () = service::app_watch::app_deleted() => {
            info!("the app maxd came in was deleted, so maxd stops and uninstalls itself");
            Stopped::AppDeleted
        }
    };
    // Held agents keep running in their holders, and the next maxd
    // reconnects to them.
    sessions.shutdown().await;
    // Dropping the listener removes the socket file.
    drop(listener);
    Ok(stopped)
}

/// Why [`serve`] stopped.
enum Stopped {
    /// A signal, an update or the listener ended it.
    Asked,
    /// The app this `maxd` came in was deleted.
    AppDeleted,
}

/// Uninstalls the service of a `maxd` whose app was deleted, and ends the
/// agents its holders kept. The data stays, in case you install the app
/// again.
fn remove_self() {
    let setup = match Setup::from_env_as_running() {
        Ok(setup) => setup,
        Err(err) => {
            warn!(%err, "couldn't uninstall maxd");
            return;
        }
    };
    let how = Uninstall {
        itself: true,
        ..Uninstall::new(false)
    };
    let result = service::uninstall(
        &setup,
        how,
        &Clis::find(),
        &mut System,
        &mut SystemHolders,
        &mut |line| info!("{line}"),
    );
    if let Err(err) = result {
        warn!(%err, "maxd didn't uninstall itself fully");
    }
}

/// What the guest links need from the rest of the device, besides the
/// session manager and the transfers.
struct GuestSetup {
    /// `maxd`'s runtime folder, which holds each proxy socket.
    runtime: PathBuf,
    /// The Codex token broker.
    codex: Arc<dyn guestlink::CodexTokens>,
    /// The GitHub token broker.
    github: Arc<dyn guestlink::GithubTokens>,
    /// What installs this build's guest in a container.
    upgrader: Arc<dyn guestlink::Upgrader>,
    /// The guest binary it installs.
    guest_binary: Option<PathBuf>,
    /// How often each guest is pinged.
    keepalive: Duration,
    /// The skill store uploads land in.
    skills: Option<Arc<dyn maxhq_core::skills::store::TreeStore>>,
}

/// The guest links and what they join.
struct Linked {
    /// The holders here and in each guest, and the links to the guests.
    guests: Arc<guestlink::Guests>,
    /// Uploads and downloads, here and through the guest links.
    transfers: transfer::Transfers,
    /// Every forward and stream.
    tunnels: tunnel::Tunnels,
    /// The hub between the guest links, the tunnels and the ports.
    links: ports::Links,
    /// Where the session manager says what it holds, for the port watch.
    holders: ports::HolderWatch,
}

/// Builds the guest links over `local`, this device's holders. Each link
/// answers token requests only for sessions `owners` places in its own
/// workspace. The transfers send through the links and the links hand
/// guest messages to the transfers, so the transfers get the links once
/// they exist.
fn link_guests(
    hub: &Hub,
    local: Arc<dyn session::holders::Holders>,
    owners: &session::Owners,
    attachments: PathBuf,
    with: GuestSetup,
) -> Linked {
    let later = later::Later::new();
    let transfers = transfer::Transfers::new(transfer::Config {
        guests: Arc::new(later.clone()),
        skills: with.skills,
        ..transfer::Config::local(hub.clone(), attachments)
    });
    let tunnels = tunnel::Tunnels::new(hub.clone());
    let holders = ports::HolderWatch::new();
    let links = ports::Links::new(hub.clone(), tunnels.clone(), holders.clone());
    let guests = Arc::new(guestlink::Guests::new(
        local,
        guestlink::Config {
            runtime: with.runtime,
            device: hub.device().id,
            owners: Arc::new(owners.clone()),
            codex: with.codex,
            github: with.github,
            upgrader: with.upgrader,
            guest_binary: with.guest_binary,
            files: Arc::new(transfers.clone()),
            tunnels: Arc::new(links.clone()),
            keepalive: with.keepalive,
        },
    ));
    later.set(Arc::downgrade(&guests));
    Linked {
        guests,
        transfers,
        tunnels,
        links,
        holders,
    }
}

/// Runs `task` and logs how it ends. Observers return at once on a device
/// with nothing for them to watch, and a panic in one leaves the rest
/// running.
/// Starts pushing to phones. A device that can't keeps running without it.
fn start_push(hub: &Hub, data: &Path) {
    let apns = match push::Apns::new() {
        Ok(apns) => apns,
        Err(err) => {
            error!("can't push to phones: {err}");
            return;
        }
    };
    let folder = match secrets::files::Folder::open(data.join(push::DIR)) {
        Ok(folder) => folder,
        Err(err) => {
            error!("can't push to phones: {err}");
            return;
        }
    };
    let clock = Arc::new(session::clock::SystemClock);
    let push = push::Push::open(
        hub.clone(),
        folder,
        clock,
        apns,
        Arc::new(secrets::apns_key),
    );
    push::install(push.clone());
    spawn_task("the phone pushes", push.watch());
}

/// Starts letting phones in. A device without a home folder, or that
/// can't keep the phones, runs without them.
fn start_phones(hub: &Hub, db: &Db, data: &Path) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        error!("can't pair phones: HOME isn't set");
        return;
    };
    let folder = match secrets::files::Folder::open(data.join(phones::DIR)) {
        Ok(folder) => folder,
        Err(err) => {
            error!("can't pair phones: {err}");
            return;
        }
    };
    phones::install(phones::Phones::open(
        hub.clone(),
        folder,
        db.clone(),
        home.join(".ssh").join("authorized_keys"),
        Arc::new(session::clock::SystemClock),
    ));
}

/// Network access, which listens while the device's switch is on.
fn start_network_access(hub: &Hub, db: &Db, data: &Path, socket: &Path) {
    let folder = match secrets::files::Folder::open(data.join(netaccess::DIR)) {
        Ok(folder) => folder,
        Err(err) => {
            error!("can't offer network access: {err}");
            return;
        }
    };
    let access = netaccess::Access::open(
        hub.clone(),
        db.clone(),
        Arc::new(session::clock::SystemClock),
    );
    netaccess::install(access.clone());
    spawn_task(
        "network access",
        netaccess::listener::run(netaccess::listener::Parts {
            access,
            folder,
            socket: socket.to_owned(),
            caps: netaccess::listener::Caps::STANDARD,
            addresses: Arc::new(netaccess::addrs::private),
        }),
    );
}

fn spawn_task(name: &'static str, task: impl Future<Output = ()> + Send + 'static) {
    let handle = tokio::spawn(task);
    tokio::spawn(async move {
        match handle.await {
            Ok(()) => info!("{name} has nothing more to do"),
            Err(err) if err.is_panic() => error!("{name} crashed and has stopped"),
            Err(_) => {}
        }
    });
}

/// `maxd proxy`. As a key's forced command, it runs only the proxy the
/// client asked for in SSH_ORIGINAL_COMMAND.
fn proxy() -> ExitCode {
    match proxy::forced(std::env::var_os("SSH_ORIGINAL_COMMAND").as_deref()) {
        Ok(mode) => proxy_with(mode),
        Err(err) => {
            eprintln!("maxd proxy: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `maxd proxy --peer`, `--phone` or `--pair`, which another device's, a
/// phone's or a one-time pairing key runs.
fn proxy_as(first: &ClientMessage) -> ExitCode {
    if let Err(err) = proxy::forced_peer(std::env::var_os("SSH_ORIGINAL_COMMAND").as_deref()) {
        eprintln!("maxd proxy: {err}");
        return ExitCode::FAILURE;
    }
    let Some(socket) = paths::socket() else {
        eprintln!("maxd proxy: {}", RunError::NoSocket);
        return ExitCode::FAILURE;
    };
    match proxy::run_as(&socket, first) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("maxd proxy: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `maxd authorize-key`: adds the line that lets `public_key` run only
/// `maxd proxy` to `~/.ssh/authorized_keys`, once. With `app`, an app key
/// and its label, the device trusts that key too while it trusts none.
fn authorize_key(public_key: &str, app: Option<(&str, &str)>) -> ExitCode {
    // Checked first, so a bad app key changes nothing.
    let app = match app.map(|(key, label)| (app_keys::canonical(key), label)) {
        Some((None, _)) => {
            eprintln!("maxd authorize-key: the app key isn't base64url of 32 bytes");
            return ExitCode::FAILURE;
        }
        Some((Some(key), label)) => Some((key, label)),
        None => None,
    };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("maxd authorize-key: HOME isn't set");
        return ExitCode::FAILURE;
    };
    let file = home.join(".ssh").join("authorized_keys");
    let added = maxhq_core::authorized_keys::line(&home.display().to_string(), public_key, None)
        .and_then(|line| {
            maxhq_core::authorized_keys::add(&file, &line).map_err(|err| err.to_string())
        });
    let trusted = added.and_then(|_| match app {
        Some((key, label)) => trust_app_key(&key, label),
        None => Ok(()),
    });
    match trusted {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("maxd authorize-key: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Trusts the app key `key` as setup's, unless the device trusts one
/// already. Then the app asks for it when it connects, so setup still
/// succeeds.
fn trust_app_key(key: &str, label: &str) -> Result<(), String> {
    let data = paths::data_dir().ok_or_else(|| RunError::NoDataDir.to_string())?;
    private_data_dir(&data).map_err(|source| {
        RunError::DataDir {
            path: data.clone(),
            source,
        }
        .to_string()
    })?;
    let device = identity::device_id(&data).map_err(|err| err.to_string())?;
    let keys = app_keys::AppKeys::open(data.join(phones::DIR), device);
    match keys.enroll(key, label, app_keys::Asker::Setup) {
        Ok(app_keys::Decision::Ask) => {
            eprintln!(
                "maxd authorize-key: this device already trusts an app key, so it didn't add \
                 this one. MaxHQ asks for it when it connects."
            );
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(err) => Err(format!("couldn't add the app key: {err}")),
    }
}

/// `maxd proxy`, carrying what `mode` says.
fn proxy_with(mode: proxy::Mode) -> ExitCode {
    let Some(socket) = paths::socket() else {
        eprintln!("maxd proxy: {}", RunError::NoSocket);
        return ExitCode::FAILURE;
    };
    let carried = match mode {
        proxy::Mode::Plain => proxy::run(&socket),
        proxy::Mode::GuestSsh(workspace) => proxy::run_guest_ssh(&socket, workspace),
    };
    match carried {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("maxd proxy: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `maxd service install`, `uninstall` or `status`.
fn service(action: Action) -> ExitCode {
    let setup = match Setup::from_env() {
        Ok(setup) => setup,
        Err(err) => return service_failed(&err),
    };
    let mut runner = System;
    let report = match action {
        Action::Install => service::install(&setup, &mut runner),
        Action::Uninstall { delete_data } => {
            // Each line goes out as it's done, since ending agents and
            // stopping processes can take a while.
            let result = service::uninstall(
                &setup,
                Uninstall::new(delete_data),
                &Clis::find(),
                &mut runner,
                &mut SystemHolders,
                &mut |line| println!("{line}"),
            );
            result.map(|()| Vec::new())
        }
        Action::Status => {
            return match service::status(&setup, &mut runner) {
                Ok(status) => {
                    print!("{status}");
                    if status.running() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(NOT_RUNNING)
                    }
                }
                Err(err) => service_failed(&err),
            };
        }
    };
    match report {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => service_failed(&err),
    }
}

/// Prints why a service action failed.
fn service_failed(err: &ServiceError) -> ExitCode {
    eprintln!("maxd service: {err}");
    ExitCode::FAILURE
}

/// `maxd host check`: prints each item and exits nonzero unless all are
/// fine.
fn host_check() -> ExitCode {
    let checked = host_runtime().and_then(|runtime| {
        runtime.block_on(async {
            let env = host::Env::this_device()?;
            let loaded = host::load(&env)?;
            let device = maxhq_protocol::ids::DeviceId::generate();
            let report = host::report(&env, device).await?;
            Ok((loaded.source, report))
        })
    });
    match checked {
        Ok((source, report)) => {
            println!("{}", host::source_line(&source));
            for item in &report.items {
                println!("{}", host::line(item));
            }
            if host::all_ok(&report) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("maxd host check: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `maxd host apply`: the part that needs no root.
fn host_apply() -> ExitCode {
    let applied = host_runtime().and_then(|runtime| {
        runtime.block_on(async {
            let env = host::Env::this_device()?;
            let loaded = host::load(&env)?;
            let device = maxhq_protocol::ids::DeviceId::generate();
            let report = host::report(&env, device).await?;
            let done =
                host::apply_user(&env, &loaded.spec, &report.items, None, host::Restart::Now)
                    .await?;
            let after = host::report(&env, device).await?;
            Ok((done, after))
        })
    });
    match applied {
        Ok((done, after)) => {
            if done.is_empty() {
                println!("Nothing maxd can fix by itself.");
            }
            for line in done {
                println!("{line}");
            }
            for item in after.items.iter().filter(|item| item.fix.is_some()) {
                println!("{}", host::line(item));
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("maxd host apply: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `maxd host apply --system`: the root part, after you say yes.
fn host_apply_system(yes: bool) -> ExitCode {
    let env = match host::system::service_env(Path::new("/")) {
        Ok(env) => env,
        Err(err) => {
            eprintln!("maxd host apply --system: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut confirm = || {
        print!("Go ahead? [y/N] ");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
    };
    let ask = if yes {
        host::system::Ask::Yes
    } else if io::stdin().is_terminal() {
        host::system::Ask::Terminal(&mut confirm)
    } else {
        host::system::Ask::NoTerminal
    };
    match host::system::apply(&env, &mut System, ask, &mut io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("maxd host apply --system: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A small runtime for the host commands, which talk to Incus.
fn host_runtime() -> Result<Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("couldn't start: {err}"))
}

/// `maxd host quiesce`: sends `WindDown` to this device's `maxd`, then
/// waits until every held agent has wound down or the window closes. With
/// no `maxd` running, nothing needs to wind down. `maxd` takes it only from
/// root, which the shutdown unit runs it as.
fn quiesce(window: Duration) -> ExitCode {
    let Some(socket) = paths::socket() else {
        eprintln!("maxd host quiesce: can't tell where maxd's socket is");
        return ExitCode::FAILURE;
    };
    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            println!("maxd isn't running, so no agents need to wind down");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("maxd host quiesce: couldn't reach maxd: {err}");
            return ExitCode::FAILURE;
        }
    };
    match wind_down(&mut stream, window) {
        Ok(ActionOutcome::Done) => {
            println!("the agents have wound down");
            ExitCode::SUCCESS
        }
        Ok(ActionOutcome::Refused { reason }) => {
            eprintln!("maxd host quiesce: {reason}");
            if !rustix::process::geteuid().is_root() {
                eprintln!(
                    "maxd host quiesce: maxd takes this only from root, as the shutdown unit runs \
                     it, or from the app"
                );
            }
            ExitCode::FAILURE
        }
        Ok(other) => {
            eprintln!("maxd host quiesce: maxd answered {other:?}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("maxd host quiesce: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Asks `maxd` to wind its agents down and waits for the answer, which
/// comes when the window closes at the latest.
fn wind_down(stream: &mut UnixStream, window: Duration) -> io::Result<ActionOutcome> {
    // A minute past the window covers a busy loop.
    stream.set_read_timeout(Some(window + Duration::from_secs(60)))?;
    let key = IdempotencyKey::generate();
    let hello = ClientMessage::Hello(ClientHello {
        protocol: ProtocolRange::OURS,
        client: ClientInfo {
            name: "maxd host quiesce".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        client_id: ClientId::generate(),
        role: ClientRole::App,
    });
    let ask = ClientMessage::WindDown {
        key,
        window_secs: u32::try_from(window.as_secs()).unwrap_or(u32::MAX),
    };
    let mut out = Vec::new();
    for message in [hello, ask] {
        frame::encode_message(&mut out, &message).map_err(io::Error::other)?;
    }
    stream.write_all(&out)?;
    let mut decoder = FrameDecoder::new();
    let mut buf = [0; 8192];
    loop {
        while let Some(frame) = decoder.next_frame().map_err(io::Error::other)? {
            let Frame::Message(json) = frame else {
                continue;
            };
            if let Ok(ServerMessage::ActionResult {
                key: answered,
                outcome,
            }) = serde_json::from_slice(json)
                && answered == key
            {
                return Ok(outcome);
            }
        }
        let read = stream.read(&mut buf)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "maxd closed the connection before the agents wound down",
            ));
        }
        decoder.push(&buf[..read]);
    }
}

/// A window such as `90s`, `20m`, `1h` or `1d`, written as the host
/// spec's `shutdown.quiesce_window` is, so the NixOS module passes it on.
fn parse_window(text: &str) -> Option<Duration> {
    maxhq_core::host_spec::Span::try_from(text.to_owned())
        .ok()
        .map(|span| Duration::from_secs(span.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_data_folder_is_private_however_it_got_there() {
        let dir = testing::TempDir::new();

        // A new folder, under folders that don't exist yet.
        let new = dir.path().join("a").join("data");
        private_data_dir(&new).unwrap();
        assert_eq!(mode(&new), 0o700);

        // One a parent process or an older maxd left open.
        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, Permissions::from_mode(0o755)).unwrap();
        std::fs::write(open.join("kept"), "kept").unwrap();
        private_data_dir(&open).unwrap();
        assert_eq!(mode(&open), 0o700);
        assert!(open.join("kept").exists());

        // And again at the next start.
        private_data_dir(&open).unwrap();
        assert_eq!(mode(&open), 0o700);
    }

    #[test]
    fn maxd_raises_its_own_limit_on_open_files() {
        use rustix::process::{getrlimit, Resource};

        raise_open_files();
        let limit = getrlimit(Resource::Nofile);
        let wanted = limit.maximum.map_or(OPEN_MAX, |hard| hard.min(OPEN_MAX));
        assert!(limit.current.is_none_or(|soft| soft >= wanted), "{limit:?}");
    }

    #[test]
    fn windows_take_a_unit() {
        assert_eq!(parse_window("20m"), Some(Duration::from_secs(1200)));
        assert_eq!(parse_window("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_window("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_window("1d"), Some(Duration::from_secs(86_400)));
        for bad in ["20", "m", "0m", "-5m", "20 m", "20min", ""] {
            assert_eq!(parse_window(bad), None, "{bad}");
        }
    }
}

/// Helpers for this crate's tests.
#[cfg(test)]
mod testing {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A valid PNG of 69 bytes whose header claims 16,777,216 pixels a side.
    /// Its pixels would take a petabyte, and failing to allocate them aborts
    /// the whole process.
    pub const HUGE_PNG: [u8; 69] = *b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\x01\0\0\0\x01\0\0\0\x08\x06\0\0\0\xf1f\xb1\xd4\0\0\0\x0cIDATx\x9cc`\xa0\x0c\0\0\0@\0\x01\xb74|\xef\0\0\0\0IEND\xaeB`\x82";

    /// Runs the test `name` over in a process of its own, for a test whose
    /// failure would abort the process, and panics unless it passes there.
    /// Answers true here, where the test is done, and false in that process,
    /// where it goes on.
    pub fn apart(name: &str) -> bool {
        const MARK: &str = "MAXD_TEST_APART";
        if std::env::var_os(MARK).is_some() {
            return false;
        }
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name])
            .env(MARK, "1")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        let errors = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && said.contains("1 passed"),
            "{name} ended with {} on its own:\n{said}\n{errors}",
            out.status
        );
        true
    }

    /// `dir` as a relative path from where this test runs, the way a
    /// relative entry on `PATH` reads.
    pub fn from_here(dir: &Path) -> PathBuf {
        let here = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let mut relative = PathBuf::new();
        for _ in 1..here.components().count() {
            relative.push("..");
        }
        relative.push(dir.strip_prefix("/").unwrap());
        relative
    }

    /// A folder of its own under the system temp folder, removed when
    /// dropped. Its name is short, since macOS allows socket paths of about
    /// 104 bytes.
    pub struct TempDir(PathBuf);

    impl TempDir {
        /// Makes a new, empty folder.
        pub fn new() -> Self {
            loop {
                let id = uuid::Uuid::now_v7().simple().to_string();
                // The end of a v7 UUID is random, and the start is the time.
                let path = std::env::temp_dir().join(format!("maxd-{}", &id[20..]));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("couldn't make {}: {err}", path.display()),
                }
            }
        }

        /// The folder.
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// An output pill for a command that failed, whose text holds a fence
    /// of its own, for the tests that send one to each kind of agent and
    /// read it back.
    pub fn output_pill() -> maxhq_protocol::context::InlineRef {
        maxhq_protocol::context::InlineRef::Output {
            command: "pnpm test --filter cache".to_owned(),
            device_name: "devbox".to_owned(),
            cwd: "~/code/example-app".to_owned(),
            exit_code: Some(1),
            running: false,
            ran_at: maxhq_protocol::time::Timestamp(1_793_368_920_000),
            duration_ms: Some(4_200),
            lines_total: 214,
            text: "FAIL src/cache.test.ts\n```\nexpected 1, got 2\n```\n1 failed".to_owned(),
            terminal: None,
            lines: None,
        }
    }
}

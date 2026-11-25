// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the daemon's
// environment, with its idle timeout off and no update notice or colour;
// `daemon.json` with its port and token; a 2 second `/health` check of the
// one there before starting another; `devices --json` to start it, since
// there's no `daemon start`; 30 seconds for it to come up, looking every
// 100 ms; `daemon stop --state-dir` in 10 seconds; and the config file of
// `daemonBaseUrl`, `daemonAuthToken` and `stateDir`, mode 0600, written
// only when it changes.

//! agent-device's HTTP daemon for one test host: starting it, checking it
//! and stopping it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The file the daemon writes once it listens, in its state folder.
pub const DAEMON_FILE: &str = "daemon.json";

/// The config file `maxd` passes with `--config`, in the state folder.
const CONFIG_FILE: &str = "config.json";

/// How long the daemon has to come up.
pub const READY_WAIT: Duration = Duration::from_secs(30);

/// How often to look for it meanwhile.
const READY_POLL: Duration = Duration::from_millis(100);

/// How long a health check may take.
const HEALTH_WAIT: Duration = Duration::from_secs(2);

/// How long `daemon stop` may take.
pub const STOP_WAIT: Duration = Duration::from_secs(10);

/// Variables that would point the CLI at another daemon or config, which
/// no command `maxd` runs inherits.
pub const STRIPPED: &[&str] = &[
    "AGENT_DEVICE_DAEMON_BASE_URL",
    "AGENT_DEVICE_DAEMON_AUTH_TOKEN",
    "AGENT_DEVICE_CONFIG",
];

/// What the daemon writes to [`DAEMON_FILE`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonFile {
    http_port: u16,
    token: String,
}

/// What [`CONFIG_FILE`] holds.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile<'a> {
    daemon_base_url: &'a str,
    daemon_auth_token: &'a str,
    state_dir: &'a Path,
}

/// A running daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Where it listens, on loopback.
    pub base_url: String,
    /// The config file that names it, for `--config`.
    pub config: PathBuf,
}

/// The environment every agent-device command runs with, for the daemon
/// whose state is in `state`.
pub fn env(state: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("AGENT_DEVICE_STATE_DIR", state.display().to_string()),
        ("AGENT_DEVICE_DAEMON_SERVER_MODE", "http".to_owned()),
        // `maxd` stops it, so it doesn't stop itself after five idle minutes.
        ("AGENT_DEVICE_DAEMON_IDLE_TIMEOUT_MS", "0".to_owned()),
        ("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1".to_owned()),
        ("FORCE_COLOR", "0".to_owned()),
        ("NO_COLOR", "1".to_owned()),
    ]
}

/// The daemon the state folder names, if it answers its health check.
async fn alive(state: &Path) -> Option<DaemonFile> {
    let text = fs::read_to_string(state.join(DAEMON_FILE)).ok()?;
    let file: DaemonFile = serde_json::from_str(&text).ok()?;
    // reqwest is built without a TLS crypto provider of its own.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(HEALTH_WAIT)
        .build()
        .ok()?;
    let url = format!("http://127.0.0.1:{}/health", file.http_port);
    let response = client.get(url).send().await.ok()?;
    (response.status() == reqwest::StatusCode::OK).then_some(file)
}

/// Uses the daemon in `state` if it's up, or starts one with `start`,
/// which runs `devices --json`. The answer is why it didn't come up.
pub async fn ensure<F>(state: &Path, start: F) -> Result<Endpoint, String>
where
    F: Future<Output = Result<maxhq_protocol::leases::RunOutput, String>>,
{
    fs::create_dir_all(state)
        .map_err(|err| format!("couldn't make agent-device's folder: {err}"))?;
    if let Some(file) = alive(state).await {
        return endpoint(state, &file);
    }
    let _ = fs::remove_file(state.join(DAEMON_FILE));
    let started = start.await;
    let deadline = Instant::now() + READY_WAIT;
    loop {
        if let Some(file) = alive(state).await {
            return endpoint(state, &file);
        }
        match &started {
            Err(err) => return Err(err.clone()),
            Ok(ran) if ran.timed_out => {
                return Err("agent-device's daemon didn't start in time.".to_owned())
            }
            Ok(ran) if ran.exit_code != Some(0) => {
                let line = super::process::last_line(ran)
                    .map_or(String::new(), |line| format!(": {line}"));
                return Err(format!("agent-device's daemon didn't start{line}"));
            }
            Ok(_) => {}
        }
        if Instant::now() >= deadline {
            return Err("agent-device's daemon didn't start in time.".to_owned());
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// The endpoint for `file`, with its config file written.
fn endpoint(state: &Path, file: &DaemonFile) -> Result<Endpoint, String> {
    let base_url = format!("http://127.0.0.1:{}", file.http_port);
    let config = state.join(CONFIG_FILE);
    let text = serde_json::to_string(&ConfigFile {
        daemon_base_url: &base_url,
        daemon_auth_token: &file.token,
        state_dir: state,
    })
    .map_err(|err| err.to_string())?;
    if fs::read_to_string(&config).ok().as_deref() != Some(text.as_str()) {
        write_private(&config, &text)
            .map_err(|err| format!("couldn't write agent-device's config: {err}"))?;
    }
    Ok(Endpoint { base_url, config })
}

/// Writes `text` to `path` through a file only its owner can read, then
/// moves it into place.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.new", uuid::Uuid::now_v7().simple()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let written = maxhq_os::fsmode::set_open_mode(&mut options, 0o600)
        .open(&temporary)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

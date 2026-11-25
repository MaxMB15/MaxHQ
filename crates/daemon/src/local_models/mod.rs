// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Local models: the catalog `maxd` offers, the model files it downloads,
//! the llama-server it runs for them on an Apple silicon Mac, and the
//! model servers you add by URL. A local agent is Claude Code pointed at
//! one of these servers. A Mac's managed server also serves the local
//! agents of the devices it links to, over the `maxd` link.

use std::path::Path;
use std::sync::{Arc, LazyLock, OnceLock};

use maxhq_protocol::ids::{AccountId, DeviceId, ModelServerId};
use maxhq_protocol::local_models::{LocalRun, ModelAction};
use maxhq_protocol::message::ActionOutcome;
use tokio::io::DuplexStream;

use crate::db::Db;
use crate::drive::ServerLogin;
use crate::hub::Hub;

mod catalog;
mod credits;
mod download;
mod hardware;
pub(crate) mod linked;
mod probe;
pub(crate) mod runner;
mod runtime;
mod service;
#[cfg(test)]
pub(crate) mod tests;

pub use credits::turned_down;
use service::{Service, Setup};

/// The service's folder under `maxd`'s data folder.
pub const DIR: &str = "local-models";

/// Where model files go, under the home folder.
const MODELS_DIR: &str = ".local/share/maxhq/models";

static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();

/// What to call when a local agent's server changed, so its footer shows
/// it.
type OnRun = Box<dyn Fn(AccountId) + Send + Sync>;

static ON_RUN: OnceLock<OnRun> = OnceLock::new();

/// Starts local models for this `maxd`, once. It publishes its servers
/// and takes the proxy's port before it returns, so local agents resumed
/// next find them. `online` lets it refresh the catalog.
pub fn start(hub: Hub, db: Db, data: &Path, online: bool) {
    let Some(home) = std::env::home_dir() else {
        tracing::warn!("local models need a home folder");
        return;
    };
    let setup = Setup {
        dir: data.join(DIR),
        models_dir: home.join(MODELS_DIR),
        home,
        hardware: hardware::read(),
        support: hardware::support(),
        files_from: download::HUGGING_FACE.to_owned(),
        install_runtime: true,
    };
    let service = match Service::new(hub, db, setup) {
        Ok(service) => service,
        Err(err) => {
            tracing::warn!(%err, "couldn't start local models");
            return;
        }
    };
    if SERVICE.set(service.clone()).is_ok() {
        let listeners = service.prepare();
        service.publish();
        tokio::spawn(service.run(online, listeners));
    }
}

/// Calls `f` with a local account whenever what its server does for its
/// agents changed.
pub fn on_run(f: impl Fn(AccountId) + Send + Sync + 'static) {
    let _ = ON_RUN.set(Box::new(f));
}

fn ran(account: AccountId) {
    if let Some(f) = ON_RUN.get() {
        f(account);
    }
}

/// Notes that a turn of a local agent on `account` ended, so its server's
/// credits are read again soon, when it meters them.
pub fn turn_ended(account: AccountId) {
    if let Some(service) = SERVICE.get() {
        service.turn_ended(account);
    }
}

/// Whether local models run on this `maxd`.
pub fn running() -> bool {
    SERVICE.get().is_some()
}

/// How a local agent on `account` reaches its model server, when the
/// server is here and ready.
pub fn login(account: AccountId) -> Option<ServerLogin> {
    SERVICE.get()?.login(account)
}

/// What a local agent's server does for it now: loading, speed and
/// context, for its footer.
pub fn run_of(account: AccountId, model: Option<&str>, used: Option<u32>) -> Option<LocalRun> {
    SERVICE.get()?.run_of(account, model, used)
}

/// Does a model action.
pub async fn act(action: ModelAction) -> ActionOutcome {
    match SERVICE.get() {
        Some(service) => service.act(action).await,
        None => ActionOutcome::Refused {
            reason: NOT_RUNNING.to_owned(),
        },
    }
}

const NOT_RUNNING: &str = "Local models aren't running on this maxd.";

/// Serves this device's managed server to the local agents of linked
/// devices, for the peer links.
pub struct Shares;

impl crate::peers::Models for Shares {
    fn serve(
        &self,
        _from: DeviceId,
        server: ModelServerId,
        socket: DuplexStream,
    ) -> Result<(), String> {
        SERVICE
            .get()
            .ok_or_else(|| NOT_RUNNING.to_owned())?
            .serve_linked(server, socket)
    }

    fn hold(
        &self,
        _from: DeviceId,
        server: ModelServerId,
        models: &[String],
    ) -> Result<(), String> {
        SERVICE
            .get()
            .ok_or_else(|| NOT_RUNNING.to_owned())?
            .hold(server, models)
    }
}

/// The HTTP client local models use. Every request it sends carries a
/// server's key, so it follows no redirects and the key never goes on to
/// another host.
pub fn http() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        crate::tools::install::fetch::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    });
    &CLIENT
}

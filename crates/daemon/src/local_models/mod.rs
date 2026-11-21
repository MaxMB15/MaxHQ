// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Local models: the catalog `maxd` offers, the model files it downloads,
//! the llama-server it runs for them on an Apple silicon Mac, and the
//! model servers you add by URL. A local agent is Claude Code pointed at
//! one of these servers.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use maxhq_protocol::ids::AccountId;
use maxhq_protocol::local_models::{LocalRun, ModelAction};
use maxhq_protocol::message::ActionOutcome;

use crate::db::Db;
use crate::drive::ServerLogin;
use crate::hub::Hub;

mod catalog;
mod download;
mod hardware;
mod probe;
mod runner;
mod runtime;
mod service;
#[cfg(test)]
mod tests;

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
        let listener = service.prepare();
        service.publish();
        tokio::spawn(service.run(online, listener));
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
            reason: "Local models aren't running on this maxd.".to_owned(),
        },
    }
}

/// The HTTP client local models use, shared with the tool downloads.
pub fn http() -> &'static reqwest::Client {
    crate::tools::install::fetch::client()
}

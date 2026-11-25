// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The registry's tests. The index comes from a local HTTP fixture and
//! installs go through a fake installer.

use std::sync::atomic::Ordering;

use futures::future::BoxFuture;
use maxhq_protocol::ids::WorkspaceId;

use super::*;
use crate::secrets::testing::hub;
use crate::secrets::{Secrets, DIR};
use crate::testing::TempDir;
use crate::tools::install::fixture::{Answer, Fixture};
use crate::tools::install::InstallError;

/// The index the fixture serves, with an entry that breaks the bounds,
/// one at no exact version and one MaxHQ runs natively.
fn registry_json(goose: &str) -> String {
    let binary = |sha: Option<&str>| {
        let target = |name: &str| {
            let mut target = serde_json::json!({
                "archive": format!("https://downloads.example.com/{name}.tar.bz2"),
                "cmd": "./dist/agent",
                "args": ["acp"],
            });
            if let Some(sha) = sha {
                target["sha256"] = sha.into();
            }
            target
        };
        serde_json::json!({
            "darwin-aarch64": target("darwin-aarch64"),
            "darwin-x86_64": target("darwin-x86_64"),
            "linux-aarch64": target("linux-aarch64"),
            "linux-x86_64": target("linux-x86_64"),
        })
    };
    serde_json::json!({
        "version": "1.0.0",
        "agents": [
            {
                "id": "gemini",
                "name": "Gemini CLI",
                "version": "0.9.0",
                "description": "An agent from npm",
                "distribution": { "npx": {
                    "package": "@example/gemini-cli@0.9.0",
                    "args": ["--experimental-acp"],
                    "env": { "AGENT_MODE": "acp", "not a name": "x" },
                } },
            },
            {
                "id": "goose",
                "name": "goose",
                "version": goose,
                "description": "",
                "distribution": { "binary": binary(None) },
            },
            {
                "id": "checked",
                "name": "Checked",
                "version": "2.0.0",
                "description": "",
                "distribution": { "binary": binary(Some(&"a".repeat(64))) },
            },
            {
                "id": "py",
                "name": "Py agent",
                "version": "0.1.0",
                "description": "",
                "distribution": { "uvx": { "package": "py-agent==0.1.0" } },
            },
            {
                "id": "Bad ID",
                "name": "Bad",
                "version": "1.0.0",
                "description": "",
                "distribution": { "npx": { "package": "bad@1.0.0" } },
            },
            {
                "id": "loose",
                "name": "Loose",
                "version": "1.0.0",
                "description": "",
                "distribution": { "npx": { "package": "loose@latest" } },
            },
            {
                "id": "claude-acp",
                "name": "Claude",
                "version": "1.0.0",
                "description": "",
                "distribution": { "npx": { "package": "claude-acp@1.0.0" } },
            },
        ],
    })
    .to_string()
}

/// What the fake agent answers `initialize` with: one method it runs and
/// one you run in a terminal.
#[cfg(unix)]
const REPLY: &str = r#"{"jsonrpc":"2.0","id":"probe","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"browser","name":"Sign in"},{"id":"login","name":"Log in","type":"terminal","args":["login"]}]}}"#;

/// An installer that records each call and says the command is `path`.
#[derive(Default)]
struct Fake {
    ensured: Mutex<Vec<(InstallDef, Option<String>, ToolPlace)>>,
    removed: Mutex<Vec<(ToolKind, ToolPlace)>>,
    path: Mutex<String>,
}

impl Installer for Fake {
    fn ensure<'a>(
        &'a self,
        def: &'a InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<Installed, InstallError>> {
        Box::pin(async move {
            self.ensured
                .lock()
                .unwrap()
                .push((def.clone(), version.map(str::to_owned), *place));
            Ok(Installed {
                version: version.unwrap_or_default().to_owned(),
                path: self.path.lock().unwrap().clone(),
                root: String::new(),
            })
        })
    }

    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), InstallError>> {
        Box::pin(async move {
            self.removed.lock().unwrap().push((kind.clone(), *place));
            Ok(())
        })
    }
}

struct Setup {
    temp: TempDir,
    fixture: Fixture,
    hub: Hub,
    db: Db,
    agents: AcpAgents,
    fake: Arc<Fake>,
    url: String,
}

impl Setup {
    async fn new() -> Self {
        let temp = TempDir::new();
        let fixture = Fixture::start().await;
        let url = fixture.serve(
            "/registry.json",
            Answer::Body(registry_json("1.2.0").into_bytes()),
        );
        let hub = hub();
        let db = Db::open_in_memory().unwrap();
        let secrets = Secrets::open(temp.path().join(DIR), hub.clone()).unwrap();
        let agents = AcpAgents::open(hub.clone(), db.clone(), secrets);
        Self {
            temp,
            fixture,
            hub,
            db,
            agents,
            fake: Arc::default(),
            url,
        }
    }

    fn registry(&self, url: &str, max_age: Duration) -> Registry {
        Registry::with(
            self.hub.clone(),
            self.db.clone(),
            self.agents.clone(),
            self.fake.clone(),
            url,
            self.temp.path(),
            max_age,
        )
    }

    /// A fake agent that answers `initialize`, as the installer's command.
    /// It's an sh script.
    #[cfg(unix)]
    fn agent(&self) -> String {
        let path = self.temp.path().join("agent");
        std::fs::write(&path, format!("#!/bin/sh\nread line\necho '{REPLY}'\n")).unwrap();
        maxhq_os::fsmode::set_mode(&path, 0o755).unwrap();
        let path = path.display().to_string();
        *self.fake.path.lock().unwrap() = path.clone();
        path
    }

    fn shown(&self) -> AcpRegistry {
        match self.hub.get(&EntityKey::AcpRegistry(self.hub.device().id)) {
            Some(Entity::AcpRegistry(registry)) => registry,
            other => panic!("no registry shown: {other:?}"),
        }
    }

    fn entry(&self, id: &str) -> RegistryAgent {
        self.shown()
            .agents
            .into_iter()
            .find(|agent| agent.id == id)
            .unwrap()
    }

    fn defined(&self, registry: &Registry, id: &str) -> Option<AcpAgent> {
        registry.defined(registry.def_id(id))
    }
}

fn reason(result: Result<JoinHandle<()>, String>) -> String {
    match result {
        Err(reason) => reason,
        Ok(_) => panic!("not refused"),
    }
}

const LONG: Duration = Duration::from_secs(3600);

fn workspace() -> ToolPlace {
    ToolPlace::Workspace {
        workspace: WorkspaceId::derive("ws-alpha"),
    }
}

#[test]
fn the_index_drops_bad_and_hidden_entries() {
    let index = index::parse(&registry_json("1.2.0")).unwrap();
    let ids: Vec<&str> = index.agents.iter().map(|agent| agent.id.as_str()).collect();
    assert_eq!(ids, ["gemini", "goose", "checked", "py"]);
    assert!(index::parse("{}").is_err());
    assert!(index::parse(&" ".repeat(index::MAX_BYTES + 1)).is_err());
}

#[test]
fn a_binary_for_the_target_comes_first_then_npx_then_uvx() {
    let index = index::parse(&registry_json("1.2.0")).unwrap();
    let find = |id: &str| index.agents.iter().find(|agent| agent.id == id).unwrap();
    let goose = index::choose(find("goose"), "aarch64-apple-darwin").unwrap();
    assert_eq!(
        goose.kind,
        Kind::Binary {
            archive: "https://downloads.example.com/darwin-aarch64.tar.bz2".into(),
            command: "agent".into(),
            sha256: None,
        }
    );
    assert_eq!(goose.args, ["acp"]);
    let gemini = index::choose(find("gemini"), "x86_64-unknown-linux-musl").unwrap();
    assert_eq!(
        gemini.kind,
        Kind::Npx {
            name: "@example/gemini-cli".into(),
            version: "0.9.0".into(),
        }
    );
    let py = index::choose(find("py"), "x86_64-unknown-linux-musl").unwrap();
    assert_eq!(
        py.kind,
        Kind::Uvx {
            name: "py-agent".into(),
            version: "0.1.0".into(),
        }
    );

    let mut both = find("goose").clone();
    both.distribution
        .binary
        .retain(|target, _| target == "darwin-aarch64");
    both.distribution.npx = find("gemini").distribution.npx.clone();
    let linux = index::choose(&both, "aarch64-unknown-linux-musl").unwrap();
    assert!(matches!(linux.kind, Kind::Npx { .. }));
    assert_eq!(
        index::registry_target("x86_64-unknown-linux-musl").as_deref(),
        Some("linux-x86_64")
    );
    assert_eq!(index::registry_target("riscv64-unknown-linux-gnu"), None);
}

#[tokio::test]
async fn the_registry_fetches_once_a_while_and_keeps_its_cache() {
    let setup = Setup::new().await;
    let registry = setup.registry(&setup.url, LONG);
    assert!(setup.shown().agents.is_empty());
    registry.refresh().await;
    registry.refresh().await;
    assert_eq!(setup.fixture.hits.load(Ordering::SeqCst), 1);
    let shown = setup.shown();
    assert!(shown.fetched_at.is_some() && shown.problem.is_none());
    let gemini = setup.entry("gemini");
    assert_eq!(gemini.description.as_deref(), Some("An agent from npm"));
    assert_eq!(
        gemini.install,
        Some(RegistryInstall::Npx {
            package: "@example/gemini-cli@0.9.0".into()
        })
    );
    assert_eq!(setup.entry("goose").description, None);
    assert!(setup.temp.path().join(CACHE).exists());

    // A fetch that fails leaves the cached index, with why.
    let missing = setup.url.replace("registry.json", "missing.json");
    let offline = setup.registry(&missing, LONG);
    assert_eq!(setup.shown().agents.len(), 4);
    offline.refresh().await;
    let shown = setup.shown();
    assert_eq!(shown.agents.len(), 4);
    assert!(shown.problem.is_some());
}

// The fake agent is an sh script.
#[cfg(unix)]
#[tokio::test]
async fn a_binary_with_no_checksum_installs_once_you_confirm_that_version() {
    let setup = Setup::new().await;
    setup.agent();
    let registry = setup.registry(&setup.url, Duration::ZERO);
    let place = ToolPlace::Host;
    let refusal = reason(
        registry
            .start_install("goose".into(), Some("1.2.0".into()), place, false)
            .await,
    );
    assert!(refusal.contains("no checksum"), "{refusal}");
    assert!(setup.fake.ensured.lock().unwrap().is_empty());

    registry
        .start_install("goose".into(), Some("1.2.0".into()), place, true)
        .await
        .unwrap()
        .await
        .unwrap();
    let (def, version, _) = setup.fake.ensured.lock().unwrap()[0].clone();
    assert_eq!(version.as_deref(), Some("1.2.0"));
    assert_eq!(def.binary.as_deref(), Some("agent"));
    assert!(matches!(
        def.source,
        Source::Archive {
            confirmed_unchecked: true,
            ..
        }
    ));
    // The confirm is kept for that version.
    registry
        .start_install("goose".into(), None, place, false)
        .await
        .unwrap()
        .await
        .unwrap();

    // A new version asks again.
    setup.fixture.serve(
        "/registry.json",
        Answer::Body(registry_json("1.3.0").into_bytes()),
    );
    let refusal = reason(
        registry
            .start_install("goose".into(), None, place, false)
            .await,
    );
    assert!(refusal.contains("goose 1.3.0"), "{refusal}");

    // One with a checksum needs no confirm.
    registry
        .start_install("checked".into(), None, place, false)
        .await
        .unwrap()
        .await
        .unwrap();
}

// The fake agent is an sh script.
#[cfg(unix)]
#[tokio::test]
async fn an_install_defines_the_agent_with_its_sign_in_methods() {
    let setup = Setup::new().await;
    let path = setup.agent();
    let registry = setup.registry(&setup.url, LONG);
    registry
        .start_install(
            "gemini".into(),
            Some("0.9.0".into()),
            ToolPlace::Host,
            false,
        )
        .await
        .unwrap()
        .await
        .unwrap();
    let (def, version, _) = setup.fake.ensured.lock().unwrap()[0].clone();
    assert_eq!(
        def.source,
        Source::Npm {
            package: "@example/gemini-cli".into()
        }
    );
    assert_eq!(
        def.kind,
        ToolKind::Acp {
            id: "gemini".into()
        }
    );
    assert_eq!(version.as_deref(), Some("0.9.0"));

    let agent = setup.defined(&registry, "gemini").unwrap();
    assert_eq!(agent.name, "Gemini CLI");
    assert_eq!(agent.program, path);
    assert_eq!(agent.args, ["--experimental-acp"]);
    assert_eq!(agent.env, ["AGENT_MODE"]);
    assert!(!agent.containers);
    assert_eq!(agent.problem, None);

    let shown = setup.entry("gemini");
    assert_eq!(shown.installed.as_deref(), Some("0.9.0"));
    assert_eq!(
        shown.auth,
        [
            RegistryAuth {
                id: "browser".into(),
                name: "Sign in".into(),
                terminal: false,
            },
            RegistryAuth {
                id: "login".into(),
                name: "Log in".into(),
                terminal: true,
            },
        ]
    );

    // Its installs come back with maxd.
    let reopened = setup.registry(&setup.url, LONG);
    drop(reopened);
    assert_eq!(setup.entry("gemini").installed.as_deref(), Some("0.9.0"));
}

#[tokio::test]
async fn one_in_a_container_runs_there_through_its_link() {
    let setup = Setup::new().await;
    *setup.fake.path.lock().unwrap() = "/home/agent/.local/bin/agent".into();
    let registry = setup.registry(&setup.url, LONG);
    registry
        .start_install("checked".into(), None, workspace(), false)
        .await
        .unwrap()
        .await
        .unwrap();
    let place = setup.fake.ensured.lock().unwrap()[0].2;
    assert_eq!(place, workspace());
    let agent = setup.defined(&registry, "checked").unwrap();
    assert_eq!(agent.program, "agent");
    assert!(agent.containers);
    assert_eq!(agent.args, ["acp"]);
    // With no install on this device, it isn't probed here.
    assert_eq!(agent.problem, None);
}

// The fake agent is an sh script.
#[cfg(unix)]
#[tokio::test]
async fn removing_the_last_install_deletes_the_definition() {
    let setup = Setup::new().await;
    setup.agent();
    let registry = setup.registry(&setup.url, LONG);
    registry
        .start_install("gemini".into(), None, ToolPlace::Host, false)
        .await
        .unwrap()
        .await
        .unwrap();
    assert!(setup.defined(&registry, "gemini").is_some());

    let refusal = reason(registry.start_remove("gemini".into(), workspace()));
    assert!(refusal.contains("didn't install"), "{refusal}");
    registry
        .start_remove("gemini".into(), ToolPlace::Host)
        .unwrap()
        .await
        .unwrap();
    assert_eq!(
        setup.fake.removed.lock().unwrap()[0],
        (
            ToolKind::Acp {
                id: "gemini".into()
            },
            ToolPlace::Host
        )
    );
    assert!(setup.defined(&registry, "gemini").is_none());
    assert_eq!(setup.entry("gemini").installed, None);
    assert_eq!(setup.entry("gemini").auth, []);
}

#[tokio::test]
async fn hidden_unknown_and_stale_installs_are_refused() {
    let setup = Setup::new().await;
    let registry = setup.registry(&setup.url, LONG);
    let host = ToolPlace::Host;
    let hidden = reason(
        registry
            .start_install("claude-acp".into(), None, host, false)
            .await,
    );
    assert!(hidden.contains("its own agents"), "{hidden}");
    let unknown = reason(
        registry
            .start_install("nobody".into(), None, host, false)
            .await,
    );
    assert!(unknown.contains("doesn't list"), "{unknown}");
    let stale = reason(
        registry
            .start_install("gemini".into(), Some("0.8.0".into()), host, false)
            .await,
    );
    assert!(stale.contains("Gemini CLI 0.9.0"), "{stale}");
    assert!(setup.fake.ensured.lock().unwrap().is_empty());
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_protocol::local_models::ServerFlavor;
use serde_json::json;

use super::super::tests::{Fake, Route, Scratch};
use super::*;
use crate::secrets::testing;

/// A model file's body, small enough for a test.
fn body() -> Vec<u8> {
    (0..120_000u32).map(|n| (n % 251) as u8).collect()
}

fn sha(bytes: &[u8]) -> String {
    crate::tools::install::fetch::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// An Apple M4 Max with 36 GB.
fn m4_max() -> DeviceHardware {
    let ram = 36 << 30;
    DeviceHardware {
        chip: Some("Apple M4 Max".to_owned()),
        gpu_cores: Some(32),
        ram,
        gpu: Some(hardware::budget(ram, None)),
    }
}

/// A service in a scratch folder, whose model files come from a fake.
struct World {
    scratch: Scratch,
    hub: Hub,
    db: Db,
    files: Fake,
    service: Arc<Service>,
}

impl World {
    async fn new(support: RuntimeSupport) -> Self {
        let scratch = Scratch::new();
        let files = Fake::start().await;
        let hub = testing::hub();
        let db = Db::open_in_memory().unwrap();
        let service = Self::open(&scratch, &hub, &db, &files, support);
        Self {
            scratch,
            hub,
            db,
            files,
            service,
        }
    }

    fn open(
        scratch: &Scratch,
        hub: &Hub,
        db: &Db,
        files: &Fake,
        support: RuntimeSupport,
    ) -> Arc<Service> {
        let service = Service::new(
            hub.clone(),
            db.clone(),
            Setup {
                dir: scratch.path().join("data").join(super::super::DIR),
                home: scratch.path().join("home"),
                models_dir: scratch.path().join("models"),
                hardware: m4_max(),
                support,
                files_from: files.base.clone(),
                install_runtime: false,
            },
        )
        .unwrap();
        service.lock().catalog = small_catalog(&body());
        service.publish();
        service
    }

    /// A second service on the same folder and database, as after a
    /// restart.
    fn restart(&self) -> Arc<Service> {
        Self::open(
            &self.scratch,
            &self.hub,
            &self.db,
            &self.files,
            RuntimeSupport::Managed,
        )
    }

    fn model(&self) -> CatalogModel {
        self.service.lock().catalog.models[0].clone()
    }

    fn file(&self) -> CatalogFile {
        self.model().files[0].clone()
    }

    /// Where the model file goes.
    fn path(&self) -> PathBuf {
        download::path(&self.service.models_dir, &self.model(), &self.file())
    }

    /// The managed model's ID.
    fn model_id(&self) -> String {
        managed_model_id(&self.model().id, &self.file().quant)
    }

    /// Puts the whole model file in place, as a finished download.
    fn whole_file(&self) {
        let path = self.path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body()).unwrap();
        self.service.publish();
    }

    /// Serves the model file at the URL its download asks, cut once after
    /// `cut` bytes.
    fn serve_file(&self, cut: Option<usize>) {
        let model = self.model();
        let file = self.file();
        let route = format!("/{}/resolve/{}/{}", model.repo, model.commit, file.name);
        self.files.route(&route, Route::File { body: body(), cut });
    }

    fn local_models(&self) -> LocalModels {
        match self.hub.get(&EntityKey::LocalModels(self.hub.device().id)) {
            Some(Entity::LocalModels(models)) => models,
            other => panic!("no local models: {other:?}"),
        }
    }

    fn file_state(&self) -> Option<ModelFile> {
        self.local_models().files.into_iter().next()
    }

    fn server(&self, id: ModelServerId) -> Option<ModelServer> {
        match self.hub.get(&EntityKey::ModelServer(id))? {
            Entity::ModelServer(server) => Some(server),
            _ => None,
        }
    }

    fn account(&self, id: AccountId) -> Option<Account> {
        match self.hub.get(&EntityKey::Account(id))? {
            Entity::Account(account) => Some(account),
            _ => None,
        }
    }

    async fn act(&self, action: ModelAction) -> ActionOutcome {
        self.service.act(action).await
    }

    /// Waits until the model file shows `done`.
    async fn wait_for(&self, done: impl Fn(&ModelFile) -> bool) -> ModelFile {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(file) = self.file_state().filter(|file| done(file)) {
                return file;
            }
            assert!(
                Instant::now() < deadline,
                "the file never got there: {:?}",
                self.file_state()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The first catalog model with one small file of `body`.
fn small_catalog(body: &[u8]) -> ModelCatalog {
    let mut catalog = catalog::builtin();
    let mut model = catalog.models.remove(0);
    let mut file = model.files.remove(0);
    file.bytes = body.len() as u64;
    file.sha256 = sha(body);
    model.files = vec![file];
    catalog.models = vec![model];
    catalog
}

/// A fake llama-server that runs local agents, with one model.
async fn model_server() -> Fake {
    let fake = Fake::start().await;
    fake.json(
        "/v1/models",
        200,
        &json!({"data": [{"id": "qwen3.6-35b-a3b@ud-q4_k_xl"}]}),
    );
    fake.json(
        "POST /v1/messages",
        200,
        &json!({"type": "message", "content": []}),
    );
    fake.json(
        "/props",
        200,
        &json!({"default_generation_settings": {"n_ctx": 32768}}),
    );
    fake
}

fn refusal(outcome: &ActionOutcome) -> &str {
    match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test]
async fn a_server_you_add_keeps_its_token_out_of_the_database_and_goes_away_whole() {
    let world = World::new(RuntimeSupport::UrlOnly).await;
    let fake = model_server().await;
    let added = world
        .act(ModelAction::AddServer {
            label: "  ".into(),
            url: format!("{}/v1/", fake.base),
            token: Some(Secret::new("  sk-local \n".to_owned())),
        })
        .await;
    let ActionOutcome::ModelServerAdded { server: id } = added else {
        panic!("not added: {added:?}");
    };
    let server = world.server(id).unwrap();
    assert_eq!(server.base_url.as_deref(), Some(fake.base.as_str()));
    assert_eq!(
        server.label,
        fake.base.trim_start_matches("http://"),
        "named by its host"
    );
    assert!(server.token);
    assert_eq!(
        server.kind,
        ModelServerKind::External {
            flavor: ServerFlavor::LlamaCpp
        }
    );
    assert_eq!(server.probe.as_ref().unwrap().context, Some(32_768));
    let asked = fake.seen();
    let messages = asked
        .iter()
        .find(|seen| seen.path == "/v1/messages")
        .unwrap();
    assert_eq!(
        messages.header("authorization"),
        Some("Bearer sk-local"),
        "trimmed"
    );

    let account_id = world.service.account_of(id);
    let account = world.account(account_id).unwrap();
    assert_eq!(account.login, LoginKind::ModelServer);
    assert_eq!(account.vendor, Vendor::Claude);
    assert_eq!(account.model_server, Some(id));
    assert!(account.models.iter().all(|model| model.no_images));
    assert!(account.models[0].default);

    let login = world.service.login(account_id).unwrap();
    assert_eq!(login.base_url, fake.base);
    assert_eq!(login.token.expose(), "sk-local");
    assert_eq!(login.model.as_deref(), Some("qwen3.6-35b-a3b@ud-q4_k_xl"));
    assert_eq!(login.context, Some(32_768));
    assert!(login.lean, "fewer tools by default");
    let run = world.service.run_of(account_id, None, Some(1_000)).unwrap();
    assert_eq!(run.context_size, Some(32_768));
    assert_eq!(run.context_used, Some(1_000));
    assert!(!run.loading);

    let records = world.db.model_servers().unwrap();
    assert_eq!(records.len(), 1);
    assert!(!records[0].contains("sk-local"), "{}", records[0]);
    let token_file = world.service.dir.join("keys").join(token_name(id));
    assert!(token_file.is_file());

    let again = world
        .act(ModelAction::AddServer {
            label: "again".into(),
            url: fake.base.clone(),
            token: None,
        })
        .await;
    assert_eq!(refusal(&again), SAME_URL);

    assert_eq!(
        world.act(ModelAction::TestServer { server: id }).await,
        ActionOutcome::Done
    );
    assert_eq!(
        world.act(ModelAction::RemoveServer { server: id }).await,
        ActionOutcome::Done
    );
    assert!(world.server(id).is_none());
    assert!(world.account(account_id).is_none());
    assert!(world.db.model_servers().unwrap().is_empty());
    assert!(!token_file.exists());
    assert!(world.service.login(account_id).is_none());
}

#[tokio::test]
async fn a_probe_says_what_answers_and_keeps_nothing() {
    let world = World::new(RuntimeSupport::UrlOnly).await;
    let fake = model_server().await;
    let probed = world
        .act(ModelAction::ProbeServer {
            url: fake.base.clone(),
            token: Some(Secret::new(" sk-probe ".to_owned())),
        })
        .await;
    let ActionOutcome::ServerProbed {
        flavor,
        probe,
        models,
    } = probed
    else {
        panic!("not probed: {probed:?}");
    };
    assert_eq!(flavor, ServerFlavor::LlamaCpp);
    assert!(probe.apis.messages);
    assert_eq!(probe.error, None);
    assert_eq!(models[0].id, "qwen3.6-35b-a3b@ud-q4_k_xl");
    let asked = fake.seen();
    let messages = asked
        .iter()
        .find(|seen| seen.path == "/v1/messages")
        .unwrap();
    assert_eq!(messages.method, "POST");
    assert_eq!(messages.header("authorization"), Some("Bearer sk-probe"));

    assert!(world.service.lock().servers.is_empty());
    assert!(world.db.model_servers().unwrap().is_empty());
    let kept: Vec<_> = std::fs::read_dir(world.service.dir.join("keys"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(kept, [MANAGED_KEY], "no token kept");

    let bad = world
        .act(ModelAction::ProbeServer {
            url: "ftp://gpu-box.example.com".into(),
            token: None,
        })
        .await;
    assert_eq!(refusal(&bad), BAD_URL);
}

#[tokio::test]
async fn a_url_has_to_be_a_plain_server_and_this_macs_own_server_stays() {
    let world = World::new(RuntimeSupport::Managed).await;
    for url in [
        "ftp://gpu-box.example.com",
        "gpu-box.example.com:8080",
        "http://user:secret@gpu-box.example.com",
        "http://gpu-box.example.com/?key=1",
        "",
    ] {
        let outcome = world
            .act(ModelAction::AddServer {
                label: String::new(),
                url: url.into(),
                token: None,
            })
            .await;
        assert_eq!(refusal(&outcome), BAD_URL, "{url}");
    }
    assert_eq!(
        base_url(" https://gpu-box.example.com:8443/v1/ "),
        Some((
            "https://gpu-box.example.com:8443".to_owned(),
            "gpu-box.example.com:8443".to_owned()
        ))
    );
    assert_eq!(
        base_url("http://gpu-box.example.com/"),
        Some((
            "http://gpu-box.example.com".to_owned(),
            "gpu-box.example.com".to_owned()
        ))
    );

    let unknown = ModelServerId::derive("nothing");
    assert_eq!(
        refusal(
            &world
                .act(ModelAction::RemoveServer { server: unknown })
                .await
        ),
        NO_SERVER
    );
    assert_eq!(
        refusal(&world.act(ModelAction::TestServer { server: unknown }).await),
        NO_SERVER
    );
    let own = world.service.managed_id();
    assert_eq!(
        refusal(&world.act(ModelAction::RemoveServer { server: own }).await),
        OWN_SERVER
    );
}

#[tokio::test]
async fn this_macs_own_server_serves_its_whole_files_through_the_proxy() {
    let world = World::new(RuntimeSupport::Managed).await;
    let own = world.service.managed_id();
    let account_id = world.service.account_of(own);
    let server = world.server(own).unwrap();
    assert_eq!(server.kind, ModelServerKind::Managed);
    assert!(server.models.is_empty(), "nothing downloaded yet");
    assert!(
        world.account(account_id).is_none(),
        "no account without a model"
    );

    world.whole_file();
    let id = world.model_id();
    let server = world.server(own).unwrap();
    assert_eq!(server.models.len(), 1);
    assert_eq!(server.models[0].id, id);
    assert_eq!(server.models[0].load, Some(ModelLoad::Unloaded));
    let account = world.account(account_id).unwrap();
    assert_eq!(account.model_server, Some(own));
    assert_eq!(account.models[0].id, id);

    assert!(world.service.login(account_id).is_none(), "no proxy yet");
    *world.service.proxy.lock().unwrap() = Some(40_123);
    let login = world.service.login(account_id).unwrap();
    assert_eq!(login.base_url, "http://127.0.0.1:40123");
    let key = world.service.keys.read(MANAGED_KEY).unwrap().unwrap();
    assert_eq!(login.token.expose(), key.expose());
    assert!(login.token.expose().len() >= 32);
    let native = world.model().native_context;
    assert_eq!(login.context, Some(native.min(65_536)));
    assert_eq!(login.model.as_deref(), Some(id.as_str()));
    assert!(login.lean);
    let run = world.service.run_of(account_id, Some(&id), None).unwrap();
    assert_eq!(run.context_size, Some(native.min(65_536)));
    assert!(!run.loading);

    // Tests never install llama-server, so a load says it isn't there.
    let load = world
        .act(ModelAction::Load {
            server: own,
            model: id.clone(),
        })
        .await;
    assert_eq!(refusal(&load), NO_RUNTIME);
    assert_eq!(
        world.service.runner.load_of(&id),
        ModelLoad::Failed {
            reason: NO_RUNTIME.to_owned()
        }
    );
}

#[tokio::test]
async fn a_cut_download_resumes_and_a_deleted_one_takes_its_model_away() {
    let world = World::new(RuntimeSupport::Managed).await;
    let model = world.model();
    let file = world.file();
    world.serve_file(Some(50_000));
    assert_eq!(
        world
            .act(ModelAction::Download {
                model: model.id.clone(),
                file: file.name.clone(),
            })
            .await,
        ActionOutcome::Done
    );
    let cut = world
        .wait_for(|file| matches!(file.state, ModelFileState::Failed { .. }))
        .await;
    assert_eq!(cut.bytes_done, 50_000);
    assert_eq!(cut.bytes_total, body().len() as u64);
    assert!(world
        .server(world.service.managed_id())
        .unwrap()
        .models
        .is_empty());

    assert_eq!(
        world
            .act(ModelAction::ResumeDownload {
                model: model.id.clone(),
                file: file.name.clone(),
            })
            .await,
        ActionOutcome::Done
    );
    let done = world
        .wait_for(|file| file.state == ModelFileState::Ready)
        .await;
    assert_eq!(done.bytes_done, done.bytes_total);
    assert_eq!(std::fs::read(world.path()).unwrap(), body());
    let asked = world.files.seen();
    assert_eq!(asked.last().unwrap().header("range"), Some("bytes=50000-"));
    let own = world.service.managed_id();
    assert_eq!(world.server(own).unwrap().models[0].id, world.model_id());
    let account_id = world.service.account_of(own);
    assert!(world.account(account_id).is_some());

    assert_eq!(
        world
            .act(ModelAction::DeleteModel {
                model: model.id.clone(),
                file: file.name.clone(),
            })
            .await,
        ActionOutcome::Done
    );
    assert!(!world.path().exists());
    assert!(world.file_state().is_none());
    assert!(world.server(own).unwrap().models.is_empty());
    assert!(world.account(account_id).is_none());
}

#[tokio::test]
async fn a_download_needs_a_mac_room_and_a_catalog_file() {
    let world = World::new(RuntimeSupport::NoGpu).await;
    let model = world.model();
    let file = world.file();
    let no_gpu = world
        .act(ModelAction::Download {
            model: model.id.clone(),
            file: file.name.clone(),
        })
        .await;
    assert_eq!(refusal(&no_gpu), NOT_MANAGED);
    assert!(
        world.server(world.service.managed_id()).is_none(),
        "no own server"
    );

    let world = World::new(RuntimeSupport::Managed).await;
    let unknown = world
        .act(ModelAction::Download {
            model: model.id.clone(),
            file: "nothing.gguf".into(),
        })
        .await;
    assert_eq!(refusal(&unknown), NOT_IN_CATALOG);
    world.service.lock().catalog.models[0].files[0].bytes = 1 << 60;
    let huge = world
        .act(ModelAction::Download {
            model: model.id.clone(),
            file: file.name.clone(),
        })
        .await;
    assert!(refusal(&huge).contains("isn't room"), "{huge:?}");
    assert!(world.files.seen().is_empty(), "nothing fetched");
}

#[tokio::test]
async fn only_this_macs_own_server_loads_a_downloaded_model() {
    let world = World::new(RuntimeSupport::Managed).await;
    let fake = model_server().await;
    let ActionOutcome::ModelServerAdded { server: added } = world
        .act(ModelAction::AddServer {
            label: "GPU box".into(),
            url: fake.base.clone(),
            token: None,
        })
        .await
    else {
        panic!("not added");
    };
    let model = "qwen3.6-35b-a3b@ud-q4_k_xl".to_owned();
    for action in [
        ModelAction::Load {
            server: added,
            model: model.clone(),
        },
        ModelAction::Unload {
            server: added,
            model: model.clone(),
        },
    ] {
        assert_eq!(refusal(&world.act(action).await), ONLY_MANAGED);
    }
    let unknown = ModelServerId::derive("nothing");
    assert_eq!(
        refusal(
            &world
                .act(ModelAction::Load {
                    server: unknown,
                    model: model.clone(),
                })
                .await
        ),
        NO_SERVER
    );
    let own = world.service.managed_id();
    let missing = world
        .act(ModelAction::Load {
            server: own,
            model: world.model_id(),
        })
        .await;
    assert!(
        refusal(&missing).contains("isn't downloaded"),
        "{missing:?}"
    );
    // A server you added wants no token, so the agent sends a stand-in.
    let login = world
        .service
        .login(world.service.account_of(added))
        .unwrap();
    assert_eq!(login.token.expose(), NO_TOKEN);
}

#[tokio::test]
async fn a_part_file_shows_paused_and_a_failure_shows_why() {
    let world = World::new(RuntimeSupport::Managed).await;
    let part = download::part(&world.path());
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, &body()[..1_000]).unwrap();
    world.service.publish();
    let paused = world.file_state().unwrap();
    assert_eq!(paused.state, ModelFileState::Paused);
    assert_eq!(paused.bytes_done, 1_000);

    let key = (world.model().id, world.file().name);
    world
        .service
        .lock()
        .failed
        .insert(key, "the checksum didn't match".to_owned());
    world.service.publish();
    assert_eq!(
        world.file_state().unwrap().state,
        ModelFileState::Failed {
            reason: "the checksum didn't match".to_owned()
        }
    );

    assert_eq!(
        world
            .act(ModelAction::PauseDownload {
                model: world.model().id,
                file: world.file().name,
            })
            .await,
        ActionOutcome::Done,
        "pausing what doesn't run changes nothing"
    );
}

#[tokio::test]
async fn a_restart_keeps_the_servers_their_tokens_the_key_and_the_port() {
    let world = World::new(RuntimeSupport::Managed).await;
    let fake = model_server().await;
    let ActionOutcome::ModelServerAdded { server: added } = world
        .act(ModelAction::AddServer {
            label: "GPU box".into(),
            url: fake.base.clone(),
            token: Some(Secret::new("sk-kept".to_owned())),
        })
        .await
    else {
        panic!("not added");
    };
    let key = world.service.key.expose().to_owned();
    let listener = world.service.bind().unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let again = world.restart();
    assert_eq!(again.key.expose(), key);
    let server = again.lock().servers.get(&added).cloned().unwrap();
    assert_eq!(server.label, "GPU box");
    assert_eq!(server.device, world.hub.device().id);
    let login = again.login(again.account_of(added)).unwrap();
    assert_eq!(login.token.expose(), "sk-kept");
    let listener = again.bind().unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), port);
    assert_eq!(
        std::fs::read_to_string(again.dir.join(PORT_FILE)).unwrap(),
        port.to_string()
    );
    assert_eq!(*again.proxy.lock().unwrap(), Some(port));
}

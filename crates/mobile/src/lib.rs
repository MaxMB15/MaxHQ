// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone app's shell, as `apps/desktop/src-tauri` is the desktop's. It
//! holds no logic. Each call passes JSON strings to and from `maxhq_app`,
//! the same JSON the desktop's commands carry, so the types in
//! `packages/shared/src/bindings.ts` describe both.
//!
//! A call that answers gives a reply, the shape the desktop's commands
//! give through tauri-specta:
//!
//! ```json
//! { "status": "ok", "data": <what the call answers> }
//! { "status": "error", "error": { "kind": "failed", "message": "..." } }
//! ```
//!
//! where `error` is a [`CommandError`]. The calls reach React Native
//! through UniFFI.

mod backup;
mod keys;
mod net;
mod phone;

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use maxhq_app::request::{CommandError, Request};
use maxhq_app::timeline::{Sink, TimelineMessage};
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::phones::PushToken;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use crate::keys::Keychain;
pub use crate::keys::{
    forget_token, push_public, save_token, seed, token, Key, KeyStore, MemoryStore, Token,
};
pub use crate::phone::{Core, PhoneKey};

uniffi::setup_scaffolding!();

/// What a call answers before [`start`].
pub const NOT_STARTED: &str = "The phone core hasn't started.";

/// The platform the app runs on, as [`start`] takes it: `"ios"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Ios,
}

/// Whether the app is on screen, as [`app_state`] takes it: `"active"` or
/// `"background"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    /// On screen. The client redials at once.
    Active,
    /// Gone to the background. The client closes its links.
    Background,
}

/// The network the phone is on, as [`network_changed`] takes it: `"wifi"`,
/// `"cellular"` or `"none"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    Wifi,
    Cellular,
    /// No network at all.
    None,
}

/// Gets the view's changes, each call a JSON array of `Delta` oldest first,
/// the same JSON as the desktop's `ViewChanged` event.
#[uniffi::export(with_foreign)]
pub trait ViewListener: Send + Sync + 'static {
    fn changed(&self, delta: String);
}

/// Gets one screen's timeline batches, each a JSON array of
/// `TimelineMessage`, the same JSON the desktop's timeline channel carries.
#[uniffi::export(with_foreign)]
pub trait TimelineListener: Send + Sync + 'static {
    /// Tells screens apart. A screen that opens again uses a new one.
    fn id(&self) -> u32;

    /// Sends a batch, and says whether the screen took it.
    fn batch(&self, messages: String) -> bool;
}

/// Hears how far a picture has got, one `TransferProgress` as JSON per call,
/// the same JSON the desktop's progress channel carries.
#[uniffi::export(with_foreign)]
pub trait MediaListener: Send + Sync + 'static {
    fn progress(&self, progress: String);
}

/// A [`TimelineListener`] as the core's [`Sink`].
pub struct Screen(pub Arc<dyn TimelineListener>);

impl Sink for Screen {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn send(&self, batch: Vec<TimelineMessage>) -> bool {
        match serde_json::to_string(&batch) {
            Ok(json) => self.0.batch(json),
            Err(_) => false,
        }
    }
}

/// The core, once [`start`] opened it. It lives as long as the app.
static CORE: OnceLock<Core> = OnceLock::new();

/// Held while [`start`] opens the core, so two starts open one.
static STARTING: Mutex<()> = Mutex::new(());

fn core() -> Result<&'static Core, CommandError> {
    CORE.get().ok_or_else(|| CommandError::Failed {
        message: NOT_STARTED.to_owned(),
    })
}

/// Opens the core with the phone's data folder `dir` and cache folder
/// `cache`, with the phone's key from the Keychain. A JS reload starts
/// again, which lets go of what the old page watched. Answers a reply with
/// `data: null`.
#[uniffi::export]
pub fn start(dir: String, cache: String, platform: String) -> String {
    reply(word::<Platform>(&platform).and_then(|_| {
        let _starting = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(core) = CORE.get() {
            core.reloaded();
            return Ok(());
        }
        let core = Core::open(
            std::path::Path::new(&dir),
            std::path::Path::new(&cache),
            keys::system()?.into(),
        )?;
        let _ = CORE.set(core);
        Ok(())
    }))
}

/// Answers one request. `json` is a `Request` as JSON, such as
/// `{"kind":"mark_seen","agent":"<id>"}`, and the reply's `data` is the
/// `Response` of its kind, such as `{"kind":"mark_seen"}`.
#[uniffi::export]
pub async fn request(json: String) -> String {
    let asked = read::<Request>(&json).and_then(|request| Ok(core()?.request(request)));
    match asked {
        Ok(answer) => reply(answer.await),
        Err(error) => reply::<()>(Err(error)),
    }
}

/// Sends each change to the view to `listener`, in place of any before.
/// Ask for a snapshot with [`request`] first, then apply each delta.
#[uniffi::export]
pub fn subscribe(listener: Arc<dyn ViewListener>) {
    match core() {
        Ok(core) => core.subscribe(listener),
        Err(_) => tracing::warn!("a view listener came before the core started"),
    }
}

/// Opens an agent's timeline on one screen. `agent` is its ID as a JSON
/// string's contents, and batches reach `listener`, starting with a reset
/// to the newest items. Answers a reply with `data: null`. Close it with
/// a `close_timeline` request.
#[uniffi::export]
pub async fn open_timeline(agent: String, listener: Arc<dyn TimelineListener>) -> String {
    let opened = word::<AgentId>(&agent)
        .and_then(|agent| Ok(core()?.open_timeline(agent, Arc::new(Screen(listener)))));
    match opened {
        Ok(opened) => reply(opened.await),
        Err(error) => reply::<()>(Err(error)),
    }
}

/// Fetches a picture or video `agent` can see into the media cache, unless
/// it's there already or is larger than `limit` bytes, with how far it has
/// got going to `listener`. `path` is as the timeline names it, and `agent`
/// is its ID as a JSON string's contents. Answers a reply whose `data` is a
/// `MediaFile`, and a cached file's `name` sits in the cache folder's `media`.
/// Stop it with a `cancel_transfer` request.
#[uniffi::export]
pub async fn media(
    agent: String,
    path: String,
    limit: u32,
    listener: Arc<dyn MediaListener>,
) -> String {
    let asked =
        word::<AgentId>(&agent).and_then(|agent| Ok(core()?.media(agent, path, limit, listener)));
    match asked {
        Ok(fetched) => reply(fetched.await),
        Err(error) => reply::<()>(Err(error)),
    }
}

/// Tells the core the app came to the screen or left it. Answers a reply
/// with `data: null`.
#[uniffi::export]
pub fn app_state(state: String) -> String {
    reply(word::<AppState>(&state).and_then(|state| {
        core()?.app_state(state);
        Ok(())
    }))
}

/// Tells the core the phone changed networks. Answers a reply with
/// `data: null`.
#[uniffi::export]
pub fn network_changed(kind: String) -> String {
    reply(word::<Network>(&kind).and_then(|kind| {
        core()?.network_changed(kind);
        Ok(())
    }))
}

/// Hands the core the APNs token, which it sends to every device it links
/// to. `token` is a `PushToken` as JSON:
/// `{"token":"<hex>","topic":"com.example.maxhq","environment":"sandbox","kinds":["needs_input"]}`.
/// It sends it again whenever a link comes back, so call it again only
/// when the token or the kinds change. Answers a reply with `data: null`.
#[uniffi::export]
pub fn push_token(token: String) -> String {
    reply(read::<PushToken>(&token).and_then(|push| {
        core()?.push_token(push);
        Ok(())
    }))
}

/// What [`presence`] takes: the agent on screen, or `null`.
#[derive(Debug, Deserialize)]
struct Presence {
    agent: Option<AgentId>,
}

/// Tells the core which agent is on screen, so no device pushes about it
/// while the app is open. `json` is `{"agent":"<id>"}`, or
/// `{"agent":null}` off an agent's screen. Answers a reply with
/// `data: null`.
#[uniffi::export]
pub fn presence(json: String) -> String {
    reply(read::<Presence>(&json).and_then(|presence| {
        core()?.presence(presence.agent);
        Ok(())
    }))
}

/// The phone's key and the line that lets it in, for you to add to a
/// device's `authorized_keys` by hand. Answers a reply whose
/// `data` is a [`PhoneKey`].
#[uniffi::export]
pub fn phone_key() -> String {
    reply(core().map(Core::key))
}

/// Pairs the phone through the device whose pairing code is `code`, the
/// text in the QR code the Mac shows, as `name`, such as the phone's own
/// name. Answers a reply whose `data` is the `PhoneBook` the phone keeps.
#[uniffi::export]
pub async fn pair(code: String, name: String) -> String {
    match core() {
        Ok(core) => reply(core.pair(&code, &name).await),
        Err(error) => reply::<()>(Err(error)),
    }
}

/// Adds a device to the phone's book by hand, without pairing:
/// `name` to show, `route` as `user@host:port`, and `host_key`, the
/// device's ed25519 host key as an OpenSSH line. A device of the same name
/// is replaced. Answers a reply with `data: null`.
#[uniffi::export]
pub fn add_device(name: String, route: String, host_key: String) -> String {
    reply(core().and_then(|core| core.add_device(&name, &route, &host_key)))
}

/// `text` with the secrets it holds masked, such as tokens and keys a
/// command printed, as the desktop masks a command's output before it
/// goes to an agent. It needs no core, so it answers the text itself, not
/// a reply.
#[uniffi::export]
pub fn mask_text(text: String) -> String {
    maxhq_core::mask::mask_text(&text).into_owned()
}

/// A call's answer as a reply.
fn reply<T: Serialize>(result: Result<T, CommandError>) -> String {
    let value = match result.and_then(|data| to_value(&data)) {
        Ok(data) => serde_json::json!({ "status": "ok", "data": data }),
        Err(error) => match to_value(&error) {
            Ok(error) => serde_json::json!({ "status": "error", "error": error }),
            Err(_) => Value::Null,
        },
    };
    value.to_string()
}

fn to_value(value: &impl Serialize) -> Result<Value, CommandError> {
    serde_json::to_value(value).map_err(failed)
}

/// Reads a call's JSON input. The error names what didn't fit, not the
/// input, which can hold a token.
fn read<T: DeserializeOwned>(json: &str) -> Result<T, CommandError> {
    serde_json::from_str(json).map_err(failed)
}

/// Reads a call's one-word input, such as `"active"`.
fn word<T: DeserializeOwned>(word: &str) -> Result<T, CommandError> {
    serde_json::from_value(Value::String(word.to_owned())).map_err(failed)
}

pub(crate) fn failed(error: serde_json::Error) -> CommandError {
    CommandError::Failed {
        message: format!("That doesn't read: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use serde_json::json;

    use super::*;

    /// Runs a call that answers before the core starts, so never waits.
    fn now<F: Future>(future: F) -> F::Output {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("a call waited"),
        }
    }

    fn parsed(reply: String) -> Value {
        serde_json::from_str(&reply).unwrap()
    }

    fn refused(message: &str) -> Value {
        json!({ "status": "error", "error": { "kind": "failed", "message": message } })
    }

    fn token() -> Value {
        json!({
            "token": "test-not-a-real-token",
            "topic": "com.example.maxhq",
            "environment": "sandbox",
            "kinds": ["needs_input"],
        })
    }

    #[test]
    fn calls_before_the_core_starts_say_so() {
        let not_started = refused(NOT_STARTED);
        assert_eq!(
            parsed(now(request(r#"{"kind":"app_info"}"#.into()))),
            not_started
        );
        assert_eq!(parsed(app_state("active".into())), not_started);
        assert_eq!(parsed(network_changed("wifi".into())), not_started);
        assert_eq!(parsed(push_token(token().to_string())), not_started);
        assert_eq!(parsed(presence(r#"{"agent":null}"#.into())), not_started);
        assert_eq!(parsed(phone_key()), not_started);
        assert_eq!(
            parsed(now(pair("maxhq:AQ".into(), "iPhone".into()))),
            not_started
        );
        let agent = AgentId::derive("agent").to_string();
        let recorder = Arc::new(Recorder::default());
        assert_eq!(
            parsed(now(open_timeline(agent.clone(), recorder.clone()))),
            not_started
        );
        assert_eq!(
            parsed(now(media(agent, "shot.png".into(), 1024, recorder))),
            not_started
        );
    }

    #[test]
    fn inputs_are_read_before_anything_runs() {
        for wrong in [
            parsed(app_state("asleep".into())),
            parsed(network_changed("satellite".into())),
            parsed(start("/data".into(), "/cache".into(), "android".into())),
            parsed(now(request(r#"{"kind":"no_such_kind"}"#.into()))),
            parsed(presence(r#"{"agent":"nobody"}"#.into())),
            parsed(now(media(
                "nobody".into(),
                "shot.png".into(),
                1024,
                Arc::new(Recorder::default()),
            ))),
        ] {
            assert_eq!(wrong["status"], "error");
            assert_eq!(wrong["error"]["kind"], "failed");
            let message = wrong["error"]["message"].as_str().unwrap();
            assert!(message.starts_with("That doesn't read"), "{message}");
        }
    }

    #[test]
    fn output_is_masked_without_the_core() {
        let token = format!("ghp_{}", "Zx7Q".repeat(9));
        assert_eq!(
            mask_text(format!("GH_TOKEN={token}\nok")),
            "GH_TOKEN=[hidden]\nok"
        );
        assert_eq!(mask_text("1 passed".into()), "1 passed");
    }

    #[test]
    fn a_bad_token_is_refused_without_its_value() {
        let wrong = parsed(push_token(r#"{"token":"test-not-a-real-token"}"#.into()));
        assert_eq!(wrong["status"], "error");
        let message = wrong["error"]["message"].as_str().unwrap();
        assert!(!message.contains("test-not-a-real-token"), "{message}");
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl TimelineListener for Recorder {
        fn id(&self) -> u32 {
            7
        }

        fn batch(&self, messages: String) -> bool {
            self.0.lock().unwrap().push(messages);
            true
        }
    }

    impl ViewListener for Recorder {
        fn changed(&self, delta: String) {
            self.0.lock().unwrap().push(delta);
        }
    }

    impl MediaListener for Recorder {
        fn progress(&self, progress: String) {
            self.0.lock().unwrap().push(progress);
        }
    }

    #[test]
    fn the_page_gets_each_batch_of_view_changes_as_one_json_array() {
        use maxhq_protocol::state::{Delta, Seq};
        let recorder = Arc::new(Recorder::default());
        let listener: Arc<dyn ViewListener> = recorder.clone();
        let emit = phone::emit_to(Arc::new(std::sync::RwLock::new(Some(listener))));
        let delta = |seq| Delta {
            seq: Seq(seq),
            changes: Vec::new(),
        };
        emit(vec![delta(1), delta(2)]);
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [r#"[{"seq":1,"changes":[]},{"seq":2,"changes":[]}]"#]
        );
    }

    #[test]
    fn a_screen_gets_batches_as_json_arrays() {
        let recorder = Arc::new(Recorder::default());
        let screen = Screen(recorder.clone());
        assert_eq!(screen.id(), 7);
        assert!(screen.send(Vec::new()));
        assert_eq!(*recorder.0.lock().unwrap(), ["[]"]);
    }

    /// A fresh app folder under the system temp folder, with a data folder
    /// and a cache folder in it as the phone has them.
    struct Data(std::path::PathBuf);

    impl Data {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "maxhq-mobile-{}",
                maxhq_protocol::ids::InstanceId::generate()
            ));
            Self(dir)
        }

        fn dir(&self) -> std::path::PathBuf {
            self.0.join("Documents/maxhq")
        }

        fn cache(&self) -> std::path::PathBuf {
            self.0.join("Library/Caches/maxhq")
        }

        fn open(&self, store: Arc<dyn KeyStore>) -> Core {
            Core::open(&self.dir(), &self.cache(), store).unwrap()
        }
    }

    impl Drop for Data {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ask(core: &Core, request: Value) -> Result<Value, CommandError> {
        let request = serde_json::from_value(request).unwrap();
        let answer = futures_lite(core.request(request))?;
        Ok(serde_json::to_value(answer).unwrap())
    }

    /// Waits for a call the core runs on its own runtime.
    fn futures_lite<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn the_core_runs_with_its_key_kept_and_no_devices() {
        let data = Data::new();
        let store: Arc<dyn KeyStore> = Arc::new(MemoryStore::default());
        let core = data.open(Arc::clone(&store));
        assert!(data.dir().join("maxhq.db").exists());
        let key = core.key();
        assert!(key.public_key.starts_with("ssh-ed25519 "), "{key:?}");
        assert_eq!(
            key.phone,
            maxhq_protocol::ids::PhoneId::derive(&key.public_key)
        );
        assert!(
            key.line.starts_with(&format!(
                "restrict,command=\"~/.local/bin/maxd proxy --phone {}\" ssh-ed25519 ",
                key.phone
            )),
            "{}",
            key.line
        );
        let snapshot = ask(&core, json!({ "kind": "view_snapshot" })).unwrap();
        assert_eq!(snapshot["kind"], "view_snapshot");
        let listener = Arc::new(Recorder::default());
        core.subscribe(listener.clone());
        core.app_state(AppState::Background);
        core.app_state(AppState::Active);
        core.presence(Some(AgentId::derive("agent")));
        core.push_token(serde_json::from_value(token()).unwrap());
        core.network_changed(Network::Cellular);
        core.reloaded();
        drop(core);

        // The same key comes back from the store.
        let again = data.open(Arc::clone(&store));
        assert_eq!(again.key(), key);
    }

    #[test]
    fn the_media_cache_moves_from_the_data_folder_to_the_cache_folder() {
        let data = Data::new();
        let store: Arc<dyn KeyStore> = Arc::new(MemoryStore::default());
        let old = data.dir().join("media");
        let media = data.cache().join("media");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("first.png"), "first").unwrap();
        drop(data.open(Arc::clone(&store)));
        assert!(!old.exists());
        assert_eq!(
            std::fs::read_to_string(media.join("first.png")).unwrap(),
            "first"
        );

        // One left behind once the cache folder has its own goes, since it
        // all fetches again.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("second.png"), "second").unwrap();
        drop(data.open(store));
        assert!(!old.exists());
        assert!(media.join("first.png").exists());
        assert!(!media.join("second.png").exists());
        assert!(data.dir().join("maxhq.db").exists());
    }

    #[test]
    fn the_phone_has_a_push_key_and_reads_a_code_before_dialing() {
        let data = Data::new();
        let store: Arc<dyn KeyStore> = Arc::new(MemoryStore::default());
        let core = data.open(Arc::clone(&store));
        let push = seed(&*store, Key::Push).unwrap();
        assert_eq!(core.key().push_public, push_public(&push));
        let wrong = futures_lite(core.pair("https://example.com", "iPhone")).unwrap_err();
        assert_eq!(
            wrong,
            CommandError::Failed {
                message: "That isn't a MaxHQ pairing code".to_owned()
            }
        );
    }

    #[test]
    fn a_device_added_by_hand_goes_in_the_book() {
        let data = Data::new();
        let core = data.open(Arc::new(MemoryStore::default()));
        let host_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample root@devbox";
        for (name, route, key) in [
            ("", "dev@devbox:22", host_key),
            ("devbox", "devbox", host_key),
            ("devbox", "dev@devbox:22", "nothing"),
        ] {
            assert!(core.add_device(name, route, key).is_err(), "{route}");
        }
        core.add_device("devbox", "dev@devbox:22", host_key)
            .unwrap();
        core.add_device("devbox", "dev@devbox.local:2222", host_key)
            .unwrap();
        let book = core.shared_book();
        assert_eq!(book.phone, core.key().phone);
        assert_eq!(book.devices.len(), 1);
        let entry = &book.devices[0];
        assert_eq!(entry.name, "devbox");
        assert_eq!(entry.routes[0].host, "devbox.local");
        assert_eq!(entry.routes[0].port, 2222);
        assert_eq!(
            entry.host_key,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample"
        );
    }
}

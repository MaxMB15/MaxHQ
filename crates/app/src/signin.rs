// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Browser sign-ins for a device: Sign in with ChatGPT, per W-5, and Sign
//! in with OpenRouter. The device starts the round and keeps its PKCE
//! verifier. The app listens on this Mac at the port the device names,
//! opens the browser at the device's page, checks the callback's `state`,
//! and hands the code to the device. Nothing here keeps the code once the
//! device has it, or logs it. A first ChatGPT sign-in's callback also names
//! the app ID OpenAI gave this device, which goes to the device with the
//! code. OpenRouter's page sends the browser to `localhost`, which it may
//! try on IPv6 first, so its round listens on ::1 as well.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use maxhq_protocol::ids::{DeviceId, IdempotencyKey, SignInId};
use maxhq_protocol::local_models::ModelAction;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::ClientMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use url::Url;

use crate::devices::{Connector, Devices};
use crate::request::CommandError;
use crate::Shell;

/// How long a round waits for the browser to come back.
pub const LIMIT: Duration = Duration::from_secs(10 * 60);

/// The path the browser comes back to, when the device's page doesn't say
/// in its `redirect_uri`. It's the one the Codex CLI listens on.
const CALLBACK_PATH: &str = "/auth/callback";

/// The host a `redirect_uri` has to use for this Mac. The listener binds
/// it alone, so `localhost`, which a browser may try on IPv6 first, isn't
/// taken.
const LOOPBACK: &str = "127.0.0.1";

/// Where a device's sign-in page has to be. `maxd` reads its endpoints from this
/// issuer and refuses any off its origin, so a page anywhere else isn't OpenAI's.
const SIGN_IN_ORIGIN: &str = "https://auth.openai.com";

/// Where a device's Sign in with OpenRouter page has to be.
const OPENROUTER_ORIGIN: &str = "https://openrouter.ai";

/// The start of an app ID OpenAI gives a device on its first sign-in.
const CLIENT_PREFIX: &str = "oaiapp_";

/// The most of a request the listener reads. A callback's is far smaller.
const HEAD_LIMIT: usize = 16 * 1024;

/// How long a connection to the listener gets to send its request. A
/// browser opens some it never uses.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs a whole Sign in with ChatGPT round for a device, browser and
/// callback included. It gives up after 10 minutes.
pub async fn sign_in_chatgpt<C: Connector>(
    devices: &Devices<C>,
    shell: &dyn Shell,
    device: DeviceId,
) -> Result<ActionOutcome, CommandError> {
    let round = OnDevice {
        devices,
        shell,
        device,
    };
    sign_in(&round, &Kind::ChatGpt, LIMIT, std::future::pending()).await
}

/// Runs a whole Sign in with OpenRouter round for a device, browser and
/// callback included, with what `kind` names. It gives up after 10
/// minutes, or when `waiting` is cancelled or starts another round.
pub async fn sign_in_openrouter<C: Connector>(
    devices: &Devices<C>,
    shell: &dyn Shell,
    device: DeviceId,
    kind: Kind,
    waiting: &Waiting,
) -> Result<ActionOutcome, CommandError> {
    let round = OnDevice {
        devices,
        shell,
        device,
    };
    sign_in(&round, &kind, LIMIT, waiting.next()).await
}

/// Which sign-in a round runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Sign in with ChatGPT, for Codex on the device.
    ChatGpt,
    /// Sign in with OpenRouter, which makes a key for the device's
    /// OpenRouter server, a new one named `label` when it has none.
    OpenRouter {
        /// The name a new server gets.
        label: Option<String>,
        /// Where OpenRouter stops the key, in US cents.
        limit_cents: Option<u32>,
    },
}

/// How a kind's page sends the browser back, and what its callback brings.
struct Shape {
    /// Where the device's page has to be.
    origin: &'static str,
    /// The page's parameter that names where the browser comes back.
    redirect: &'static str,
    /// The host that parameter has to name.
    host: &'static str,
    /// The path the browser comes back to when the page names none. With
    /// none here, the page has to name it.
    path: Option<&'static str>,
    /// Whether a callback may name an app ID for the device.
    client_id: bool,
    /// Whether the round listens on ::1 too, for a `host` a browser may
    /// try on IPv6 first.
    ipv6: bool,
    /// Why a callback with neither `state` nor a code ends the round, for
    /// a kind whose page leaves `state` out when you deny it, as
    /// OpenRouter's does.
    bare_ends: Option<&'static str>,
}

const CHATGPT: Shape = Shape {
    origin: SIGN_IN_ORIGIN,
    redirect: "redirect_uri",
    host: LOOPBACK,
    path: Some(CALLBACK_PATH),
    client_id: true,
    ipv6: false,
    bare_ends: None,
};

const OPENROUTER: Shape = Shape {
    origin: OPENROUTER_ORIGIN,
    redirect: "callback_url",
    host: "localhost",
    path: None,
    client_id: false,
    ipv6: true,
    bare_ends: Some("OpenRouter sent you back without a key"),
};

impl Kind {
    fn shape(&self) -> &'static Shape {
        match self {
            Self::ChatGpt => &CHATGPT,
            Self::OpenRouter { .. } => &OPENROUTER,
        }
    }

    /// What asks the device to start a round.
    fn begin(&self) -> ClientMessage {
        let key = IdempotencyKey::generate();
        match self {
            Self::ChatGpt => ClientMessage::BeginChatGptSignIn { key },
            Self::OpenRouter { label, limit_cents } => ClientMessage::ModelAction {
                key,
                action: ModelAction::BeginOpenRouterSignIn {
                    label: label.clone(),
                    limit_cents: *limit_cents,
                },
            },
        }
    }

    /// What hands the device the code the callback brought.
    fn finish(&self, sign_in: SignInId, code: Code, state: String) -> ClientMessage {
        let key = IdempotencyKey::generate();
        match self {
            Self::ChatGpt => ClientMessage::FinishChatGptSignIn {
                key,
                sign_in,
                code: code.code,
                state,
                client_id: code.client_id,
            },
            Self::OpenRouter { .. } => ClientMessage::ModelAction {
                key,
                action: ModelAction::FinishOpenRouterSignIn {
                    sign_in,
                    code: code.code,
                },
            },
        }
    }
}

/// The Sign in with OpenRouter round this app waits on, which Cancel ends.
#[derive(Default)]
pub struct Waiting {
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

impl Waiting {
    /// What ends the next round, with why: a cancel, or a newer round
    /// taking its place.
    fn next(&self) -> impl Future<Output = &'static str> + Send + use<> {
        let (stop, stopped) = oneshot::channel();
        let older = self
            .stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(stop);
        drop(older);
        async move {
            match stopped.await {
                Ok(()) => "You cancelled the sign-in",
                Err(_) => "A newer sign-in took this one's place",
            }
        }
    }

    /// Ends the round that waits, if one does.
    pub fn cancel(&self) {
        let stop = self
            .stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stop) = stop {
            let _ = stop.send(());
        }
    }
}

/// What a round needs from the world: the device, and a browser.
pub trait Round: Sync {
    /// Sends an action to the device and waits for its result.
    fn act(
        &self,
        action: ClientMessage,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send;

    /// Opens a page in the browser.
    fn open(&self, url: &str) -> impl Future<Output = Result<(), String>> + Send;

    /// Listens on `host`, a loopback address, at `port` for the browser to
    /// come back.
    fn listen(
        &self,
        host: IpAddr,
        port: u16,
    ) -> impl Future<Output = std::io::Result<TcpListener>> + Send {
        TcpListener::bind((host, port))
    }
}

/// A round with a device the app has a link to, and the shell's browser.
struct OnDevice<'a, C: Connector> {
    devices: &'a Devices<C>,
    shell: &'a dyn Shell,
    device: DeviceId,
}

impl<C: Connector> Round for OnDevice<'_, C> {
    fn act(
        &self,
        action: ClientMessage,
    ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send {
        self.devices.act_on(self.device, action)
    }

    fn open(&self, url: &str) -> impl Future<Output = Result<(), String>> + Send {
        self.shell.open_browser(url.to_owned())
    }
}

/// Runs a round of `kind`: begin on the device, listen, open the browser,
/// wait for the callback with the round's `state`, and finish on the
/// device. An outcome other than a waiting round, such as a refusal, comes
/// back as it is. The listeners close before the code goes to the device.
/// The round stops at `limit`, or when `stopped` ends with why.
pub async fn sign_in(
    round: &impl Round,
    kind: &Kind,
    limit: Duration,
    stopped: impl Future<Output = &'static str>,
) -> Result<ActionOutcome, CommandError> {
    let begun = round.act(kind.begin()).await?;
    let ActionOutcome::SignIn {
        sign_in,
        url,
        callback_port,
    } = begun
    else {
        return Ok(begun);
    };
    let shape = kind.shape();
    let expected = Expected::from_page(&url, callback_port, shape)?;
    let listeners = Listeners::open(round, callback_port, shape).await?;
    round.open(&url).await.map_err(failed)?;

    let callback = tokio::select! {
        callback = tokio::time::timeout(limit, wait(&listeners, &expected)) => callback,
        why = stopped => return Err(failed(why.to_owned())),
    };
    drop(listeners);
    let (code, browser) = match callback {
        Ok(callback) => callback?,
        Err(_) => {
            return Err(failed(format!(
                "Nobody finished signing in within {} minutes, so the sign-in stopped",
                LIMIT.as_secs() / 60
            )));
        }
    };

    let finished = round.act(kind.finish(sign_in, code, expected.state)).await;
    let page = match &finished {
        Ok(ActionOutcome::Refused { reason }) => Page::failed(reason),
        Ok(_) => Page::signed_in(),
        Err(error) => Page::failed(&error.to_string()),
    };
    page.send(browser).await;
    finished
}

/// The listeners the browser comes back to: 127.0.0.1, and ::1 too for a
/// round whose page sends it to `localhost`.
struct Listeners {
    v4: TcpListener,
    v6: Option<TcpListener>,
}

impl Listeners {
    /// Listens at `port`. 127.0.0.1 has to be free. When ::1 isn't, the
    /// round goes on without it, since a browser that finds nothing there
    /// tries 127.0.0.1.
    async fn open(round: &impl Round, port: u16, shape: &Shape) -> Result<Self, CommandError> {
        let v4 = round
            .listen(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
            .await
            .map_err(|_| {
                failed(format!(
                    "Port {port} on this Mac is busy, so the sign-in can't come back. \
                     Quit what's using it, such as another sign-in, and try again"
                ))
            })?;
        let v6 = if shape.ipv6 {
            round
                .listen(IpAddr::V6(Ipv6Addr::LOCALHOST), port)
                .await
                .ok()
        } else {
            None
        };
        Ok(Self { v4, v6 })
    }

    /// The next connection to either.
    async fn accept(&self) -> std::io::Result<TcpStream> {
        let accepted = match &self.v6 {
            None => self.v4.accept().await,
            Some(v6) => tokio::select! {
                accepted = self.v4.accept() => accepted,
                accepted = v6.accept() => accepted,
            },
        };
        accepted.map(|(stream, _)| stream)
    }
}

/// What the callback has to match, from the device's page.
#[derive(Debug, PartialEq)]
struct Expected {
    /// The round's `state`.
    state: String,
    /// The path the browser comes back to.
    path: String,
    /// Whether the callback may name an app ID.
    client_id: bool,
    /// Why a callback with neither `state` nor a code ends the round, if
    /// one does.
    bare_ends: Option<&'static str>,
}

impl Expected {
    /// Reads the page the device wants opened. It has to be on the kind's
    /// origin, carry a `state`, and send the browser back to this Mac at
    /// `port`.
    fn from_page(page: &str, port: u16, shape: &Shape) -> Result<Self, CommandError> {
        let unusable = || failed("The device's sign-in page can't be used".to_owned());
        let page = Url::parse(page).map_err(|_| unusable())?;
        if page.origin().ascii_serialization() != shape.origin || port == 0 {
            return Err(unusable());
        }
        let param = |name: &str| {
            page.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let state = param("state").filter(|state| !state.is_empty());
        let Some(state) = state else {
            return Err(unusable());
        };
        let path = match (param(shape.redirect), shape.path) {
            (None, Some(path)) => path.to_owned(),
            (None, None) => return Err(unusable()),
            (Some(redirect), _) => {
                let redirect = Url::parse(&redirect).map_err(|_| unusable())?;
                let here = redirect.scheme() == "http"
                    && redirect.host_str().is_some_and(|host| host == shape.host)
                    && redirect.port_or_known_default() == Some(port);
                if !here {
                    return Err(failed(
                        "The device's sign-in page sends the browser somewhere this Mac \
                         doesn't listen"
                            .to_owned(),
                    ));
                }
                redirect.path().to_owned()
            }
        };
        Ok(Self {
            state,
            path,
            client_id: shape.client_id,
            bare_ends: shape.bare_ends,
        })
    }
}

/// What the right round's callback brought.
struct Code {
    code: Secret,
    /// The app ID a first sign-in got, if the callback names one.
    client_id: Option<String>,
}

/// What one request to the listener asked for.
enum Callback {
    /// The right round, with its code.
    Code(Code),
    /// The right round, which the sign-in page ended with this error.
    Error(String),
    /// Anything else, with the page that says so.
    Other(Page),
}

impl Callback {
    /// Reads a request's target, such as `/auth/callback?code=...`.
    fn read(target: &str, expected: &Expected) -> Self {
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            return Self::Other(Page::not_found());
        };
        if url.path() != expected.path {
            return Self::Other(Page::not_found());
        }
        let mut state = None;
        let mut code = None;
        let mut error = None;
        let mut description = None;
        let mut client_id = None;
        for (key, value) in url.query_pairs() {
            match &*key {
                "state" => state = Some(value.into_owned()),
                "code" => code = Some(Secret::new(value.into_owned())),
                "error" => error = Some(value.into_owned()),
                "error_description" => description = Some(value.into_owned()),
                "client_id" => client_id = Some(value.into_owned()),
                _ => {}
            }
        }
        if let (None, None, Some(why)) = (&state, &code, expected.bare_ends) {
            // Nothing here names the round, so its words aren't shown.
            return Self::Error(why.to_owned());
        }
        if state.as_deref() != Some(expected.state.as_str()) {
            return Self::Other(Page::wrong_round());
        }
        if let Some(error) = error {
            return Self::Error(description.unwrap_or(error));
        }
        if !expected.client_id {
            client_id = None;
        }
        if client_id.as_deref().is_some_and(|id| !is_client_id(id)) {
            return Self::Other(Page::wrong_round());
        }
        match code {
            Some(code) if !code.is_empty() => Self::Code(Code { code, client_id }),
            _ => Self::Other(Page::wrong_round()),
        }
    }
}

/// Whether `id` looks like an app ID OpenAI gives: `oaiapp_` and then
/// letters, digits, `_` or `-`.
fn is_client_id(id: &str) -> bool {
    id.strip_prefix(CLIENT_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    })
}

/// Answers requests to the listener until one brings the round's code, and
/// returns it with that request's connection, still waiting for its page.
/// A request for another round is refused, and the listener keeps going.
async fn wait(
    listeners: &Listeners,
    expected: &Expected,
) -> Result<(Code, TcpStream), CommandError> {
    let mut reading = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listeners.accept() => {
                if let Ok(stream) = accepted {
                    reading.spawn(read_target(stream));
                }
            }
            Some(read) = reading.join_next() => {
                let Ok((stream, Some(target))) = read else {
                    continue;
                };
                match Callback::read(&target, expected) {
                    Callback::Code(code) => return Ok((code, stream)),
                    Callback::Error(error) => {
                        Page::failed(&error).send(stream).await;
                        return Err(failed(format!("The sign-in didn't finish: {error}")));
                    }
                    Callback::Other(page) => page.send(stream).await,
                }
            }
        }
    }
}

/// Reads a request's head and returns its target, or none for one that
/// isn't a GET, is too long, or doesn't come in time. Reading the whole head
/// lets the page that answers it arrive whole.
async fn read_target(mut stream: TcpStream) -> (TcpStream, Option<String>) {
    let mut head = Vec::new();
    let mut buf = [0; 2048];
    let read = tokio::time::timeout(READ_TIMEOUT, async {
        while !head.windows(4).any(|end| end == b"\r\n\r\n") {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
            if head.len() > HEAD_LIMIT {
                return false;
            }
        }
        true
    })
    .await;
    if read != Ok(true) {
        return (stream, None);
    }
    let target = std::str::from_utf8(&head).ok().and_then(|head| {
        let line = head.lines().next()?;
        let mut parts = line.split(' ');
        let method = parts.next()?;
        let target = parts.next()?;
        (method == "GET" && target.starts_with('/')).then(|| target.to_owned())
    });
    (stream, target)
}

/// A page the listener answers with.
struct Page {
    status: &'static str,
    title: &'static str,
    text: String,
}

impl Page {
    fn signed_in() -> Self {
        Self {
            status: "200 OK",
            title: "Signed in",
            text: "You can close this tab and go back to MaxHQ.".to_owned(),
        }
    }

    fn failed(reason: &str) -> Self {
        Self {
            status: "200 OK",
            title: "The sign-in didn't finish",
            text: format!("{reason}. You can close this tab and try again in MaxHQ."),
        }
    }

    fn wrong_round() -> Self {
        Self {
            status: "400 Bad Request",
            title: "This isn't the sign-in MaxHQ started",
            text: "Close this tab and use the one MaxHQ opened.".to_owned(),
        }
    }

    fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            title: "Not found",
            text: "MaxHQ is waiting for a sign-in here.".to_owned(),
        }
    }

    /// Sends the page and closes the connection. A browser that already
    /// left misses it, which is fine.
    async fn send(self, mut stream: TcpStream) {
        let body = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
             <body style=\"font: 15px -apple-system, sans-serif; margin: 3em\">\
             <h1 style=\"font-size: 20px\">{title}</h1><p>{text}</p></body>",
            title = escape(self.title),
            text = escape(&self.text),
        );
        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            self.status,
            body.len(),
        );
        let sent = async {
            stream.write_all(response.as_bytes()).await?;
            stream.shutdown().await
        };
        let _ = tokio::time::timeout(READ_TIMEOUT, sent).await;
    }
}

/// Text made safe to put in HTML.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

fn failed(message: String) -> CommandError {
    CommandError::Failed { message }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tests::block_on;
    use maxhq_protocol::ids::SignInId;
    use tokio::net::TcpSocket;

    use super::*;

    const STATE: &str = "test-state";
    const CODE: &str = "test-not-a-real-code";
    const V4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

    /// A device and browser for a test. The browser makes `requests` to the
    /// listeners, in order, each on the loopback it names, and keeps the
    /// status of each answer.
    struct Fake {
        port: u16,
        /// The listeners the round gets, bound before the round starts so
        /// no other test can take their port in between. A loopback with
        /// none here is busy.
        listeners: Mutex<Vec<(IpAddr, std::net::TcpListener)>>,
        url: String,
        requests: Vec<(IpAddr, String)>,
        /// What the device was asked.
        actions: Mutex<Vec<ClientMessage>>,
        /// The status line of each answer the browser got.
        answers: Arc<Mutex<Vec<String>>>,
        /// What the device answers a finish with.
        finish: ActionOutcome,
        /// Whether a socket still held the listener's port when the device
        /// was asked to finish.
        held_at_finish: Mutex<Option<bool>>,
    }

    impl Fake {
        /// A Sign in with ChatGPT round.
        fn new(requests: &[&str]) -> Self {
            let (port, listeners) = bind(false);
            let redirect = format!("http%3A%2F%2F127.0.0.1%3A{port}%2Fauth%2Fcallback");
            let url = format!(
                "https://auth.openai.com/oauth/authorize?response_type=code\
                 &redirect_uri={redirect}&state={STATE}"
            );
            let requests = requests
                .iter()
                .map(|&target| (V4, target))
                .collect::<Vec<_>>();
            Self::with(port, listeners, url, &requests)
        }

        /// A Sign in with OpenRouter round, with ::1 free when `ipv6` is
        /// set and the machine has it.
        fn openrouter(ipv6: bool, requests: &[(IpAddr, &str)]) -> Self {
            let (port, listeners) = bind(ipv6);
            let url = format!(
                "https://openrouter.ai/auth?callback_url=http%3A%2F%2Flocalhost%3A{port}%2Fcallback\
                 &code_challenge=challenge&code_challenge_method=S256&state={STATE}"
            );
            Self::with(port, listeners, url, requests)
        }

        fn with(
            port: u16,
            listeners: Vec<(IpAddr, std::net::TcpListener)>,
            url: String,
            requests: &[(IpAddr, &str)],
        ) -> Self {
            Self {
                port,
                listeners: Mutex::new(listeners),
                url,
                requests: requests
                    .iter()
                    .map(|&(host, target)| (host, target.to_owned()))
                    .collect(),
                actions: Mutex::new(Vec::new()),
                answers: Arc::new(Mutex::new(Vec::new())),
                finish: ActionOutcome::Done,
                held_at_finish: Mutex::new(None),
            }
        }

        fn actions(&self) -> Vec<ClientMessage> {
            self.actions.lock().unwrap().clone()
        }

        fn answers(&self) -> Vec<String> {
            self.answers.lock().unwrap().clone()
        }

        /// Waits for the browser to get `count` answers, and returns them.
        fn answered(&self, count: usize) -> Vec<String> {
            let start = std::time::Instant::now();
            while self.answers().len() < count {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "the browser got its pages"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            self.answers()
        }
    }

    /// Binds 127.0.0.1 at a free port, and ::1 at the same port too when
    /// `ipv6` is set and the machine has it.
    fn bind(ipv6: bool) -> (u16, Vec<(IpAddr, std::net::TcpListener)>) {
        for _ in 0..20 {
            let v4 = std::net::TcpListener::bind((V4, 0)).unwrap();
            v4.set_nonblocking(true).unwrap();
            let port = v4.local_addr().unwrap().port();
            let mut listeners = vec![(V4, v4)];
            if ipv6 && has_ipv6() {
                let Ok(v6) = std::net::TcpListener::bind((V6, port)) else {
                    continue;
                };
                v6.set_nonblocking(true).unwrap();
                listeners.push((V6, v6));
            }
            return (port, listeners);
        }
        panic!("no port free on both loopbacks");
    }

    /// Whether this machine listens on ::1 at all.
    fn has_ipv6() -> bool {
        std::net::TcpListener::bind((V6, 0)).is_ok()
    }

    impl Round for Fake {
        fn act(
            &self,
            action: ClientMessage,
        ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send {
            let begins = matches!(
                &action,
                ClientMessage::BeginChatGptSignIn { .. }
                    | ClientMessage::ModelAction {
                        action: ModelAction::BeginOpenRouterSignIn { .. },
                        ..
                    }
            );
            let outcome = if begins {
                ActionOutcome::SignIn {
                    sign_in: sign_in_id(),
                    url: self.url.clone(),
                    callback_port: self.port,
                }
            } else {
                *self.held_at_finish.lock().unwrap() = Some(held(self.port));
                self.finish.clone()
            };
            self.actions.lock().unwrap().push(action);
            async move { Ok(outcome) }
        }

        fn open(&self, url: &str) -> impl Future<Output = Result<(), String>> + Send {
            assert_eq!(url, self.url, "the device's page");
            let port = self.port;
            let requests = self.requests.clone();
            let answers = Arc::clone(&self.answers);
            tokio::spawn(async move {
                for (host, target) in requests {
                    let status = get(host, port, &target).await;
                    answers.lock().unwrap().push(status);
                }
            });
            async { Ok(()) }
        }

        fn listen(
            &self,
            host: IpAddr,
            port: u16,
        ) -> impl Future<Output = std::io::Result<TcpListener>> + Send {
            assert_eq!(port, self.port, "the device's port");
            let mut listeners = self.listeners.lock().unwrap();
            let found = listeners.iter().position(|(bound, _)| *bound == host);
            let listener = found.map(|at| listeners.remove(at).1);
            async move {
                match listener {
                    Some(listener) => TcpListener::from_std(listener),
                    None => Err(std::io::ErrorKind::AddrInUse.into()),
                }
            }
        }
    }

    fn sign_in_id() -> SignInId {
        "0199a7c2-0000-7000-8000-0000000000d4".parse().unwrap()
    }

    /// Whether a socket holds `port` on 127.0.0.1 and keeps it for a moment.
    /// It binds the port to find out rather than connecting to it, since once
    /// the listener lets go, another test or test run may listen there. The
    /// bind sets `SO_REUSEADDR`, so a connection the round answered and
    /// closed, still waiting out its time on the port, doesn't count. On a
    /// busy machine a closed listener can still show for a moment, or another
    /// process can take the port briefly, so one look isn't enough: the
    /// round's own listener, if it were still open, would hold it throughout.
    fn held(port: u16) -> bool {
        (0..20).all(|_| {
            let socket = TcpSocket::new_v4().unwrap();
            socket.set_reuseaddr(true).unwrap();
            let taken = socket.bind((Ipv4Addr::LOCALHOST, port).into()).is_err();
            if taken {
                std::thread::sleep(Duration::from_millis(10));
            }
            taken
        })
    }

    /// Makes a request as a browser would, and returns the answer's status
    /// line.
    async fn get(host: IpAddr, port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((host, port)).await.unwrap();
        let request = format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer.lines().next().unwrap_or_default().to_owned()
    }

    fn callback(state: &str) -> String {
        format!("/auth/callback?code={CODE}&state={state}&scope=openid")
    }

    #[test]
    fn a_first_sign_ins_app_id_goes_to_the_device_and_a_strange_one_is_refused() {
        let fake = Fake::new(&[
            &format!("{}&client_id=not-an-app", callback(STATE)),
            &format!("{}&client_id=oaiapp_test-1", callback(STATE)),
        ]);
        let outcome = chatgpt(&fake, Duration::from_secs(5));
        assert_eq!(outcome, Ok(ActionOutcome::Done));
        let actions = fake.actions();
        let [_, ClientMessage::FinishChatGptSignIn { client_id, .. }] = actions.as_slice() else {
            panic!("expected a begin and one finish, got {actions:?}");
        };
        assert_eq!(client_id.as_deref(), Some("oaiapp_test-1"));
        for id in ["oaiapp_", "oaiapp_a b", "app_x"] {
            assert!(!is_client_id(id), "{id}");
        }
    }

    #[test]
    fn a_callback_with_the_wrong_state_is_refused_and_the_round_waits_on() {
        let fake = Fake::new(&["/favicon.ico", &callback("someone-else"), &callback(STATE)]);
        let outcome = chatgpt(&fake, Duration::from_secs(5));
        assert_eq!(outcome, Ok(ActionOutcome::Done));

        let actions = fake.actions();
        let [ClientMessage::BeginChatGptSignIn { .. }, ClientMessage::FinishChatGptSignIn {
            sign_in,
            code,
            state,
            ..
        }] = actions.as_slice()
        else {
            panic!("expected a begin and one finish, got {actions:?}");
        };
        assert_eq!(*sign_in, sign_in_id());
        assert_eq!(code.expose(), CODE);
        assert_eq!(state, STATE);

        assert_eq!(
            fake.answered(3),
            [
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 400 Bad Request",
                "HTTP/1.1 200 OK"
            ]
        );
        // The listener had closed when the code went to the device. The
        // check binds the port as the code goes out, so it doesn't count on
        // the port staying free while the browser gets its page.
        assert_eq!(*fake.held_at_finish.lock().unwrap(), Some(false));
    }

    #[test]
    fn a_round_nobody_finishes_stops_at_the_limit() {
        let fake = Fake::new(&[&callback("someone-else")]);
        let outcome = chatgpt(&fake, Duration::from_millis(300));
        assert!(
            matches!(outcome, Err(CommandError::Failed { .. })),
            "{outcome:?}"
        );
        assert_eq!(fake.actions().len(), 1, "nothing was finished");
    }

    #[test]
    fn a_sign_in_page_that_ends_in_an_error_stops_the_round() {
        let fake = Fake::new(&[&format!(
            "/auth/callback?error=access_denied&error_description=You+said+no&state={STATE}"
        )]);
        let outcome = chatgpt(&fake, Duration::from_secs(5));
        assert_eq!(
            outcome,
            Err(failed("The sign-in didn't finish: You said no".to_owned()))
        );
        assert_eq!(fake.actions().len(), 1, "nothing was finished");
    }

    #[test]
    fn a_refusal_to_begin_comes_back_as_it_is() {
        struct Refuses;
        impl Round for Refuses {
            async fn act(&self, _action: ClientMessage) -> Result<ActionOutcome, CommandError> {
                Ok(ActionOutcome::Refused {
                    reason: "Codex isn't installed".into(),
                })
            }

            async fn open(&self, _url: &str) -> Result<(), String> {
                panic!("nothing to open")
            }
        }
        assert_eq!(
            block_on(sign_in(
                &Refuses,
                &Kind::ChatGpt,
                LIMIT,
                std::future::pending()
            )),
            Ok(ActionOutcome::Refused {
                reason: "Codex isn't installed".into()
            })
        );
    }

    #[test]
    fn the_device_page_has_to_be_openais_and_come_back_here() {
        let port = 1455;
        let page = |query: &str| format!("https://auth.openai.com/oauth/authorize?{query}");
        let expected = |path: &str| Expected {
            state: "s".into(),
            path: path.into(),
            client_id: true,
            bare_ends: None,
        };
        assert_eq!(
            Expected::from_page(&page("state=s"), port, &CHATGPT),
            Ok(expected(CALLBACK_PATH))
        );
        assert_eq!(
            Expected::from_page(
                &page("state=s&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fcb"),
                port,
                &CHATGPT
            ),
            Ok(expected("/cb"))
        );
        for unusable in [
            "http://auth.openai.com/oauth/authorize?state=s".to_owned(),
            "https://auth.example.com/oauth/authorize?state=s".to_owned(),
            "https://auth.openai.com.example.net/oauth/authorize?state=s".to_owned(),
            "https://auth.openai.com@example.net/oauth/authorize?state=s".to_owned(),
            "https://auth.openai.com:8443/oauth/authorize?state=s".to_owned(),
            page("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"),
            page("state="),
            page("state=s&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"),
            page("state=s&redirect_uri=http%3A%2F%2F127.0.0.1%3A9999%2Fauth%2Fcallback"),
            page("state=s&redirect_uri=http%3A%2F%2Fevil.example.com%3A1455%2Fauth%2Fcallback"),
            page("state=s&redirect_uri=https%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"),
            "not a url".to_owned(),
            "https://openrouter.ai/auth?state=s".to_owned(),
        ] {
            assert!(
                Expected::from_page(&unusable, port, &CHATGPT).is_err(),
                "{unusable} should be refused"
            );
        }
        assert!(Expected::from_page(&page("state=s"), 0, &CHATGPT).is_err());
    }

    #[test]
    fn the_openrouter_page_has_to_be_openrouters_and_come_back_to_localhost() {
        let port = 50_123;
        let page = |query: &str| format!("https://openrouter.ai/auth?{query}");
        assert_eq!(
            Expected::from_page(
                &page("state=s&callback_url=http%3A%2F%2Flocalhost%3A50123%2Fcallback"),
                port,
                &OPENROUTER
            ),
            Ok(Expected {
                state: "s".into(),
                path: "/callback".into(),
                client_id: false,
                bare_ends: OPENROUTER.bare_ends,
            })
        );
        for unusable in [
            page("state=s"),
            page("callback_url=http%3A%2F%2Flocalhost%3A50123%2Fcallback"),
            page("state=s&callback_url=http%3A%2F%2F127.0.0.1%3A50123%2Fcallback"),
            page("state=s&callback_url=http%3A%2F%2Flocalhost%3A50124%2Fcallback"),
            page("state=s&callback_url=https%3A%2F%2Flocalhost%3A50123%2Fcallback"),
            page("state=s&redirect_uri=http%3A%2F%2Flocalhost%3A50123%2Fcallback"),
            "https://openrouter.ai.example.net/auth?state=s\
             &callback_url=http%3A%2F%2Flocalhost%3A50123%2Fcallback"
                .to_owned(),
            "https://auth.openai.com/oauth/authorize?state=s\
             &callback_url=http%3A%2F%2Flocalhost%3A50123%2Fcallback"
                .to_owned(),
        ] {
            assert!(
                Expected::from_page(&unusable, port, &OPENROUTER).is_err(),
                "{unusable} should be refused"
            );
        }
    }

    fn openrouter() -> Kind {
        Kind::OpenRouter {
            label: Some("OpenRouter".into()),
            limit_cents: Some(2_500),
        }
    }

    fn chatgpt(fake: &Fake, limit: Duration) -> Result<ActionOutcome, CommandError> {
        block_on(sign_in(fake, &Kind::ChatGpt, limit, std::future::pending()))
    }

    fn openrouter_round(fake: &Fake) -> Result<ActionOutcome, CommandError> {
        let limit = Duration::from_secs(5);
        block_on(sign_in(fake, &openrouter(), limit, std::future::pending()))
    }

    fn openrouter_callback(state: &str) -> String {
        format!("/callback?code={CODE}&state={state}")
    }

    #[test]
    fn an_openrouter_round_comes_back_on_ipv6_and_hands_the_device_its_code() {
        // ::1 when this machine has it, as a browser that tries `localhost`
        // on IPv6 first would.
        let host = if has_ipv6() { V6 } else { V4 };
        let fake = Fake::openrouter(
            true,
            &[
                (host, &openrouter_callback("someone-else")),
                (
                    host,
                    &format!("{}&client_id=oaiapp_x", openrouter_callback(STATE)),
                ),
            ],
        );
        assert_eq!(openrouter_round(&fake), Ok(ActionOutcome::Done));

        let actions = fake.actions();
        let [ClientMessage::ModelAction {
            action: begin @ ModelAction::BeginOpenRouterSignIn { .. },
            ..
        }, ClientMessage::ModelAction {
            action: ModelAction::FinishOpenRouterSignIn { sign_in, code },
            ..
        }] = actions.as_slice()
        else {
            panic!("expected a begin and one finish, got {actions:?}");
        };
        assert_eq!(
            *begin,
            ModelAction::BeginOpenRouterSignIn {
                label: Some("OpenRouter".into()),
                limit_cents: Some(2_500),
            }
        );
        assert_eq!(*sign_in, sign_in_id());
        assert_eq!(code.expose(), CODE);
        assert_eq!(
            fake.answered(2),
            ["HTTP/1.1 400 Bad Request", "HTTP/1.1 200 OK"]
        );
        assert_eq!(*fake.held_at_finish.lock().unwrap(), Some(false));
    }

    #[test]
    fn an_openrouter_round_goes_on_without_ipv6() {
        let fake = Fake::openrouter(false, &[(V4, &openrouter_callback(STATE))]);
        assert_eq!(openrouter_round(&fake), Ok(ActionOutcome::Done));
        assert_eq!(fake.actions().len(), 2, "a begin and a finish");
    }

    #[test]
    fn coming_back_from_openrouter_with_nothing_ends_the_round() {
        // A code without the round's `state` can't be trusted, so the
        // round waits on. Coming back with neither is how OpenRouter says
        // you denied the key.
        let fake = Fake::openrouter(
            false,
            &[(V4, &format!("/callback?code={CODE}")), (V4, "/callback")],
        );
        assert_eq!(
            openrouter_round(&fake),
            Err(failed(
                "The sign-in didn't finish: OpenRouter sent you back without a key".to_owned()
            ))
        );
        assert_eq!(fake.actions().len(), 1, "nothing was finished");
        assert_eq!(
            fake.answered(2),
            ["HTTP/1.1 400 Bad Request", "HTTP/1.1 200 OK"]
        );

        // A ChatGPT round takes the same as a stranger's, and waits on.
        let chatgpt = Expected::from_page(
            "https://auth.openai.com/oauth/authorize?state=s",
            1455,
            &CHATGPT,
        )
        .unwrap();
        assert!(matches!(
            Callback::read("/auth/callback", &chatgpt),
            Callback::Other(_)
        ));
    }

    #[test]
    fn cancel_ends_the_round_that_waits() {
        let waiting = Waiting::default();
        // Nothing waits yet, so this does nothing.
        waiting.cancel();
        let fake = Fake::openrouter(false, &[]);
        let stopped = waiting.next();
        waiting.cancel();
        let outcome = block_on(sign_in(&fake, &openrouter(), LIMIT, stopped));
        assert_eq!(outcome, Err(failed("You cancelled the sign-in".to_owned())));
        assert_eq!(fake.actions().len(), 1, "nothing was finished");

        // A newer round ends the one before.
        let older = waiting.next();
        let _newer = waiting.next();
        assert_eq!(block_on(older), "A newer sign-in took this one's place");
    }

    #[test]
    fn pages_escape_what_they_show() {
        assert_eq!(
            escape("<b>\"Tom & Jerry's\"</b>"),
            "&lt;b&gt;&quot;Tom &amp; Jerry&#39;s&quot;&lt;/b&gt;"
        );
    }
}

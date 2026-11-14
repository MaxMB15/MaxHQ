// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sign in with ChatGPT for a device, per W-5. The device starts the round
//! and keeps its PKCE verifier. The app listens on 127.0.0.1 at the port the
//! device names, opens the browser at the device's page, checks the
//! callback's `state`, and hands the code to the device. Nothing here keeps
//! the code once the device has it, or logs it. A first sign-in's callback
//! also names the app ID OpenAI gave this device, which goes to the device
//! with the code.

use std::future::Future;
use std::net::Ipv4Addr;
use std::time::Duration;

use maxhq_protocol::ids::{DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::ClientMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
    sign_in(&round, LIMIT).await
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

    /// Listens on 127.0.0.1 at `port` for the browser to come back.
    fn listen(&self, port: u16) -> impl Future<Output = std::io::Result<TcpListener>> + Send {
        TcpListener::bind((Ipv4Addr::LOCALHOST, port))
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

/// Runs the round: begin on the device, listen, open the browser, wait for
/// the callback with the round's `state`, and finish on the device. An
/// outcome other than a waiting round, such as a refusal, comes back as it
/// is. The listener closes before the code goes to the device.
pub async fn sign_in(round: &impl Round, limit: Duration) -> Result<ActionOutcome, CommandError> {
    let begun = round
        .act(ClientMessage::BeginChatGptSignIn {
            key: IdempotencyKey::generate(),
        })
        .await?;
    let ActionOutcome::SignIn {
        sign_in,
        url,
        callback_port,
    } = begun
    else {
        return Ok(begun);
    };
    let expected = Expected::from_page(&url, callback_port)?;
    let listener = round.listen(callback_port).await.map_err(|_| {
        failed(format!(
            "Port {callback_port} on this Mac is busy, so the sign-in can't come back. \
             Quit what's using it, such as another sign-in, and try again"
        ))
    })?;
    round.open(&url).await.map_err(failed)?;

    let callback = tokio::time::timeout(limit, wait(&listener, &expected)).await;
    drop(listener);
    let (Code { code, client_id }, browser) = match callback {
        Ok(callback) => callback?,
        Err(_) => {
            return Err(failed(format!(
                "Nobody finished signing in within {} minutes, so the sign-in stopped",
                LIMIT.as_secs() / 60
            )));
        }
    };

    let finished = round
        .act(ClientMessage::FinishChatGptSignIn {
            key: IdempotencyKey::generate(),
            sign_in,
            code,
            state: expected.state,
            client_id,
        })
        .await;
    let page = match &finished {
        Ok(ActionOutcome::Refused { reason }) => Page::failed(reason),
        Ok(_) => Page::signed_in(),
        Err(error) => Page::failed(&error.to_string()),
    };
    page.send(browser).await;
    finished
}

/// What the callback has to match, from the device's page.
#[derive(Debug, PartialEq)]
struct Expected {
    /// The round's `state`.
    state: String,
    /// The path the browser comes back to.
    path: String,
}

impl Expected {
    /// Reads the page the device wants opened. It has to be on OpenAI's sign-in
    /// origin, carry a `state`, and send the browser back to this Mac at `port`.
    fn from_page(page: &str, port: u16) -> Result<Self, CommandError> {
        let unusable = || failed("The device's sign-in page can't be used".to_owned());
        let page = Url::parse(page).map_err(|_| unusable())?;
        if page.origin().ascii_serialization() != SIGN_IN_ORIGIN || port == 0 {
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
        let path = match param("redirect_uri") {
            None => CALLBACK_PATH.to_owned(),
            Some(redirect) => {
                let redirect = Url::parse(&redirect).map_err(|_| unusable())?;
                let here = redirect.scheme() == "http"
                    && redirect.host_str().is_some_and(|host| host == LOOPBACK)
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
        Ok(Self { state, path })
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
        if state.as_deref() != Some(expected.state.as_str()) {
            return Self::Other(Page::wrong_round());
        }
        if let Some(error) = error {
            return Self::Error(description.unwrap_or(error));
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
    listener: &TcpListener,
    expected: &Expected,
) -> Result<(Code, TcpStream), CommandError> {
    let mut reading = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
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

    /// A device and browser for a test. The browser makes `requests` to the
    /// listener, in order, and keeps the status of each answer.
    struct Fake {
        port: u16,
        /// The listener the round gets, bound before the round starts so no
        /// other test can take its port in between.
        listener: Mutex<Option<std::net::TcpListener>>,
        url: String,
        requests: Vec<String>,
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
        fn new(requests: &[&str]) -> Self {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let redirect = format!("http%3A%2F%2F127.0.0.1%3A{port}%2Fauth%2Fcallback");
            Self {
                port,
                listener: Mutex::new(Some(listener)),
                url: format!(
                    "https://auth.openai.com/oauth/authorize?response_type=code\
                     &redirect_uri={redirect}&state={STATE}"
                ),
                requests: requests.iter().map(|&target| target.to_owned()).collect(),
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
    }

    impl Round for Fake {
        fn act(
            &self,
            action: ClientMessage,
        ) -> impl Future<Output = Result<ActionOutcome, CommandError>> + Send {
            let outcome = match &action {
                ClientMessage::BeginChatGptSignIn { .. } => ActionOutcome::SignIn {
                    sign_in: sign_in_id(),
                    url: self.url.clone(),
                    callback_port: self.port,
                },
                _ => self.finish.clone(),
            };
            if let ClientMessage::FinishChatGptSignIn { .. } = action {
                *self.held_at_finish.lock().unwrap() = Some(held(self.port));
            }
            self.actions.lock().unwrap().push(action);
            async move { Ok(outcome) }
        }

        fn open(&self, url: &str) -> impl Future<Output = Result<(), String>> + Send {
            assert_eq!(url, self.url, "the device's page");
            let port = self.port;
            let requests = self.requests.clone();
            let answers = Arc::clone(&self.answers);
            tokio::spawn(async move {
                for target in requests {
                    let status = get(port, &target).await;
                    answers.lock().unwrap().push(status);
                }
            });
            async { Ok(()) }
        }

        fn listen(&self, port: u16) -> impl Future<Output = std::io::Result<TcpListener>> + Send {
            assert_eq!(port, self.port, "the device's port");
            let listener = self.listener.lock().unwrap().take().expect("one listen");
            async move { TcpListener::from_std(listener) }
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
    async fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
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
        let outcome = block_on(sign_in(&fake, Duration::from_secs(5)));
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
        let outcome = block_on(sign_in(&fake, Duration::from_secs(5)));
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

        let answered = || fake.answers().len() == 3;
        let start = std::time::Instant::now();
        while !answered() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the browser got its pages"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            fake.answers(),
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
        let outcome = block_on(sign_in(&fake, Duration::from_millis(300)));
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
        let outcome = block_on(sign_in(&fake, Duration::from_secs(5)));
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
            block_on(sign_in(&Refuses, LIMIT)),
            Ok(ActionOutcome::Refused {
                reason: "Codex isn't installed".into()
            })
        );
    }

    #[test]
    fn the_device_page_has_to_be_openais_and_come_back_here() {
        let port = 1455;
        let page = |query: &str| format!("https://auth.openai.com/oauth/authorize?{query}");
        assert_eq!(
            Expected::from_page(&page("state=s"), port),
            Ok(Expected {
                state: "s".into(),
                path: CALLBACK_PATH.into(),
            })
        );
        assert_eq!(
            Expected::from_page(
                &page("state=s&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fcb"),
                port
            ),
            Ok(Expected {
                state: "s".into(),
                path: "/cb".into(),
            })
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
        ] {
            assert!(
                Expected::from_page(&unusable, port).is_err(),
                "{unusable} should be refused"
            );
        }
        assert!(Expected::from_page(&page("state=s"), 0).is_err());
    }

    #[test]
    fn pages_escape_what_they_show() {
        assert_eq!(
            escape("<b>\"Tom & Jerry's\"</b>"),
            "&lt;b&gt;&quot;Tom &amp; Jerry&#39;s&quot;&lt;/b&gt;"
        );
    }
}

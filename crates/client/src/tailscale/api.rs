// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finding and calling Tailscale's LocalAPI, per decision N-3. The macOS
//! Standalone build listens on localhost with a token in `/Library`, the App
//! Store build keeps the token in a file name only its extension holds open,
//! and plain `tailscaled` listens on a Unix socket. On Windows `tailscaled`
//! listens on a named pipe under `ProtectedPrefix\Administrators`, a prefix
//! only an administrator or the system can make pipes in, so whatever
//! answers there is the service and not some program of the user's. The
//! CLI is the fallback.

use std::fmt;
use std::io;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, HOST};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::task::JoinHandle;

use super::wire::Status;

/// The `Host` header the LocalAPI checks for.
const LOCALAPI_HOST: &str = "local-tailscaled.sock";

/// How long a status request or a CLI run gets.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The Standalone build's port, as a symlink whose target is the number.
const MACSYS_PORT: &str = "/Library/Tailscale/ipnport";

/// Where the Standalone build's token lives, before the port.
const MACSYS_TOKEN_PREFIX: &str = "/Library/Tailscale/sameuserproof-";

/// The App Store build's app, which has no files in `/Library/Tailscale`.
const MAC_APP: &str = "/Applications/Tailscale.app";

/// What the App Store build's token file name starts with.
const APP_STORE_TOKEN_MARKER: &str = ".tailscale.ipn.macos/sameuserproof-";

/// `tailscaled` sockets: plain `tailscaled` on macOS, then Linux.
#[cfg(unix)]
const UNIX_SOCKETS: &[&str] = &[
    "/var/run/tailscaled.socket",
    "/var/run/tailscale/tailscaled.sock",
    "/run/tailscale/tailscaled.sock",
];

/// Windows has no Unix sockets to look for.
#[cfg(windows)]
const UNIX_SOCKETS: &[&str] = &[];

/// The pipe `tailscaled` serves the LocalAPI on, on Windows.
#[cfg(windows)]
const WINDOWS_PIPE: &str = r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled";

/// The CLI by absolute path, since an app opened from Finder doesn't get
/// `/usr/local/bin` on its `PATH`.
#[cfg(unix)]
const CLIS: &[&str] = &[
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/usr/bin/tailscale",
];

/// The CLI where Tailscale's installer puts it.
#[cfg(windows)]
const CLIS: &[&str] = &[r"C:\Program Files\Tailscale\tailscale.exe"];

/// Paths whose presence means Tailscale is installed, even when it isn't
/// running.
#[cfg(unix)]
const INSTALL_MARKERS: &[&str] = &[MAC_APP, "/Library/Tailscale", "/var/lib/tailscale"];

/// The install folder and the service's state folder, on Windows.
#[cfg(windows)]
const INSTALL_MARKERS: &[&str] = &[r"C:\Program Files\Tailscale", r"C:\ProgramData\Tailscale"];

/// Where the LocalAPI listens.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// Localhost with a token, as the macOS GUI builds do.
    Tcp {
        /// The port.
        port: u16,
        /// The password for Basic auth.
        token: String,
    },
    /// A Unix socket, which needs no auth.
    #[cfg(unix)]
    Unix(PathBuf),
    /// The Windows service's named pipe, which checks who connects itself.
    #[cfg(windows)]
    Pipe(&'static str),
}

impl fmt::Debug for Endpoint {
    /// Leaves the token out.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp { port, .. } => f.debug_struct("Tcp").field("port", port).finish(),
            #[cfg(unix)]
            Self::Unix(path) => f.debug_tuple("Unix").field(path).finish(),
            #[cfg(windows)]
            Self::Pipe(name) => f.debug_tuple("Pipe").field(name).finish(),
        }
    }
}

/// Why a LocalAPI call failed.
#[derive(Debug, Error)]
pub(crate) enum ApiError {
    /// Nothing answered at the endpoint.
    #[error("can't connect: {0}")]
    Connect(#[from] io::Error),
    /// The token was refused, which happens when the port changed.
    #[error("the token was refused")]
    Auth,
    /// Any other status.
    #[error("status {0}")]
    Status(StatusCode),
    /// HTTP itself failed.
    #[error("http: {0}")]
    Http(#[from] hyper::Error),
    /// The body wasn't the JSON expected.
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
    /// The call took too long.
    #[error("timed out")]
    Timeout,
}

/// Whether Tailscale looks installed on this machine.
pub(crate) async fn installed() -> bool {
    for path in INSTALL_MARKERS.iter().chain(UNIX_SOCKETS).chain(CLIS) {
        if tokio::fs::symlink_metadata(path).await.is_ok() {
            return true;
        }
    }
    false
}

/// Finds the LocalAPI, reading the port and token afresh.
pub(crate) async fn discover() -> Option<Endpoint> {
    if let Some(endpoint) = macsys().await {
        return Some(endpoint);
    }
    if cfg!(target_os = "macos")
        && tokio::fs::metadata(MACSYS_PORT).await.is_err()
        && tokio::fs::metadata(MAC_APP).await.is_ok()
        && let Some(endpoint) = app_store().await
    {
        return Some(endpoint);
    }
    local_endpoint().await
}

/// `tailscaled`'s own socket.
#[cfg(unix)]
async fn local_endpoint() -> Option<Endpoint> {
    for path in UNIX_SOCKETS {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(Endpoint::Unix(PathBuf::from(path)));
        }
    }
    None
}

/// The service's pipe, once Tailscale is installed. Looking for a pipe
/// opens it, which takes one of the service's instances, so it's only
/// opened to call it, and a call that finds nothing falls back to the CLI.
#[cfg(windows)]
async fn local_endpoint() -> Option<Endpoint> {
    installed().await.then_some(Endpoint::Pipe(WINDOWS_PIPE))
}

/// The Standalone build: the port from the symlink, and the token from the
/// file named after it.
async fn macsys() -> Option<Endpoint> {
    let port = match tokio::fs::read_link(MACSYS_PORT).await {
        Ok(target) => target.to_string_lossy().trim().to_owned(),
        Err(_) => tokio::fs::read_to_string(MACSYS_PORT)
            .await
            .ok()?
            .trim()
            .to_owned(),
    };
    let port: u16 = port.parse().ok()?;
    let token = tokio::fs::read_to_string(format!("{MACSYS_TOKEN_PREFIX}{port}"))
        .await
        .ok()?;
    let token = token.trim().to_owned();
    (!token.is_empty()).then_some(Endpoint::Tcp { port, token })
}

/// The App Store build, the way its own CLI finds it: the extension holds
/// open a file whose name carries the port and the token.
async fn app_store() -> Option<Endpoint> {
    let mut command = Command::new("/usr/sbin/lsof");
    command
        .args(["-n", "-a", "-c", "IPNExtension", "-F", "n"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(REQUEST_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(app_store_endpoint)
}

/// Reads the port and token out of one line of `lsof -F n` output.
pub(crate) fn app_store_endpoint(line: &str) -> Option<Endpoint> {
    let (_, rest) = line.split_once(APP_STORE_TOKEN_MARKER)?;
    let (port, token) = rest.split_once('-')?;
    let port = port.parse().ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| Endpoint::Tcp {
        port,
        token: token.to_owned(),
    })
}

/// `tailscale status --json` from the first CLI that answers.
pub(crate) async fn cli_status() -> Option<Status> {
    for cli in CLIS {
        let Some(output) = run_cli(cli, &["status", "--json"], REQUEST_TIMEOUT).await else {
            continue;
        };
        // The CLI exits non-zero when stopped but still prints the status.
        if let Ok(status) = serde_json::from_slice(&output.stdout) {
            return Some(status);
        }
    }
    None
}

/// The first CLI this machine has, looked for without the runtime.
pub(crate) fn find_cli_now() -> Option<&'static str> {
    CLIS.iter()
        .copied()
        .find(|cli| std::path::Path::new(cli).exists())
}

/// The first CLI this machine has.
pub(crate) async fn find_cli() -> Option<&'static str> {
    for cli in CLIS {
        if tokio::fs::metadata(cli).await.is_ok() {
            return Some(cli);
        }
    }
    None
}

/// Runs `cli` with `args` for at most `timeout`. `None` when it's missing,
/// didn't start or didn't end in time.
pub(crate) async fn run_cli(
    cli: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<std::process::Output> {
    if tokio::fs::metadata(cli).await.is_err() {
        return None;
    }
    let mut command = Command::new(cli);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()
}

/// A response, with the task that drives its connection. Dropping it ends
/// the connection.
pub(crate) struct ApiResponse {
    /// The response.
    pub response: Response<Incoming>,
    /// Drives the connection while the body streams.
    _connection: AbortOnDrop,
}

/// Aborts a task when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sends a GET and checks the status.
pub(crate) async fn get(endpoint: &Endpoint, path: &str) -> Result<ApiResponse, ApiError> {
    match endpoint {
        Endpoint::Tcp { port, token } => {
            let stream = TcpStream::connect(("127.0.0.1", *port)).await?;
            send(stream, path, Some(token)).await
        }
        #[cfg(unix)]
        Endpoint::Unix(socket) => {
            let stream = UnixStream::connect(socket).await?;
            send(stream, path, None).await
        }
        #[cfg(windows)]
        Endpoint::Pipe(name) => send(open_pipe(name).await?, path, None).await,
    }
}

/// How long to wait for a free instance of the service's pipe.
#[cfg(windows)]
const PIPE_BUSY_WAIT: Duration = Duration::from_secs(2);

/// Opens the service's pipe, waiting while every instance is busy.
#[cfg(windows)]
async fn open_pipe(name: &str) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    /// `ERROR_PIPE_BUSY`: every instance has a client.
    const PIPE_BUSY: i32 = 231;

    let deadline = tokio::time::Instant::now() + PIPE_BUSY_WAIT;
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(name) {
            Err(err)
                if err.raw_os_error() == Some(PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            result => return result,
        }
    }
}

/// Sends a GET over an open stream.
async fn send<S>(stream: S, path: &str, token: Option<&str>) -> Result<ApiResponse, ApiError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    let connection = AbortOnDrop(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let mut request = Request::get(path).header(HOST, LOCALAPI_HOST);
    if let Some(token) = token {
        let credentials = base64::engine::general_purpose::STANDARD.encode(format!(":{token}"));
        request = request.header(AUTHORIZATION, format!("Basic {credentials}"));
    }
    let request = request
        .body(Empty::<Bytes>::new())
        .expect("the request is well formed");
    let response = sender.send_request(request).await?;
    match response.status() {
        StatusCode::OK => Ok(ApiResponse {
            response,
            _connection: connection,
        }),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(ApiError::Auth),
        status => Err(ApiError::Status(status)),
    }
}

/// `/localapi/v0/status`.
pub(crate) async fn status(endpoint: &Endpoint) -> Result<Status, ApiError> {
    let call = async {
        let reply = get(endpoint, "/localapi/v0/status").await?;
        let body = reply.response.into_body().collect().await?.to_bytes();
        Ok(serde_json::from_slice(&body)?)
    };
    tokio::time::timeout(REQUEST_TIMEOUT, call)
        .await
        .map_err(|_| ApiError::Timeout)?
}

/// The IPN bus path. The mask is InitialState, InitialStatus and
/// PeerPatches. Bits 3 and 8 are left out because 1.104 refuses them, and
/// 0 and 16 because the watcher doesn't want what they bring.
pub(crate) const BUS_PATH: &str = "/localapi/v0/watch-ipn-bus?mask=49154";

/// Splits a streamed body into lines, however the chunks fall.
#[derive(Debug, Default)]
pub(crate) struct Lines {
    buf: Vec<u8>,
}

impl Lines {
    /// Adds a chunk and returns every line it completes, without the
    /// newline.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=end).collect();
            line.pop();
            if !line.iter().all(u8::is_ascii_whitespace) {
                lines.push(line);
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_store_tokens_come_from_the_file_name() {
        let line = "n/Users/max/Library/Group Containers/io.tailscale.ipn.macos/sameuserproof-61234-abcDEF123";
        assert_eq!(
            app_store_endpoint(line),
            Some(Endpoint::Tcp {
                port: 61234,
                token: "abcDEF123".into()
            })
        );
        assert_eq!(app_store_endpoint("n/dev/null"), None);
        assert_eq!(
            app_store_endpoint("n/x/.tailscale.ipn.macos/sameuserproof-nope-abc"),
            None
        );
    }

    #[test]
    fn debug_output_hides_the_token() {
        let endpoint = Endpoint::Tcp {
            port: 1,
            token: "secret".into(),
        };
        assert!(!format!("{endpoint:?}").contains("secret"));
    }

    #[test]
    fn lines_split_across_chunks() {
        let mut lines = Lines::default();
        assert!(lines.push(b"{\"a\":").is_empty());
        assert_eq!(lines.push(b"1}\n\n{\"b\""), vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(lines.push(b":2}\n"), vec![b"{\"b\":2}".to_vec()]);
    }
}

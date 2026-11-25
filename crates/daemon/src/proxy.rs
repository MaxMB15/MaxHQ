// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd proxy` joins its stdin and stdout to the local socket, so
//! `ssh host maxd proxy` carries the protocol to another device. It copies
//! bytes and reads none of them.
//!
//! `maxd proxy --guest-ssh <workspace>` is a small client instead. It says
//! hello, opens an SSH session with the container workspace's guest, and
//! carries that one stream's bytes on stdin and stdout, so an editor's
//! Remote-SSH can use it as a ProxyCommand.
//!
//! `maxd proxy --peer <device>` is what another device's key runs. It
//! tells `maxd` which device dialed in a frame of its own before the
//! client's bytes, and that frame is all that gives a connection a peer
//! link's rights. `maxd proxy --phone <phone>` and `maxd proxy --pair
//! <pair>` do the same for a phone's key and a one-time pairing key.

use std::ffi::OsStr;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_os::ipc::{self, LocalStream, ReadHalf, WriteHalf};
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{
    self, DecodeError, EncodeError, Frame, FrameDecoder, MAX_TUNNEL_CHUNK,
};
use maxhq_protocol::ids::{ClientId, DeviceId, PairId, PhoneId, StreamId, WorkspaceId};
use maxhq_protocol::message::{ClientHello, ClientInfo, TunnelEnd};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::tunnel::STREAM_CREDIT;

/// How much is copied at once in each direction.
const CHUNK: usize = 64 * 1024;

/// Why the proxy stopped with an error.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Nothing answered on the socket.
    #[error("couldn't reach maxd at {}: {source}. Is it running? Start it with `maxd service install`", path.display())]
    Connect {
        /// The socket.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// Copying failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// `maxd` doesn't open SSH sessions with guests.
    #[error("this device's maxd doesn't open SSH sessions with containers. Update maxd here")]
    NoGuestSsh,
    /// `maxd` turned the client or the session away.
    #[error("{0}")]
    Refused(String),
    /// `maxd` ended the session before its end.
    #[error("maxd reset the SSH session")]
    Reset,
    /// The socket closed before the session ended.
    #[error("maxd closed the connection before the SSH session ended")]
    Closed,
    /// `maxd` sent more than this end let it.
    #[error("maxd sent more of the session than it was let")]
    Overrun,
    /// The socket carried something that isn't frames.
    #[error("maxd sent a broken frame: {0}")]
    Decode(#[from] DecodeError),
    /// A message didn't frame.
    #[error("{0}")]
    Encode(#[from] EncodeError),
}

/// What a forced command was asked to run, when it isn't a proxy.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("this key only runs maxd proxy, and the command asked for something else")]
pub struct NotAProxy;

/// What `maxd proxy` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The protocol, byte for byte.
    Plain,
    /// One SSH session with a container workspace's guest.
    GuestSsh(WorkspaceId),
}

/// The name a guest SSH proxy says hello with.
const CLIENT: &str = "maxd-proxy";

/// What wraps the command when a client runs it over ssh, so `maxd` is
/// found where the installer puts it.
const WRAP_START: &str = r#"sh -c 'PATH="$PATH:$HOME/.local/bin"; export PATH; exec "#;
const WRAP_END: &str = "'";

/// The command, after the wrapper.
const COMMAND: &str = "maxd proxy";

/// The flag that names a workspace.
const GUEST_SSH: &str = " --guest-ssh ";

/// What a key whose forced command is `maxd proxy` lets through, from the
/// command the client asked for, which sshd puts in SSH_ORIGINAL_COMMAND.
/// No command is the plain proxy. A command must be exactly
/// `maxd proxy`, or `maxd proxy --guest-ssh <workspace>`, either bare or
/// in the wrapper a client adds. Nothing in it reaches a shell.
pub fn forced(original: Option<&OsStr>) -> Result<Mode, NotAProxy> {
    let Some(original) = original.filter(|original| !original.is_empty()) else {
        return Ok(Mode::Plain);
    };
    let original = original.to_str().ok_or(NotAProxy)?;
    let command = original
        .strip_prefix(WRAP_START)
        .and_then(|rest| rest.strip_suffix(WRAP_END))
        .unwrap_or(original);
    let rest = command.strip_prefix(COMMAND).ok_or(NotAProxy)?;
    if rest.is_empty() {
        return Ok(Mode::Plain);
    }
    rest.strip_prefix(GUEST_SSH)
        .and_then(workspace)
        .map(Mode::GuestSsh)
        .ok_or(NotAProxy)
}

/// What a peer, phone or pairing key, whose forced command is `maxd proxy
/// --peer`, `--phone` or `--pair`, lets through: only the plain proxy,
/// which is all a peer link or a phone runs.
pub fn forced_peer(original: Option<&OsStr>) -> Result<(), NotAProxy> {
    match forced(original)? {
        Mode::Plain => Ok(()),
        Mode::GuestSsh(_) => Err(NotAProxy),
    }
}

/// The workspace `text` names, written only as a lowercase hyphenated
/// UUID, as `maxd` and the app write them.
pub fn workspace(text: &str) -> Option<WorkspaceId> {
    canonical(text).map(WorkspaceId)
}

/// The device `text` names, written the same way.
pub fn device(text: &str) -> Option<DeviceId> {
    canonical(text).map(DeviceId)
}

/// The phone `text` names, written the same way.
pub fn phone(text: &str) -> Option<PhoneId> {
    canonical(text).map(PhoneId)
}

/// The pairing `text` names, written the same way.
pub fn pair(text: &str) -> Option<PairId> {
    canonical(text).map(PairId)
}

fn canonical(text: &str) -> Option<Uuid> {
    let uuid = Uuid::try_parse(text).ok()?;
    (uuid.hyphenated().to_string() == text).then_some(uuid)
}

/// The socket at `path`.
async fn connect(path: &Path) -> Result<LocalStream, ProxyError> {
    LocalStream::connect(path)
        .await
        .map_err(|source| ProxyError::Connect {
            path: path.into(),
            source,
        })
}

/// Runs `carry` on a runtime of its own. A read of stdin can't be called
/// off, so the runtime goes down without waiting for one, and the caller
/// should exit.
fn on_runtime(carry: impl Future<Output = Result<(), ProxyError>>) -> Result<(), ProxyError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let carried = runtime.block_on(carry);
    runtime.shutdown_background();
    carried
}

/// Joins stdin and stdout to the socket at `path` until either side closes.
pub fn run(path: &Path) -> Result<(), ProxyError> {
    on_runtime(async {
        let stream = connect(path).await?;
        bridge(tokio::io::stdin(), tokio::io::stdout(), stream).await?;
        Ok(())
    })
}

/// Joins stdin and stdout to the socket at `path` after `first`, the frame
/// that says which device, phone or pairing dialed.
pub fn run_as(path: &Path, first: &ClientMessage) -> Result<(), ProxyError> {
    on_runtime(async {
        let stream = connect(path).await?;
        first_bridge(tokio::io::stdin(), tokio::io::stdout(), stream, first).await
    })
}

/// Writes `first`, then bridges.
async fn first_bridge(
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
    mut stream: LocalStream,
    message: &ClientMessage,
) -> Result<(), ProxyError> {
    stream.write_all(&first_frame(message)?).await?;
    bridge(input, output, stream).await?;
    Ok(())
}

/// The frame that says which device, phone or pairing dialed, which goes
/// before the client's bytes. Network access sends it too.
pub fn first_frame(message: &ClientMessage) -> Result<Vec<u8>, EncodeError> {
    let mut first = Vec::new();
    frame::encode_message(&mut first, message)?;
    Ok(first)
}

/// Carries an SSH session with `workspace`'s guest on stdin and stdout,
/// through the `maxd` at `path`, until the session ends.
pub fn run_guest_ssh(path: &Path, workspace: WorkspaceId) -> Result<(), ProxyError> {
    on_runtime(async {
        let stream = connect(path).await?;
        guest_ssh(tokio::io::stdin(), tokio::io::stdout(), stream, workspace).await
    })
}

/// Copies `input` to the socket and the socket to `output`, both at once.
///
/// When `input` ends, the socket's sending side shuts, and `maxd` closes the
/// connection once it has read the rest, while its replies still come. A
/// Windows pipe can't shut one side, so there the end of input ends the
/// bridge, as a phone's end does through network access. When the socket
/// closes, the bridge returns at once, and stops reading `input`.
pub async fn bridge(
    input: impl AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin,
    stream: LocalStream,
) -> io::Result<()> {
    let (from_maxd, mut to_maxd) = stream.into_split();
    let sending = async {
        let copied = copy(input, &mut to_maxd).await;
        // At the end of input only the sending side shuts, since replies
        // may still be on their way. After an error nothing more will come,
        // so the bridge returns.
        if copied.is_ok() && ipc::HALF_CLOSES {
            let _ = to_maxd.shutdown().await;
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = sending => Ok(()),
        copied = copy(from_maxd, output) => match copied {
            // The other end going away is how a proxy normally ends.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                Ok(())
            }
            other => other,
        },
    }
}

/// Copies until `from` ends, flushing after every read so nothing waits in
/// a buffer while the other side waits for it.
async fn copy(mut from: impl AsyncRead + Unpin, mut to: impl AsyncWrite + Unpin) -> io::Result<()> {
    let mut buf = vec![0; CHUNK];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        to.write_all(&buf[..n]).await?;
        to.flush().await?;
    }
}

/// How many bytes written to the output earn `maxd` more credit at once.
const GRANT_STEP: u64 = MAX_TUNNEL_CHUNK as u64;

/// Carries one SSH session with `workspace`'s guest: says hello on
/// `socket`, opens the session, then sends `input` as the stream's bytes
/// and writes the bytes that come back to `output`.
///
/// This end sends nothing until `maxd` says the stream opened. Then it may
/// send [`STREAM_CREDIT`] bytes, and more as `maxd` grants. `maxd` may send
/// as much, and more as this end writes its bytes out. Input goes out from
/// a task of its own, which may still be waiting when this returns, so the
/// caller should exit.
pub async fn guest_ssh(
    input: impl AsyncRead + Unpin + Send + 'static,
    mut output: impl AsyncWrite + Unpin,
    socket: LocalStream,
    workspace: WorkspaceId,
) -> Result<(), ProxyError> {
    let (read, write) = socket.into_split();
    let writer = Writer(Arc::new(tokio::sync::Mutex::new(write)));
    let mut reader = Reader::new(read);
    writer
        .send(&ClientMessage::Hello(ClientHello {
            protocol: ProtocolRange::OURS,
            client: ClientInfo {
                name: CLIENT.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            client_id: ClientId::generate(),
            role: ClientRole::App,
        }))
        .await?;
    loop {
        match reader.next().await? {
            Incoming::Message(ServerMessage::Hello(hello)) => {
                if !hello.capabilities.guest_ssh {
                    return Err(ProxyError::NoGuestSsh);
                }
                break;
            }
            Incoming::Message(ServerMessage::Error { message, .. }) => {
                return Err(ProxyError::Refused(message));
            }
            _ => {}
        }
    }

    let stream = StreamId::generate();
    writer
        .send(&ClientMessage::GuestSshOpen { stream, workspace })
        .await?;
    let credit = Arc::new(Credit::default());
    // Told when reading input fails, which ends the session.
    let failed = Arc::new(Notify::new());
    // Bytes `maxd` may still send, and bytes written out but not yet
    // granted back.
    let mut window = STREAM_CREDIT;
    let mut ungranted = 0;
    // Taken by the input task once the stream opens.
    let mut input = Some(input);
    loop {
        let incoming = tokio::select! {
            incoming = reader.next() => incoming?,
            () = failed.notified() => return Err(ProxyError::Closed),
        };
        match incoming {
            Incoming::Message(message) => match message {
                ServerMessage::TunnelOpened { stream: id } if id == stream => {
                    if let Some(input) = input.take() {
                        credit.grant(STREAM_CREDIT);
                        let (writer, credit, failed) =
                            (writer.clone(), credit.clone(), failed.clone());
                        tokio::spawn(async move {
                            if send_input(input, stream, &writer, &credit).await.is_err() {
                                failed.notify_one();
                            }
                        });
                    }
                }
                ServerMessage::TunnelRefused {
                    stream: id,
                    message,
                    ..
                } if id == stream => return Err(ProxyError::Refused(message)),
                ServerMessage::TunnelCredit {
                    stream: id,
                    granted,
                } if id == stream => {
                    credit.grant(granted);
                }
                ServerMessage::TunnelClose { stream: id, how } if id == stream => {
                    return match how {
                        // The guest has said all it will, so the session
                        // is over.
                        TunnelEnd::Write => Ok(()),
                        TunnelEnd::Reset => Err(ProxyError::Reset),
                    };
                }
                ServerMessage::Error { message, .. } => {
                    return Err(ProxyError::Refused(message));
                }
                _ => {}
            },
            Incoming::Data { stream: id, data } if id == stream => {
                let len = data.len() as u64;
                if len > window {
                    let _ = writer
                        .send(&ClientMessage::TunnelClose {
                            stream,
                            how: TunnelEnd::Reset,
                        })
                        .await;
                    return Err(ProxyError::Overrun);
                }
                window -= len;
                let written = match output.write_all(&data).await {
                    Ok(()) => output.flush().await,
                    Err(err) => Err(err),
                };
                if let Err(err) = written {
                    let _ = writer
                        .send(&ClientMessage::TunnelClose {
                            stream,
                            how: TunnelEnd::Reset,
                        })
                        .await;
                    return Err(err.into());
                }
                ungranted += len;
                if ungranted >= GRANT_STEP {
                    writer
                        .send(&ClientMessage::TunnelCredit {
                            stream,
                            granted: ungranted,
                        })
                        .await?;
                    window += ungranted;
                    ungranted = 0;
                }
            }
            Incoming::Data { .. } => {}
        }
    }
}

/// Sends `input` as `stream`'s bytes as credit allows, then says this
/// end is done writing. A failed read resets the stream and fails, which
/// ends the session.
async fn send_input(
    mut input: impl AsyncRead + Unpin,
    stream: StreamId,
    writer: &Writer,
    credit: &Credit,
) -> Result<(), ()> {
    let mut buf = vec![0; MAX_TUNNEL_CHUNK];
    loop {
        let room = usize::try_from(credit.wait().await).unwrap_or(usize::MAX);
        let n = match input.read(&mut buf[..room.min(MAX_TUNNEL_CHUNK)]).await {
            Ok(0) => {
                let _ = writer
                    .send(&ClientMessage::TunnelClose {
                        stream,
                        how: TunnelEnd::Write,
                    })
                    .await;
                return Ok(());
            }
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = writer
                    .send(&ClientMessage::TunnelClose {
                        stream,
                        how: TunnelEnd::Reset,
                    })
                    .await;
                return Err(());
            }
        };
        credit.spend(n as u64);
        let mut frame = Vec::with_capacity(n + 32);
        frame::encode_tunnel_data(&mut frame, stream, &buf[..n]);
        if writer.write(&frame).await.is_err() {
            return Ok(());
        }
    }
}

/// Bytes this end may still send, and a wake for when it may send more.
#[derive(Default)]
struct Credit {
    left: Mutex<u64>,
    more: Notify,
}

impl Credit {
    fn grant(&self, granted: u64) {
        let mut left = lock(&self.left);
        *left = left.saturating_add(granted);
        self.more.notify_one();
    }

    /// Waits until there's credit, and gives how much.
    async fn wait(&self) -> u64 {
        loop {
            // Made before looking, so a grant in between still wakes it.
            let more = self.more.notified();
            let left = *lock(&self.left);
            if left > 0 {
                return left;
            }
            more.await;
        }
    }

    fn spend(&self, n: u64) {
        let mut left = lock(&self.left);
        *left = left.saturating_sub(n);
    }
}

/// The socket's sending side, shared by the input task and the reader,
/// so each frame goes out whole.
#[derive(Clone)]
struct Writer(Arc<tokio::sync::Mutex<WriteHalf>>);

impl Writer {
    async fn send(&self, message: &ClientMessage) -> Result<(), ProxyError> {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message)?;
        Ok(self.write(&out).await?)
    }

    async fn write(&self, frame: &[u8]) -> io::Result<()> {
        let mut socket = self.0.lock().await;
        socket.write_all(frame).await?;
        socket.flush().await
    }
}

/// What came in on the socket.
enum Incoming {
    Message(ServerMessage),
    Data { stream: StreamId, data: Vec<u8> },
}

/// The socket's receiving side, read a frame at a time.
struct Reader {
    socket: ReadHalf,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Reader {
    fn new(socket: ReadHalf) -> Self {
        Self {
            socket,
            decoder: FrameDecoder::new(),
            buf: vec![0; CHUNK],
        }
    }

    /// The next message or tunnel frame. Messages this build doesn't read
    /// and other kinds of frame are skipped. Called off between frames,
    /// it loses nothing.
    async fn next(&mut self) -> Result<Incoming, ProxyError> {
        loop {
            match self.decoder.next_frame() {
                Ok(Some(Frame::Message(json))) => {
                    if let Ok(message) = serde_json::from_slice(json) {
                        return Ok(Incoming::Message(message));
                    }
                    continue;
                }
                Ok(Some(Frame::TunnelData { stream, data })) => {
                    let data = data.to_vec();
                    return Ok(Incoming::Data { stream, data });
                }
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(DecodeError::UnknownKind(_) | DecodeError::ShortHeader(_)) => continue,
                Err(err) => return Err(err.into()),
            }
            let n = match self.socket.read(&mut self.buf).await {
                Ok(0) => return Err(ProxyError::Closed),
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            };
            self.decoder.push(&self.buf[..n]);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{duplex, DuplexStream};
    use tokio::task::JoinHandle;

    // Long, since a loaded machine can leave a task waiting for seconds.
    // Only a failing test waits it out.
    const WAIT: Duration = Duration::from_secs(30);

    /// `future`, or a failure once it's taken too long.
    async fn soon<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(WAIT, future)
            .await
            .expect("it happens in time")
    }

    /// Runs a bridge in a task, with in-memory pipes standing in for stdin
    /// and stdout. Returns maxd's end, the test's ends of stdin and stdout,
    /// and the bridge's task.
    fn start() -> (
        LocalStream,
        DuplexStream,
        DuplexStream,
        JoinHandle<io::Result<()>>,
    ) {
        let (stream, maxd) = ipc::pair().unwrap();
        let (input, stdin) = duplex(CHUNK);
        let (output, stdout) = duplex(CHUNK);
        let bridged = tokio::spawn(bridge(input, output, stream));
        (maxd, stdin, stdout, bridged)
    }

    #[tokio::test]
    async fn bytes_flow_both_ways() {
        let (mut maxd, mut stdin, mut stdout, bridged) = start();
        stdin.write_all(b"hello").await.unwrap();
        let mut got = [0; 5];
        soon(maxd.read_exact(&mut got)).await.unwrap();
        assert_eq!(&got, b"hello");

        maxd.write_all(b"world").await.unwrap();
        soon(stdout.read_exact(&mut got)).await.unwrap();
        assert_eq!(&got, b"world");

        drop(maxd);
        soon(bridged).await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_end_of_input_reaches_maxd_and_replies_still_arrive() {
        let (mut maxd, mut stdin, mut stdout, bridged) = start();
        stdin.write_all(b"ping").await.unwrap();
        drop(stdin);
        let mut got = Vec::new();
        soon(maxd.read_to_end(&mut got)).await.unwrap();
        assert_eq!(got, b"ping", "maxd reads to the end");

        maxd.write_all(b"pong").await.unwrap();
        drop(maxd);
        soon(bridged).await.unwrap().unwrap();
        let mut reply = Vec::new();
        soon(stdout.read_to_end(&mut reply)).await.unwrap();
        assert_eq!(reply, b"pong");
    }

    /// A pipe can't shut one side, so the end of input ends the bridge,
    /// and maxd reads the rest, then the end.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_end_of_input_ends_the_bridge_and_reaches_maxd() {
        let (mut maxd, mut stdin, _stdout, bridged) = start();
        stdin.write_all(b"ping").await.unwrap();
        drop(stdin);
        soon(bridged).await.unwrap().unwrap();
        let mut got = Vec::new();
        soon(maxd.read_to_end(&mut got)).await.unwrap();
        assert_eq!(got, b"ping");
    }

    #[tokio::test]
    async fn maxd_closing_ends_the_proxy_while_input_is_open() {
        let (maxd, _stdin, _stdout, bridged) = start();
        drop(maxd);
        soon(bridged).await.unwrap().unwrap();
    }

    #[test]
    fn a_missing_socket_says_where_it_looked() {
        let err = run(Path::new("/nonexistent/maxd.sock")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/maxd.sock"));
    }

    #[test]
    fn a_forced_command_passes_only_a_proxy() {
        let id = "0199a7c2-0000-7000-8000-0000000000aa";
        let workspace = Mode::GuestSsh(id.parse().map(WorkspaceId).unwrap());
        let wrap = |command: &str| format!("{WRAP_START}{command}{WRAP_END}");
        let passed = [
            (None, Mode::Plain),
            (Some(String::new()), Mode::Plain),
            (Some("maxd proxy".to_owned()), Mode::Plain),
            (Some(wrap("maxd proxy")), Mode::Plain),
            (Some(format!("maxd proxy --guest-ssh {id}")), workspace),
            (
                Some(wrap(&format!("maxd proxy --guest-ssh {id}"))),
                workspace,
            ),
        ];
        for (original, mode) in passed {
            let got = forced(original.as_deref().map(OsStr::new));
            assert_eq!(got, Ok(mode), "{original:?}");
        }

        let upper = id.to_uppercase();
        let simple = id.replace('-', "");
        let refused = [
            "maxd run".to_owned(),
            "maxd".to_owned(),
            "maxd proxy ".to_owned(),
            " maxd proxy".to_owned(),
            "maxd  proxy".to_owned(),
            "/usr/local/bin/maxd proxy".to_owned(),
            "maxd proxy --guest-ssh".to_owned(),
            "maxd proxy --guest-ssh ".to_owned(),
            format!("maxd proxy --guest-ssh  {id}"),
            format!("maxd proxy --guest-ssh {id} "),
            format!("maxd proxy --guest-ssh {upper}"),
            format!("maxd proxy --guest-ssh {simple}"),
            format!("maxd proxy --guest-ssh {{{id}}}"),
            format!("maxd proxy --guest-ssh urn:uuid:{id}"),
            format!("maxd proxy --guest-ssh={id}"),
            format!("maxd proxy --guest-ssh {id} --guest-ssh {id}"),
            format!("maxd proxy --guest-ssh {id}; rm -rf ~"),
            format!("maxd proxy --guest-ssh {id} && id"),
            format!("maxd proxy --guest-ssh {id}\nid"),
            format!("maxd proxy --guest-ssh $(id){}", &id[5..]),
            format!("maxd proxy --guest-ssh `id`{}", &id[4..]),
            "maxd proxy; id".to_owned(),
            "maxd proxy | sh".to_owned(),
            "maxd proxy\nid".to_owned(),
            "maxd proxy\0".to_owned(),
            wrap("maxd proxy; id"),
            wrap(&format!("maxd proxy --guest-ssh {id}'; id; '")),
            format!("{}{WRAP_END}", wrap("maxd proxy")),
            wrap("sh"),
            format!("sh -c 'exec maxd proxy --guest-ssh {id}'"),
            format!("sh -c '{}'", wrap("maxd proxy")),
            "bash -i".to_owned(),
            "scp -t /tmp".to_owned(),
            "internal-sftp".to_owned(),
        ];
        for original in refused {
            assert_eq!(
                forced(Some(OsStr::new(&original))),
                Err(NotAProxy),
                "{original:?}"
            );
        }
        use std::os::unix::ffi::OsStrExt;
        let bytes = OsStr::from_bytes(b"maxd proxy \xff");
        assert_eq!(forced(Some(bytes)), Err(NotAProxy));
    }

    #[test]
    fn a_peer_key_passes_only_the_plain_proxy() {
        let id = "0199a7c2-0000-7000-8000-0000000000aa";
        let wrap = |command: &str| format!("{WRAP_START}{command}{WRAP_END}");
        for original in [
            None,
            Some(String::new()),
            Some("maxd proxy".into()),
            Some(wrap("maxd proxy")),
        ] {
            assert_eq!(
                forced_peer(original.as_deref().map(OsStr::new)),
                Ok(()),
                "{original:?}"
            );
        }
        let refused = [
            format!("maxd proxy --guest-ssh {id}"),
            wrap(&format!("maxd proxy --guest-ssh {id}")),
            format!("maxd proxy --peer {id}"),
            "maxd proxy --peer".to_owned(),
            "maxd proxy ".to_owned(),
            "maxd run".to_owned(),
            "maxd proxy; id".to_owned(),
            "bash -i".to_owned(),
        ];
        for original in refused {
            assert_eq!(
                forced_peer(Some(OsStr::new(&original))),
                Err(NotAProxy),
                "{original:?}"
            );
        }
    }

    #[test]
    fn a_peer_device_is_only_a_plain_id() {
        let id = "0199a7c2-0000-7000-8000-0000000000aa";
        assert_eq!(device(id), Some(DeviceId(Uuid::try_parse(id).unwrap())));
        for bad in [
            String::new(),
            id.to_uppercase(),
            id.replace('-', ""),
            format!("{{{id}}}"),
            format!("urn:uuid:{id}"),
            format!("{id} "),
            format!(" {id}"),
            format!("{id};id"),
            "--guest-ssh".to_owned(),
        ] {
            assert_eq!(device(&bad), None, "{bad:?}");
        }
    }

    /// The frame `bytes` start with, which must be a message.
    fn first_message(bytes: &[u8]) -> ClientMessage {
        let mut decoder = FrameDecoder::new();
        decoder.push(bytes);
        match decoder.next_frame().unwrap() {
            Some(Frame::Message(text)) => serde_json::from_slice(text).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn the_peer_frame_comes_before_the_clients_bytes() {
        let device = DeviceId(Uuid::try_parse("0199a7c2-0000-7000-8000-0000000000aa").unwrap());
        let (stream, mut maxd) = ipc::pair().unwrap();
        let (input, mut stdin) = duplex(CHUNK);
        let (output, _stdout) = duplex(CHUNK);
        tokio::spawn(async move {
            first_bridge(input, output, stream, &ClientMessage::Peer { device }).await
        });
        stdin.write_all(b"hello").await.unwrap();
        drop(stdin);
        let mut got = Vec::new();
        soon(maxd.read_to_end(&mut got)).await.unwrap();
        assert_eq!(first_message(&got), ClientMessage::Peer { device });
        assert!(got.ends_with(b"hello"));
    }

    #[tokio::test]
    async fn a_phone_or_pairing_frame_comes_first_and_names_only_a_plain_id() {
        let id = "0199a7c2-0000-7000-8000-0000000000bb";
        let uuid = Uuid::try_parse(id).unwrap();
        assert_eq!(phone(id), Some(PhoneId(uuid)));
        assert_eq!(pair(id), Some(PairId(uuid)));
        assert_eq!(phone(&id.to_uppercase()), None);
        assert_eq!(pair(&format!("{id};id")), None);
        for first in [
            ClientMessage::Phone {
                phone: PhoneId(uuid),
            },
            ClientMessage::Pair { pair: PairId(uuid) },
        ] {
            let (stream, mut maxd) = ipc::pair().unwrap();
            let (input, stdin) = duplex(CHUNK);
            let (output, _stdout) = duplex(CHUNK);
            let sent = first.clone();
            tokio::spawn(async move { first_bridge(input, output, stream, &sent).await });
            drop(stdin);
            let mut got = Vec::new();
            soon(maxd.read_to_end(&mut got)).await.unwrap();
            assert_eq!(first_message(&got), first);
        }
    }

    /// The device end of a guest SSH proxy's socket.
    struct Maxd {
        socket: LocalStream,
        decoder: FrameDecoder,
    }

    /// What the proxy sent.
    #[derive(Debug)]
    enum Sent {
        Message(ClientMessage),
        Data(StreamId, Vec<u8>),
    }

    impl Maxd {
        /// The next thing the proxy sent, or none when nothing comes for
        /// `wait`.
        async fn within(&mut self, wait: Duration) -> Option<Sent> {
            let mut buf = vec![0; CHUNK];
            loop {
                match self.decoder.next_frame().unwrap() {
                    Some(Frame::Message(json)) => {
                        return Some(Sent::Message(serde_json::from_slice(json).unwrap()));
                    }
                    Some(Frame::TunnelData { stream, data }) => {
                        return Some(Sent::Data(stream, data.to_vec()));
                    }
                    Some(frame) => panic!("the proxy sent {frame:?}"),
                    None => {}
                }
                match tokio::time::timeout(wait, self.socket.read(&mut buf)).await {
                    Err(_) => return None,
                    Ok(Ok(0)) => panic!("the proxy closed the socket"),
                    Ok(Ok(n)) => self.decoder.push(&buf[..n]),
                    Ok(Err(err)) => panic!("{err}"),
                }
            }
        }

        async fn next(&mut self) -> Sent {
            self.within(WAIT).await.expect("the proxy sends something")
        }

        async fn message(&mut self) -> ClientMessage {
            match self.next().await {
                Sent::Message(message) => message,
                sent => panic!("expected a message, got {sent:?}"),
            }
        }

        async fn send(&mut self, message: &ServerMessage) {
            let mut out = Vec::new();
            frame::encode_message(&mut out, message).unwrap();
            self.socket.write_all(&out).await.unwrap();
        }

        async fn data(&mut self, stream: StreamId, bytes: &[u8]) {
            let mut out = Vec::new();
            frame::encode_tunnel_data(&mut out, stream, bytes);
            self.socket.write_all(&out).await.unwrap();
        }

        /// Takes the hello and answers it, saying whether this device
        /// opens guest SSH sessions.
        async fn greet(&mut self, guest_ssh: bool) {
            let ClientMessage::Hello(hello) = self.message().await else {
                panic!("the proxy says hello first");
            };
            assert_eq!(hello.client.name, CLIENT);
            self.send(&ServerMessage::Hello(
                maxhq_protocol::message::ServerHello {
                    protocol: ProtocolRange::OURS,
                    maxd_version: "0.1.0".into(),
                    device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
                    boot_id: maxhq_protocol::ids::BootId("boot".into()),
                    instance: "0199a7c2-0000-7000-8000-0000000000d7".parse().unwrap(),
                    capabilities: maxhq_protocol::message::Capabilities {
                        guest_ssh,
                        ..Default::default()
                    },
                    build: None,
                    target: None,
                    challenge: None,
                },
            ))
            .await;
        }

        /// Takes the session's open, and gives its stream.
        async fn opening(&mut self, workspace: WorkspaceId) -> StreamId {
            match self.message().await {
                ClientMessage::GuestSshOpen {
                    stream,
                    workspace: asked,
                } => {
                    assert_eq!(asked, workspace);
                    stream
                }
                message => panic!("expected the open, got {message:?}"),
            }
        }
    }

    /// Runs a guest SSH proxy in a task, with in-memory pipes standing in
    /// for stdin and stdout. Returns maxd's end, the test's ends of stdin
    /// and stdout, and the proxy's task.
    fn start_guest_ssh(
        workspace: WorkspaceId,
    ) -> (
        Maxd,
        DuplexStream,
        DuplexStream,
        JoinHandle<Result<(), ProxyError>>,
    ) {
        let (socket, maxd) = ipc::pair().unwrap();
        let (input, stdin) = duplex(CHUNK);
        let (output, stdout) = duplex(CHUNK);
        let carried = tokio::spawn(guest_ssh(input, output, socket, workspace));
        let maxd = Maxd {
            socket: maxd,
            decoder: FrameDecoder::new(),
        };
        (maxd, stdin, stdout, carried)
    }

    #[tokio::test]
    async fn a_guest_ssh_session_flows_both_ways_with_credit() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, mut stdin, mut stdout, carried) = start_guest_ssh(workspace);
        maxd.greet(true).await;
        let stream = maxd.opening(workspace).await;

        // Nothing goes before the stream opens.
        let sent = STREAM_CREDIT as usize + 1000;
        let writer = tokio::spawn(async move {
            stdin.write_all(&vec![7; sent]).await.unwrap();
            stdin
        });
        assert!(maxd.within(Duration::from_millis(200)).await.is_none());
        maxd.send(&ServerMessage::TunnelOpened { stream }).await;

        // Then the stream's credit, and no more until more is granted.
        let mut got = 0;
        while got < STREAM_CREDIT as usize {
            match maxd.next().await {
                Sent::Data(id, data) => {
                    assert_eq!(id, stream);
                    assert!(data.len() <= MAX_TUNNEL_CHUNK);
                    assert!(data.iter().all(|&byte| byte == 7));
                    got += data.len();
                }
                sent => panic!("expected bytes, got {sent:?}"),
            }
        }
        assert_eq!(got, STREAM_CREDIT as usize);
        assert!(maxd.within(Duration::from_millis(200)).await.is_none());
        maxd.send(&ServerMessage::TunnelCredit {
            stream,
            granted: 4096,
        })
        .await;
        let Sent::Data(_, rest) = maxd.next().await else {
            panic!("the rest comes once there's credit");
        };
        assert_eq!(rest.len(), 1000);

        // The end of input ends this end's half.
        drop(soon(writer).await.unwrap());
        assert_eq!(
            maxd.message().await,
            ClientMessage::TunnelClose {
                stream,
                how: TunnelEnd::Write
            }
        );

        // The guest's bytes reach stdout, and earn maxd more credit.
        let reply: Vec<u8> = (0..2 * MAX_TUNNEL_CHUNK).map(|i| i as u8).collect();
        // Stdout is read as maxd writes, as ssh reads its ProxyCommand.
        let reading = tokio::spawn(async move {
            let mut out = vec![0; 2 * MAX_TUNNEL_CHUNK];
            stdout.read_exact(&mut out).await.unwrap();
            out
        });
        for chunk in reply.chunks(MAX_TUNNEL_CHUNK) {
            maxd.data(stream, chunk).await;
        }
        assert_eq!(soon(reading).await.unwrap(), reply);
        let mut granted = 0;
        while granted < reply.len() as u64 {
            match maxd.message().await {
                ClientMessage::TunnelCredit {
                    stream: id,
                    granted: more,
                } => {
                    assert_eq!(id, stream);
                    granted += more;
                }
                message => panic!("expected credit, got {message:?}"),
            }
        }
        assert_eq!(granted, reply.len() as u64);

        // The guest's end of writing ends the session.
        maxd.send(&ServerMessage::TunnelClose {
            stream,
            how: TunnelEnd::Write,
        })
        .await;
        soon(carried).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_refused_session_says_why() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, _stdin, _stdout, carried) = start_guest_ssh(workspace);
        maxd.greet(true).await;
        let stream = maxd.opening(workspace).await;
        maxd.send(&ServerMessage::TunnelRefused {
            stream,
            code: maxhq_protocol::message::ErrorCode::NoSuchWorkspace,
            message: "That workspace isn't running.".into(),
        })
        .await;
        let err = soon(carried).await.unwrap().unwrap_err();
        assert_eq!(err.to_string(), "That workspace isn't running.");
    }

    #[tokio::test]
    async fn a_device_without_guest_ssh_opens_nothing() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, _stdin, _stdout, carried) = start_guest_ssh(workspace);
        maxd.greet(false).await;
        let err = soon(carried).await.unwrap().unwrap_err();
        assert!(matches!(err, ProxyError::NoGuestSsh), "{err}");
    }
}

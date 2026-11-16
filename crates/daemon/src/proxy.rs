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
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{
    self, DecodeError, EncodeError, Frame, FrameDecoder, MAX_TUNNEL_CHUNK,
};
use maxhq_protocol::ids::{ClientId, DeviceId, PairId, PhoneId, StreamId, WorkspaceId};
use maxhq_protocol::message::{ClientHello, ClientInfo, TunnelEnd};
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use thiserror::Error;
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
fn connect(path: &Path) -> Result<UnixStream, ProxyError> {
    UnixStream::connect(path).map_err(|source| ProxyError::Connect {
        path: path.into(),
        source,
    })
}

/// Joins stdin and stdout to the socket at `path` until either side closes.
pub fn run(path: &Path) -> Result<(), ProxyError> {
    bridge(io::stdin(), io::stdout(), connect(path)?)?;
    Ok(())
}

/// Joins stdin and stdout to the socket at `path` after `first`, the frame
/// that says which device, phone or pairing dialed.
pub fn run_as(path: &Path, first: &ClientMessage) -> Result<(), ProxyError> {
    first_bridge(io::stdin(), io::stdout(), connect(path)?, first)
}

/// Writes `first`, then bridges.
fn first_bridge(
    input: impl Read + Send + 'static,
    output: impl Write,
    mut stream: UnixStream,
    message: &ClientMessage,
) -> Result<(), ProxyError> {
    stream.write_all(&first_frame(message)?)?;
    bridge(input, output, stream)?;
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
    guest_ssh(io::stdin(), io::stdout(), connect(path)?, workspace)
}

/// Copies `input` to the socket on a thread of its own, and the socket to
/// `output` on this one.
///
/// When `input` ends, the socket's sending side shuts, and `maxd` closes the
/// connection once it has read the rest. When the socket closes, the bridge
/// returns at once and leaves the input thread waiting, so the caller should
/// exit rather than wait for it.
pub fn bridge(
    input: impl Read + Send + 'static,
    output: impl Write,
    stream: UnixStream,
) -> io::Result<()> {
    let to_maxd = stream.try_clone()?;
    thread::spawn(move || {
        let copied = copy(input, &to_maxd);
        // At the end of input only the sending side shuts, since replies
        // may still be on their way. After an error nothing more will come,
        // so both shut and the bridge returns.
        let how = if copied.is_ok() {
            Shutdown::Write
        } else {
            Shutdown::Both
        };
        let _ = to_maxd.shutdown(how);
    });
    match copy(&stream, output) {
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
    }
}

/// Copies until `from` ends, flushing after every read so nothing waits in
/// a buffer while the other side waits for it.
fn copy(mut from: impl Read, mut to: impl Write) -> io::Result<()> {
    let mut buf = vec![0; CHUNK];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        to.write_all(&buf[..n])?;
        to.flush()?;
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
/// as much, and more as this end writes its bytes out. As with
/// [`bridge`], the input thread may still be waiting when this returns,
/// so the caller should exit.
pub fn guest_ssh(
    input: impl Read + Send + 'static,
    mut output: impl Write,
    socket: UnixStream,
    workspace: WorkspaceId,
) -> Result<(), ProxyError> {
    let writer = Writer(Arc::new(Mutex::new(socket.try_clone()?)));
    let mut reader = Reader::new(socket);
    writer.send(&ClientMessage::Hello(ClientHello {
        protocol: ProtocolRange::OURS,
        client: ClientInfo {
            name: CLIENT.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        client_id: ClientId::generate(),
        role: ClientRole::App,
    }))?;
    loop {
        match reader.next()? {
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
    writer.send(&ClientMessage::GuestSshOpen { stream, workspace })?;
    let credit = Arc::new(Credit::default());
    // Bytes `maxd` may still send, and bytes written out but not yet
    // granted back.
    let mut window = STREAM_CREDIT;
    let mut ungranted = 0;
    // Taken by the input thread once the stream opens.
    let mut input = Some(input);
    loop {
        match reader.next()? {
            Incoming::Message(message) => match message {
                ServerMessage::TunnelOpened { stream: id } if id == stream => {
                    if let Some(input) = input.take() {
                        credit.grant(STREAM_CREDIT);
                        let (writer, credit) = (writer.clone(), credit.clone());
                        thread::spawn(move || send_input(input, stream, &writer, &credit));
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
                    let _ = writer.send(&ClientMessage::TunnelClose {
                        stream,
                        how: TunnelEnd::Reset,
                    });
                    return Err(ProxyError::Overrun);
                }
                window -= len;
                if let Err(err) = output.write_all(&data).and_then(|()| output.flush()) {
                    let _ = writer.send(&ClientMessage::TunnelClose {
                        stream,
                        how: TunnelEnd::Reset,
                    });
                    return Err(err.into());
                }
                ungranted += len;
                if ungranted >= GRANT_STEP {
                    writer.send(&ClientMessage::TunnelCredit {
                        stream,
                        granted: ungranted,
                    })?;
                    window += ungranted;
                    ungranted = 0;
                }
            }
            Incoming::Data { .. } => {}
        }
    }
}

/// Sends `input` as `stream`'s bytes as credit allows, then says this
/// end is done writing. A failed read resets the stream.
fn send_input(mut input: impl Read, stream: StreamId, writer: &Writer, credit: &Credit) {
    let mut buf = vec![0; MAX_TUNNEL_CHUNK];
    loop {
        let room = usize::try_from(credit.wait()).unwrap_or(usize::MAX);
        let n = match input.read(&mut buf[..room.min(MAX_TUNNEL_CHUNK)]) {
            Ok(0) => {
                let _ = writer.send(&ClientMessage::TunnelClose {
                    stream,
                    how: TunnelEnd::Write,
                });
                return;
            }
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = writer.send(&ClientMessage::TunnelClose {
                    stream,
                    how: TunnelEnd::Reset,
                });
                writer.shut();
                return;
            }
        };
        credit.spend(n as u64);
        let mut frame = Vec::with_capacity(n + 32);
        frame::encode_tunnel_data(&mut frame, stream, &buf[..n]);
        if writer.write(&frame).is_err() {
            return;
        }
    }
}

/// Bytes this end may still send, and a wake for when it may send more.
#[derive(Default)]
struct Credit {
    left: Mutex<u64>,
    more: Condvar,
}

impl Credit {
    fn grant(&self, granted: u64) {
        let mut left = lock(&self.left);
        *left = left.saturating_add(granted);
        self.more.notify_one();
    }

    /// Waits until there's credit, and gives how much.
    fn wait(&self) -> u64 {
        let mut left = lock(&self.left);
        while *left == 0 {
            left = self.more.wait(left).unwrap_or_else(PoisonError::into_inner);
        }
        *left
    }

    fn spend(&self, n: u64) {
        let mut left = lock(&self.left);
        *left = left.saturating_sub(n);
    }
}

/// The socket's sending side, shared by the input thread and the reader,
/// so each frame goes out whole.
#[derive(Clone)]
struct Writer(Arc<Mutex<UnixStream>>);

impl Writer {
    fn send(&self, message: &ClientMessage) -> Result<(), ProxyError> {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message)?;
        Ok(self.write(&out)?)
    }

    fn write(&self, frame: &[u8]) -> io::Result<()> {
        lock(&self.0).write_all(frame)
    }

    /// Shuts the socket both ways, so the reader stops too.
    fn shut(&self) {
        let _ = lock(&self.0).shutdown(Shutdown::Both);
    }
}

/// What came in on the socket.
enum Incoming {
    Message(ServerMessage),
    Data { stream: StreamId, data: Vec<u8> },
}

/// The socket's receiving side, read a frame at a time.
struct Reader {
    socket: UnixStream,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Reader {
    fn new(socket: UnixStream) -> Self {
        Self {
            socket,
            decoder: FrameDecoder::new(),
            buf: vec![0; CHUNK],
        }
    }

    /// The next message or tunnel frame. Messages this build doesn't read
    /// and other kinds of frame are skipped.
    fn next(&mut self) -> Result<Incoming, ProxyError> {
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
            let n = match self.socket.read(&mut self.buf) {
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
    use std::sync::mpsc;
    use std::time::Duration;

    // Long, since a loaded machine can leave a thread waiting for
    // seconds. Only a failing test waits it out.
    const WAIT: Duration = Duration::from_secs(30);

    /// Runs a bridge on a thread, with sockets standing in for stdin and
    /// stdout. Returns maxd's end, the test's ends of stdin and stdout, and
    /// a channel that gets the bridge's result.
    fn start() -> (
        UnixStream,
        UnixStream,
        UnixStream,
        mpsc::Receiver<io::Result<()>>,
    ) {
        let (stream, maxd) = UnixStream::pair().unwrap();
        let (input, stdin) = UnixStream::pair().unwrap();
        let (output, stdout) = UnixStream::pair().unwrap();
        let (done, result) = mpsc::channel();
        thread::spawn(move || done.send(bridge(input, output, stream)));
        for end in [&maxd, &stdin, &stdout] {
            end.set_read_timeout(Some(WAIT)).unwrap();
        }
        (maxd, stdin, stdout, result)
    }

    #[test]
    fn bytes_flow_both_ways() {
        let (mut maxd, mut stdin, mut stdout, result) = start();
        stdin.write_all(b"hello").unwrap();
        let mut got = [0; 5];
        maxd.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello");

        maxd.write_all(b"world").unwrap();
        stdout.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"world");

        drop(maxd);
        result.recv_timeout(WAIT).unwrap().unwrap();
    }

    #[test]
    fn the_end_of_input_reaches_maxd_and_replies_still_arrive() {
        let (mut maxd, mut stdin, mut stdout, result) = start();
        stdin.write_all(b"ping").unwrap();
        drop(stdin);
        let mut got = Vec::new();
        maxd.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"ping", "maxd reads to the end");

        maxd.write_all(b"pong").unwrap();
        drop(maxd);
        result.recv_timeout(WAIT).unwrap().unwrap();
        let mut reply = Vec::new();
        stdout.read_to_end(&mut reply).unwrap();
        assert_eq!(reply, b"pong");
    }

    #[test]
    fn maxd_closing_ends_the_proxy_while_input_is_open() {
        let (maxd, _stdin, _stdout, result) = start();
        drop(maxd);
        result.recv_timeout(WAIT).unwrap().unwrap();
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

    #[test]
    fn the_peer_frame_comes_before_the_clients_bytes() {
        let device = DeviceId(Uuid::try_parse("0199a7c2-0000-7000-8000-0000000000aa").unwrap());
        let (stream, mut maxd) = UnixStream::pair().unwrap();
        let (input, mut stdin) = UnixStream::pair().unwrap();
        let (output, _stdout) = UnixStream::pair().unwrap();
        maxd.set_read_timeout(Some(WAIT)).unwrap();
        thread::spawn(move || first_bridge(input, output, stream, &ClientMessage::Peer { device }));
        stdin.write_all(b"hello").unwrap();
        drop(stdin);
        let mut got = Vec::new();
        maxd.read_to_end(&mut got).unwrap();
        let mut decoder = FrameDecoder::new();
        decoder.push(&got);
        match decoder.next_frame().unwrap() {
            Some(Frame::Message(text)) => assert_eq!(
                serde_json::from_slice::<ClientMessage>(text).unwrap(),
                ClientMessage::Peer { device }
            ),
            other => panic!("{other:?}"),
        }
        assert!(got.ends_with(b"hello"));
    }

    #[test]
    fn a_phone_or_pairing_frame_comes_first_and_names_only_a_plain_id() {
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
            let (stream, mut maxd) = UnixStream::pair().unwrap();
            let (input, stdin) = UnixStream::pair().unwrap();
            let (output, _stdout) = UnixStream::pair().unwrap();
            maxd.set_read_timeout(Some(WAIT)).unwrap();
            let sent = first.clone();
            thread::spawn(move || first_bridge(input, output, stream, &sent));
            drop(stdin);
            let mut got = Vec::new();
            maxd.read_to_end(&mut got).unwrap();
            let mut decoder = FrameDecoder::new();
            decoder.push(&got);
            match decoder.next_frame().unwrap() {
                Some(Frame::Message(text)) => {
                    assert_eq!(
                        serde_json::from_slice::<ClientMessage>(text).unwrap(),
                        first
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// The device end of a guest SSH proxy's socket.
    struct Maxd {
        socket: UnixStream,
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
        fn within(&mut self, wait: Duration) -> Option<Sent> {
            self.socket.set_read_timeout(Some(wait)).unwrap();
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
                match self.socket.read(&mut buf) {
                    Ok(0) => panic!("the proxy closed the socket"),
                    Ok(n) => self.decoder.push(&buf[..n]),
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        return None;
                    }
                    Err(err) => panic!("{err}"),
                }
            }
        }

        fn next(&mut self) -> Sent {
            self.within(WAIT).expect("the proxy sends something")
        }

        fn message(&mut self) -> ClientMessage {
            match self.next() {
                Sent::Message(message) => message,
                sent => panic!("expected a message, got {sent:?}"),
            }
        }

        fn send(&mut self, message: &ServerMessage) {
            let mut out = Vec::new();
            frame::encode_message(&mut out, message).unwrap();
            self.socket.write_all(&out).unwrap();
        }

        fn data(&mut self, stream: StreamId, bytes: &[u8]) {
            let mut out = Vec::new();
            frame::encode_tunnel_data(&mut out, stream, bytes);
            self.socket.write_all(&out).unwrap();
        }

        /// Takes the hello and answers it, saying whether this device
        /// opens guest SSH sessions.
        fn greet(&mut self, guest_ssh: bool) {
            let ClientMessage::Hello(hello) = self.message() else {
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
            ));
        }

        /// Takes the session's open, and gives its stream.
        fn opening(&mut self, workspace: WorkspaceId) -> StreamId {
            match self.message() {
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

    /// Runs a guest SSH proxy on a thread, with sockets standing in for
    /// stdin and stdout. Returns maxd's end, the test's ends of stdin and
    /// stdout, and a channel that gets the proxy's result.
    fn start_guest_ssh(
        workspace: WorkspaceId,
    ) -> (
        Maxd,
        UnixStream,
        UnixStream,
        mpsc::Receiver<Result<(), ProxyError>>,
    ) {
        let (socket, maxd) = UnixStream::pair().unwrap();
        let (input, stdin) = UnixStream::pair().unwrap();
        let (output, stdout) = UnixStream::pair().unwrap();
        let (done, result) = mpsc::channel();
        thread::spawn(move || done.send(guest_ssh(input, output, socket, workspace)));
        stdout.set_read_timeout(Some(WAIT)).unwrap();
        let maxd = Maxd {
            socket: maxd,
            decoder: FrameDecoder::new(),
        };
        (maxd, stdin, stdout, result)
    }

    #[test]
    fn a_guest_ssh_session_flows_both_ways_with_credit() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, mut stdin, stdout, result) = start_guest_ssh(workspace);
        maxd.greet(true);
        let stream = maxd.opening(workspace);

        // Nothing goes before the stream opens.
        let sent = STREAM_CREDIT as usize + 1000;
        let writer = thread::spawn(move || {
            stdin.write_all(&vec![7; sent]).unwrap();
            stdin
        });
        assert!(maxd.within(Duration::from_millis(200)).is_none());
        maxd.send(&ServerMessage::TunnelOpened { stream });

        // Then the stream's credit, and no more until more is granted.
        let mut got = 0;
        while got < STREAM_CREDIT as usize {
            match maxd.next() {
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
        assert!(maxd.within(Duration::from_millis(200)).is_none());
        maxd.send(&ServerMessage::TunnelCredit {
            stream,
            granted: 4096,
        });
        let Sent::Data(_, rest) = maxd.next() else {
            panic!("the rest comes once there's credit");
        };
        assert_eq!(rest.len(), 1000);

        // The end of input ends this end's half.
        drop(writer.join().unwrap());
        assert_eq!(
            maxd.message(),
            ClientMessage::TunnelClose {
                stream,
                how: TunnelEnd::Write
            }
        );

        // The guest's bytes reach stdout, and earn maxd more credit.
        let reply: Vec<u8> = (0..2 * MAX_TUNNEL_CHUNK).map(|i| i as u8).collect();
        // Stdout is read as maxd writes, as ssh reads its ProxyCommand.
        let reading = thread::spawn(move || {
            let mut stdout = stdout;
            let mut out = vec![0; 2 * MAX_TUNNEL_CHUNK];
            stdout.read_exact(&mut out).unwrap();
            out
        });
        for chunk in reply.chunks(MAX_TUNNEL_CHUNK) {
            maxd.data(stream, chunk);
        }
        assert_eq!(reading.join().unwrap(), reply);
        let mut granted = 0;
        while granted < reply.len() as u64 {
            match maxd.message() {
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
        });
        result.recv_timeout(WAIT).unwrap().unwrap();
    }

    #[test]
    fn a_refused_session_says_why() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, _stdin, _stdout, result) = start_guest_ssh(workspace);
        maxd.greet(true);
        let stream = maxd.opening(workspace);
        maxd.send(&ServerMessage::TunnelRefused {
            stream,
            code: maxhq_protocol::message::ErrorCode::NoSuchWorkspace,
            message: "That workspace isn't running.".into(),
        });
        let err = result.recv_timeout(WAIT).unwrap().unwrap_err();
        assert_eq!(err.to_string(), "That workspace isn't running.");
    }

    #[test]
    fn a_device_without_guest_ssh_opens_nothing() {
        let workspace = WorkspaceId::derive("w");
        let (mut maxd, _stdin, _stdout, result) = start_guest_ssh(workspace);
        maxd.greet(false);
        let err = result.recv_timeout(WAIT).unwrap().unwrap_err();
        assert!(matches!(err, ProxyError::NoGuestSsh), "{err}");
    }
}

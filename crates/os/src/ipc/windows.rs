// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Named pipes, at [`super::pipe_name`].
//!
//! Who gets in, and what each side checks:
//!
//! - The pipe's DACL lets in this process's user alone, so another user's
//!   process can't open it. An elevated process of the same user can.
//! - Remote clients are refused, so the pipe is never reached over SMB.
//! - The first instance is made with `FILE_FLAG_FIRST_PIPE_INSTANCE`:
//!   when another process already holds the name, [`bind`] fails instead
//!   of serving beside it.
//! - A client checks that the process serving the pipe runs as its own
//!   user before it sends a byte, so a pipe another user made first
//!   (squatting) is refused. It opens the pipe at the identification
//!   level, so a server can learn who it is but can't act as it.
//!
//! Pipes have no half-close: shutting a stream down only flushes it, and
//! the other end sees the end once this one is dropped.

use std::ffi::c_void;
use std::future::Future;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};

use super::{Found, Peer};
use crate::win;

/// The pipe `maxd` serves on, with the instance the next client takes.
#[derive(Debug)]
pub struct Listener {
    name: String,
    next: tokio::sync::Mutex<NamedPipeServer>,
}

/// One end of a pipe.
#[derive(Debug)]
pub enum Stream {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}

pub type ReadHalf = tokio::io::ReadHalf<Stream>;
pub type WriteHalf = tokio::io::WriteHalf<Stream>;

/// How long [`connect`] waits for a free instance while every one is
/// taken.
const BUSY_WAIT: Duration = Duration::from_secs(5);

pub fn bind(path: &Path) -> io::Result<Listener> {
    let name = super::pipe_name(path);
    let first = create(&name, true)?;
    Ok(Listener {
        name,
        next: tokio::sync::Mutex::new(first),
    })
}

pub async fn accept(listener: &Listener) -> io::Result<Stream> {
    let mut next = listener.next.lock().await;
    loop {
        match next.connect().await {
            Ok(()) => break,
            // A client that came and went before it was taken.
            Err(err) if err.raw_os_error() == Some(ERROR_NO_DATA as i32) => {
                next.disconnect()?;
            }
            Err(err) => return Err(err),
        }
    }
    // The next instance is there before this one is handed out, so a
    // client never finds the name gone. Should making it fail, the
    // connected one stays put and the next call hands it out.
    let fresh = create(&listener.name, false)?;
    Ok(Stream::Server(std::mem::replace(&mut *next, fresh)))
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    let name = super::pipe_name(path);
    let deadline = Instant::now() + BUSY_WAIT;
    let client = loop {
        // The default quality of service is the identification level.
        match ClientOptions::new().open(&name) {
            Ok(client) => break client,
            Err(err)
                if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => return Err(err),
        }
    };
    let pid = server_pid(&client)?;
    served_by_this_user(&name, pid)?;
    Ok(Stream::Client(client))
}

/// Refuses a pipe whose server, process `pid`, runs as another user.
pub(crate) fn served_by_this_user(name: &str, pid: u32) -> io::Result<()> {
    let refuse = |why: String| io::Error::new(io::ErrorKind::PermissionDenied, why);
    let user = win::token_of_pid(pid)
        .and_then(|token| win::user_of(&token))
        .map_err(|err| refuse(format!("couldn't check who serves {name}: {err}")))?;
    if user == win::own_user()? {
        Ok(())
    } else {
        Err(refuse(format!(
            "{name} is served by process {pid}, which runs as another user"
        )))
    }
}

pub fn peer(stream: &Stream) -> io::Result<Peer> {
    let pid = match stream {
        Stream::Server(server) => client_pid(server)?,
        Stream::Client(client) => server_pid(client)?,
    };
    let token = win::token_of_pid(pid)?;
    Ok(Peer {
        pid: Some(pid),
        same_user: win::user_of(&token)? == win::own_user()?,
        elevated: win::elevated(&token)?,
    })
}

#[allow(unsafe_code)]
fn client_pid(server: &NamedPipeServer) -> io::Result<u32> {
    let mut pid = 0u32;
    // SAFETY: the pipe is open for the call, which writes one u32.
    win::check(unsafe { GetNamedPipeClientProcessId(win::raw(server), &mut pid) })?;
    Ok(pid)
}

#[allow(unsafe_code)]
fn server_pid(client: &NamedPipeClient) -> io::Result<u32> {
    let mut pid = 0u32;
    // SAFETY: the pipe is open for the call, which writes one u32.
    win::check(unsafe { GetNamedPipeServerProcessId(win::raw(client), &mut pid) })?;
    Ok(pid)
}

pub fn into_split(stream: Stream) -> (ReadHalf, WriteHalf) {
    tokio::io::split(stream)
}

pub fn pair() -> io::Result<(Stream, Stream)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        r"\\.\pipe\maxhq-pair-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let server = create(&name, true)?;
    let client = ClientOptions::new().open(&name)?;
    // The client is connected, but the server end reads nothing until it
    // has seen the connection, which it does at once now.
    let connected = {
        let connect = std::pin::pin!(server.connect());
        connect.poll(&mut Context::from_waker(std::task::Waker::noop()))
    };
    match connected {
        Poll::Ready(connected) => connected?,
        Poll::Pending => {
            return Err(io::Error::other(
                "the pipe's server end didn't see its client connect",
            ));
        }
    }
    Ok((Stream::Server(server), Stream::Client(client)))
}

pub fn answers(path: &Path) -> bool {
    probe(path).is_ok_and(|found| found == Found::Listening)
}

pub fn probe(path: &Path) -> io::Result<Found> {
    let name = super::pipe_name(path);
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(&name);
    match opened {
        Ok(_) => Ok(Found::Listening),
        Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => Ok(Found::Listening),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Found::Nothing),
        // A pipe of that name this user can't open: someone else's.
        Err(err) if err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => Ok(Found::Other),
        Err(err) => Err(err),
    }
}

/// Makes an instance of the pipe `name`, for this user alone. The first
/// fails when the name is taken.
#[allow(unsafe_code)]
fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let descriptor = Descriptor::for_this_user()?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).map_err(io::Error::other)?,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES, and the
    // descriptor it points at lives until the call returns. The pipe
    // keeps a copy of it.
    let made =
        unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) };
    made.map_err(|err| {
        if first && err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another process already serves {name}"),
            )
        } else {
            err
        }
    })
}

/// A security descriptor whose DACL grants this user everything and
/// nobody else anything, inherited from nowhere.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    #[allow(unsafe_code)]
    fn for_this_user() -> io::Result<Self> {
        let sid = win::sid_string(&win::own_user()?)?;
        let sddl = win::wide(format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated, and the call writes one
        // pointer to a descriptor it allocated with LocalAlloc.
        win::check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        })?;
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the descriptor came from LocalAlloc and is freed once.
        unsafe { LocalFree(self.0.cast::<c_void>()) };
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(pipe) => Pin::new(pipe).poll_read(cx, buf),
            Stream::Client(pipe) => Pin::new(pipe).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Server(pipe) => Pin::new(pipe).poll_write(cx, buf),
            Stream::Client(pipe) => Pin::new(pipe).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(pipe) => Pin::new(pipe).poll_flush(cx),
            Stream::Client(pipe) => Pin::new(pipe).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(pipe) => Pin::new(pipe).poll_shutdown(cx),
            Stream::Client(pipe) => Pin::new(pipe).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Server(pipe) => Pin::new(pipe).poll_write_vectored(cx, bufs),
            Stream::Client(pipe) => Pin::new(pipe).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Stream::Server(pipe) => pipe.is_write_vectored(),
            Stream::Client(pipe) => pipe.is_write_vectored(),
        }
    }
}

/// The DACL of the pipe `stream` is an end of, in SDDL.
#[cfg(test)]
#[allow(unsafe_code)]
pub(crate) fn dacl_of(stream: &Stream) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    let handle = match stream {
        Stream::Server(pipe) => win::raw(pipe),
        Stream::Client(pipe) => win::raw(pipe),
    };
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the pipe is open for the call, which writes one pointer to
    // a descriptor it allocated with LocalAlloc.
    let failed = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if failed != 0 {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(failed).unwrap_or(i32::MAX),
        ));
    }
    let descriptor = Descriptor(descriptor);
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: the descriptor is valid, and the call writes one pointer to
    // a string it allocated with LocalAlloc.
    win::check(unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            std::ptr::null_mut(),
        )
    })?;
    // SAFETY: the call allocated `text` with LocalAlloc.
    Ok(unsafe { win::take_local_string(text) })
}

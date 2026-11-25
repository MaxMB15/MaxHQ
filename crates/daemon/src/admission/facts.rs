// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a card says about the program it's about: how it reached `maxd`
//! and, on a Mac or Windows, which program it is.

use std::path::PathBuf;
use std::sync::OnceLock;

use maxhq_protocol::time::Timestamp;

use crate::codesign;

/// How a program reached `maxd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// It runs on the device and dialled the socket itself.
    Socket,
    /// It came over SSH, through `maxd proxy`.
    Proxy,
    /// It came through network access, which `maxd` dials in for.
    Network,
    /// The system didn't say.
    Unknown,
}

/// The connection a card is about, as the device saw it.
#[derive(Clone, Debug)]
pub struct Caller {
    /// The process on the other end of the socket.
    pub pid: Option<i32>,
    /// Its audit token as it connected.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub token: Option<codesign::Token>,
    /// The name and version from its hello.
    pub says: Option<String>,
    /// Whether its first frame named a paired phone.
    pub phone: bool,
    /// When it first asked.
    pub asked_at: Timestamp,
}

/// What a card says about its program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// The name and version from its hello.
    pub says: Option<String>,
    /// How it reached `maxd`.
    pub transport: Transport,
    /// Whether its first frame named a paired phone.
    pub phone: bool,
    /// On a Mac or Windows, the program that dialled the socket.
    pub program: Option<codesign::Program>,
    /// When it first asked.
    pub asked_at: Timestamp,
    /// Whether it dialled a Mac's socket while a proven app is connected
    /// through the same socket, so it's most likely not MaxHQ.
    pub same_mac: bool,
}

impl Facts {
    /// The facts about `caller`, while proven desktop apps are connected
    /// from the processes `apps`. Blocks on the file system and, on a Mac,
    /// the Security framework.
    pub fn find(caller: &Caller, apps: &[i32]) -> Self {
        let how = transport(caller.pid, caller.phone);
        let local = cfg!(any(target_os = "macos", windows)) && how == Transport::Socket;
        Self {
            says: caller.says.clone(),
            transport: how,
            phone: caller.phone,
            program: if local { program(caller) } else { None },
            asked_at: caller.asked_at,
            same_mac: cfg!(target_os = "macos")
                && local
                && apps
                    .iter()
                    .any(|&app| transport(Some(app), false) == Transport::Socket),
        }
    }
}

/// The program `caller` runs, from its token, or from its process when
/// the token names none, as it never does on Windows. A connection from
/// `maxd` itself runs `maxd`.
#[cfg(any(target_os = "macos", windows))]
fn program(caller: &Caller) -> Option<codesign::Program> {
    if caller.pid.is_some_and(own) {
        return Some(codesign::own(own_exe().cloned()));
    }
    let mut program = codesign::program(caller.token);
    if program.path.is_none() {
        program.path = caller.pid.and_then(exe_of);
    }
    Some(program)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn program(_caller: &Caller) -> Option<codesign::Program> {
    None
}

/// How the process `pid` reached `maxd`, when its first frame named a
/// phone or not.
fn transport(pid: Option<i32>, phone: bool) -> Transport {
    let Some(pid) = pid else {
        return Transport::Unknown;
    };
    classify(own(pid), phone, exe_of(pid).as_ref(), own_exe())
}

/// Whether `pid` is `maxd`'s own process.
fn own(pid: i32) -> bool {
    u32::try_from(pid).is_ok_and(|pid| pid == std::process::id())
}

/// How a process reached `maxd`: `own` when it's `maxd` itself, with its
/// program at `exe` and `maxd`'s at `maxd`.
pub(super) fn classify(
    own: bool,
    phone: bool,
    exe: Option<&PathBuf>,
    maxd: Option<&PathBuf>,
) -> Transport {
    match (own, exe) {
        // Network access dials in for a phone. Anything else from `maxd`
        // itself is a test's client.
        (true, _) if phone => Transport::Network,
        (true, _) => Transport::Socket,
        // `maxd proxy` is `maxd`'s own program.
        (false, Some(exe)) if Some(exe) == maxd => Transport::Proxy,
        (false, Some(_)) => Transport::Socket,
        (false, None) => Transport::Unknown,
    }
}

/// The program the process `pid` runs.
fn exe_of(pid: i32) -> Option<PathBuf> {
    u32::try_from(pid)
        .ok()
        .and_then(maxhq_os::proc::exe_of)
        .map(normal)
}

/// `path` with links resolved, and without the mark Linux adds to a
/// program replaced while it runs, as an update does.
fn normal(path: PathBuf) -> PathBuf {
    let path = match path
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
    {
        Some(path) => PathBuf::from(path),
        None => path,
    };
    path.canonicalize().unwrap_or(path)
}

/// The program this `maxd` runs.
fn own_exe() -> Option<&'static PathBuf> {
    static OWN: OnceLock<Option<PathBuf>> = OnceLock::new();
    OWN.get_or_init(|| std::env::current_exe().ok().map(normal))
        .as_ref()
}

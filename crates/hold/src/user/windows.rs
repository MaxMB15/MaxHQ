// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::convert::Infallible;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::RunAs;

const UNSUPPORTED: &str = "running the agent as another user needs a Unix host";

/// A user's record in the user database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub shell: Option<PathBuf>,
}

/// The record for `user`, which Windows has no database of in this sense.
pub(crate) fn lookup(_user: &str) -> io::Result<Option<Entry>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, UNSUPPORTED))
}

/// What starting as another user needs, which a Windows holder never has.
pub(crate) struct Prepared {
    never: Infallible,
    pub env: Vec<(OsString, OsString)>,
    pub shell: Option<PathBuf>,
}

pub(crate) fn prepare(_run_as: &RunAs, _cwd: &Path) -> Result<Prepared, String> {
    Err(UNSUPPORTED.into())
}

impl Prepared {
    pub(crate) fn apply(self, _command: &mut Command) -> Result<(), String> {
        match self.never {}
    }
}

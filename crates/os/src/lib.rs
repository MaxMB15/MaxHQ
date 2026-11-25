// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The operating system calls MaxHQ makes, behind one API. Each module has
//! a `unix.rs` and a `windows.rs` with the same functions, so the code that
//! calls them has no `cfg` of its own.
//!
//! Where Windows can't do something yet, the call returns an error of kind
//! [`std::io::ErrorKind::Unsupported`] that says what's missing.

pub mod fsmode;
pub mod home;
pub mod ipc;
pub mod link;
pub mod proc;
pub mod which;
#[cfg(windows)]
mod win;

/// The error a call returns on a system that can't make it yet.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn unsupported(what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{what} isn't available on this system yet"),
    )
}

/// Held by the tests that start processes or make sockets. macOS marks a
/// socket close-on-exec only after making it, so a child started between
/// the two keeps it open, and a listener a test dropped would still answer.
#[cfg(test)]
pub(crate) fn forks() -> std::sync::MutexGuard<'static, ()> {
    static FORKS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    FORKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `test` on a runtime of its own, for a test that holds [`forks`]
/// across its awaits.
#[cfg(test)]
pub(crate) fn block_on<F: std::future::Future>(test: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(test)
}

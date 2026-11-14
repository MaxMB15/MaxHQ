// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// the CLI's Keychain item, `cursor-access-token` for `cursor-user`, a
// 30-second wait for the macOS prompt, and a read still waiting on it kept
// for the next try instead of starting another.

//! The Cursor CLI's saved login, read from the Mac's Keychain. macOS asks
//! you before `maxd` may read it.

use std::time::Duration;

use tokio::task::JoinHandle;
use zeroize::Zeroizing;

/// The Keychain item the CLI keeps its access token in.
#[cfg(target_os = "macos")]
const SERVICE: &str = "cursor-access-token";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "cursor-user";

/// How long the macOS prompt may wait for you.
const WAIT: Duration = Duration::from_secs(30);

/// What one read gives: the token, none when the CLI saved no login, or
/// why it couldn't be read.
pub type Read = Result<Option<Zeroizing<String>>, String>;

/// The CLI's login in the Keychain.
pub struct Keychain {
    /// How a read is done, which tests stand in for.
    read: fn() -> Read,
    /// How long a read may wait.
    wait: Duration,
    /// A read still waiting on the prompt.
    pending: Option<JoinHandle<Read>>,
}

impl Default for Keychain {
    fn default() -> Self {
        Self {
            read,
            wait: WAIT,
            pending: None,
        }
    }
}

impl Keychain {
    /// Reads the token, waiting for the prompt if macOS shows one.
    pub async fn token(&mut self) -> Read {
        let mut reading = match self.pending.take() {
            Some(reading) => reading,
            None => tokio::task::spawn_blocking(self.read),
        };
        match tokio::time::timeout(self.wait, &mut reading).await {
            Ok(done) => done.map_err(|_| "the Keychain read stopped".to_owned())?,
            Err(_) => {
                self.pending = Some(reading);
                Err("the Keychain prompt wasn't answered".into())
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn read() -> Read {
    use security_framework::passwords::get_generic_password;

    /// The item isn't there.
    const NOT_FOUND: i32 = -25300;
    /// You pressed Deny.
    const DENIED: [i32; 2] = [-128, -25293];

    match get_generic_password(SERVICE, ACCOUNT) {
        Ok(bytes) => {
            let token = Zeroizing::new(bytes);
            let text = std::str::from_utf8(&token).map_err(|_| "the saved login isn't text")?;
            let text = text.trim();
            Ok((!text.is_empty()).then(|| Zeroizing::new(text.to_owned())))
        }
        Err(err) if err.code() == NOT_FOUND => Ok(None),
        Err(err) if DENIED.contains(&err.code()) => {
            Err("macOS didn't allow reading the Keychain".into())
        }
        Err(_) => Err("couldn't read the Keychain".into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn read() -> Read {
    Err("Cursor usage works only on a Mac".into())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    use super::*;

    static READS: AtomicUsize = AtomicUsize::new(0);
    /// Whether you answered the stand-in prompt.
    static ANSWERED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

    /// A read that waits on the prompt, then gives a token.
    fn slow() -> Read {
        READS.fetch_add(1, Ordering::SeqCst);
        let (answered, wake) = &ANSWERED;
        let _answered = wake
            .wait_while(answered.lock().unwrap(), |answered| !*answered)
            .unwrap();
        Ok(Some(Zeroizing::new("test-token".into())))
    }

    #[tokio::test]
    async fn a_read_waiting_on_the_prompt_is_kept_for_the_next_try() {
        let mut keychain = Keychain {
            read: slow,
            wait: Duration::from_millis(50),
            pending: None,
        };
        assert_eq!(
            keychain.token().await.unwrap_err(),
            "the Keychain prompt wasn't answered"
        );
        *ANSWERED.0.lock().unwrap() = true;
        ANSWERED.1.notify_all();
        let token = keychain.token().await.unwrap().unwrap();
        assert_eq!(token.as_str(), "test-token");
        assert_eq!(READS.load(Ordering::SeqCst), 1, "one prompt, not two");
    }
}

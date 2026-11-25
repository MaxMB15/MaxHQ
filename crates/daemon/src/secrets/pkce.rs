// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a browser sign-in with PKCE needs on the device, for Sign in with
//! ChatGPT and Sign in with OpenRouter alike: random values, the S256
//! challenge of a verifier, a port for the app to listen on, and the
//! rounds that wait for their code, each for 10 minutes at most.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::ids::SignInId;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use zeroize::Zeroizing;

/// How long a round waits for its code.
pub(crate) const ROUND_LIMIT: Duration = Duration::from_secs(10 * 60);

/// The most rounds that wait at once. A new one past it ends the oldest.
const MAX_ROUNDS: usize = 4;

/// The first port of the dynamic range, where a callback port is picked.
const FIRST_PORT: u16 = 49_152;

/// `len` random bytes in base64url.
pub(crate) fn random(rng: &SystemRandom, len: usize) -> Option<String> {
    let mut bytes = Zeroizing::new(vec![0; len]);
    rng.fill(&mut bytes).ok()?;
    Some(URL_SAFE_NO_PAD.encode(&*bytes))
}

/// The S256 challenge of `verifier`: its SHA-256 in base64url.
pub(crate) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()))
}

/// A random port in the dynamic range for the browser to come back to.
pub(crate) fn callback_port(rng: &SystemRandom) -> Option<u16> {
    let mut port = [0; 2];
    rng.fill(&mut port).ok()?;
    Some(FIRST_PORT + u16::from_be_bytes(port) % (u16::MAX - FIRST_PORT + 1))
}

/// The rounds waiting for their code.
pub(crate) struct Rounds<T> {
    rounds: Mutex<HashMap<SignInId, (T, Instant)>>,
}

impl<T> Default for Rounds<T> {
    fn default() -> Self {
        Self {
            rounds: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> Rounds<T> {
    fn lock(&self) -> MutexGuard<'_, HashMap<SignInId, (T, Instant)>> {
        self.rounds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `round` for [`ROUND_LIMIT`] and names it. Rounds that ended
    /// go, and so does the oldest while too many wait.
    pub(crate) fn start(&self, round: T) -> SignInId {
        let sign_in = SignInId::generate();
        let now = Instant::now();
        let mut rounds = self.lock();
        rounds.retain(|_, (_, until)| *until > now);
        while rounds.len() >= MAX_ROUNDS {
            let oldest = rounds
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(id, _)| *id);
            oldest.map(|id| rounds.remove(&id));
        }
        rounds.insert(sign_in, (round, now + ROUND_LIMIT));
        sign_in
    }

    /// Takes the round, once, while it lasts.
    pub(crate) fn take(&self, sign_in: SignInId) -> Option<T> {
        let (round, until) = self.lock().remove(&sign_in)?;
        (until > Instant::now()).then_some(round)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_is_taken_once() {
        let rounds = Rounds::default();
        let sign_in = rounds.start("verifier");
        assert_eq!(rounds.take(sign_in), Some("verifier"));
        assert_eq!(rounds.take(sign_in), None);
        assert_eq!(rounds.take(SignInId::generate()), None);
    }

    #[test]
    fn too_many_rounds_end_the_oldest() {
        let rounds = Rounds::default();
        let first = rounds.start(0);
        std::thread::sleep(Duration::from_millis(2));
        let later: Vec<_> = (1..=MAX_ROUNDS).map(|n| rounds.start(n)).collect();
        assert_eq!(rounds.take(first), None);
        for (n, sign_in) in later.into_iter().enumerate() {
            assert_eq!(rounds.take(sign_in), Some(n + 1));
        }
    }

    #[test]
    fn the_challenge_is_the_verifiers_sha256() {
        // SHA-256 of "abc" is ba7816bf...f20015ad, here in base64url
        // without padding.
        assert_eq!(
            challenge("abc"),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
    }

    #[test]
    fn a_callback_port_is_in_the_dynamic_range() {
        let rng = SystemRandom::new();
        for _ in 0..100 {
            assert!(callback_port(&rng).unwrap() >= FIRST_PORT);
        }
        assert_eq!(random(&rng, 32).unwrap().len(), 43);
    }
}

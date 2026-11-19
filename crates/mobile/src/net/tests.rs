// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::time::Duration;

use maxhq_client::PhoneSsh;
use maxhq_core::dpop::{self, Expected};
use maxhq_protocol::ids::{PairId, PhoneId, PhoneSessionId};
use maxhq_protocol::phones::{PhoneBookEntry, PhoneMac};
use serde_json::{json, Value};

use super::*;
use crate::keys::MemoryStore;

/// The phone's SSH key, which signs its proofs.
const SEED: [u8; 32] = [5; 32];

/// The token the phone holds, and the ones the fake device gives.
const HELD: &str = "held-token-0123456789";
const FRESH: &str = "fresh-token-9876543210";
const CODE: &str = "one-time-code-abcdef";

/// One request the fake device took.
struct Asked {
    path: &'static str,
    url: String,
    headers: Vec<(String, String)>,
    body: Value,
}

type Seen = Arc<Mutex<Vec<Asked>>>;

/// A device that answers each POST with the next of `answers`, asking for
/// the headers as `PhoneWs::post` does, and keeps what it was asked. Past
/// its answers it can't be reached.
fn device(answers: Vec<Result<(u16, Value), &'static str>>) -> (Post, Seen) {
    let seen = Seen::default();
    let asked = Arc::clone(&seen);
    let answers = Arc::new(Mutex::new(answers.into_iter()));
    let post: Post = Arc::new(move |route, path, headers, body| {
        let asked = Arc::clone(&asked);
        let answers = Arc::clone(&answers);
        Box::pin(async move {
            let url = format!("https://{}:{}{path}", route.host, route.port);
            let headers = headers("POST", url.clone()).await?;
            asked.lock().unwrap().push(Asked {
                path,
                url,
                headers,
                body: serde_json::from_slice(&body).unwrap(),
            });
            let next = answers.lock().unwrap().next();
            let (status, body) = next.unwrap_or(Err("the device didn't answer"))?;
            Ok(Posted {
                status,
                body: serde_json::to_vec(&body).unwrap(),
            })
        })
    });
    (post, seen)
}

fn token(token: &str, days: u64) -> Value {
    json!(NetToken {
        token: Secret::new(token.to_owned()),
        session: PhoneSessionId::derive("session"),
        expires_at: Timestamp::now().after(Duration::from_secs(days * 24 * 60 * 60)),
    })
}

fn refused(error: NetError) -> Result<(u16, Value), &'static str> {
    Ok((error.status(), json!(NetRefusal { error })))
}

fn signing(post: Post) -> (Arc<SignIns>, Arc<dyn KeyStore>) {
    let store: Arc<dyn KeyStore> = Arc::new(MemoryStore::default());
    let sign_ins = Arc::new(SignIns::new(Arc::clone(&store), Zeroizing::new(SEED), post));
    (sign_ins, store)
}

/// Keeps `token` for `device`, with `days` left on it.
fn holding(store: &dyn KeyStore, token: &str, days: u64) {
    let token = Token {
        token: Zeroizing::new(token.to_owned()),
        expires: Timestamp::now().after(Duration::from_secs(days * 24 * 60 * 60)),
    };
    keys::save_token(store, devbox(), &token).unwrap();
}

fn held(store: &dyn KeyStore) -> Option<String> {
    keys::token(store, devbox())
        .unwrap()
        .map(|token| token.token.to_string())
}

fn devbox() -> DeviceId {
    DeviceId::derive("devbox")
}

fn route() -> WsRoute {
    WsRoute {
        host: "192.168.1.20".into(),
        port: 7466,
        pin: URL_SAFE_NO_PAD.encode([7; 32]),
    }
}

/// The thumbprint of the phone's key, as a device checks it.
fn phones_thumbprint() -> String {
    let line = PhoneSsh::new(&SEED).public_key();
    let blob = STANDARD
        .decode(line.split_whitespace().nth(1).unwrap())
        .unwrap();
    dpop::thumbprint(&blob[blob.len() - 32..].try_into().unwrap())
}

/// Checks `headers` carry a proof for `method` on `url` from the phone's
/// key, naming `token` when one rides, and the token.
fn assert_signed(headers: &[(String, String)], method: &str, url: &str, token: Option<&str>) {
    let get = |name: &str| {
        headers
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, value)| value.as_str())
    };
    let thumbprint = phones_thumbprint();
    let pin = route().pin;
    let expected = Expected {
        method,
        url,
        now: i64::try_from(Timestamp::now().0 / 1000).unwrap(),
        thumbprint: Some(&thumbprint),
        access_token: token,
        pin: Some(&pin),
    };
    dpop::verify(get("DPoP").unwrap(), &expected).unwrap();
    let elsewhere = Expected {
        pin: Some("another listener's pin"),
        ..expected
    };
    assert_eq!(
        dpop::verify(get("DPoP").unwrap(), &elsewhere),
        Err(dpop::Failure::PinMismatch)
    );
    assert_eq!(
        get("Authorization"),
        token.map(|token| format!("DPoP {token}")).as_deref()
    );
}

#[tokio::test]
async fn the_link_carries_the_token_and_a_proof_for_the_url_it_names() {
    let (post, seen) = device(Vec::new());
    let (sign_ins, store) = signing(post);
    holding(&*store, HELD, 20);
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), None);
    let url = "wss://192.168.1.20:7466/link";
    let first = headers("GET", url.to_owned()).await.unwrap();
    assert_signed(&first, "GET", url, Some(HELD));
    // Each request gets a proof of its own.
    let second = headers("GET", url.to_owned()).await.unwrap();
    assert_ne!(first, second);
    assert!(seen.lock().unwrap().is_empty(), "nothing to refresh");
}

#[tokio::test]
async fn the_first_link_trades_the_code_and_keeps_the_token() {
    let (post, seen) = device(vec![Ok((200, token(FRESH, 30)))]);
    let (sign_ins, store) = signing(post);
    let code = Secret::new(CODE.to_owned());
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), Some(&code));
    let url = "wss://192.168.1.20:7466/link";
    let link = headers("GET", url.to_owned()).await.unwrap();
    assert_signed(&link, "GET", url, Some(FRESH));
    assert_eq!(held(&*store).as_deref(), Some(FRESH));
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, TOKEN_PATH);
        assert_eq!(seen[0].body, json!({ "grant": "code", "code": CODE }));
        assert_signed(&seen[0].headers, "POST", &seen[0].url, None);
    }

    // The token stays, so the code isn't traded again.
    headers("GET", url.to_owned()).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_token_with_under_15_days_left_is_refreshed_on_connect() {
    let (post, seen) = device(vec![Ok((200, token(FRESH, 30)))]);
    let (sign_ins, store) = signing(post);
    holding(&*store, HELD, 14);
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), None);
    let url = "wss://192.168.1.20:7466/link";
    let link = headers("GET", url.to_owned()).await.unwrap();
    assert_signed(&link, "GET", url, Some(FRESH));
    assert_eq!(held(&*store).as_deref(), Some(FRESH));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].body, json!({ "grant": "refresh" }));
    assert_eq!(seen[0].url, "https://192.168.1.20:7466/token");
    assert_signed(&seen[0].headers, "POST", &seen[0].url, Some(HELD));
}

#[tokio::test]
async fn a_refresh_that_cant_reach_the_device_keeps_the_token_while_it_lasts() {
    let (post, _) = device(vec![Err("the device didn't answer")]);
    let (sign_ins, store) = signing(post);
    holding(&*store, HELD, 3);
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), None);
    let link = headers("GET", "wss://192.168.1.20:7466/link".into()).await;
    assert_eq!(link.unwrap()[1].1, format!("DPoP {HELD}"));
    assert_eq!(held(&*store).as_deref(), Some(HELD));
}

#[tokio::test]
async fn an_ended_sign_in_forgets_the_token_and_says_pair_again() {
    for error in [NetError::Expired, NetError::Revoked] {
        let (post, _) = device(vec![refused(error)]);
        let (sign_ins, store) = signing(post);
        holding(&*store, HELD, 1);
        let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), None);
        let link = headers("GET", "wss://192.168.1.20:7466/link".into()).await;
        assert_eq!(link.unwrap_err(), SIGN_IN_ENDED, "{error:?}");
        assert_eq!(held(&*store), None, "{error:?}");
    }

    // A used code, or none, can't sign the phone in either.
    let (post, _) = device(vec![refused(NetError::BadCode)]);
    let (sign_ins, store) = signing(post);
    let code = Secret::new(CODE.to_owned());
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), Some(&code));
    let link = headers("GET", "wss://192.168.1.20:7466/link".into()).await;
    assert_eq!(link.unwrap_err(), SIGN_IN_ENDED);
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), None);
    let link = headers("GET", "wss://192.168.1.20:7466/link".into()).await;
    assert_eq!(link.unwrap_err(), SIGN_IN_ENDED);
    assert_eq!(held(&*store), None);

    // A code that ran out says to make a new one, not to pair again.
    let (post, _) = device(vec![refused(NetError::Expired)]);
    let (sign_ins, store) = signing(post);
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), Some(&code));
    let link = headers("GET", "wss://192.168.1.20:7466/link".into()).await;
    assert_eq!(link.unwrap_err(), code_ran_out("Home Mac"));
    assert_eq!(held(&*store), None);

    // The link's refusal forgets it too.
    holding(&*store, HELD, 20);
    let (_, ended) = sign_ins.route(devbox(), "Home Mac", &route(), None);
    ended();
    assert_eq!(held(&*store), None);
}

fn code(network: Option<PhoneNetwork>) -> PairCode {
    PairCode {
        pair: PairId::derive("pairing"),
        seed: Zeroizing::new([9; 32]),
        device: devbox(),
        host_key: String::new(),
        routes: Vec::new(),
        network,
    }
}

fn phone_key() -> PhoneKey {
    let public_key = PhoneSsh::new(&SEED).public_key();
    PhoneKey {
        phone: PhoneId::derive(&public_key),
        line: String::new(),
        push_public: STANDARD.encode([3; 32]),
        public_key,
    }
}

#[tokio::test]
async fn pairing_posts_to_the_devices_network_access_and_keeps_the_token() {
    let key = phone_key();
    let entry = PhoneBookEntry {
        device: devbox(),
        name: "devbox".into(),
        routes: Vec::new(),
        host_key: String::new(),
        network: None,
    };
    let home = PhoneMac {
        device: DeviceId::derive("Home Mac"),
        name: "Home Mac".into(),
    };
    let paired = json!(NetPaired {
        phone: key.phone,
        book: vec![entry.clone()],
        token: serde_json::from_value(token(FRESH, 30)).unwrap(),
        mac: Some(home.clone()),
    });
    // The first host doesn't answer, so the second pairs.
    let (post, seen) = device(vec![Err("the device didn't answer"), Ok((200, paired))]);
    let (sign_ins, store) = signing(post);
    let network = PhoneNetwork {
        hosts: vec!["192.168.1.20".into(), "100.64.0.7".into()],
        port: 7466,
        pin: URL_SAFE_NO_PAD.encode([7; 32]),
        code: None,
    };
    let code = code(Some(network.clone()));
    let book = sign_ins
        .pair(&code, &network, "iPhone", &key)
        .await
        .unwrap();
    assert_eq!(book.phone, key.phone);
    assert_eq!(book.device, devbox());
    assert_eq!(book.devices, [entry]);
    assert_eq!(
        book.mac,
        Some(home),
        "the book names the Mac that paired it"
    );
    assert_eq!(held(&*store).as_deref(), Some(FRESH));
    let seen = seen.lock().unwrap();
    let urls: Vec<&str> = seen.iter().map(|asked| asked.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://192.168.1.20:7466/pair",
            "https://100.64.0.7:7466/pair"
        ]
    );
    assert_eq!(seen[1].path, PAIR_PATH);
    assert_signed(&seen[1].headers, "POST", &seen[1].url, None);
    assert_eq!(
        seen[1].body,
        json!({
            "pair": code.pair,
            "seed": STANDARD.encode([9; 32]),
            "name": "iPhone",
            "ssh_public": key.public_key,
            "push_public": key.push_public,
        })
    );
}

#[tokio::test]
async fn a_used_pairing_code_says_to_scan_a_new_one() {
    let (post, _) = device(vec![refused(NetError::BadCode)]);
    let (sign_ins, store) = signing(post);
    let network = PhoneNetwork {
        hosts: vec!["192.168.1.20".into()],
        port: 7466,
        pin: URL_SAFE_NO_PAD.encode([7; 32]),
        code: None,
    };
    let refused = sign_ins
        .pair(
            &code(Some(network.clone())),
            &network,
            "iPhone",
            &phone_key(),
        )
        .await;
    assert_eq!(
        refused.unwrap_err(),
        "That pairing code is used or over. Scan a new one."
    );
    assert_eq!(held(&*store), None);
}

/// Everything written to the log, shared with the subscriber.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn signing_in_logs_no_token_code_seed_or_proof() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    // A trade, a refresh, and a refresh the device refuses.
    let (post, seen) = device(vec![
        Ok((200, token(HELD, 1))),
        Ok((200, token(FRESH, 1))),
        refused(NetError::Revoked),
    ]);
    let (sign_ins, _) = signing(post);
    let code = Secret::new(CODE.to_owned());
    let (headers, _) = sign_ins.route(devbox(), "Home Mac", &route(), Some(&code));
    let mut proofs = Vec::new();
    for _ in 0..3 {
        if let Ok(signed) = headers("GET", "wss://192.168.1.20:7466/link".into()).await {
            proofs.extend(signed.into_iter().map(|(_, value)| value));
        }
    }
    assert_eq!(proofs.len(), 4, "two links signed");
    for asked in seen.lock().unwrap().iter() {
        proofs.extend(asked.headers.iter().map(|(_, value)| value.clone()));
    }
    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    let seed = STANDARD.encode(SEED);
    for secret in [HELD, FRESH, CODE, seed.as_str()]
        .into_iter()
        .chain(proofs.iter().map(String::as_str))
    {
        assert!(!text.contains(secret), "the log holds a secret");
    }
}

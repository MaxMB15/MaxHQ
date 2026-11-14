// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Helpers for the store's tests: a hub, a clock the test moves, and keys
//! made for these tests alone. No key here was ever a GitHub App's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::ids::{AccountId, BootId, DeviceId};
use maxhq_protocol::model::{Account, Credential, Device, DeviceSettings};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;

use super::{Now, Secrets};
use crate::hub::Hub;
use crate::observe::claude::time::parse_iso8601;

/// A setup-token's shape, with nothing real in it.
pub const CLAUDE_TOKEN: &str = "sk-ant-oat01-test-not-a-real-token-AAAAAAAAAAAAAAAAAAAA";

/// The sign-in server for tests that never sign in to ChatGPT. Nothing
/// listens there.
pub const UNUSED_ISSUER: &str = "http://127.0.0.1:9";

/// The hex SHA-256 of [`RSA_PKCS1`]'s public half, as
/// `openssl rsa -pubout -outform DER | openssl sha256` gives it.
pub const RSA_SHA256: &str = "847dcde3a78a68e4dbebb7063a52138e714bbdce472c77a90358f6c0a3a6dbc2";

/// A PEM block with `label` around `body`. The blocks are put together
/// here so the source holds no line a secret scanner takes for a key.
pub fn pem(label: &str, body: &str) -> String {
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
}

/// The test App's key, as GitHub gives it.
pub fn app_key() -> String {
    pem("RSA PRIVATE KEY", RSA_PKCS1)
}

/// The account the store shows as `id`.
pub fn account(secrets: &Secrets, id: AccountId) -> Option<Account> {
    match secrets.inner.hub.get(&EntityKey::Account(id)) {
        Some(Entity::Account(account)) => Some(account),
        _ => None,
    }
}

/// Every credential the store shows.
pub fn credentials(secrets: &Secrets) -> Vec<Credential> {
    secrets
        .inner
        .hub
        .snapshot()
        .entities
        .into_iter()
        .filter_map(|entity| match entity {
            Entity::Credential(credential) => Some(credential),
            _ => None,
        })
        .collect()
}

/// A hub for a device of its own.
pub fn hub() -> Hub {
    hub_for("host")
}

/// A hub for a device of its own that takes Claude setup-tokens.
pub fn token_hub() -> Hub {
    let hub = hub();
    let Some(Entity::DeviceSettings(settings)) =
        hub.get(&EntityKey::DeviceSettings(hub.device().id))
    else {
        panic!("a new hub has settings");
    };
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(DeviceSettings {
            claude_setup_token: true,
            ..settings
        }),
    }]);
    hub
}

/// A hub for the device called `name`.
pub fn hub_for(name: &str) -> Hub {
    Hub::new(
        Device {
            id: DeviceId::derive(name),
            name: name.into(),
            os: "linux".into(),
            boot_id: BootId("b".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
        },
        HashMap::new(),
    )
}

/// A clock that moves only when the test moves it.
#[derive(Clone)]
pub struct Clock(Arc<AtomicU64>);

impl Clock {
    /// A clock showing `time`, in ISO 8601.
    pub fn at(time: &str) -> Self {
        Self(Arc::new(AtomicU64::new(at(time).0)))
    }

    /// The store's view of it.
    pub fn now(&self) -> Now {
        let clock = self.0.clone();
        Arc::new(move || Timestamp(clock.load(Ordering::SeqCst)))
    }

    /// Moves it to `time`.
    pub fn set(&self, time: Timestamp) {
        self.0.store(time.0, Ordering::SeqCst);
    }

    /// Moves it on by `by`.
    pub fn advance(&self, by: Duration) {
        let millis = u64::try_from(by.as_millis()).unwrap();
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

/// `time`, in ISO 8601, as a timestamp.
pub fn at(time: &str) -> Timestamp {
    parse_iso8601(time).expect("a time in ISO 8601")
}

/// A throwaway 2048-bit RSA key in PKCS #1, as GitHub gives App keys.
pub const RSA_PKCS1: &str = concat!(
    "MIIEowIBAAKCAQEAsOqmkR87uVmdGDQs0Trd2e7JUko37f7p5hCN9M6SjBfTbyj0",
    "DjSnfuRbhgLSrwxswjcdwYKwyeGDk9nNvn0g8dezvPcOmT9KPgEia8Qi9mUOPjws",
    "LsvI2l4nHKIR8B0cWdUWNurecPwBpa7/kZ5GBY/e0eCzyPE++Osr7toFnrra7V73",
    "KTGrOhokAV3A/qeZkoEpGxUalNLWy4KjdrzZZ4zPD1Jeutx9yLiD9wtwa2Ji8yQD",
    "6SeCNAmfMJxANKDVu9h2WFajhdl9cwSy3mT2t2OeKarGRFuCm0xunxnYeQGm0l/S",
    "yJ67GIZHdiaAGYu89yl4Vtkacqecdsxyhn1TWwIDAQABAoIBAHBndhMIXCLke/pP",
    "1BXPp/r6SzEcdLKi3qtM41SiFtwpmy2qQH3yGGV342qeyotrgoJXEHHNflOWE8pR",
    "fPZFfDQMD2fFNKJLYvlIMT11LZ2NEWKbSpsk4OVVItZ/ZpQ/eq7cwzN7QH9arhU9",
    "vaUggW6xPSnt8Wx/UrpGePBV13JESYWOeaMc0cmtP8I5Zyciiwd3Wl8XiLGLn4VG",
    "F4gbWVegK9QdELWvxngBD3BzXkcp1ZdX//E+bQXo+2AjUn/R/5HkGrDUakr1Uiai",
    "ORUv6la4GaBaSgpvl5LnA/ck4clYWYH2y9pjUc0SxlyIog38O/5A5Cn+8Ft8fT7H",
    "OP3eWdkCgYEA3Xso5BzsmvRkiEb4zzVOt/77LRkaifBPpFXfT/lGFpNW6peOZIDR",
    "mgeBsNjWxRDvUIxF2c5GQ69x1WH93MPC/5PVtxWHdirUpvnrIoQswO/IVh5wIlrD",
    "HwCbeNjwpT9RcSXcb5/ewvGAQddA323R+1iTjaOTf3GkRMdEJLZCMVcCgYEAzH1r",
    "UMuwAQDpgfuxLapgAXqF3lvjj2mbE2siIBjYWlrQP+NYZ9FcwBwJWcnhPEKmSwZK",
    "RUWZwkHJrzQgGlhv6Z8uVu9AAx2s+yP3DnJy/Ri9eAInru55E6uFZUMtTrS0V5KN",
    "3WNYoARrJ72bT+fiAHZQ/oW7f7p7svPKrhkT150CgYEAg30m3aa+YZBOTwmzlXDU",
    "dzAN9sw8kT/2SF4i4dvf3G9E3qlzg/9mcMecG13gw8Y2Dd/5aEGqPWzfbAttJ6+h",
    "CRK/gp5peaIb9bXR/v/xgenZSidv8zVnOfSmWKyS0cgxlbcgk7SaD6pIts4QWZVd",
    "YJdJooFGhBb3q02SCymZSqkCgYBHaABibU4fe5cOubosDROLUKKQfBvhh2oMhNuc",
    "w7BBFPclaBkF37j4qJGnVfSd21RAXIjw2ZYQnHJ2CMixCnLB7MD8wHzk2ab+55cs",
    "51A6CzqWPoJm9oU3Fiz1qFiY5EcZUWbSiQ94iTJ15SLn6zwjFy2/QymvN4pcef8r",
    "fjxMjQKBgAVUf8Ukg/TxNLFHPfbu98D1YYkdvVbxofuf1pZM6gd5xLLAQimhWujl",
    "ZCg3hbo2lE2wJHPZvV+qk2DH9EbtoJpFOxF1JKDy7bxIRplwbwwN0CoKL9RY0dB1",
    "ikkmNhcMo2fI+0TIV1hYdKZOw1+lWS0DyUNDu+gqjG/W+tzOYWbg",
);

/// The same key in PKCS #8.
pub const RSA_PKCS8: &str = concat!(
    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCw6qaRHzu5WZ0Y",
    "NCzROt3Z7slSSjft/unmEI30zpKMF9NvKPQONKd+5FuGAtKvDGzCNx3BgrDJ4YOT",
    "2c2+fSDx17O89w6ZP0o+ASJrxCL2ZQ4+PCwuy8jaXiccohHwHRxZ1RY26t5w/AGl",
    "rv+RnkYFj97R4LPI8T746yvu2gWeutrtXvcpMas6GiQBXcD+p5mSgSkbFRqU0tbL",
    "gqN2vNlnjM8PUl663H3IuIP3C3BrYmLzJAPpJ4I0CZ8wnEA0oNW72HZYVqOF2X1z",
    "BLLeZPa3Y54pqsZEW4KbTG6fGdh5AabSX9LInrsYhkd2JoAZi7z3KXhW2Rpyp5x2",
    "zHKGfVNbAgMBAAECggEAcGd2EwhcIuR7+k/UFc+n+vpLMRx0sqLeq0zjVKIW3Cmb",
    "LapAffIYZXfjap7Ki2uCglcQcc1+U5YTylF89kV8NAwPZ8U0okti+UgxPXUtnY0R",
    "YptKmyTg5VUi1n9mlD96rtzDM3tAf1quFT29pSCBbrE9Ke3xbH9SukZ48FXXckRJ",
    "hY55oxzRya0/wjlnJyKLB3daXxeIsYufhUYXiBtZV6Ar1B0Qta/GeAEPcHNeRynV",
    "l1f/8T5tBej7YCNSf9H/keQasNRqSvVSJqI5FS/qVrgZoFpKCm+XkucD9yThyVhZ",
    "gfbL2mNRzRLGXIiiDfw7/kDkKf7wW3x9Psc4/d5Z2QKBgQDdeyjkHOya9GSIRvjP",
    "NU63/vstGRqJ8E+kVd9P+UYWk1bql45kgNGaB4Gw2NbFEO9QjEXZzkZDr3HVYf3c",
    "w8L/k9W3FYd2KtSm+esihCzA78hWHnAiWsMfAJt42PClP1FxJdxvn97C8YBB10Df",
    "bdH7WJONo5N/caREx0QktkIxVwKBgQDMfWtQy7ABAOmB+7EtqmABeoXeW+OPaZsT",
    "ayIgGNhaWtA/41hn0VzAHAlZyeE8QqZLBkpFRZnCQcmvNCAaWG/pny5W70ADHaz7",
    "I/cOcnL9GL14Aieu7nkTq4VlQy1OtLRXko3dY1igBGsnvZtP5+IAdlD+hbt/unuy",
    "88quGRPXnQKBgQCDfSbdpr5hkE5PCbOVcNR3MA32zDyRP/ZIXiLh29/cb0TeqXOD",
    "/2Zwx5wbXeDDxjYN3/loQao9bN9sC20nr6EJEr+Cnml5ohv1tdH+//GB6dlKJ2/z",
    "NWc59KZYrJLRyDGVtyCTtJoPqki2zhBZlV1gl0migUaEFverTZILKZlKqQKBgEdo",
    "AGJtTh97lw65uiwNE4tQopB8G+GHagyE25zDsEEU9yVoGQXfuPiokadV9J3bVEBc",
    "iPDZlhCccnYIyLEKcsHswPzAfOTZpv7nlyznUDoLOpY+gmb2hTcWLPWoWJjkRxlR",
    "ZtKJD3iJMnXlIufrPCMXLb9DKa83ilx5/yt+PEyNAoGABVR/xSSD9PE0sUc99u73",
    "wPVhiR29VvGh+5/WlkzqB3nEssBCKaFa6OVkKDeFujaUTbAkc9m9X6qTYMf0Ru2g",
    "mkU7EXUkoPLtvEhGmXBvDA3QKgov1FjR0HWKSSY2FwyjZ8j7RMhXWFh0pk7DX6VZ",
    "LQPJQ0O76CqMb9b63M5hZuA=",
);

/// A throwaway P-256 key in PKCS #8, which no GitHub App uses.
pub const EC_PKCS8: &str = concat!(
    "MIIBeQIBADCCAQMGByqGSM49AgEwgfcCAQEwLAYHKoZIzj0BAQIhAP////8AAAAB",
    "AAAAAAAAAAAAAAAA////////////////MFsEIP////8AAAABAAAAAAAAAAAAAAAA",
    "///////////////8BCBaxjXYqjqT57PrvVV2mIa8ZR0GsMxTsPY7zjw+J9JgSwMV",
    "AMSdNgiG5wSTamZ44ROdJreBn36QBEEEaxfR8uEsQkf4vOblY6RA8ncDfYEt6zOg",
    "9KE5RdiYwpZP40Li/hp/m47n60p8D54WK84zV2sxXs7LtkBoN79R9QIhAP////8A",
    "AAAA//////////+85vqtpxeehPO5ysL8YyVRAgEBBG0wawIBAQQghY4RqnVwULEP",
    "pDSdhd+S6rNncAoczsVFcamVBTY9NwShRANCAAR0d4mX7lk7PSdy9T9l9d8/izpV",
    "RjulBb+BnsZjE85Zwgyw+E0RYfK/p9y17GBwG7NqgBmjX83x1EGglhkxar/W",
);

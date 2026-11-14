// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's sign-in to each device's network access. It pairs through
//! `POST /pair`, keeps the token each device gives it in the Keychain,
//! trades a device's one-time code for one at `POST /token` and refreshes
//! it there, and signs every request with a new DPoP proof from the
//! phone's own key. Nothing here logs a token, code, seed or proof.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures::future::BoxFuture;
use maxhq_app::pair_code::PairCode;
use maxhq_app::phone::ws_routes;
use maxhq_client::{Ended, Headers, PhoneWs, Posted, WsRoute, SIGN_IN_ENDED};
use maxhq_core::dpop;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::netaccess::{
    NetError, NetGrant, NetPair, NetPaired, NetRefusal, NetToken, PhoneNetwork, PAIR_PATH,
    REFRESH_WITHIN, TOKEN_PATH,
};
use maxhq_protocol::phones::PhoneBook;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use crate::keys::{self, KeyStore, Token};
use crate::phone::PhoneKey;

/// Why the phone can't sign in to a device whose one-time code ran out
/// before the phone traded it. The Mac makes a new one when asked.
pub(crate) const CODE_RAN_OUT: &str =
    "the code for this device ran out, so make a new one in the Mac's Phones settings";

/// Posts a JSON body to a path on a route and answers what came back:
/// [`PhoneWs::post`] on the phone, a fake in tests.
pub(crate) type Post = Arc<
    dyn Fn(WsRoute, &'static str, Headers, Vec<u8>) -> BoxFuture<'static, Result<Posted, String>>
        + Send
        + Sync,
>;

/// Posts through `ws`.
pub(crate) fn posting(ws: PhoneWs) -> Post {
    Arc::new(move |route, path, headers, body| {
        let ws = ws.clone();
        Box::pin(async move { ws.post(&route, path, &headers, body).await })
    })
}

/// The phone's sign-ins, one per device.
pub(crate) struct SignIns {
    store: Arc<dyn KeyStore>,
    seed: Zeroizing<[u8; 32]>,
    post: Post,
    /// Each device's turn, so two dials never trade or refresh its token
    /// at once: the second would find the first's token refreshed away.
    turns: Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
}

impl SignIns {
    /// Signs with the phone's SSH key `seed`, keeps tokens in `store`, and
    /// reaches devices through `post`.
    pub(crate) fn new(store: Arc<dyn KeyStore>, seed: Zeroizing<[u8; 32]>, post: Post) -> Self {
        Self {
            store,
            seed,
            post,
            turns: Mutex::default(),
        }
    }

    /// Pairs through `network`, the network access in `code`, trying each
    /// of its hosts in turn. Keeps the token the device answers, and
    /// answers the book. The error is a sentence for the phone to show.
    pub(crate) async fn pair(
        self: &Arc<Self>,
        code: &PairCode,
        network: &PhoneNetwork,
        name: &str,
        key: &PhoneKey,
    ) -> Result<PhoneBook, String> {
        let pairing = NetPair {
            pair: code.pair,
            seed: Secret::new(STANDARD.encode(code.seed.as_slice())),
            name: name.to_owned(),
            ssh_public: key.public_key.clone(),
            push_public: key.push_public.clone(),
        };
        let body = Zeroizing::new(json(&pairing)?);
        let mut reason = "the pairing code has no address".to_owned();
        for route in ws_routes(network) {
            let headers = self.headers(None, &route.pin);
            let posted = (self.post)(route, PAIR_PATH, headers, body.to_vec()).await;
            let paired: NetPaired = match posted.map(read) {
                Ok(Ok(paired)) => paired,
                Ok(Err(Some(NetError::BadCode))) => {
                    return Err("That pairing code is used or over. Scan a new one.".to_owned());
                }
                Ok(Err(_)) => {
                    return Err("The device didn't pair the phone. Scan a new code.".to_owned())
                }
                Err(failed) => {
                    reason = failed;
                    continue;
                }
            };
            if paired.phone != key.phone {
                return Err(
                    "The device knows this phone by another name. Scan a new code.".to_owned(),
                );
            }
            keep(&*self.store, code.device, &paired.token)?;
            return Ok(PhoneBook {
                phone: paired.phone,
                device: code.device,
                devices: paired.book,
            });
        }
        Err(format!("Couldn't reach the device: {reason}"))
    }

    /// The headers for each request on `route`, one of `device`'s, and
    /// what forgets its token when the device ends the sign-in. `code` is
    /// the device's one-time code while the book has one.
    pub(crate) fn route(
        self: &Arc<Self>,
        device: DeviceId,
        route: &WsRoute,
        code: Option<&Secret>,
    ) -> (Headers, Ended) {
        let me = Arc::clone(self);
        let route = route.clone();
        let code = code.cloned();
        let headers: Headers = Arc::new(move |method, url| {
            let me = Arc::clone(&me);
            let route = route.clone();
            let code = code.clone();
            Box::pin(async move {
                let token = me.token(device, &route, code.as_ref()).await?;
                me.signed(method, &url, Some(&token), &route.pin)
            })
        });
        let store = Arc::clone(&self.store);
        let ended: Ended = Arc::new(move || {
            // Gone either way: the device takes it no more.
            let _ = keys::forget_token(&*store, device);
        });
        (headers, ended)
    }

    /// `device`'s token, traded for with `code` the first time and
    /// refreshed on `route` once less than [`REFRESH_WITHIN`] is left. A
    /// device that ended the sign-in gets its token forgotten, and the
    /// error says to pair again, or to make a new code when the code ran
    /// out first.
    async fn token(
        self: &Arc<Self>,
        device: DeviceId,
        route: &WsRoute,
        code: Option<&Secret>,
    ) -> Result<Zeroizing<String>, String> {
        let turn = Arc::clone(
            self.turns
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(device)
                .or_default(),
        );
        let _turn = turn.lock().await;
        let now = Timestamp::now();
        let kept = keys::token(&*self.store, device)?;
        let grant = match (&kept, code) {
            (Some(kept), _) if now.after(REFRESH_WITHIN) < kept.expires => {
                return Ok(kept.token.clone());
            }
            (Some(_), _) => NetGrant::Refresh,
            (None, Some(code)) => NetGrant::Code { code: code.clone() },
            (None, None) => return Err(SIGN_IN_ENDED.to_owned()),
        };
        let by_code = matches!(grant, NetGrant::Code { .. });
        let riding = kept.as_ref().map(|kept| kept.token.clone());
        let posted = (self.post)(
            route.clone(),
            TOKEN_PATH,
            self.headers(riding, &route.pin),
            json(&grant)?,
        )
        .await;
        let failed = match posted.map(read::<NetToken>) {
            Ok(Ok(token)) => return keep(&*self.store, device, &token),
            Ok(Err(Some(NetError::Expired))) if by_code => return Err(CODE_RAN_OUT.to_owned()),
            Ok(Err(Some(error))) if ends(error) => {
                keys::forget_token(&*self.store, device)?;
                return Err(SIGN_IN_ENDED.to_owned());
            }
            Ok(Err(_)) => "the device didn't give the phone a token".to_owned(),
            Err(reason) => reason,
        };
        // Not refreshed now, so it goes as it is while it lasts.
        match kept {
            Some(kept) if now < kept.expires => Ok(kept.token),
            _ => Err(failed),
        }
    }

    /// Headers that sign each request for the listener that shows `pin`,
    /// with `token` riding when there is one.
    fn headers(self: &Arc<Self>, token: Option<Zeroizing<String>>, pin: &str) -> Headers {
        let me = Arc::clone(self);
        let pin = pin.to_owned();
        Arc::new(move |method, url| {
            let signed = me.signed(method, &url, token.as_deref().map(String::as_str), &pin);
            Box::pin(async move { signed })
        })
    }

    /// A new proof for `method` on `url`, naming `token` when it rides and
    /// `pin`, the certificate the connection checks, and the token.
    fn signed(
        &self,
        method: &str,
        url: &str,
        token: Option<&str>,
        pin: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let mut jti = [0u8; 16];
        getrandom::fill(&mut jti).map_err(|_| "the phone couldn't sign the request".to_owned())?;
        let now = i64::try_from(Timestamp::now().0 / 1000).unwrap_or(i64::MAX);
        let proof = dpop::sign_for_pin(
            &self.seed,
            method,
            url,
            token,
            Some(pin),
            &URL_SAFE_NO_PAD.encode(jti),
            now,
        )
        .ok_or_else(|| "the phone couldn't sign the request".to_owned())?;
        let mut headers = vec![("DPoP".to_owned(), proof)];
        if let Some(token) = token {
            headers.push(("Authorization".to_owned(), format!("DPoP {token}")));
        }
        Ok(headers)
    }
}

/// Whether a refusal means the device ended the phone's sign-in, so only
/// pairing again brings it back.
fn ends(error: NetError) -> bool {
    matches!(
        error,
        NetError::Expired | NetError::Revoked | NetError::BadCode | NetError::NotPaired
    )
}

/// Keeps `token` as `device`'s and answers it.
fn keep(
    store: &dyn KeyStore,
    device: DeviceId,
    token: &NetToken,
) -> Result<Zeroizing<String>, String> {
    let token = Token {
        token: Zeroizing::new(token.token.expose().to_owned()),
        expires: token.expires_at,
    };
    keys::save_token(store, device, &token)?;
    Ok(token.token)
}

/// What a device answered: the body as `T`, or its refusal, which is
/// None when it doesn't read.
fn read<T: DeserializeOwned>(posted: Posted) -> Result<T, Option<NetError>> {
    if posted.status == 200 {
        serde_json::from_slice(&posted.body).map_err(|_| None)
    } else {
        Err(serde_json::from_slice::<NetRefusal>(&posted.body)
            .ok()
            .map(|refusal| refusal.error))
    }
}

fn json(body: &impl serde::Serialize) -> Result<Vec<u8>, String> {
    serde_json::to_vec(body).map_err(|_| "the phone couldn't write the request".to_owned())
}

#[cfg(test)]
mod tests;

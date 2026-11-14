// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An OpenAI sign-in server that runs inside the test, on 127.0.0.1. It
//! stands in for the browser too: [`Fake::authorize`] reads a round's page
//! the way the real one does and gives the callback. It registers a client
//! per host ID, checks each code against its PKCE challenge, signs
//! identity tokens with the test key, rotates refresh tokens and catches a
//! spent one, and revokes. Every value it mints has "test" in it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::digest::{digest, SHA256};
use ring::rand::SystemRandom;
use ring::rsa::PublicKeyComponents;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use super::Issuer;
use crate::secrets::testing::RSA_PKCS1;
use crate::secrets::Now;

/// Who signs in, unless the test says otherwise.
pub const SUBJECT: &str = "user-test-1";
/// Their email.
pub const EMAIL: &str = "dev@example.com";
/// The ID the test key goes by.
const KID: &str = "test-key";

/// The server, and what it knows.
pub struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}

/// How the next identity token is spoiled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Spoil {
    Nothing,
    Nonce,
    Audience,
    Signature,
    Expired,
}

/// What the fake server holds.
pub struct State {
    now: Now,
    key: RsaKeyPair,
    /// Who signs in.
    pub subject: String,
    pub email: Option<String>,
    /// Whether a sign-in allows plan usage.
    pub plan: bool,
    /// How long access tokens last, in seconds.
    pub expires_in: u64,
    /// How the next identity token is spoiled.
    pub spoil: Spoil,
    /// Whether codes run out before they're used.
    pub stale_codes: bool,
    /// How long a renewal takes.
    pub refresh_delay: Duration,
    /// What revocation answers.
    pub revoke_status: u16,
    /// How many renewals it gave.
    pub refreshes: usize,
    /// How many revocations it got.
    pub revocations: usize,
    /// The refresh tokens it revoked.
    pub revoked: Vec<String>,
    /// The client each host ID registered.
    pub clients: HashMap<String, String>,
    codes: HashMap<String, Code>,
    /// Each refresh token that works, and its client.
    pub live: HashMap<String, String>,
    spent: HashSet<String>,
    minted: u64,
}

/// A code waiting to be traded.
struct Code {
    client_id: String,
    challenge: String,
    redirect_uri: String,
    nonce: String,
    scope: String,
}

/// What the browser hands back to the callback.
pub struct Callback {
    pub code: String,
    pub state: String,
    pub client_id: Option<String>,
}

impl Fake {
    /// Starts a server whose identity tokens read the time from `now`.
    pub async fn start(now: Now) -> Self {
        let der = STANDARD.decode(RSA_PKCS1).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            now,
            key: RsaKeyPair::from_der(&der).unwrap(),
            subject: SUBJECT.to_owned(),
            email: Some(EMAIL.to_owned()),
            plan: true,
            expires_in: 3600,
            spoil: Spoil::Nothing,
            stale_codes: false,
            refresh_delay: Duration::ZERO,
            revoke_status: 200,
            refreshes: 0,
            revocations: 0,
            revoked: Vec::new(),
            clients: HashMap::new(),
            codes: HashMap::new(),
            live: HashMap::new(),
            spent: HashSet::new(),
            minted: 0,
        }));
        let serving = state.clone();
        let issuer = base.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = serving.clone();
                let issuer = issuer.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, &state, &issuer).await;
                });
            }
        });
        Self { base, state }
    }

    /// What the server knows, for the test to read or change.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// The issuer, as the store reaches it.
    pub fn issuer(&self) -> Issuer {
        Issuer::local(&self.base)
    }

    /// Opens a round's `page` as the browser would, signs in, and gives
    /// what comes back to the callback.
    pub fn authorize(&self, page: &str) -> Callback {
        let url = Url::parse(page).unwrap();
        assert!(page.starts_with(&format!("{}/oauth/authorize?", self.base)));
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let get = |name: &str| query.get(name).cloned().unwrap_or_default();
        assert_eq!(get("response_type"), "code");
        assert_eq!(get("code_challenge_method"), "S256");
        assert!(get("redirect_uri").starts_with("http://127.0.0.1:"));
        let host_id = get("ext_agent_host_id");
        assert!(host_id.starts_with("urn:uuid:"));

        let mut state = self.state();
        state.minted += 1;
        let minted = state.minted;
        let (client_id, registered) = if get("client_id") == "dynamic_agent_client" {
            assert_eq!(get("agent_name_hint"), "MaxHQ");
            let client_id = format!("oaiapp_test-{minted}");
            state.clients.insert(host_id, client_id.clone());
            (client_id, true)
        } else {
            assert!(!query.contains_key("agent_name_hint"));
            assert_eq!(state.clients.get(&host_id), Some(&get("client_id")));
            (get("client_id"), false)
        };
        let code = format!("test-code-{minted}");
        let mut scope = get("scope");
        if !state.plan {
            scope = scope.replace(" chatgpt.tokens.use.direct", "");
        }
        state.codes.insert(
            code.clone(),
            Code {
                client_id: client_id.clone(),
                challenge: get("code_challenge"),
                redirect_uri: get("redirect_uri"),
                nonce: get("nonce"),
                scope,
            },
        );
        Callback {
            code,
            state: get("state"),
            client_id: registered.then_some(client_id),
        }
    }
}

/// Answers one request on `stream`, then closes it.
async fn serve(mut stream: TcpStream, state: &Mutex<State>, issuer: &str) -> std::io::Result<()> {
    let mut read = Vec::new();
    let mut chunk = [0; 4096];
    let head_end = loop {
        let got = stream.read(&mut chunk).await?;
        if got == 0 {
            return Ok(());
        }
        read.extend_from_slice(&chunk[..got]);
        if let Some(at) = read.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&read[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|first| first.split(' ').nth(1))
        .unwrap_or_default()
        .to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    while read.len() < head_end + length {
        let got = stream.read(&mut chunk).await?;
        if got == 0 {
            break;
        }
        read.extend_from_slice(&chunk[..got]);
    }
    let form: HashMap<String, String> = url::form_urlencoded::parse(&read[head_end..])
        .into_owned()
        .collect();

    let (status, reply, delay) = state.lock().unwrap().answer(&path, &form, issuer);
    tokio::time::sleep(delay).await;
    let reply = reply.to_string();
    let response = format!(
        "HTTP/1.1 {status} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn oauth_error(code: &str) -> (u16, Value, Duration) {
    (400, json!({ "error": code }), Duration::ZERO)
}

impl State {
    fn answer(
        &mut self,
        path: &str,
        form: &HashMap<String, String>,
        issuer: &str,
    ) -> (u16, Value, Duration) {
        let get = |name: &str| form.get(name).cloned().unwrap_or_default();
        match path {
            "/.well-known/openid-configuration" => (
                200,
                json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/oauth/authorize"),
                    "token_endpoint": format!("{issuer}/oauth/token"),
                    "jwks_uri": format!("{issuer}/.well-known/jwks.json"),
                    "revocation_endpoint": format!("{issuer}/oauth/revoke"),
                }),
                Duration::ZERO,
            ),
            "/.well-known/jwks.json" => {
                let public = PublicKeyComponents::<Vec<u8>>::from(self.key.public());
                let jwks = json!({ "keys": [{
                    "kty": "RSA",
                    "kid": KID,
                    "alg": "RS256",
                    "use": "sig",
                    "n": URL_SAFE_NO_PAD.encode(public.n),
                    "e": URL_SAFE_NO_PAD.encode(public.e),
                }]});
                (200, jwks, Duration::ZERO)
            }
            "/oauth/token" if get("grant_type") == "authorization_code" => {
                let Some(code) = self.codes.remove(&get("code")) else {
                    return oauth_error("invalid_grant");
                };
                if code.client_id != get("client_id") {
                    return oauth_error("invalid_client");
                }
                let challenge =
                    URL_SAFE_NO_PAD.encode(digest(&SHA256, get("code_verifier").as_bytes()));
                if self.stale_codes
                    || challenge != code.challenge
                    || code.redirect_uri != get("redirect_uri")
                    || get("resource") != super::RESOURCE
                {
                    return oauth_error("invalid_grant");
                }
                let id_token = self.id_token(issuer, &code.client_id, &code.nonce);
                let mut tokens = self.mint(&code.client_id, &code.scope);
                tokens["id_token"] = json!(id_token);
                (200, tokens, Duration::ZERO)
            }
            "/oauth/token" if get("grant_type") == "refresh_token" => {
                let refresh = get("refresh_token");
                if self.spent.contains(&refresh) {
                    // A spent token ends its session, as OpenAI's does.
                    self.live.clear();
                    return oauth_error("refresh_token_reused");
                }
                let Some(client_id) = self.live.remove(&refresh) else {
                    return oauth_error("invalid_grant");
                };
                if client_id != get("client_id") {
                    return oauth_error("invalid_client");
                }
                self.spent.insert(refresh);
                self.refreshes += 1;
                let scope = if self.plan {
                    super::SCOPES
                } else {
                    "openid profile email offline_access resource.invoke"
                };
                (200, self.mint(&client_id, scope), self.refresh_delay)
            }
            "/oauth/revoke" => {
                self.revocations += 1;
                if self.revoke_status == 200 {
                    let token = get("token");
                    self.live.remove(&token);
                    self.revoked.push(token);
                }
                (self.revoke_status, json!({}), Duration::ZERO)
            }
            _ => (404, json!({ "error": "not_found" }), Duration::ZERO),
        }
    }

    /// A new access and refresh token for `client_id`.
    fn mint(&mut self, client_id: &str, scope: &str) -> Value {
        self.minted += 1;
        let refresh = format!("test-refresh-{}", self.minted);
        self.live.insert(refresh.clone(), client_id.to_owned());
        json!({
            "access_token": format!("test-access-{}", self.minted),
            "refresh_token": refresh,
            "token_type": "Bearer",
            "expires_in": self.expires_in,
            "scope": scope,
        })
    }

    /// An identity token signed with the test key.
    fn id_token(&self, issuer: &str, client_id: &str, nonce: &str) -> String {
        let now = (self.now)().0 / 1000;
        let spoil = self.spoil;
        let claims = json!({
            "iss": issuer,
            "aud": if spoil == Spoil::Audience { "oaiapp_someone-else" } else { client_id },
            "sub": self.subject,
            "email": self.email,
            "nonce": if spoil == Spoil::Nonce { "test-other-nonce" } else { nonce },
            "iat": now,
            "exp": if spoil == Spoil::Expired { now - 60 } else { now + 3600 },
        });
        let header = json!({ "alg": "RS256", "kid": KID, "typ": "JWT" });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .unwrap();
        if spoil == Spoil::Signature {
            signature[0] ^= 1;
        }
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
}

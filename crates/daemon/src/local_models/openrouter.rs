// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sign in with OpenRouter, by OAuth PKCE. The device starts the round
//! and keeps its verifier. The app listens on the Mac at the port the
//! round names, opens OpenRouter's page and brings back the code. The
//! device trades the code for a key, which never leaves it, and the
//! service keeps the key on the device's OpenRouter server. Nothing here
//! logs the code, the verifier or the key.

use std::time::Duration;

use maxhq_protocol::ids::SignInId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::secret::Secret;
use reqwest::header::CONTENT_TYPE;
use reqwest::StatusCode;
use ring::rand::SystemRandom;
use serde::Deserialize;
use tracing::{info, warn};
use url::Url;

use super::cloud;
use crate::secrets::pkce::{self, Rounds};

/// OpenRouter's host. Its subdomains are OpenRouter's too.
pub(crate) const HOST: &str = "openrouter.ai";

/// OpenRouter's site, https on [`HOST`]: the sign-in page, the exchange
/// and the API are all on it.
pub const ORIGIN: &str = "https://openrouter.ai";

/// The name a new OpenRouter server gets when the round names none.
const LABEL: &str = "OpenRouter";

/// The path the browser comes back to on the Mac.
const CALLBACK_PATH: &str = "/callback";

/// The highest spending limit OpenRouter takes, in cents: less than
/// $1,000,000.
const MAX_LIMIT_CENTS: u32 = 99_999_999;

/// How long the exchange may take.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

const ENDED: &str = "That sign-in ended. Start it again.";

/// Whether the server at `base_url` is OpenRouter: https, on [`HOST`] or a
/// subdomain of it. Its credits can be read, and a sign-in keeps its key
/// on it.
pub(crate) fn is_openrouter(base_url: &str) -> bool {
    #[cfg(test)]
    if fakes::has(base_url) {
        return true;
    }
    cloud::https_host(base_url).is_some_and(|host| cloud::on(&host, HOST))
}

/// A round waiting for its code.
struct Round {
    verifier: Secret,
    label: String,
}

/// What a finished round made: the key, and the server to keep it on.
pub struct Signed {
    /// The name a new server gets.
    pub label: String,
    /// The OpenRouter API's base URL.
    pub base: String,
    /// The new key.
    pub key: Secret,
}

/// The Sign in with OpenRouter rounds of one `maxd`.
pub struct SignIns {
    /// OpenRouter's origin, which tests point at a local fake.
    origin: String,
    rounds: Rounds<Round>,
}

/// The key the exchange answers with.
#[derive(Deserialize)]
struct Made {
    key: Secret,
}

impl SignIns {
    /// The rounds of a device that signs in at `origin`, such as [`ORIGIN`].
    pub fn new(origin: &str) -> Self {
        Self {
            origin: origin.trim_end_matches('/').to_owned(),
            rounds: Rounds::default(),
        }
    }

    /// The API base URL of the server a sign-in keeps its key on.
    pub fn base(&self) -> String {
        format!("{}/api", self.origin)
    }

    /// Starts a round: a verifier and its challenge, a `state` and a port
    /// for the app to listen on. The key is named after `device`, and
    /// OpenRouter stops it at `limit_cents` when one is given.
    pub fn begin(
        &self,
        device: &str,
        label: Option<String>,
        limit_cents: Option<u32>,
    ) -> Result<ActionOutcome, String> {
        if limit_cents.is_some_and(|cents| cents == 0 || cents > MAX_LIMIT_CENTS) {
            return Err(
                "A spending limit is more than $0 and less than $1,000,000. Change it and try again."
                    .to_owned(),
            );
        }
        let rng = SystemRandom::new();
        let (Some(verifier), Some(state), Some(port)) = (
            pkce::random(&rng, 64),
            pkce::random(&rng, 32),
            pkce::callback_port(&rng),
        ) else {
            return Err("This device couldn't start a sign-in.".to_owned());
        };
        let mut url = Url::parse(&format!("{}/auth", self.origin))
            .map_err(|_| "This device couldn't start a sign-in.".to_owned())?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair(
                    "callback_url",
                    &format!("http://localhost:{port}{CALLBACK_PATH}"),
                )
                .append_pair("code_challenge", &pkce::challenge(&verifier))
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", &state)
                .append_pair("key_label", &format!("MaxHQ on {device}"));
            if let Some(cents) = limit_cents {
                query.append_pair("limit", &dollars(cents));
            }
        }
        let label = label
            .map(|label| label.trim().to_owned())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| LABEL.to_owned());
        let sign_in = self.rounds.start(Round {
            verifier: Secret::new(verifier),
            label,
        });
        info!(%sign_in, "started an OpenRouter sign-in");
        Ok(ActionOutcome::SignIn {
            sign_in,
            url: url.into(),
            callback_port: port,
        })
    }

    /// Finishes a round: trades `code` for a key, once. A round that
    /// ended, or a code OpenRouter refuses, says why.
    pub async fn finish(&self, sign_in: SignInId, code: Secret) -> Result<Signed, String> {
        let round = self.rounds.take(sign_in).ok_or(ENDED)?;
        if code.is_empty() {
            return Err(ENDED.to_owned());
        }
        let body = serde_json::json!({
            "code": code.expose(),
            "code_verifier": round.verifier.expose(),
            "code_challenge_method": "S256",
        });
        let response = super::http()
            .post(format!("{}/api/v1/auth/keys", self.origin))
            .timeout(EXCHANGE_TIMEOUT)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|err| {
                warn!(%sign_in, "couldn't reach OpenRouter to finish a sign-in: {}", err.without_url());
                "Couldn't reach OpenRouter. Check this device's connection and try again."
                    .to_owned()
            })?;
        let status = response.status();
        if !status.is_success() {
            warn!(%sign_in, %status, "OpenRouter refused a sign-in's code");
            return Err(refusal(status));
        }
        let made: Made = response
            .bytes()
            .await
            .ok()
            .and_then(|body| serde_json::from_slice(&body).ok())
            .ok_or(NO_KEY)?;
        let key = Secret::new(made.key.expose().trim().to_owned());
        if key.is_empty() {
            return Err(NO_KEY.to_owned());
        }
        info!(%sign_in, "finished an OpenRouter sign-in");
        Ok(Signed {
            label: round.label,
            base: self.base(),
            key,
        })
    }
}

const NO_KEY: &str = "OpenRouter didn't send a key. Start the sign-in again.";

/// Why OpenRouter refused a code, in words.
fn refusal(status: StatusCode) -> String {
    match status {
        StatusCode::FORBIDDEN => {
            "OpenRouter didn't take the sign-in's code. It lasts 10 minutes, so start the sign-in again."
                .to_owned()
        }
        StatusCode::BAD_REQUEST => "OpenRouter refused the sign-in. Start it again.".to_owned(),
        _ => format!("OpenRouter answered {status}, so the sign-in didn't finish. Try again."),
    }
}

/// Cents as the US dollars OpenRouter's `limit` takes, such as `12.50`.
fn dollars(cents: u32) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// Local fakes that tests treat as OpenRouter. Nothing else can add one.
#[cfg(test)]
pub mod fakes {
    use std::sync::{Mutex, PoisonError};

    static FAKES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Treats the server at `base`, a local fake, as OpenRouter.
    pub fn add(base: &str) {
        let origin = url::Url::parse(base)
            .unwrap()
            .origin()
            .ascii_serialization();
        FAKES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(origin);
    }

    pub(super) fn has(base: &str) -> bool {
        let Ok(url) = url::Url::parse(base.trim()) else {
            return false;
        };
        let origin = url.origin().ascii_serialization();
        FAKES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::tests::Fake;
    use serde_json::json;

    fn page(outcome: &ActionOutcome) -> (SignInId, Url, u16) {
        let ActionOutcome::SignIn {
            sign_in,
            url,
            callback_port,
        } = outcome
        else {
            panic!("not a round: {outcome:?}");
        };
        (*sign_in, Url::parse(url).unwrap(), *callback_port)
    }

    fn param(url: &Url, name: &str) -> Option<String> {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    #[test]
    fn the_origin_is_https_on_the_host() {
        assert_eq!(ORIGIN, format!("https://{HOST}"));
        assert!(is_openrouter(&SignIns::new(ORIGIN).base()));
    }

    #[test]
    fn only_openrouter_over_https_is_openrouter() {
        assert!(is_openrouter("https://openrouter.ai/api"));
        assert!(is_openrouter("https://eu.openrouter.ai/api"));
        assert!(is_openrouter("https://OpenRouter.AI/api"));
        assert!(!is_openrouter("http://openrouter.ai/api"));
        assert!(!is_openrouter("https://openrouter.ai.example.com/api"));
        assert!(!is_openrouter("https://notopenrouter.ai/api"));
        assert!(!is_openrouter("https://api.z.ai/api/anthropic"));
        assert!(!is_openrouter("http://127.0.0.1:11434"));
        assert!(!is_openrouter("not a url"));
    }

    #[test]
    fn a_round_opens_openrouters_page_with_a_challenge() {
        let rounds = SignIns::new(ORIGIN);
        let outcome = rounds.begin("Home Mac", None, Some(1_250)).unwrap();
        let (_, url, port) = page(&outcome);
        assert_eq!(url.origin().ascii_serialization(), ORIGIN);
        assert_eq!(url.path(), "/auth");
        assert_eq!(
            param(&url, "callback_url").as_deref(),
            Some(format!("http://localhost:{port}/callback").as_str())
        );
        assert_eq!(
            param(&url, "code_challenge_method").as_deref(),
            Some("S256")
        );
        assert_eq!(param(&url, "code_challenge").map(|c| c.len()), Some(43));
        assert!(param(&url, "state").is_some_and(|state| state.len() >= 43));
        assert_eq!(
            param(&url, "key_label").as_deref(),
            Some("MaxHQ on Home Mac")
        );
        assert_eq!(param(&url, "limit").as_deref(), Some("12.50"));
        assert!(port >= 49_152);

        let unlimited = rounds.begin("Home Mac", None, None).unwrap();
        assert_eq!(param(&page(&unlimited).1, "limit"), None);
    }

    #[test]
    fn a_limit_has_to_be_one_openrouter_takes() {
        let rounds = SignIns::new(ORIGIN);
        assert!(rounds.begin("Home Mac", None, Some(0)).is_err());
        assert!(rounds
            .begin("Home Mac", None, Some(MAX_LIMIT_CENTS + 1))
            .is_err());
        assert!(rounds
            .begin("Home Mac", None, Some(MAX_LIMIT_CENTS))
            .is_ok());
        assert_eq!(dollars(1), "0.01");
        assert_eq!(dollars(2_500), "25.00");
        assert_eq!(dollars(MAX_LIMIT_CENTS), "999999.99");
    }

    #[tokio::test]
    async fn the_code_and_verifier_make_a_key() {
        let fake = Fake::start().await;
        fake.json(
            "POST /api/v1/auth/keys",
            200,
            &json!({"key": "sk-or-v1-example", "user_id": "user-example"}),
        );
        let rounds = SignIns::new(&fake.base);
        let outcome = rounds.begin("devbox", Some("  ".into()), None).unwrap();
        let (sign_in, url, _) = page(&outcome);
        let signed = rounds
            .finish(sign_in, Secret::new("code-example".into()))
            .await
            .unwrap();
        assert_eq!(signed.key.expose(), "sk-or-v1-example");
        assert_eq!(signed.label, "OpenRouter");
        assert_eq!(signed.base, format!("{}/api", fake.base));

        let seen = fake.seen();
        let exchange = seen
            .iter()
            .find(|seen| seen.path == "/api/v1/auth/keys")
            .unwrap();
        assert_eq!(exchange.method, "POST");
        let body: serde_json::Value = serde_json::from_str(&exchange.body).unwrap();
        assert_eq!(body["code"], "code-example");
        assert_eq!(body["code_challenge_method"], "S256");
        // The verifier is the one the page's challenge is of.
        let verifier = body["code_verifier"].as_str().unwrap();
        assert_eq!(
            param(&url, "code_challenge").unwrap(),
            pkce::challenge(verifier)
        );

        // A round is good for one code.
        let again = rounds
            .finish(sign_in, Secret::new("code-example".into()))
            .await;
        assert_eq!(again.err().as_deref(), Some(ENDED));
    }

    #[tokio::test]
    async fn a_refused_code_says_why() {
        let fake = Fake::start().await;
        fake.json(
            "POST /api/v1/auth/keys",
            403,
            &json!({"error": {"message": "Invalid code or code_verifier"}}),
        );
        let rounds = SignIns::new(&fake.base);
        let (sign_in, _, _) = page(&rounds.begin("devbox", None, None).unwrap());
        let refused = rounds
            .finish(sign_in, Secret::new("code-example".into()))
            .await
            .err()
            .unwrap();
        assert!(refused.contains("10 minutes"), "{refused}");

        let unknown = rounds
            .finish(SignInId::generate(), Secret::new("code-example".into()))
            .await;
        assert_eq!(unknown.err().as_deref(), Some(ENDED));
    }

    #[tokio::test]
    async fn an_answer_without_a_key_makes_none() {
        let fake = Fake::start().await;
        fake.json("POST /api/v1/auth/keys", 200, &json!({"key": " "}));
        let rounds = SignIns::new(&fake.base);
        let (sign_in, _, _) = page(&rounds.begin("devbox", None, None).unwrap());
        let refused = rounds
            .finish(sign_in, Secret::new("code-example".into()))
            .await;
        assert_eq!(refused.err().as_deref(), Some(NO_KEY));
    }
}

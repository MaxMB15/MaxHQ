// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Talks to `codex app-server`, which speaks JSON lines on stdio with no
//! `jsonrpc` field. Only the methods in [`Method`] can be sent, so nothing
//! here can change the account or start a turn. Every one only reads but
//! [`Method::UseResetCredit`], which spends a banked reset, and only
//! [`super::reset`] sends it, for an explicit click in the app after you
//! confirm.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// How long one request may take. A cold start answers `initialize` in
/// under a second, and the limits in a quarter of one.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the app-server has to exit once its input closes, before it's
/// killed.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// The requests `maxd` sends. Every one only reads but
/// [`Method::UseResetCredit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Starts the exchange.
    Initialize,
    /// Reads who's logged in, without refreshing the token.
    ReadAccount,
    /// Reads the rate limits.
    ReadRateLimits,
    /// Lists the models, one page at a time.
    ListModels,
    /// Spends one banked reset, which can't be undone. Sent only by
    /// [`super::reset`], when the app asks after you confirm.
    UseResetCredit,
}

impl Method {
    /// The method's name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ReadAccount => "account/read",
            Self::ReadRateLimits => "account/rateLimits/read",
            Self::ListModels => "model/list",
            Self::UseResetCredit => "account/rateLimitResetCredit/consume",
        }
    }
}

/// Why a check through the app-server didn't work. The text is what the
/// app shows.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No `codex` executable was found.
    #[error("codex not found")]
    NotFound,
    /// The app-server didn't start.
    #[error("couldn't start codex app-server: {0}")]
    Spawn(#[source] io::Error),
    /// The app-server exited.
    #[error("codex app-server exited")]
    Closed,
    /// The app-server didn't answer in time.
    #[error("codex app-server didn't answer {0} in time")]
    Timeout(&'static str),
    /// Reading from or writing to the app-server failed.
    #[error("couldn't talk to codex app-server: {0}")]
    Io(#[source] io::Error),
    /// The app-server answered with an error.
    #[error("codex app-server: {0}")]
    Remote(String),
    /// No one is logged in to this `CODEX_HOME`.
    #[error("logged out")]
    LoggedOut,
}

impl Error {
    /// Whether the app-server has to be started again before the next try.
    pub fn needs_restart(&self) -> bool {
        !matches!(self, Self::Remote(_) | Self::LoggedOut)
    }
}

/// A request on the wire.
#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    method: &'a str,
    params: Value,
}

/// A running `codex app-server`. Dropping it kills the process, and
/// [`AppServer::stop`] lets it exit first.
pub struct AppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl AppServer {
    /// Starts `program app-server` for the login in `home` and completes
    /// the opening exchange.
    pub async fn start(program: &Path, home: &Path) -> Result<Self, Error> {
        let mut child = crate::drive::programs::command(program)
            .arg("app-server")
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::Spawn)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Error::Closed);
        };
        let mut server = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
        };
        let params = json!({
            "clientInfo": {
                "name": "maxd",
                "title": "MaxHQ",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": { "experimentalApi": false },
        });
        server.call(Method::Initialize, params).await?;
        server.write(&json!({ "method": "initialized" })).await?;
        Ok(server)
    }

    /// Closes the app-server's input, which ends it, and waits for it to
    /// exit. One that doesn't in time is killed.
    pub async fn stop(self) {
        let Self {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        if tokio::time::timeout(STOP_GRACE, child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }

    /// Sends a request and waits for its result, skipping notifications
    /// that arrive first.
    pub async fn call(&mut self, method: Method, params: Value) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            method: method.name(),
            params,
        };
        self.write(&request).await?;
        tokio::time::timeout(CALL_TIMEOUT, self.answer(id))
            .await
            .map_err(|_| Error::Timeout(method.name()))?
    }

    /// The next message the app-server sends on its own. Safe to cancel,
    /// so a caller can wait on it to learn when the process exits.
    pub async fn next_message(&mut self) -> Result<Value, Error> {
        loop {
            let Some(line) = self.stdout.next_line().await.map_err(Error::Io)? else {
                return Err(Error::Closed);
            };
            if let Ok(message) = serde_json::from_str::<Value>(&line) {
                return Ok(message);
            }
        }
    }

    /// Waits for the answer to request `id`.
    async fn answer(&mut self, id: u64) -> Result<Value, Error> {
        loop {
            let message = self.next_message().await?;
            // Requests from the server carry a method and their own ID.
            if message.get("method").is_some()
                || message.get("id").and_then(Value::as_u64) != Some(id)
            {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                return Err(Error::Remote(text.to_owned()));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Sends one message as a line.
    async fn write(&mut self, message: &impl Serialize) -> Result<(), Error> {
        let mut line = serde_json::to_vec(message).map_err(|error| Error::Io(error.into()))?;
        line.push(b'\n');
        self.stdin.write_all(&line).await.map_err(Error::Io)?;
        self.stdin.flush().await.map_err(Error::Io)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A stand-in for `codex app-server` that logs every line it gets to
    /// `$CODEX_HOME/requests.log` and answers from files in that folder:
    /// `account.json` for `account/read`, `limits.json` for the rate
    /// limits, or an error when that file is missing, and `models.json` for
    /// the first page of `model/list` and `models-next.json` for any page
    /// asked for by cursor. With `exit` there, it quits instead of
    /// answering the rate limits. It answers a reset from `consume.json`,
    /// but quits instead once when `drop-consume` is there, which it
    /// removes, and every time with `drop-every-consume`, and waits while
    /// `hold` is there. Once its input closes it adds a line to `stops.log`
    /// and exits.
    const FAKE: &str = r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$CODEX_HOME/requests.log"
  id=$(printf '%s' "$line" | sed -n 's/^{"id":\([0-9]*\),.*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"method":"remoteControl/status/changed","params":{}}\n'
      printf '{"id":%s,"result":{"userAgent":"fake/0.0.0","codexHome":"%s"}}\n' "$id" "$CODEX_HOME" ;;
    *'"method":"account/read"'*)
      printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$CODEX_HOME/account.json")" ;;
    *'"method":"account/rateLimits/read"'*)
      if [ -e "$CODEX_HOME/exit" ]; then exit 0; fi
      if [ -e "$CODEX_HOME/limits.json" ]; then
        printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$CODEX_HOME/limits.json")"
      else
        printf '{"id":%s,"error":{"code":-32603,"message":"rate limits unavailable"}}\n' "$id"
      fi ;;
    *'"method":"model/list"'*)
      page=models
      case "$line" in *'"cursor":'*) page=models-next ;; esac
      printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$CODEX_HOME/$page.json")" ;;
    *'"method":"account/rateLimitResetCredit/consume"'*)
      if [ -e "$CODEX_HOME/drop-consume" ]; then rm "$CODEX_HOME/drop-consume"; exit 0; fi
      if [ -e "$CODEX_HOME/drop-every-consume" ]; then exit 0; fi
      while [ -e "$CODEX_HOME/hold" ]; do sleep 0.05; done
      printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$CODEX_HOME/consume.json")" ;;
  esac
done
echo stopped >> "$CODEX_HOME/stops.log"
"#;

    /// A temporary `CODEX_HOME` with the fake app-server beside it.
    pub struct FakeHome {
        /// The folder to pass as `CODEX_HOME`.
        pub home: PathBuf,
        /// The fake `codex` executable.
        pub program: PathBuf,
        root: PathBuf,
    }

    impl FakeHome {
        /// Sets one up under a name unique to this test.
        pub fn new(name: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("maxd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let home = root.join("codex-home");
            std::fs::create_dir_all(&home).unwrap();
            let program = root.join("codex");
            std::fs::write(&program, FAKE).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let fake = Self {
                home,
                program,
                root,
            };
            fake.write("account.json", &json!({
                "account": { "type": "chatgpt", "email": "dev@example.com", "planType": "plus" },
                "requiresOpenaiAuth": true,
            }));
            let limits: Value =
                serde_json::from_str(include_str!("fixtures/rate_limits.json")).unwrap();
            fake.write("limits.json", &limits);
            fake.write("models.json", &json!({
                "data": [
                    { "id": "gpt-6.1-sol", "model": "gpt-6.1-sol", "displayName": "GPT-6.1 Sol", "isDefault": true },
                    { "id": "gpt-old", "model": "gpt-old", "hidden": true },
                ],
                "nextCursor": "2",
            }));
            fake.write("models-next.json", &json!({
                "data": [{ "id": "gpt-5.6-luna", "model": "gpt-5.6-luna", "displayName": "GPT-5.6 Luna" }],
                "nextCursor": null,
            }));
            fake
        }

        /// Writes a file the fake answers from, on one line.
        pub fn write(&self, name: &str, value: &Value) {
            std::fs::write(self.home.join(name), value.to_string()).unwrap();
        }

        /// Removes a file the fake answers from.
        pub fn remove(&self, name: &str) {
            std::fs::remove_file(self.home.join(name)).unwrap();
        }

        /// Every line the fake was sent, parsed.
        pub fn requests(&self) -> Vec<Value> {
            std::fs::read_to_string(self.home.join("requests.log"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }

        /// How many times the fake exited once its input closed.
        pub fn stops(&self) -> usize {
            std::fs::read_to_string(self.home.join("stops.log"))
                .unwrap_or_default()
                .lines()
                .count()
        }
    }

    impl Drop for FakeHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn the_exchange_uses_only_read_methods() {
        let fake = FakeHome::new("codex-exchange");
        let mut server = AppServer::start(&fake.program, &fake.home).await.unwrap();
        let account = server
            .call(Method::ReadAccount, json!({ "refreshToken": false }))
            .await
            .unwrap();
        assert_eq!(account["account"]["planType"], "plus");
        let limits = server
            .call(
                Method::ReadRateLimits,
                json!({ "excludeResetCreditDetails": true }),
            )
            .await
            .unwrap();
        assert_eq!(
            limits["rateLimitsByLimitId"]["codex"]["primary"]["usedPercent"],
            41
        );
        let models = server
            .call(Method::ListModels, json!({ "cursor": "2" }))
            .await
            .unwrap();
        assert_eq!(models["data"][0]["model"], "gpt-5.6-luna");
        drop(server);

        let requests = fake.requests();
        let methods: Vec<&str> = requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "account/read",
                "account/rateLimits/read",
                "model/list"
            ]
        );
        assert_eq!(requests[0]["id"], 1);
        assert_eq!(requests[0]["params"]["clientInfo"]["name"], "maxd");
        assert_eq!(
            requests[0]["params"]["capabilities"]["experimentalApi"],
            false
        );
        assert!(
            requests[1].get("id").is_none(),
            "initialized is a notification"
        );
        assert_eq!(requests[2]["params"], json!({ "refreshToken": false }));
        assert_eq!(
            requests[3]["params"],
            json!({ "excludeResetCreditDetails": true })
        );
        assert_eq!(requests[4]["params"], json!({ "cursor": "2" }));
    }

    #[tokio::test]
    async fn errors_say_whether_to_restart() {
        let fake = FakeHome::new("codex-exchange-errors");
        fake.remove("limits.json");
        let mut server = AppServer::start(&fake.program, &fake.home).await.unwrap();
        let error = server
            .call(
                Method::ReadRateLimits,
                json!({ "excludeResetCreditDetails": true }),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "codex app-server: rate limits unavailable"
        );
        assert!(!error.needs_restart());

        std::fs::write(fake.home.join("exit"), "").unwrap();
        let error = server
            .call(
                Method::ReadRateLimits,
                json!({ "excludeResetCreditDetails": true }),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Closed), "{error}");
        assert!(error.needs_restart());

        let missing = fake.home.join("no-such-codex");
        let error = AppServer::start(&missing, &fake.home).await.err().unwrap();
        assert!(matches!(error, Error::Spawn(_)), "{error}");
    }

    #[tokio::test]
    async fn a_stop_waits_for_the_app_server_to_exit() {
        let fake = FakeHome::new("codex-exchange-stop");
        let server = AppServer::start(&fake.program, &fake.home).await.unwrap();
        server.stop().await;
        assert_eq!(fake.stops(), 1);
    }
}

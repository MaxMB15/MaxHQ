// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd guest codex`, which a Codex app-server on a ChatGPT sign-in
//! starts through. It fetches Codex's model catalog with a token from the
//! guest, keeps it in `CODEX_HOME`, and then becomes Codex with the file
//! named in `model_catalog_json`. When the catalog can't be had, an older
//! file still goes, and with none Codex starts without one.

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use maxhq_core::fs::{write_atomic, Atomic};
use reqwest::header::ACCEPT;
use reqwest::Client;
use serde_json::Value;

use super::agent::AgentWant;
use super::helper::ask_at;
use crate::secrets::http;

/// Where the catalog comes from.
const CATALOG_URL: &str = "https://api.openai.com/v1/models";

/// The catalog's file in `CODEX_HOME`.
const CATALOG_FILE: &str = "maxhq-models.json";

/// How long the fetch may take in all.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The biggest catalog taken.
const BODY_MAX: usize = 4 * 1024 * 1024;

/// Fetches the catalog, then runs `command` in this process.
pub(crate) fn codex(socket: &Path, command: &[&str]) -> ExitCode {
    let home = std::env::var_os("CODEX_HOME");
    let argv = argv(
        home.as_deref().map(Path::new),
        command,
        &mut io::stderr(),
        |path| {
            let client = http::client(true)?;
            refresh(socket, &client, CATALOG_URL, path)
        },
    );
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    eprintln!("couldn't start {}: {err}", argv[0].to_string_lossy());
    ExitCode::FAILURE
}

/// `command`, followed by the catalog flag when there's a file for it in
/// `home` after `refresh` tries to write one. Why it couldn't goes to `log`
/// on one line.
fn argv(
    home: Option<&Path>,
    command: &[&str],
    log: &mut impl Write,
    refresh: impl FnOnce(&Path) -> Result<(), String>,
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = command.iter().map(OsString::from).collect();
    let Some(home) = home else {
        let _ = writeln!(log, "no model catalog for Codex: CODEX_HOME isn't set");
        return argv;
    };
    let path = home.join(CATALOG_FILE);
    if let Err(reason) = refresh(&path) {
        let _ = writeln!(log, "couldn't refresh Codex's model catalog: {reason}");
    }
    // Codex reads the setting as TOML, and a JSON string is a TOML one.
    if let (true, Some(text)) = (path.is_file(), path.to_str()) {
        let quoted = serde_json::to_string(text).unwrap_or_default();
        argv.extend(["-c".into(), format!("model_catalog_json={quoted}").into()]);
    }
    argv
}

/// Fetches the catalog from `url` and writes it to `path`, leaving any
/// older file alone when that fails.
fn refresh(socket: &Path, client: &Client, url: &str, path: &Path) -> Result<(), String> {
    let (token, _) =
        ask_at(socket, &AgentWant::Codex).map_err(|reason| format!("no token: {reason}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("couldn't start the runtime: {err}"))?;
    let body = runtime.block_on(fetch(client, url, token.expose()))?;
    check(&body)?;
    // Written whole, so Codex never reads half of one.
    write_atomic(path, &body, Atomic::PRIVATE)
        .map_err(|err| format!("couldn't write {}: {err}", path.display()))
}

/// The body `url` answers, up to [`BODY_MAX`], with `token` as the bearer.
/// No error names the token.
async fn fetch(client: &Client, url: &str, token: &str) -> Result<Vec<u8>, String> {
    let unreachable = |err: reqwest::Error| {
        if err.is_timeout() {
            "OpenAI took too long to answer".to_owned()
        } else {
            "couldn't reach OpenAI".to_owned()
        }
    };
    let mut response = client
        .get(url)
        .bearer_auth(token)
        .header(ACCEPT, "application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("OpenAI answered {}", status.as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        if body.len() + chunk.len() > BODY_MAX {
            return Err("the catalog is too big".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Checks `body` is a catalog: an object whose `models` each have a slug.
/// The rest of each model is Codex's to read.
fn check(body: &[u8]) -> Result<(), String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "the catalog isn't JSON".to_owned())?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or("the catalog has no models")?;
    let named = |model: &Value| {
        model
            .get("slug")
            .and_then(Value::as_str)
            .is_some_and(|slug| !slug.is_empty())
    };
    if !models.iter().all(named) {
        return Err("a model in the catalog has no slug".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread;

    use maxhq_protocol::guest::TokenReply;
    use maxhq_protocol::secret::Secret;
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::testing::TempDir;

    const TOKEN: &str = "test-not-a-real-token";

    const CATALOG: &str =
        r#"{"models":[{"slug":"gpt-test","prefer_websockets":true,"extra":{"kept":1}}]}"#;

    /// A guest that answers one ask with `reply`.
    fn guest(temp: &TempDir, reply: TokenReply) -> PathBuf {
        let socket = temp.path().join("agent.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut asked = String::new();
            BufReader::new(&stream).read_line(&mut asked).unwrap();
            let mut line = serde_json::to_vec(&reply).unwrap();
            line.push(b'\n');
            (&stream).write_all(&line).unwrap();
        });
        socket
    }

    fn token() -> TokenReply {
        TokenReply::Token {
            value: Secret::new(TOKEN.into()),
            expires_at: Timestamp(1_800_000_000_000),
        }
    }

    /// A server on 127.0.0.1 that answers one request with `status` and
    /// `body`, and hands back the request it got.
    fn serve(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/models", listener.local_addr().unwrap());
        let reply = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut chunk = [0; 4096];
            while !got.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "the request ended early");
                got.extend_from_slice(&chunk[..read]);
            }
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(got).unwrap()
        });
        (url, handle)
    }

    /// The argv for `codex app-server` with a guest at `socket`, a
    /// catalog at `url` and `home` as `CODEX_HOME`, and what was logged.
    fn run(socket: &Path, url: &str, home: &Path) -> (Vec<String>, String) {
        let client = http::client(false).unwrap();
        let mut log = Vec::new();
        let argv = argv(
            Some(home),
            &["/usr/local/bin/codex", "app-server"],
            &mut log,
            |path| refresh(socket, &client, url, path),
        );
        let argv = argv
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        let log = String::from_utf8(log).unwrap();
        assert!(!log.contains(TOKEN));
        (argv, log)
    }

    fn flag(home: &Path) -> String {
        let path = home.join(CATALOG_FILE);
        format!("model_catalog_json=\"{}\"", path.display())
    }

    #[test]
    fn a_good_catalog_is_kept_and_handed_to_codex() {
        let temp = TempDir::new();
        let socket = guest(&temp, token());
        let (url, server) = serve("200 OK", CATALOG);
        let (argv, log) = run(&socket, &url, temp.path());
        assert_eq!(
            argv,
            [
                "/usr/local/bin/codex",
                "app-server",
                "-c",
                &flag(temp.path())
            ]
        );
        assert_eq!(log, "");
        let path = temp.path().join(CATALOG_FILE);
        assert_eq!(fs::read_to_string(&path).unwrap(), CATALOG);
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");

        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/models "));
        assert!(request.contains(&format!("authorization: bearer {TOKEN}\r\n")));
        assert!(request.contains("accept: application/json\r\n"));
    }

    /// Each way the catalog can't be had: the guest's folder, its
    /// socket, the catalog's address and what the log says.
    fn failures() -> Vec<(TempDir, PathBuf, String, &'static str)> {
        let refused = TempDir::new();
        let socket = guest(
            &refused,
            TokenReply::Refused {
                reason: "this device isn't signed in to Codex".into(),
            },
        );
        // Nothing listens there, and nothing should ask it.
        let nowhere = "http://127.0.0.1:9/v1/models".to_owned();
        let mut cases = vec![(refused, socket, nowhere, "no token")];
        for (status, body, reason) in [
            ("401 Unauthorized", CATALOG, "OpenAI answered 401"),
            ("200 OK", "not json", "the catalog isn't JSON"),
            ("200 OK", r#"{"models":[]}"#, "the catalog has no models"),
            (
                "200 OK",
                r#"[{"slug":"gpt-test"}]"#,
                "the catalog has no models",
            ),
            (
                "200 OK",
                r#"{"models":[{"slug":"gpt-test"},{"slug":""}]}"#,
                "a model in the catalog has no slug",
            ),
        ] {
            let asking = TempDir::new();
            let socket = guest(&asking, token());
            let (url, _) = serve(status, body);
            cases.push((asking, socket, url, reason));
        }
        cases
    }

    #[test]
    fn without_a_catalog_codex_starts_without_the_flag() {
        for (_guest, socket, url, reason) in failures() {
            let home = TempDir::new();
            let (argv, log) = run(&socket, &url, home.path());
            assert_eq!(argv, ["/usr/local/bin/codex", "app-server"], "{reason}");
            assert!(
                log.starts_with("couldn't refresh Codex's model catalog: "),
                "{log}"
            );
            assert!(log.contains(reason), "{log}");
            assert_eq!(log.lines().count(), 1);
            assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0, "{reason}");
        }
    }

    #[test]
    fn an_older_catalog_still_goes_when_a_new_one_cant_be_had() {
        for (_guest, socket, url, reason) in failures() {
            let home = TempDir::new();
            let path = home.path().join(CATALOG_FILE);
            fs::write(&path, "older").unwrap();
            let (argv, log) = run(&socket, &url, home.path());
            assert_eq!(
                argv,
                [
                    "/usr/local/bin/codex",
                    "app-server",
                    "-c",
                    &flag(home.path())
                ],
                "{reason}"
            );
            assert!(log.contains(reason), "{log}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "older");
            assert_eq!(fs::read_dir(home.path()).unwrap().count(), 1, "{reason}");
        }
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Downloads. maxd fetches every file itself, hashes it as it comes and
//! writes it into the place through [`Exec`], so a container needs no curl
//! and a checksum is never left to the place to check.

use std::sync::LazyLock;
use std::time::Duration;

use ring::digest::{Context, SHA256};

use crate::recipe::{Exec, ExecError, ExecUser};

/// How much of a download goes into the place per write.
const CHUNK: usize = 8 * 1024 * 1024;
/// How long a download may wait for its first byte or between bytes.
const STALL: Duration = Duration::from_secs(60);
/// The most any one download may be, when its source gives no size.
const MOST: u64 = 1024 * 1024 * 1024;

/// What a download wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// Its SHA-256, in lowercase hex.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
}

/// Told the bytes so far and the bytes in all, when known.
pub type Progress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

/// The one HTTP client downloads use.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        // reqwest is built without a TLS crypto provider of its own.
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .connect_timeout(STALL)
            .read_timeout(STALL)
            .user_agent(concat!("maxd/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    });
    &CLIENT
}

/// Reads a small text file, such as a checksum list or a release's JSON.
pub async fn text(url: &str) -> Result<String, String> {
    let response = client()
        .get(url)
        .header("accept", "application/json, text/plain, */*")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| format!("couldn't read {}: {}", short(url), plain(&err)))?;
    response
        .text()
        .await
        .map_err(|err| format!("couldn't read {}: {}", short(url), plain(&err)))
}

/// Downloads `url` to `path` in the place as `user`, telling `progress` as
/// it comes. It stops past `most` bytes. A file that's there is replaced.
pub async fn download<E: Exec + ?Sized>(
    exec: &E,
    url: &str,
    path: &str,
    user: ExecUser,
    most: Option<u64>,
    progress: Progress<'_>,
) -> Result<Fetched, ExecError> {
    let failed = |err: reqwest::Error| {
        ExecError::Transport(format!(
            "the download of {} failed: {}",
            short(url),
            plain(&err)
        ))
    };
    let mut response = client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    let total = response.content_length();
    let most = most.unwrap_or(MOST);
    let mut hash = Context::new(&SHA256);
    let mut size = 0u64;
    let mut buffer = Vec::with_capacity(CHUNK.min(total.unwrap_or(0) as usize));
    let mut first = true;
    progress(0, total);
    while let Some(bytes) = response.chunk().await.map_err(failed)? {
        size += bytes.len() as u64;
        if size > most {
            return Err(ExecError::Transport(format!(
                "the download of {} is bigger than its {most} bytes",
                short(url)
            )));
        }
        hash.update(&bytes);
        buffer.extend_from_slice(&bytes);
        if buffer.len() >= CHUNK {
            put(exec, path, user, &buffer, first).await?;
            first = false;
            buffer.clear();
        }
        progress(size, total);
    }
    if !buffer.is_empty() || first {
        put(exec, path, user, &buffer, first).await?;
    }
    Ok(Fetched {
        sha256: hex(hash.finish().as_ref()),
        size,
    })
}

/// Writes or appends one piece of a download.
async fn put<E: Exec + ?Sized>(
    exec: &E,
    path: &str,
    user: ExecUser,
    bytes: &[u8],
    first: bool,
) -> Result<(), ExecError> {
    let script = if first {
        r#"cat > "$1""#
    } else {
        r#"cat >> "$1""#
    };
    let cmd = ["sh", "-c", script, "sh", path];
    let output = exec.run(&cmd, user, Some(bytes)).await?;
    if output.success() {
        Ok(())
    } else {
        Err(ExecError::failed(&cmd, &output))
    }
}

/// `bytes` in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// A URL without its query, which may carry a signature.
fn short(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// What went wrong, in words and without the URL, which may carry a
/// signature.
fn plain(err: &reqwest::Error) -> String {
    if let Some(status) = err.status() {
        format!("the server said {status}")
    } else if err.is_timeout() {
        "it timed out".to_owned()
    } else if err.is_connect() {
        "couldn't connect".to_owned()
    } else {
        "the connection broke".to_owned()
    }
}

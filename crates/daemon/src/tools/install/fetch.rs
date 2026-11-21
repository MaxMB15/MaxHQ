// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Downloads. maxd fetches every file itself, hashes it as it comes and
//! writes it into the place through [`Exec`], so a container needs no curl
//! and a checksum is never left to the place to check.

use std::path::{Path, PathBuf};
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
    static CLIENT: LazyLock<reqwest::Client> =
        LazyLock::new(|| builder().build().unwrap_or_default());
    &CLIENT
}

/// A client builder with the downloads' timeouts and user agent.
pub fn builder() -> reqwest::ClientBuilder {
    // reqwest is built without a TLS crypto provider of its own.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .connect_timeout(STALL)
        .read_timeout(STALL)
        .user_agent(concat!("maxd/", env!("CARGO_PKG_VERSION")))
}

/// Reads a small text file, such as a checksum list or a release's JSON.
pub async fn text(url: &str) -> Result<String, String> {
    text_up_to(url, MOST).await
}

/// Reads a text file of at most `most` bytes. A bigger one is refused
/// before it's all read.
pub async fn text_up_to(url: &str, most: u64) -> Result<String, String> {
    let couldnt = |err: &reqwest::Error| format!("couldn't read {}: {}", short(url), plain(err));
    let too_big = || format!("{} is bigger than its {most} bytes", short(url));
    let mut response = client()
        .get(url)
        .header("accept", "application/json, text/plain, */*")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| couldnt(&err))?;
    if response
        .content_length()
        .is_some_and(|length| length > most)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(bytes) = response.chunk().await.map_err(|err| couldnt(&err))? {
        if (body.len() + bytes.len()) as u64 > most {
            return Err(too_big());
        }
        body.extend_from_slice(&bytes);
    }
    String::from_utf8(body).map_err(|_| format!("{} isn't text", short(url)))
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
    let mut response = client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| Placed::<E>::fail(failure(url, &err)))?;
    let total = response.content_length();
    let mut hash = Context::new(&SHA256);
    let mut sink = Placed { exec, path, user };
    let most = most.unwrap_or(Kind::Tool.most());
    let size = pour(
        &mut response,
        url,
        &mut hash,
        0,
        total,
        most,
        &mut sink,
        progress,
    )
    .await?;
    Ok(Fetched {
        sha256: hex(hash.finish().as_ref()),
        size,
    })
}

/// What a download is, which sets the most it may be when its source
/// gives no size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A tool's archive or script.
    Tool,
    /// A model's weights, which run to tens of gigabytes.
    Model,
}

impl Kind {
    /// The most a download of this kind may be.
    pub const fn most(self) -> u64 {
        match self {
            Self::Tool => MOST,
            Self::Model => 512 * MOST,
        }
    }
}

/// Downloads `url` into `part` on this host, carrying on from what's
/// there: it hashes the bytes already in the file, asks for the rest with
/// a `Range` header and appends them. A server that sends the whole file
/// again starts it over. It stops past `size` when given, else past the
/// kind's most. The hash and size are the whole file's. Dropping the
/// future pauses it, keeping what came.
pub async fn resume(
    url: &str,
    part: &Path,
    size: Option<u64>,
    kind: Kind,
    progress: Progress<'_>,
) -> Result<Fetched, String> {
    let most = size.unwrap_or(kind.most()).min(kind.most());
    let (mut hash, mut have) = hash_file(part.to_owned(), most).await?;
    if size == Some(have) && have > 0 {
        progress(have, size);
        return Ok(Fetched {
            sha256: hex(hash.finish().as_ref()),
            size: have,
        });
    }
    let mut request = client().get(url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| failure(url, &err))?;
    let resumed = have > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resumed {
        hash = Context::new(&SHA256);
        have = 0;
    }
    let total = response.content_length().map(|rest| rest + have);
    if let (Some(total), Some(size)) = (total, size)
        && total != size
    {
        return Err(format!(
            "{} is {total} bytes there, not the {size} expected",
            short(url)
        ));
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)
        .await
        .map_err(|err| format!("couldn't write {}: {err}", part.display()))?;
    let mut sink = HostFile(file);
    let got = pour(
        &mut response,
        url,
        &mut hash,
        have,
        total.or(size),
        most,
        &mut sink,
        progress,
    )
    .await;
    sink.0
        .sync_all()
        .await
        .map_err(|err| format!("couldn't write {}: {err}", part.display()))?;
    let size = got?;
    if let Some(total) = total
        && size < total
    {
        return Err(format!(
            "the download of {} stopped after {size} of {total} bytes",
            short(url)
        ));
    }
    Ok(Fetched {
        sha256: hex(hash.finish().as_ref()),
        size,
    })
}

/// The hash and length of what a file holds, or a fresh hash for none. A
/// file past `most` can't be the start of the download, so it's emptied.
async fn hash_file(path: PathBuf, most: u64) -> Result<(Context, u64), String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut hash = Context::new(&SHA256);
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((hash, 0)),
            Err(err) => return Err(format!("couldn't read {}: {err}", path.display())),
        };
        let mut buffer = vec![0u8; 1 << 20];
        let mut have = 0u64;
        loop {
            let n = file
                .read(&mut buffer)
                .map_err(|err| format!("couldn't read {}: {err}", path.display()))?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            have += n as u64;
        }
        if have > most {
            return Ok((Context::new(&SHA256), 0));
        }
        Ok((hash, have))
    })
    .await
    .map_err(|err| format!("couldn't read {}: {err}", "the download"))?
}

/// Where a download's bytes go.
trait Sink {
    type Error;
    /// The error for a broken or oversized download.
    fn fail(message: String) -> Self::Error;
    /// Writes one piece. The first creates or empties the file.
    async fn put(&mut self, bytes: &[u8], first: bool) -> Result<(), Self::Error>;
}

/// A file in a place, written through [`Exec`].
struct Placed<'a, E: ?Sized> {
    exec: &'a E,
    path: &'a str,
    user: ExecUser,
}

impl<E: Exec + ?Sized> Sink for Placed<'_, E> {
    type Error = ExecError;

    fn fail(message: String) -> ExecError {
        ExecError::Transport(message)
    }

    async fn put(&mut self, bytes: &[u8], first: bool) -> Result<(), ExecError> {
        let script = if first {
            r#"cat > "$1""#
        } else {
            r#"cat >> "$1""#
        };
        let cmd = ["sh", "-c", script, "sh", self.path];
        let output = self.exec.run(&cmd, self.user, Some(bytes)).await?;
        if output.success() {
            Ok(())
        } else {
            Err(ExecError::failed(&cmd, &output))
        }
    }
}

/// A file on this host, opened for the download.
struct HostFile(tokio::fs::File);

impl Sink for HostFile {
    type Error = String;

    fn fail(message: String) -> String {
        message
    }

    async fn put(&mut self, bytes: &[u8], _first: bool) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        self.0
            .write_all(bytes)
            .await
            .map_err(|err| format!("couldn't write the download: {err}"))
    }
}

/// Streams a response into `sink` from byte `size` on, hashing as it
/// comes. It stops past `most` bytes in all.
#[expect(clippy::too_many_arguments, reason = "one loop for every download")]
async fn pour<S: Sink>(
    response: &mut reqwest::Response,
    url: &str,
    hash: &mut Context,
    mut size: u64,
    total: Option<u64>,
    most: u64,
    sink: &mut S,
    progress: Progress<'_>,
) -> Result<u64, S::Error> {
    let mut buffer = Vec::with_capacity(CHUNK.min(total.unwrap_or(0) as usize));
    let mut first = size == 0;
    progress(size, total);
    loop {
        let bytes = match response.chunk().await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(err) => {
                // What came is kept, so a resume carries on from it.
                if !buffer.is_empty() {
                    sink.put(&buffer, first).await?;
                }
                return Err(S::fail(failure(url, &err)));
            }
        };
        size += bytes.len() as u64;
        if size > most {
            return Err(S::fail(format!(
                "the download of {} is bigger than its {most} bytes",
                short(url)
            )));
        }
        hash.update(&bytes);
        buffer.extend_from_slice(&bytes);
        if buffer.len() >= CHUNK {
            sink.put(&buffer, first).await?;
            first = false;
            buffer.clear();
        }
        progress(size, total);
    }
    if !buffer.is_empty() || first {
        sink.put(&buffer, first).await?;
    }
    Ok(size)
}

fn failure(url: &str, err: &reqwest::Error) -> String {
    format!("the download of {} failed: {}", short(url), plain(err))
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
pub fn short(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// What went wrong, in words and without the URL, which may carry a
/// signature.
pub fn plain(err: &reqwest::Error) -> String {
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

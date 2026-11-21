// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The webview's way into the media cache. A shell registers [`SCHEME`]
//! and hands each request to [`serve`], which answers only for a file the
//! cache named, so a page can't read anything else on this machine
//! through it. It answers byte ranges, which is what lets a video seek
//! without the whole file in memory. The app's own page may also read a
//! file's bytes, through [`serve_to`], to hand them to the file viewer.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use maxhq_client::media;

/// The URI scheme the webview loads cached pictures and videos through.
pub const SCHEME: &str = "maxmedia";

/// The most bytes one answer carries. A player asks for the next range.
const CHUNK: u64 = 4 * 1024 * 1024;

/// The largest file sent whole to a request that names no range. A larger
/// one gets its first chunk, as if that range had been asked.
const WHOLE: u64 = 32 * 1024 * 1024;

/// How the cache's files are kept: their names come from their bytes, so
/// a name always holds the same file.
const CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// A picture opened on its own runs nothing and loads nothing.
const CONTENT_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// An answer for the shell to send.
#[derive(Debug, PartialEq, Eq)]
pub struct Served {
    /// The HTTP status.
    pub status: u16,
    /// The headers, by name.
    pub headers: Vec<(&'static str, String)>,
    /// The body, empty for `HEAD` and for errors.
    pub body: Vec<u8>,
}

impl Served {
    fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// Answers a request for `path`, as the URI has it, from the cache in
/// `folder`. `range` is the request's `Range` header, if it had one.
pub fn serve(folder: &Path, method: &str, path: &str, range: Option<&str>) -> Served {
    let head = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return Served::status(405),
    };
    let name = path.strip_prefix('/').unwrap_or(path);
    if !media::is_cached_name(name) {
        return Served::status(404);
    }
    let mime = name
        .rsplit_once('.')
        .and_then(|(_, extension)| media::mime(extension))
        .unwrap_or("application/octet-stream");
    match answer(&folder.join(name), mime, range, head) {
        Ok(served) => served,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Served::status(404),
        Err(err) => {
            tracing::debug!(%err, name, "couldn't serve a cached file");
            Served::status(500)
        }
    }
}

fn answer(path: &Path, mime: &str, range: Option<&str>, head: bool) -> io::Result<Served> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut headers = vec![
        ("Content-Type", mime.to_owned()),
        ("Accept-Ranges", "bytes".to_owned()),
        ("Cache-Control", CACHE_CONTROL.to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Content-Security-Policy", CONTENT_POLICY.to_owned()),
    ];
    let (status, start, end) = match wanted(range, len) {
        Wanted::Whole if len <= WHOLE => (200, 0, len),
        Wanted::Whole => (206, 0, len.min(CHUNK)),
        Wanted::Part { start, last } => (206, start, (last + 1).min(start + CHUNK)),
        Wanted::Past => {
            headers.push(("Content-Range", format!("bytes */{len}")));
            return Ok(Served {
                status: 416,
                headers,
                body: Vec::new(),
            });
        }
    };
    if status == 206 {
        headers.push(("Content-Range", format!("bytes {start}-{}/{len}", end - 1)));
    }
    headers.push(("Content-Length", (end - start).to_string()));
    let mut body = Vec::new();
    if !head {
        file.seek(SeekFrom::Start(start))?;
        file.take(end - start).read_to_end(&mut body)?;
    }
    Ok(Served {
        status,
        headers,
        body,
    })
}

/// The dev server's origin, which `tauri dev` loads the app's page from.
pub(crate) const DEV_ORIGIN: &str = "http://localhost:1420";

/// The origins the app's own page loads from: the bundle's on macOS and
/// Linux, then on Windows, then the dev server's.
pub(crate) const APP_ORIGINS: [&str; 4] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    DEV_ORIGIN,
];

/// Answers like [`serve`], and lets a request from the app's own page,
/// which `origin` names, read the answer, ranges included. A page on any
/// other origin, such as the viewer's sandboxed frame, can still show a
/// cached picture but can't read its bytes.
pub fn serve_to(
    folder: &Path,
    method: &str,
    path: &str,
    range: Option<&str>,
    origin: Option<&str>,
) -> Served {
    let app = origin.filter(|origin| APP_ORIGINS.contains(origin));
    if method == "OPTIONS" {
        let Some(origin) = app else {
            return Served::status(405);
        };
        return Served {
            status: 204,
            headers: vec![
                ("Access-Control-Allow-Origin", origin.to_owned()),
                ("Access-Control-Allow-Methods", "GET, HEAD".to_owned()),
                ("Access-Control-Allow-Headers", "Range".to_owned()),
                ("Vary", "Origin".to_owned()),
            ],
            body: Vec::new(),
        };
    }
    let mut served = serve(folder, method, path, range);
    if let Some(origin) = app {
        served.headers.extend([
            ("Access-Control-Allow-Origin", origin.to_owned()),
            ("Access-Control-Expose-Headers", "Content-Range".to_owned()),
        ]);
    }
    served.headers.push(("Vary", "Origin".to_owned()));
    served
}

/// What a `Range` header asks for.
#[derive(Debug, PartialEq, Eq)]
enum Wanted {
    /// The whole file, as when there's no header or one this doesn't read.
    Whole,
    /// From `start` to `last`, both in the file.
    Part { start: u64, last: u64 },
    /// Nothing the file has.
    Past,
}

/// The first range a `Range` header asks for. A player asks for one at a
/// time, so only the first of several is answered. A header that doesn't
/// parse is left out, as HTTP says.
fn wanted(range: Option<&str>, len: u64) -> Wanted {
    let Some(spec) = range
        .map(str::trim)
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|ranges| ranges.split(',').next())
        .map(str::trim)
    else {
        return Wanted::Whole;
    };
    let Some((from, to)) = spec.split_once('-') else {
        return Wanted::Whole;
    };
    let (from, to) = (from.trim(), to.trim());
    if from.is_empty() {
        // The last `to` bytes.
        return match to.parse::<u64>() {
            Ok(0) => Wanted::Past,
            Ok(_) if len == 0 => Wanted::Past,
            Ok(suffix) => Wanted::Part {
                start: len.saturating_sub(suffix),
                last: len - 1,
            },
            Err(_) => Wanted::Whole,
        };
    }
    let Ok(start) = from.parse::<u64>() else {
        return Wanted::Whole;
    };
    let last = if to.is_empty() {
        u64::MAX
    } else {
        match to.parse::<u64>() {
            Ok(last) if last >= start => last,
            _ => return Wanted::Whole,
        }
    };
    if start >= len {
        return Wanted::Past;
    }
    Wanted::Part {
        start,
        last: last.min(len - 1),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use maxhq_protocol::ids::LinkId;

    use super::*;

    const SHA: &str = "a3c1e2f4b5d6a7c8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2";

    /// A cache folder holding one video of `len` bytes, removed when it
    /// drops.
    struct Cache(PathBuf);

    impl Cache {
        fn holding(len: usize) -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-media-{}", LinkId::generate()));
            std::fs::create_dir_all(&dir).unwrap();
            let bytes: Vec<u8> = (0..len).map(|at| (at % 251) as u8).collect();
            std::fs::write(dir.join(format!("{SHA}.mp4")), bytes).unwrap();
            std::fs::write(dir.join("secret.txt"), "not for the page").unwrap();
            Self(dir)
        }

        fn get(&self, range: Option<&str>) -> Served {
            serve(&self.0, "GET", &format!("/{SHA}.mp4"), range)
        }
    }

    impl Drop for Cache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn header<'a>(served: &'a Served, name: &str) -> Option<&'a str> {
        served
            .headers
            .iter()
            .find(|(named, _)| *named == name)
            .map(|(_, value)| value.as_str())
    }

    fn byte(at: u64) -> u8 {
        (at % 251) as u8
    }

    #[test]
    fn a_file_comes_whole_with_its_type() {
        let cache = Cache::holding(1000);
        let served = cache.get(None);
        assert_eq!(served.status, 200);
        assert_eq!(served.body.len(), 1000);
        assert_eq!(header(&served, "Content-Type"), Some("video/mp4"));
        assert_eq!(header(&served, "Content-Length"), Some("1000"));
        assert_eq!(header(&served, "Accept-Ranges"), Some("bytes"));
        assert_eq!(header(&served, "Content-Range"), None);
    }

    #[test]
    fn a_range_comes_as_part_of_the_file() {
        let cache = Cache::holding(1000);
        let served = cache.get(Some("bytes=100-199"));
        assert_eq!(served.status, 206);
        assert_eq!(header(&served, "Content-Range"), Some("bytes 100-199/1000"));
        assert_eq!(header(&served, "Content-Length"), Some("100"));
        assert_eq!(served.body[0], byte(100));
        assert_eq!(served.body.len(), 100);

        let open = cache.get(Some("bytes=900-"));
        assert_eq!(header(&open, "Content-Range"), Some("bytes 900-999/1000"));
        let suffix = cache.get(Some("bytes=-10"));
        assert_eq!(header(&suffix, "Content-Range"), Some("bytes 990-999/1000"));
        let past_the_end = cache.get(Some("bytes=990-5000"));
        assert_eq!(past_the_end.body.len(), 10);
        let several = cache.get(Some("bytes=0-1, 5-6"));
        assert_eq!(header(&several, "Content-Range"), Some("bytes 0-1/1000"));
    }

    #[test]
    fn a_range_outside_the_file_says_how_long_it_is() {
        let cache = Cache::holding(1000);
        for range in ["bytes=1000-", "bytes=-0"] {
            let served = cache.get(Some(range));
            assert_eq!(served.status, 416, "{range}");
            assert_eq!(header(&served, "Content-Range"), Some("bytes */1000"));
            assert!(served.body.is_empty());
        }
        // One that doesn't parse is left out.
        for range in ["items=0-1", "bytes=5-1", "bytes=x-"] {
            assert_eq!(cache.get(Some(range)).status, 200, "{range}");
        }
    }

    #[test]
    fn a_large_file_comes_a_chunk_at_a_time() {
        let len = (WHOLE + 10) as usize;
        let cache = Cache::holding(len);
        let first = cache.get(None);
        assert_eq!(first.status, 206);
        assert_eq!(first.body.len() as u64, CHUNK);
        assert_eq!(
            header(&first, "Content-Range"),
            Some(&*format!("bytes 0-{}/{len}", CHUNK - 1))
        );
        let later = cache.get(Some(&format!("bytes={}-", WHOLE)));
        assert_eq!(later.body.len(), 10);
        assert_eq!(later.body[0], byte(WHOLE));
    }

    #[test]
    fn only_a_name_the_cache_gave_is_served() {
        let cache = Cache::holding(10);
        for path in [
            "/secret.txt",
            "/../secret.txt",
            &format!("/{SHA}.mp4/../secret.txt"),
            &format!("/{SHA}.exe"),
            &format!("/sub/{SHA}.mp4"),
        ] {
            assert_eq!(serve(&cache.0, "GET", path, None).status, 404, "{path}");
        }
        let missing = SHA.replace('a', "b");
        assert_eq!(
            serve(&cache.0, "GET", &format!("/{missing}.png"), None).status,
            404
        );
        assert_eq!(
            serve(&cache.0, "POST", &format!("/{SHA}.mp4"), None).status,
            405
        );
    }

    #[test]
    fn code_comes_as_plain_text_that_never_renders() {
        let cache = Cache::holding(10);
        for name in ["html", "svg", "rs"] {
            std::fs::write(cache.0.join(format!("{SHA}.{name}")), "<script>1</script>").unwrap();
        }
        let page = serve(&cache.0, "GET", &format!("/{SHA}.html"), None);
        assert_eq!(page.status, 200);
        assert_eq!(
            header(&page, "Content-Type"),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(header(&page, "X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(
            header(&page, "Content-Security-Policy"),
            Some(CONTENT_POLICY)
        );
        let code = serve(&cache.0, "GET", &format!("/{SHA}.rs"), None);
        assert_eq!(
            header(&code, "Content-Type"),
            Some("text/plain; charset=utf-8")
        );
        let picture = serve(&cache.0, "GET", &format!("/{SHA}.svg"), None);
        assert_eq!(header(&picture, "Content-Type"), Some("image/svg+xml"));
        assert_eq!(header(&picture, "X-Content-Type-Options"), Some("nosniff"));
    }

    #[test]
    fn a_head_request_has_the_headers_and_no_body() {
        let cache = Cache::holding(1000);
        let served = serve(&cache.0, "HEAD", &format!("/{SHA}.mp4"), None);
        assert_eq!(served.status, 200);
        assert_eq!(header(&served, "Content-Length"), Some("1000"));
        assert!(served.body.is_empty());
    }

    #[test]
    fn only_the_app_page_reads_the_bytes() {
        let cache = Cache::holding(1000);
        let path = format!("/{SHA}.mp4");
        let read = serve_to(
            &cache.0,
            "GET",
            &path,
            Some("bytes=0-99"),
            Some("tauri://localhost"),
        );
        assert_eq!(read.status, 206);
        assert_eq!(
            header(&read, "Access-Control-Allow-Origin"),
            Some("tauri://localhost")
        );
        assert_eq!(
            header(&read, "Access-Control-Expose-Headers"),
            Some("Content-Range")
        );
        for origin in [None, Some("null"), Some("https://example.com")] {
            let served = serve_to(&cache.0, "GET", &path, None, origin);
            assert_eq!(served.status, 200, "{origin:?}");
            assert_eq!(
                header(&served, "Access-Control-Allow-Origin"),
                None,
                "{origin:?}"
            );
            let asked = serve_to(&cache.0, "OPTIONS", &path, None, origin);
            assert_eq!(asked.status, 405, "{origin:?}");
        }
        let asked = serve_to(
            &cache.0,
            "OPTIONS",
            &path,
            None,
            Some("http://tauri.localhost"),
        );
        assert_eq!(asked.status, 204);
        assert_eq!(
            header(&asked, "Access-Control-Allow-Headers"),
            Some("Range")
        );
    }
}

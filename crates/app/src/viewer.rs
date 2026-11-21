// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The file viewer's frame. A shell registers [`SCHEME`] and hands each
//! request to [`serve`], which answers with the viewer's page and the built
//! scripts and styles it loads, read from the app's own bundled files. The
//! frame runs sandboxed on an opaque origin, so its scripts come as
//! cross-origin requests and each answer allows any origin. Nothing else is
//! reachable through the scheme, and the page's content policy keeps it off
//! the network: it gets a file's bytes from the pane that holds it.
//!
//! Visuals, web pages agents make, show through the same scheme on a page of
//! their own, [`VISUAL_PAGE`]. Its policy lets the page's own scripts run and
//! load the libraries the app serves at [`LIBRARY`], and nothing from the
//! network but the hosts its frame's URL names, which the user let through
//! for that visual. It can't connect anywhere, even then.

use crate::media::Served;

/// The URI scheme the viewer frame loads from.
pub const SCHEME: &str = "maxview";

/// The viewer's page, as the frame's URL names it.
pub const PAGE: &str = "/viewer.html";

/// What the frame may load: its own scripts and styles, pictures and media
/// it makes from the file's bytes, and WebAssembly. No network, no frames,
/// no forms.
pub const CONTENT_POLICY: &str = "default-src 'none'; \
     script-src maxview: http://maxview.localhost 'wasm-unsafe-eval'; \
     style-src maxview: http://maxview.localhost 'unsafe-inline'; \
     img-src blob: data:; \
     media-src blob: data:; \
     font-src data: maxview: http://maxview.localhost; \
     connect-src blob: data:; \
     worker-src blob:; \
     frame-src 'none'; \
     form-action 'none'; \
     base-uri 'none'";

/// The visual frame's page, as the frame's URL names it.
pub const VISUAL_PAGE: &str = "/visual.html";

/// Where the visual frame serves the libraries a page names as `maxhq:lib/...`.
pub const LIBRARY: &str = "/visual-lib/";

/// The libraries a visual can load, by their file in [`LIBRARY`]. three.js
/// is a module that imports its core from beside it.
pub const LIBRARIES: [&str; 7] = [
    "chart.js",
    "d3.js",
    "three.js",
    "three.core.js",
    "mermaid.js",
    "katex.js",
    "katex.css",
];

/// The most hosts a visual's frame may name.
pub const MOST_HOSTS: usize = 4;

/// What a visual may load: inline scripts and `eval`, which pages and the
/// libraries use, the libraries and its own data, and from the network only
/// `hosts`, each a plain host name that [`plain_host`] took. It can't connect,
/// frame, post a form or change its base URL.
pub fn visual_policy(hosts: &[&str]) -> String {
    let own = "maxview: http://maxview.localhost";
    let extra: String = hosts
        .iter()
        .map(|host| format!(" https://{host}"))
        .collect();
    format!(
        "default-src 'none'; \
         script-src {own} 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob: data:{extra}; \
         style-src {own} 'unsafe-inline' blob: data:{extra}; \
         img-src {own} blob: data:{extra}; \
         font-src {own} data:{extra}; \
         media-src {own} blob: data:; \
         connect-src 'none'; \
         worker-src blob:; \
         frame-src 'none'; \
         form-action 'none'; \
         base-uri 'none'"
    )
}

/// The hosts a visual frame's query names in `hosts=`, comma separated,
/// keeping only plain ones and only the first few. Anything else is left out.
pub fn hosts_in(query: Option<&str>) -> Vec<&str> {
    query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter_map(|pair| pair.strip_prefix("hosts="))
        .flat_map(|hosts| hosts.split(','))
        .filter(|host| plain_host(host))
        .take(MOST_HOSTS)
        .collect()
}

/// Whether `host` is a plain web host name with an optional port, such as
/// `cdn.example.com:8443`: lower case, two labels or more, the last starting
/// with a letter. That leaves out addresses, `localhost` and anything that
/// could add a source of its own to a policy.
pub fn plain_host(host: &str) -> bool {
    let (name, port) = match host.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    let port_ok = port.is_none_or(|port| {
        !port.starts_with('0')
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok()
    });
    let labels: Vec<&str> = name.split('.').collect();
    port_ok
        && name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        && labels
            .last()
            .is_some_and(|last| last.starts_with(|c: char| c.is_ascii_lowercase()))
}

/// A file the app bundles: its bytes and media type.
pub struct Bundled {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Its media type.
    pub mime: String,
}

/// Answers a request for `path` with `query`, as the URI has them, from the
/// files `read` finds in the app's bundle by their path in it.
pub fn serve(
    method: &str,
    path: &str,
    query: Option<&str>,
    read: impl FnOnce(&str) -> Option<Bundled>,
) -> Served {
    let head = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return status(405),
    };
    if !allowed(path) {
        return status(404);
    }
    let Some(Bundled { bytes, mut mime }) = read(path) else {
        return status(404);
    };
    // The bundle hands back the app's own page for a name it doesn't have.
    if path != PAGE && path != VISUAL_PAGE && mime.starts_with("text/html") {
        return status(404);
    }
    if path.ends_with(".woff2") {
        mime = "font/woff2".to_owned();
    }
    let mut headers = vec![
        ("Content-Type", mime),
        ("Access-Control-Allow-Origin", "*".to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-cache".to_owned()),
        ("Content-Length", bytes.len().to_string()),
    ];
    if path == PAGE {
        headers.push(("Content-Security-Policy", CONTENT_POLICY.to_owned()));
    } else if path == VISUAL_PAGE {
        headers.push(("Content-Security-Policy", visual_policy(&hosts_in(query))));
        // The policy doesn't cover looking up a link's host ahead of a click.
        headers.push(("X-DNS-Prefetch-Control", "off".to_owned()));
    }
    Served {
        status: 200,
        headers,
        body: if head { Vec::new() } else { bytes },
    }
}

/// Whether `path` is one of the two pages, a built file in the assets
/// folder, a library or one of KaTeX's fonts, with nothing in its name that
/// could step out of its folder.
fn allowed(path: &str) -> bool {
    if path == PAGE || path == VISUAL_PAGE {
        return true;
    }
    if let Some(name) = path.strip_prefix("/assets/") {
        return plain_name(name);
    }
    let Some(name) = path.strip_prefix(LIBRARY) else {
        return false;
    };
    LIBRARIES.contains(&name)
        || name
            .strip_prefix("fonts/")
            .is_some_and(|font| plain_name(font) && font.ends_with(".woff2"))
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn status(status: u16) -> Served {
    Served {
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(path: &str) -> Option<Bundled> {
        let mime = match path.rsplit_once('.')?.1 {
            "html" => "text/html",
            "js" => "text/javascript",
            "css" => "text/css",
            _ => "application/octet-stream",
        };
        Some(Bundled {
            bytes: path.as_bytes().to_vec(),
            mime: mime.to_owned(),
        })
    }

    fn header<'a>(served: &'a Served, name: &str) -> Option<&'a str> {
        served
            .headers
            .iter()
            .find(|(named, _)| *named == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn the_page_comes_with_its_policy() {
        let served = serve("GET", PAGE, None, bundle);
        assert_eq!(served.status, 200);
        assert_eq!(served.body, PAGE.as_bytes());
        assert_eq!(header(&served, "Content-Type"), Some("text/html"));
        assert_eq!(
            header(&served, "Content-Security-Policy"),
            Some(CONTENT_POLICY)
        );
        assert_eq!(header(&served, "Access-Control-Allow-Origin"), Some("*"));
    }

    #[test]
    fn a_built_script_loads_from_the_opaque_frame() {
        let served = serve("GET", "/assets/viewer-AbC_12.js", None, bundle);
        assert_eq!(served.status, 200);
        assert_eq!(header(&served, "Content-Type"), Some("text/javascript"));
        assert_eq!(header(&served, "Access-Control-Allow-Origin"), Some("*"));
        assert_eq!(header(&served, "Content-Security-Policy"), None);
    }

    #[test]
    fn nothing_else_in_the_bundle_is_served() {
        for path in [
            "/",
            "/index.html",
            "/favicon.svg",
            "/assets/",
            "/assets/../index.html",
            "/assets/sub/x.js",
            "/assets/.hidden",
            "/assets/a%2F..%2Fb.js",
            "/viewer.html/x",
            "/visual.html/x",
            "/visual-lib/",
            "/visual-lib/other.js",
            "/visual-lib/../index.html",
            "/visual-lib/chart.js/x",
            "/visual-lib/fonts/../x.woff2",
            "/visual-lib/fonts/.woff2",
            "/visual-lib/fonts/sub/x.woff2",
            "/visual-lib/fonts/KaTeX_Main-Regular.ttf",
        ] {
            assert_eq!(serve("GET", path, None, bundle).status, 404, "{path}");
        }
        assert_eq!(
            serve("GET", "/assets/missing.js", None, |_| None).status,
            404
        );
        let app_page = |_: &str| {
            Some(Bundled {
                bytes: b"<html>".to_vec(),
                mime: "text/html".to_owned(),
            })
        };
        assert_eq!(
            serve("GET", "/assets/missing.js", None, app_page).status,
            404
        );
        assert_eq!(
            serve("GET", "/visual-lib/chart.js", None, app_page).status,
            404
        );
        assert_eq!(serve("POST", PAGE, None, bundle).status, 405);
        assert!(serve("HEAD", PAGE, None, bundle).body.is_empty());
    }

    #[test]
    fn the_policy_shuts_off_the_network() {
        let directives: Vec<&str> = CONTENT_POLICY.split(';').map(str::trim).collect();
        for directive in [
            "default-src 'none'",
            "connect-src blob: data:",
            "frame-src 'none'",
            "form-action 'none'",
        ] {
            assert!(directives.contains(&directive), "{directive}");
        }
        // The only web origin named is the scheme's own on Windows.
        for source in CONTENT_POLICY.split([';', ' ']) {
            if source.starts_with("http") {
                assert_eq!(source, "http://maxview.localhost");
            }
        }
        assert!(!CONTENT_POLICY.contains('*'));
    }

    #[test]
    fn the_visual_page_comes_with_its_own_policy() {
        let served = serve("GET", VISUAL_PAGE, Some("theme=dark"), bundle);
        assert_eq!(served.status, 200);
        assert_eq!(header(&served, "Content-Type"), Some("text/html"));
        let policy = header(&served, "Content-Security-Policy").unwrap();
        assert_eq!(policy, visual_policy(&[]));
        assert_eq!(header(&served, "X-DNS-Prefetch-Control"), Some("off"));
        let directives: Vec<&str> = policy.split(';').map(str::trim).collect();
        for directive in [
            "default-src 'none'",
            "connect-src 'none'",
            "frame-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
        ] {
            assert!(directives.contains(&directive), "{directive}");
        }
        let scripts = directives
            .iter()
            .find(|directive| directive.starts_with("script-src "))
            .unwrap();
        for source in ["'unsafe-inline'", "'unsafe-eval'", "maxview:"] {
            assert!(scripts.split(' ').any(|named| named == source), "{source}");
        }
        // With no hosts let through, it names no web origin but its own.
        for source in policy.split([';', ' ']) {
            if source.starts_with("http") {
                assert_eq!(source, "http://maxview.localhost");
            }
        }
        assert!(!policy.contains('*'));
    }

    #[test]
    fn hosts_let_through_load_code_styles_fonts_and_pictures_but_never_connect() {
        let served = serve(
            "GET",
            VISUAL_PAGE,
            Some("theme=light&hosts=cdn.example.com,fonts.example.org:8443"),
            bundle,
        );
        let policy = header(&served, "Content-Security-Policy").unwrap();
        for directive in policy.split(';').map(str::trim) {
            let named = directive.contains(" https://cdn.example.com")
                && directive.contains(" https://fonts.example.org:8443");
            let kind = directive.split(' ').next().unwrap();
            assert_eq!(
                named,
                matches!(kind, "script-src" | "style-src" | "img-src" | "font-src"),
                "{directive}"
            );
        }
    }

    #[test]
    fn only_plain_hosts_reach_the_policy() {
        assert_eq!(
            hosts_in(Some(
                "theme=dark&hosts=a.example.com,localhost,127.0.0.1,*.example.com,\
                 b.example.com:443,x.example.com%20'unsafe-eval',c.example.com;script-src,\
                 d.example.com,e.example.com,f.example.com"
            )),
            vec![
                "a.example.com",
                "b.example.com:443",
                "d.example.com",
                "e.example.com"
            ]
        );
        assert!(hosts_in(None).is_empty());
        assert!(hosts_in(Some("theme=dark")).is_empty());
        for host in [
            "cdn.example.com",
            "a-b.example.co.uk",
            "cdn.example.com:65535",
        ] {
            assert!(plain_host(host), "{host}");
        }
        for host in [
            "",
            "localhost",
            "example",
            "10.0.0.1",
            "Example.com",
            "-a.example.com",
            "a-.example.com",
            "a..example.com",
            "example.com:",
            "example.com:0",
            "example.com:080",
            "example.com:65536",
            "example.com:1:2",
            "example.com/x",
            "example.com 'self'",
            "[::1]",
        ] {
            assert!(!plain_host(host), "{host}");
        }
    }

    #[test]
    fn each_library_and_katex_font_loads_from_the_frame() {
        for name in LIBRARIES {
            let path = format!("{LIBRARY}{name}");
            let served = serve("GET", &path, None, bundle);
            assert_eq!(served.status, 200, "{path}");
            assert_eq!(header(&served, "Access-Control-Allow-Origin"), Some("*"));
            assert_eq!(header(&served, "Content-Security-Policy"), None);
        }
        let font = serve(
            "GET",
            "/visual-lib/fonts/KaTeX_Main-Regular.woff2",
            None,
            bundle,
        );
        assert_eq!(font.status, 200);
        assert_eq!(header(&font, "Content-Type"), Some("font/woff2"));
    }
}

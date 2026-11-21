// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where the desktop app's window may go. It shows the app's own page and
//! the frames that page makes, and nothing else, so a link or a script can't
//! turn the window or one of its frames into a web page. The shell sends
//! anything else to the browser instead.

use url::Url;

use crate::media::{APP_ORIGINS, DEV_ORIGIN};
use crate::viewer;

/// Whether the window, or a frame in it, may go to `url`: the app's own
/// page, the viewer's frame pages on its scheme, or an empty frame. The dev
/// server counts only when `dev`, as in a debug build.
pub fn stays_in_app(url: &str, dev: bool) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    if url.scheme() == "about" {
        return matches!(url.path(), "blank" | "srcdoc");
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    let viewer = [
        format!("{}://localhost", viewer::SCHEME),
        format!("http://{}.localhost", viewer::SCHEME),
        format!("https://{}.localhost", viewer::SCHEME),
    ];
    APP_ORIGINS
        .iter()
        .filter(|app| dev || **app != DEV_ORIGIN)
        .copied()
        .chain(viewer.iter().map(String::as_str))
        .any(|own| own.eq_ignore_ascii_case(&origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_and_its_frames_stay() {
        for url in [
            "tauri://localhost",
            "tauri://localhost/index.html#/agents",
            "http://tauri.localhost/",
            "https://tauri.localhost/settings",
            "maxview://localhost/visual.html?theme=dark",
            "maxview://localhost/viewer.html",
            "http://maxview.localhost/visual.html",
            "about:blank",
            "about:srcdoc",
        ] {
            assert!(stays_in_app(url, false), "{url}");
        }
        assert!(stays_in_app("http://localhost:1420/visual.html", true));
    }

    #[test]
    fn anything_else_goes_to_the_browser() {
        for url in [
            "https://example.com/",
            "http://localhost:1420/",
            "http://localhost:1421/",
            "http://localhost/",
            "https://tauri.localhost.example.com/",
            "tauri://example.com/",
            "maxview://example.com/visual.html",
            "maxmedia://localhost/a.png",
            "mailto:someone@example.com",
            "data:text/html,<p>hi</p>",
            "blob:tauri://localhost/0b8e",
            "javascript:alert(1)",
            "file:///etc/hosts",
            "about:config",
            "not a url",
        ] {
            assert!(!stays_in_app(url, false), "{url}");
        }
        assert!(!stays_in_app("https://example.com/", true));
    }
}

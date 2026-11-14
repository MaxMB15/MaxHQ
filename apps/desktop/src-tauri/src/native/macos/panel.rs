// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The picker, an `NSOpenPanel` that takes files and folders together,
//! which the webview's own file input can't do.

use std::cell::Cell;
use std::path::PathBuf;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSView, NSWindow};
use objc2_foundation::NSString;
use tokio::sync::oneshot;

/// Opens the picker as a sheet on `window`, or as its own window when
/// there's none. The answer comes once you choose or cancel, with no paths
/// for a cancel.
pub fn begin(
    view: &NSView,
    window: Option<&NSWindow>,
    labels: &crate::native::AttachmentPickerLabels,
) -> oneshot::Receiver<Vec<PathBuf>> {
    let mtm = MainThreadMarker::from(view);
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(true);
    panel.setAllowsMultipleSelection(true);
    panel.setResolvesAliases(true);
    panel.setPrompt(Some(&NSString::from_str(&labels.prompt)));
    panel.setMessage(Some(&NSString::from_str(&labels.message)));

    let (send, answer) = oneshot::channel();
    let send = Cell::new(Some(send));
    let chosen = panel.clone();
    let done = RcBlock::new(move |response: NSModalResponse| {
        let paths = if response == NSModalResponseOK {
            chosen
                .URLs()
                .iter()
                .filter_map(|url| url.to_file_path())
                .collect()
        } else {
            Vec::new()
        };
        if let Some(send) = send.take() {
            let _ = send.send(paths);
        }
    });
    match window {
        Some(window) => panel.beginSheetModalForWindow_completionHandler(window, &done),
        None => panel.beginWithCompletionHandler(&done),
    }
    answer
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminal bytes between the page and the app, which skip JSON both ways.
//! Each window has one terminal channel, whose every message is one record
//! as [`maxhq_app::terminals`] lays it out, sent as raw bytes so the page
//! gets an `ArrayBuffer`. What you type comes as a raw body, with the
//! terminal in a header, and nothing here keeps or logs it.
//!
//! These commands are written by hand, like the native ones that answer
//! with raw bytes, so the generated bindings don't list them.

use std::sync::{Arc, OnceLock};

use maxhq_app::timeline::Sink;
use maxhq_app::App;
use maxhq_protocol::ids::TerminalId;
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request};
use tauri::{State, Webview};

use crate::commands::ChannelName;

/// The commands here, by name.
pub const COMMANDS: [&str; 3] = ["terminal_attach", "terminal_detach", "terminal_input"];

/// The header that names the terminal input goes to.
const TERMINAL_HEADER: &str = "maxhq-terminal";

/// The window's terminal channel as the page names it.
type RecordChannelName = ChannelName<Vec<u8>>;

/// Attaches a terminal to this window from byte `after` of its output on,
/// and gives the window's number for it, which its records carry. The
/// output comes through `channel`, the window's one terminal channel.
#[tauri::command]
pub async fn terminal_attach(
    webview: Webview,
    app: State<'_, App>,
    channel: RecordChannelName,
    terminal: TerminalId,
    after: Option<u64>,
) -> Result<u32, String> {
    let label = webview.label().to_owned();
    let sink = Arc::new(RecordChannel::new(channel, webview));
    app.attach_terminal(&label, sink, terminal, after)
        .await
        .map_err(|error| error.to_string())
}

/// Takes back this window's attach of a terminal.
#[tauri::command]
pub async fn terminal_detach(
    webview: Webview,
    app: State<'_, App>,
    terminal: TerminalId,
) -> Result<(), String> {
    app.detach_terminal(webview.label(), terminal).await;
    Ok(())
}

/// Sends the bytes in the body to the terminal the header names.
#[tauri::command]
pub async fn terminal_input(app: State<'_, App>, request: Request<'_>) -> Result<(), String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("Terminal input must come as raw bytes".to_owned());
    };
    let terminal = request
        .headers()
        .get(TERMINAL_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<TerminalId>().ok())
        .ok_or_else(|| format!("Terminal input needs the terminal in {TERMINAL_HEADER}"))?;
    app.terminal_input(terminal, bytes.clone())
        .await
        .map_err(|error| error.to_string())
}

/// A window's terminal channel, which sends each record as raw bytes. As
/// with the other channels, Tauri's half is only made for the first
/// record, since dropping it tells the page the channel ended.
struct RecordChannel {
    name: RecordChannelName,
    webview: Webview,
    channel: OnceLock<Channel<InvokeResponseBody>>,
}

impl RecordChannel {
    fn new(name: RecordChannelName, webview: Webview) -> Self {
        Self {
            name,
            webview,
            channel: OnceLock::new(),
        }
    }
}

impl Sink<Vec<u8>> for RecordChannel {
    fn id(&self) -> u32 {
        self.name.id
    }

    fn send(&self, records: Vec<Vec<u8>>) -> bool {
        let channel = self
            .channel
            .get_or_init(|| self.name.name.channel_on(self.webview.clone()));
        records
            .into_iter()
            .all(|record| channel.send(InvokeResponseBody::Raw(record)).is_ok())
    }
}

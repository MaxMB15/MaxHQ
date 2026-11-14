// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stops one agent on a device, for cleaning up after the `turn` example
//! when its own stop didn't get through.
//!
//! `cargo run -p maxhq-client --example stop -- local <agent>` stops the
//! agent on this machine. It tries again while the link connects.

use std::time::Duration;

use maxhq_client::{Client, LinkError, LinkSpec};
use maxhq_protocol::ids::{AgentId, IdempotencyKey, LinkId};
use maxhq_protocol::link::Route;
use maxhq_protocol::message::ClientMessage;

/// How long to keep trying while the link connects.
const LIMIT: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [host, agent] = <[String; 2]>::try_from(args).unwrap_or_else(|_| {
        eprintln!("usage: stop local|<ssh host> <agent>");
        std::process::exit(2);
    });
    let agent: AgentId = agent.parse().unwrap_or_else(|_| {
        eprintln!("{agent} isn't an agent ID");
        std::process::exit(2);
    });
    let route = if host == "local" {
        Route::Local
    } else {
        Route::Ssh { host: host.clone() }
    };
    let (link, mut events) = Client::new().link(LinkSpec {
        id: LinkId::derive(&host),
        name: host,
        route,
        fallbacks: Vec::new(),
        last_good: Default::default(),
    });
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    let key = IdempotencyKey::generate();
    let deadline = tokio::time::Instant::now() + LIMIT;
    loop {
        match link.act(ClientMessage::StopAgent { key, agent }).await {
            Ok(outcome) => {
                println!("stop: {outcome:?}");
                return;
            }
            Err(LinkError::NotConnected) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => {
                eprintln!("stop failed: {error}");
                std::process::exit(1);
            }
        }
    }
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Measures how quick requests stay while a big upload goes over a link.
//!
//! `cargo run --release -p maxhq-client --example upload-bench -- local`
//! times pings to `maxd` on this machine while idle, then again during a
//! 200 MB upload to the device's attachments. It passes when the 95th
//! percentile under load stays within 200 ms of the idle median. Give an
//! SSH host in place of `local` to measure that device instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use maxhq_client::{Client, LinkError, LinkHandle, LinkSpec, TransferState};
use maxhq_protocol::ids::LinkId;
use maxhq_protocol::link::Route;
use maxhq_protocol::message::UploadTarget;

/// How long to keep trying while the link connects.
const LIMIT: Duration = Duration::from_secs(60);
/// How much the upload sends.
const SIZE: usize = 200 * 1000 * 1000;
/// How many pings to time while idle.
const IDLE: usize = 50;
/// The pause between pings.
const GAP: Duration = Duration::from_millis(50);
/// How far the loaded 95th percentile may be above the idle median.
const SLACK: Duration = Duration::from_millis(200);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [host] = <[String; 1]>::try_from(args).unwrap_or_else(|_| {
        eprintln!("usage: upload-bench local|<ssh host>");
        std::process::exit(2);
    });
    let route = if host == "local" {
        Route::Local
    } else {
        Route::Ssh { host: host.clone() }
    };
    let (link, mut events) = Client::new().link(LinkSpec {
        id: LinkId::derive(&host),
        name: host.clone(),
        route,
        fallbacks: Vec::new(),
        last_good: Default::default(),
    });
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    wait_for(&link).await;
    if !link.capabilities().transfers {
        eprintln!("maxd on {host} doesn't do file transfers, so there's nothing to measure");
        std::process::exit(3);
    }

    let mut idle = Vec::with_capacity(IDLE);
    for _ in 0..IDLE {
        idle.push(ping(&link).await);
        tokio::time::sleep(GAP).await;
    }

    let bytes: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    let uploading = AtomicBool::new(true);
    let started = Instant::now();
    let (result, mut loaded) = tokio::join!(
        async {
            let result = link
                .upload_bytes(
                    UploadTarget::Device,
                    "upload-bench.bin".into(),
                    "application/octet-stream",
                    bytes,
                    |progress| {
                        if progress.state == TransferState::Paused {
                            eprintln!("the link dropped at {} bytes", progress.sent);
                        }
                    },
                )
                .await;
            uploading.store(false, Ordering::Relaxed);
            result
        },
        async {
            let mut loaded = Vec::new();
            while uploading.load(Ordering::Relaxed) {
                loaded.push(ping(&link).await);
                tokio::time::sleep(GAP).await;
            }
            loaded
        }
    );
    let took = started.elapsed();
    if let Err(error) = result {
        fail(&format!("the upload failed: {error}"));
    }
    if loaded.is_empty() {
        fail("the upload ended before a ping went out");
    }

    let rate = SIZE as f64 / took.as_secs_f64() / 1e6;
    println!(
        "upload: {SIZE} bytes in {:.1} s, {rate:.1} MB/s",
        took.as_secs_f64()
    );
    idle.sort_unstable();
    loaded.sort_unstable();
    let (idle_p50, idle_p95) = (percentile(&idle, 0.5), percentile(&idle, 0.95));
    let (loaded_p50, loaded_p95) = (percentile(&loaded, 0.5), percentile(&loaded, 0.95));
    println!(
        "idle:   {} pings, p50 {}, p95 {}",
        idle.len(),
        ms(idle_p50),
        ms(idle_p95)
    );
    println!(
        "loaded: {} pings, p50 {}, p95 {}",
        loaded.len(),
        ms(loaded_p50),
        ms(loaded_p95)
    );
    if loaded_p95 <= idle_p50 + SLACK {
        println!("pass: loaded p95 is within {} of idle p50", ms(SLACK));
    } else {
        println!("fail: loaded p95 is more than {} over idle p50", ms(SLACK));
        std::process::exit(1);
    }
}

/// Pings until one gets through, while the link connects.
async fn wait_for(link: &LinkHandle) {
    let deadline = tokio::time::Instant::now() + LIMIT;
    loop {
        match link.ping().await {
            Ok(_) => return,
            Err(LinkError::NotConnected) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => fail(&format!("couldn't reach maxd: {error}")),
        }
    }
}

/// Times one ping, and stops the bench if it gets no pong.
async fn ping(link: &LinkHandle) -> Duration {
    link.ping()
        .await
        .unwrap_or_else(|error| fail(&format!("a ping failed: {error}")))
}

/// The value `q` of the way through samples sorted already.
fn percentile(sorted: &[Duration], q: f64) -> Duration {
    let at = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[at]
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn fail(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(1);
}

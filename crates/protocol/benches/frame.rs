// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Frame codec throughput, for the nightly regression check.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use maxhq_protocol::frame::{encode_message, encode_terminal_output, Frame, FrameDecoder};
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::state::{Delta, Seq};
use maxhq_protocol::ServerMessage;

/// A PTY read is at most 64 KiB, so that's the frame size that matters.
const CHUNK: usize = 64 * 1024;
const CHUNKS: usize = 64;

fn terminal(c: &mut Criterion) {
    let session = SessionId::generate();
    let bytes = vec![b'y'; CHUNK];
    let mut group = c.benchmark_group("terminal");
    group.throughput(Throughput::Bytes((CHUNK * CHUNKS) as u64));

    group.bench_function("encode", |b| {
        let mut out = Vec::with_capacity((CHUNK + 32) * CHUNKS);
        b.iter(|| {
            out.clear();
            for _ in 0..CHUNKS {
                encode_terminal_output(&mut out, session, black_box(&bytes));
            }
            black_box(&out);
        });
    });

    let mut stream = Vec::new();
    for _ in 0..CHUNKS {
        encode_terminal_output(&mut stream, session, &bytes);
    }
    group.bench_function("decode", |b| {
        let mut decoder = FrameDecoder::new();
        b.iter(|| {
            // Arrives in 16 KiB reads, as from a socket.
            for read in stream.chunks(16 * 1024) {
                decoder.push(black_box(read));
                while let Some(frame) = decoder.next_frame().unwrap() {
                    if let Frame::TerminalOutput { bytes, .. } = frame {
                        black_box(bytes);
                    }
                }
            }
        });
    });
    group.finish();
}

fn messages(c: &mut Criterion) {
    let delta = ServerMessage::Delta(Delta {
        seq: Seq(1),
        changes: Vec::new(),
    });
    c.bench_function("message/encode_empty_delta", |b| {
        let mut out = Vec::new();
        b.iter(|| {
            out.clear();
            encode_message(&mut out, black_box(&delta)).unwrap();
        });
    });
}

criterion_group!(benches, terminal, messages);
criterion_main!(benches);

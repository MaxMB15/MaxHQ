// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals over a link: output frames to the sink, input frames to
//! `maxd`, and attaching again after a reconnect.

use maxhq_protocol::frame::MAX_TERMINAL_CHUNK;
use maxhq_protocol::ids::TerminalId;

use super::*;
use crate::terminals::tests::{terminal, Noted};

impl FakeMaxd {
    /// Sends a terminal's output from `offset`.
    async fn output(&mut self, terminal: TerminalId, offset: u64, bytes: &[u8]) {
        let mut out = Vec::new();
        frame::encode_terminal_output(&mut out, terminal, offset, bytes);
        self.stream.write_all(&out).await.unwrap();
    }

    /// The next terminal input frame, skipping messages.
    async fn input(&mut self) -> (TerminalId, Vec<u8>) {
        let mut buf = [0; 4096];
        loop {
            if let Some(Frame::TerminalInput { terminal, bytes }) =
                self.decoder.next_frame().unwrap()
            {
                return (terminal, bytes.to_vec());
            }
            let read = tokio::time::timeout(PATIENCE, self.stream.read(&mut buf))
                .await
                .expect("a frame in time");
            match read {
                Ok(0) | Err(_) => panic!("the client closed"),
                Ok(n) => self.decoder.push(&buf[..n]),
            }
        }
    }
}

/// Waits until the sink has `count` notes, and gives them.
async fn notes(sink: &Noted, count: usize) -> Vec<(TerminalId, u64, Option<Vec<u8>>)> {
    let wait = async {
        while sink.notes().len() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .expect("the notes in time");
    sink.notes()
}

#[tokio::test]
async fn a_terminal_streams_takes_input_and_attaches_again_after_a_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;
    let (shell, other) = (terminal(1), terminal(2));
    let sink = Arc::new(Noted::default());

    handle
        .attach_terminal(shell, None, sink.clone())
        .await
        .unwrap();
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::TerminalAttach {
            terminal: shell,
            after: None
        })
    );
    maxd.send(&ServerMessage::TerminalAttached {
        terminal: shell,
        from: 0,
        reset: false,
    })
    .await;
    maxd.output(shell, 0, b"$ ").await;
    // Output for a terminal this link never attached goes nowhere.
    maxd.output(other, 0, b"not ours").await;
    maxd.output(shell, 2, b"ls\r\n").await;
    assert_eq!(
        notes(&sink, 2).await,
        vec![
            (shell, 0, Some(b"$ ".to_vec())),
            (shell, 2, Some(b"ls\r\n".to_vec()))
        ]
    );

    // Input goes in frames of at most a chunk each. The test compares it
    // without printing it.
    let typed = vec![b'x'; MAX_TERMINAL_CHUNK + 10];
    handle.terminal_input(shell, &typed).await.unwrap();
    let (first, head) = maxd.input().await;
    let (second, tail) = maxd.input().await;
    assert!(first == shell && second == shell, "both went to the shell");
    assert!(
        head.len() == MAX_TERMINAL_CHUNK && [head, tail].concat() == typed,
        "the input came through whole, in two frames"
    );

    // A reconnect attaches it again from where its output got to.
    drop(maxd);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(
        handle.terminal_input(shell, b"y").await,
        Err(LinkError::NotConnected),
        "input waits for nothing"
    );
    let mut maxd = rig.next_maxd().await;
    maxd.accept(instance, "boot-1").await;
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::TerminalAttach {
            terminal: shell,
            after: Some(5)
        })
    );
    // The ring moved on meanwhile, so the shell starts over.
    maxd.send(&ServerMessage::TerminalAttached {
        terminal: shell,
        from: 4_096,
        reset: true,
    })
    .await;
    maxd.output(shell, 4_096, b"% ").await;
    assert_eq!(
        notes(&sink, 4).await[2..],
        [(shell, 4_096, None), (shell, 4_096, Some(b"% ".to_vec()))]
    );

    handle.detach_terminal(shell).await.unwrap();
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::TerminalDetach { terminal: shell })
    );
    // Detaching twice sends nothing more.
    handle.detach_terminal(shell).await.unwrap();
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
}

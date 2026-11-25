// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd mcp --key-file <path>`: the agent tools as a stdio MCP server, for
//! a tool outside MaxHQ that holds an access key, or a Cursor agent's CLI
//! with its session key. It reads one JSON-RPC
//! message a line, signs in to the `maxd` on this device with the key, and
//! sends each tool call there. It logs only to stderr, since stdout is the
//! MCP line, and never logs the key.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use maxhq_core::paths;
use maxhq_core::tools::{self, Mcp, Tool};
use maxhq_os::ipc::LocalStream;
use maxhq_protocol::agents::{ClientRole, ToolCall};
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{ClientId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, ClientHello, ClientInfo};
use maxhq_protocol::{ClientMessage, ProtocolRange, Secret, ServerMessage};
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, Lines,
};
use tokio::time::Instant;

/// `maxd mcp`. Serves until stdin closes, or `maxd` goes away and doesn't
/// come back.
pub fn run(key_file: &Path) -> ExitCode {
    let key = match super::read_key(key_file) {
        Ok(key) => key,
        Err(err) => {
            eprintln!("maxd mcp: couldn't read the key file: {err}");
            return ExitCode::FAILURE;
        }
    };
    let Some(socket) = paths::socket() else {
        eprintln!("maxd mcp: can't tell where maxd's socket is");
        return ExitCode::FAILURE;
    };
    // The read of stdin can't be cancelled, and a runtime dropped the
    // usual way waits for it, so the process would stay up after an
    // error until stdin sent a line. This one goes down without waiting.
    let served = maxhq_os::rt::block_on(async {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let connect = || LocalStream::connect(&socket);
        serve(key, connect, stdin, tokio::io::stdout()).await
    });
    let served = match served {
        Ok(served) => served,
        Err(err) => {
            eprintln!("maxd mcp: couldn't start: {err}");
            return ExitCode::FAILURE;
        }
    };
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("maxd mcp: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The tools a key gets.
fn key_tools() -> Vec<Tool> {
    Tool::ALL
        .into_iter()
        .filter(|tool| tool.for_key())
        .collect()
}

/// What a call in flight answers when `maxd` goes away before its answer.
const RESTARTED: &str = "MaxHQ restarted during this call. Try it again.";

/// How long a `maxd` that went away has to come back, as one does when
/// it's updated: its agents' CLIs outlive it through `maxd hold`.
const REDIAL_FOR: Duration = Duration::from_secs(120);

/// The first pause between dials, which doubles up to [`REDIAL_PAUSE_MAX`].
const REDIAL_PAUSE: Duration = Duration::from_millis(100);
const REDIAL_PAUSE_MAX: Duration = Duration::from_secs(2);

/// Signs in to the `maxd` that `connect` reaches with `key`, then answers
/// MCP on `input` and `output` until `input` ends. When `maxd` goes away,
/// each call in flight fails, and `connect` dials it again until it's
/// back or [`REDIAL_FOR`] runs out.
pub async fn serve<C, F, M, I, O>(
    key: Secret,
    mut connect: C,
    input: I,
    output: O,
) -> io::Result<()>
where
    C: FnMut() -> F,
    F: Future<Output = io::Result<M>>,
    M: AsyncRead + AsyncWrite + Unpin,
    I: AsyncBufRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let mut maxd = connect()
        .await
        .map_err(|err| io::Error::other(format!("couldn't reach maxd: {err}")))?;
    let mut serving = Serving {
        tools: key_tools(),
        client: ClientId::generate(),
        lines: input.lines(),
        waiting: Vec::new(),
        calls: HashMap::new(),
        input_open: true,
        output,
    };
    // When `maxd` went away with no sign-in since.
    let mut down_since = None;
    loop {
        if serving.over(&key, maxd).await? {
            down_since = None;
        }
        for (_, id) in serving.calls.drain() {
            write_line(
                &mut serving.output,
                &tools::mcp_result(&id, RESTARTED, true),
            )
            .await?;
        }
        if !serving.input_open {
            return Ok(());
        }
        let since = *down_since.get_or_insert_with(Instant::now);
        maxd = redial(&mut connect, since, REDIAL_FOR).await?;
    }
}

/// Dials `maxd` again until it answers or `window` from `since` runs out.
async fn redial<C, F, M>(connect: &mut C, since: Instant, window: Duration) -> io::Result<M>
where
    C: FnMut() -> F,
    F: Future<Output = io::Result<M>>,
{
    let mut pause = REDIAL_PAUSE;
    loop {
        if since.elapsed() >= window {
            return Err(io::Error::other("maxd closed the connection"));
        }
        if let Ok(maxd) = connect().await {
            return Ok(maxd);
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(REDIAL_PAUSE_MAX);
    }
}

/// What `maxd mcp` keeps from one connection to `maxd` to the next.
struct Serving<I, O> {
    tools: Vec<Tool>,
    client: ClientId,
    lines: Lines<I>,
    /// The lines that came before the sign-in finished.
    waiting: Vec<String>,
    /// The calls in flight, by their key, with the MCP ID each answers.
    calls: HashMap<IdempotencyKey, Value>,
    input_open: bool,
    output: O,
}

impl<I: AsyncBufRead + Unpin, O: AsyncWrite + Unpin> Serving<I, O> {
    /// Signs in over `maxd` and serves until `input` ends with nothing in
    /// flight, or `maxd` goes away. Says whether the sign-in finished.
    async fn over<M: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        key: &Secret,
        maxd: M,
    ) -> io::Result<bool> {
        let (mut from_maxd, mut to_maxd) = tokio::io::split(maxd);
        let signed_in = IdempotencyKey::generate();
        let hello = ClientMessage::Hello(ClientHello {
            protocol: ProtocolRange::OURS,
            client: ClientInfo {
                name: "maxd mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            client_id: self.client,
            role: ClientRole::Mcp,
        });
        let auth = ClientMessage::AuthKey {
            key: signed_in,
            secret: key.clone(),
        };
        if send(&mut to_maxd, &hello).await.is_err() || send(&mut to_maxd, &auth).await.is_err() {
            return Ok(false);
        }
        let mut decoder = FrameDecoder::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut ready = false;
        loop {
            tokio::select! {
                line = self.lines.next_line(), if self.input_open => match line? {
                    Some(line) if ready => {
                        handle(&line, &self.tools, &mut self.calls, &mut to_maxd, &mut self.output).await?;
                    }
                    Some(line) => self.waiting.push(line),
                    None => {
                        self.input_open = false;
                        if self.calls.is_empty() {
                            return Ok(ready);
                        }
                    }
                },
                read = from_maxd.read(&mut buf) => {
                    let read = match read {
                        Ok(0) | Err(_) => return Ok(ready),
                        Ok(read) => read,
                    };
                    decoder.push(&buf[..read]);
                    while let Some(frame) = decoder.next_frame().map_err(io::Error::other)? {
                        let Frame::Message(json) = frame else { continue };
                        let Ok(ServerMessage::ActionResult { key, outcome }) = serde_json::from_slice(json) else {
                            continue;
                        };
                        if key == signed_in {
                            match outcome {
                                ActionOutcome::Done => ready = true,
                                ActionOutcome::Refused { reason } => return Err(io::Error::other(reason)),
                                other => return Err(io::Error::other(format!("maxd answered {other:?}"))),
                            }
                            for line in std::mem::take(&mut self.waiting) {
                                handle(&line, &self.tools, &mut self.calls, &mut to_maxd, &mut self.output).await?;
                            }
                            continue;
                        }
                        let Some(id) = self.calls.remove(&key) else { continue };
                        let answer = match outcome {
                            ActionOutcome::Tool { answer } => tools::mcp_answer(&id, &answer.text, &answer.images, answer.failed),
                            ActionOutcome::Refused { reason } => tools::mcp_result(&id, &reason, true),
                            other => tools::mcp_result(&id, &format!("MaxHQ answered {other:?}."), true),
                        };
                        write_line(&mut self.output, &answer).await?;
                    }
                    if !self.input_open && self.calls.is_empty() {
                        return Ok(ready);
                    }
                }
            }
        }
    }
}

/// Answers one MCP line, or sends its tool call to `maxd`.
async fn handle<W: AsyncWrite + Unpin, O: AsyncWrite + Unpin>(
    line: &str,
    tools: &[Tool],
    calls: &mut HashMap<IdempotencyKey, Value>,
    to_maxd: &mut W,
    output: &mut O,
) -> io::Result<()> {
    if line.trim().is_empty() {
        return Ok(());
    }
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        let error = serde_json::json!({
            "jsonrpc": "2.0", "id": null,
            "error": {"code": -32700, "message": "The line isn't JSON."},
        });
        return write_line(output, &error).await;
    };
    // A stdio server answers no notification.
    if message.get("id").is_none_or(Value::is_null) {
        return Ok(());
    }
    match tools::mcp(&message, env!("CARGO_PKG_VERSION"), tools) {
        Mcp::Answer(answer) => write_line(output, &answer).await,
        Mcp::Run { id, call, .. } => {
            let params = message.get("params").unwrap_or(&Value::Null);
            let key = IdempotencyKey::generate();
            calls.insert(key, id);
            let call = ToolCall {
                tool: call.tool().name().to_owned(),
                arguments: params.get("arguments").cloned().unwrap_or(Value::Null),
            };
            send(to_maxd, &ClientMessage::CallTool { key, call }).await
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(to: &mut W, message: &ClientMessage) -> io::Result<()> {
    let mut out = Vec::new();
    frame::encode_message(&mut out, message).map_err(io::Error::other)?;
    to.write_all(&out).await?;
    to.flush().await
}

async fn write_line<O: AsyncWrite + Unpin>(output: &mut O, value: &Value) -> io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    output.write_all(line.as_bytes()).await?;
    output.flush().await
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::agents::{ToolAnswer, ToolImage};
    use serde_json::json;
    use tokio::io::{duplex, BufReader, DuplexStream};

    use super::*;

    /// Reads the client messages `maxd mcp` sent so far.
    async fn heard(
        maxd: &mut DuplexStream,
        decoder: &mut FrameDecoder,
        want: usize,
    ) -> Vec<ClientMessage> {
        let mut found = Vec::new();
        let mut buf = vec![0u8; 8192];
        loop {
            while let Some(frame) = decoder.next_frame().unwrap() {
                if let Frame::Message(json) = frame {
                    found.push(serde_json::from_slice(json).unwrap());
                }
            }
            if found.len() >= want {
                return found;
            }
            let read = maxd.read(&mut buf).await.unwrap();
            assert!(read > 0, "maxd mcp hung up");
            decoder.push(&buf[..read]);
        }
    }

    /// A connect function that hands out `streams` in turn, then finds
    /// nothing listening.
    fn dials(
        streams: Vec<DuplexStream>,
    ) -> impl FnMut() -> std::future::Ready<io::Result<DuplexStream>> {
        let mut streams = streams.into_iter();
        move || {
            std::future::ready(
                streams
                    .next()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::ConnectionRefused)),
            )
        }
    }

    /// The sign-in `maxd mcp` sent over `maxd`, answered `Done`.
    async fn signed_in(maxd: &mut DuplexStream, decoder: &mut FrameDecoder) {
        let first = heard(maxd, decoder, 2).await;
        let ClientMessage::AuthKey { key, .. } = &first[1] else {
            panic!("no sign-in");
        };
        answer(maxd, *key, ActionOutcome::Done).await;
    }

    /// The key of the next call `maxd mcp` sent over `maxd`.
    async fn called(maxd: &mut DuplexStream, decoder: &mut FrameDecoder) -> IdempotencyKey {
        match &heard(maxd, decoder, 1).await[0] {
            ClientMessage::CallTool { key, .. } => *key,
            other => panic!("not a call: {other:?}"),
        }
    }

    async fn answer(maxd: &mut DuplexStream, key: IdempotencyKey, outcome: ActionOutcome) {
        let mut out = Vec::new();
        frame::encode_message(&mut out, &ServerMessage::ActionResult { key, outcome }).unwrap();
        maxd.write_all(&out).await.unwrap();
    }

    #[tokio::test]
    async fn a_key_signs_in_and_its_calls_go_to_maxd() {
        let (ours, mut maxd) = duplex(1 << 16);
        let (mut stdin, input) = duplex(1 << 16);
        let (output, stdout) = duplex(1 << 16);
        let server = tokio::spawn(serve(
            Secret::new("mhq_test_key".into()),
            dials(vec![ours]),
            BufReader::new(input),
            output,
        ));
        let mut decoder = FrameDecoder::new();
        let first = heard(&mut maxd, &mut decoder, 2).await;
        let ClientMessage::Hello(hello) = &first[0] else {
            panic!("no hello");
        };
        assert_eq!(hello.role, ClientRole::Mcp);
        let ClientMessage::AuthKey { key, secret } = &first[1] else {
            panic!("no sign-in");
        };
        assert_eq!(secret.expose(), "mhq_test_key");
        let lines = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list_targets", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "send_attachments", "arguments": {"paths": ["a"]}}}),
        ];
        for line in lines {
            stdin
                .write_all(format!("{line}\n").as_bytes())
                .await
                .unwrap();
        }
        // Lines wait for the sign-in.
        answer(&mut maxd, *key, ActionOutcome::Done).await;
        let call = heard(&mut maxd, &mut decoder, 1).await;
        let ClientMessage::CallTool { key, call } = &call[0] else {
            panic!("no call");
        };
        assert_eq!(call.tool, "list_targets");
        answer(
            &mut maxd,
            *key,
            ActionOutcome::Tool {
                answer: ToolAnswer {
                    text: "{}".into(),
                    failed: false,
                    images: vec![ToolImage {
                        mime: "image/png".into(),
                        data: "iVBORw0K".into(),
                    }],
                },
            },
        )
        .await;
        drop(stdin);
        let mut read = BufReader::new(stdout).lines();
        let mut answers = Vec::new();
        while let Some(line) = read.next_line().await.unwrap() {
            answers.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        server.await.unwrap().expect("it serves");
        assert_eq!(answers.len(), 3, "{answers:?}");
        let listed: Vec<&str> = answers[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(listed.contains(&"start_agent") && !listed.contains(&"send_attachments"));
        assert!(listed.contains(&"lease_screenshot"));
        assert_eq!(
            answers[1]["id"], 3,
            "a key's call to a tool it lacks fails at once"
        );
        assert_eq!(answers[2]["id"], 2);
        assert_eq!(answers[2]["result"]["content"][0]["text"], "{}");
        // A screenshot reaches the client as MCP image content.
        assert_eq!(
            answers[2]["result"]["content"][1],
            json!({"type": "image", "data": "iVBORw0K", "mimeType": "image/png"})
        );
    }

    #[tokio::test]
    async fn a_refused_key_ends_the_server() {
        let (ours, mut maxd) = duplex(1 << 16);
        let (_stdin, input) = duplex(1 << 16);
        let server = tokio::spawn(serve(
            Secret::new("mhq_bad".into()),
            dials(vec![ours]),
            BufReader::new(input),
            tokio::io::sink(),
        ));
        let mut decoder = FrameDecoder::new();
        let first = heard(&mut maxd, &mut decoder, 2).await;
        let ClientMessage::AuthKey { key, .. } = &first[1] else {
            panic!("no sign-in");
        };
        answer(
            &mut maxd,
            *key,
            ActionOutcome::Refused {
                reason: "That access key isn't valid.".into(),
            },
        )
        .await;
        let err = server.await.unwrap().expect_err("it stops");
        assert!(err.to_string().contains("isn't valid"));
    }

    #[tokio::test]
    async fn a_restarted_maxd_fails_the_calls_in_flight_and_serves_the_next() {
        let (first, mut old) = duplex(1 << 16);
        let (second, mut new) = duplex(1 << 16);
        let (mut stdin, input) = duplex(1 << 16);
        let (output, stdout) = duplex(1 << 16);
        let server = tokio::spawn(serve(
            Secret::new("mhq_test_key".into()),
            dials(vec![first, second]),
            BufReader::new(input),
            output,
        ));
        let mut stdout = BufReader::new(stdout).lines();
        let call = |id: u64| {
            let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "list_targets", "arguments": {}}});
            format!("{line}\n")
        };
        let mut decoder = FrameDecoder::new();
        signed_in(&mut old, &mut decoder).await;
        stdin.write_all(call(1).as_bytes()).await.unwrap();
        called(&mut old, &mut decoder).await;
        // maxd goes away with the call in flight.
        drop(old);
        let failed: Value =
            serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(failed["id"], 1);
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(failed["result"]["content"][0]["text"], RESTARTED);
        // The new maxd hears the sign-in again, and a call made meanwhile
        // waits for it.
        stdin.write_all(call(2).as_bytes()).await.unwrap();
        let mut decoder = FrameDecoder::new();
        signed_in(&mut new, &mut decoder).await;
        let key = called(&mut new, &mut decoder).await;
        let done = ToolAnswer {
            text: "{}".into(),
            failed: false,
            images: Vec::new(),
        };
        answer(&mut new, key, ActionOutcome::Tool { answer: done }).await;
        let answered: Value =
            serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(answered["id"], 2);
        assert_eq!(answered["result"]["content"][0]["text"], "{}");
        drop(stdin);
        server.await.unwrap().expect("it serves");
    }

    #[tokio::test]
    async fn a_maxd_gone_for_good_is_given_up_on() {
        let since = Instant::now();
        let window = Duration::from_millis(300);
        let err = redial(&mut dials(Vec::new()), since, window)
            .await
            .expect_err("nothing listens");
        assert!(err.to_string().contains("closed the connection"));
        assert!(since.elapsed() >= window);
    }

    #[tokio::test]
    async fn a_key_refused_after_a_restart_ends_the_server() {
        let (first, mut old) = duplex(1 << 16);
        let (second, mut new) = duplex(1 << 16);
        let (_stdin, input) = duplex(1 << 16);
        let server = tokio::spawn(serve(
            Secret::new("mhq_test_key".into()),
            dials(vec![first, second]),
            BufReader::new(input),
            tokio::io::sink(),
        ));
        let mut decoder = FrameDecoder::new();
        signed_in(&mut old, &mut decoder).await;
        drop(old);
        let mut decoder = FrameDecoder::new();
        let first = heard(&mut new, &mut decoder, 2).await;
        let ClientMessage::AuthKey { key, .. } = &first[1] else {
            panic!("no sign-in");
        };
        let refused = ActionOutcome::Refused {
            reason: "That access key isn't valid.".into(),
        };
        answer(&mut new, *key, refused).await;
        let err = server.await.unwrap().expect_err("it stops");
        assert!(err.to_string().contains("isn't valid"));
    }
}

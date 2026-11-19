// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tests of real terminal holders running `/bin/sh` on a pty.
//!
//! The scripts run with `sh -c`, or `sh -i`, bash, zsh or fish with `HOME`,
//! `ENV`, `ZDOTDIR` and the XDG folders pointed at the test's own folder,
//! so no shell reads anyone's startup files.

use maxhq_hold::{
    Commands, Mark, Reached, ShellCommand, TermEvent, TermExit, TermLaunch, TermLink, Terminals,
};
use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::time::Timestamp;

use super::*;

/// How long a holder stays after its program ends in these tests.
const LINGER: Duration = Duration::from_secs(3);

fn terminals(ctx: &Ctx) -> Terminals {
    Terminals::new(ctx.dirs.clone())
        .with_holder(exe(), vec!["__hold".into()])
        .with_detach(Detach::Setsid)
        .with_linger(LINGER)
}

fn terminal_dir(ctx: &Ctx, terminal: TerminalId) -> PathBuf {
    ctx.dirs.sessions.join(terminal.to_string())
}

/// `/bin/sh -c script` in the test's folder, with no startup files.
fn sh(ctx: &Ctx, script: &str, run: bool) -> TermLaunch {
    TermLaunch {
        terminal: TerminalId::generate(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env: quiet_env(ctx),
        cwd: ctx.root.clone(),
        cols: 80,
        rows: 24,
        run,
        nonce: None,
    }
}

fn quiet_env(ctx: &Ctx) -> Vec<(OsString, OsString)> {
    vec![
        ("TERM".into(), "xterm-256color".into()),
        ("HOME".into(), ctx.root.clone().into()),
        ("ENV".into(), "/dev/null".into()),
        ("PS1".into(), "$ ".into()),
    ]
}

async fn start(ctx: &Ctx, launch: &TermLaunch) -> TermLink {
    let terminals = terminals(ctx);
    terminals.spawn(launch).await.unwrap();
    live(&terminals, launch.terminal, None).await
}

async fn live(terminals: &Terminals, terminal: TerminalId, after: Option<u64>) -> TermLink {
    match terminals.connect(terminal, after).await.unwrap() {
        Reached::Live(link) => link,
        Reached::Ended(exit) => panic!("the terminal ended: {exit:?}"),
        Reached::Missing => panic!("the terminal has no folder"),
    }
}

/// What a link has told, with the output and the offset just past it.
struct Seen {
    bytes: Vec<u8>,
    end: u64,
    exit: Option<TermExit>,
    password: bool,
    busy: bool,
    title: Option<String>,
    cwd: Option<String>,
    marks: Vec<(u64, Mark)>,
}

/// The state with only the end of the output, which can run to megabytes.
impl std::fmt::Debug for Seen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tail = &self.bytes[self.bytes.len().saturating_sub(300)..];
        f.debug_struct("Seen")
            .field("tail", &String::from_utf8_lossy(tail))
            .field("end", &self.end)
            .field("exit", &self.exit)
            .field("password", &self.password)
            .field("busy", &self.busy)
            .field("title", &self.title)
            .field("cwd", &self.cwd)
            .field("marks", &self.marks)
            .finish()
    }
}

impl Seen {
    fn from(link: &TermLink) -> Self {
        Self {
            bytes: Vec::new(),
            end: link.hello.from,
            exit: None,
            password: link.hello.password,
            busy: link.hello.busy,
            title: link.hello.title.clone(),
            cwd: link.hello.cwd.clone(),
            marks: Vec::new(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// Reads events until `done` says so, checking output comes with no
    /// gap unless `gaps`.
    async fn until(&mut self, link: &mut TermLink, gaps: bool, done: impl Fn(&Self) -> bool) {
        while !done(self) {
            let event = match tokio::time::timeout(WAIT, link.events.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => panic!("the events ended at {self:?}"),
                Err(_) => panic!("nothing new within {WAIT:?} at {self:?}"),
            };
            match event {
                TermEvent::Output { offset, data } => {
                    if !gaps {
                        assert_eq!(offset, self.end, "a gap before {:?}", self.text());
                    }
                    assert!(offset >= self.end, "output went back");
                    self.end = offset + data.len() as u64;
                    self.bytes.extend(data);
                }
                TermEvent::Password(on) => self.password = on,
                TermEvent::Busy(on) => self.busy = on,
                TermEvent::Title(title) => self.title = Some(title),
                TermEvent::Cwd(cwd) => self.cwd = Some(cwd),
                TermEvent::Mark { offset, mark, .. } => self.marks.push((offset, mark)),
                TermEvent::Exit(exit) => {
                    assert!(self.exit.is_none(), "two exits");
                    self.exit = Some(exit);
                }
            }
        }
    }

    async fn until_text(&mut self, link: &mut TermLink, text: &str) {
        self.until(link, false, |seen| seen.text().contains(text))
            .await;
    }

    async fn until_exit(&mut self, link: &mut TermLink) -> TermExit {
        self.until(link, false, |seen| seen.exit.is_some()).await;
        self.exit.clone().unwrap()
    }
}

fn alive(pid: u32) -> bool {
    rustix::process::test_kill_process(super::pid(pid)).is_ok()
}

pub(super) async fn a_terminal_replays_from_an_offset_without_a_gap(ctx: Ctx) {
    let script = "stty -echo; echo ready; while read line; do echo \"got:$line\"; stty size; done";
    let launch = sh(&ctx, script, false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    assert_eq!(link.hello.from, 0);
    assert!(!link.hello.reset);
    assert_eq!((link.hello.cols, link.hello.rows), (80, 24));
    let mut seen = Seen::from(&link);
    seen.until_text(&mut link, "ready").await;
    assert!(link.control.input(b"one\n".to_vec()));
    seen.until_text(&mut link, "24 80").await;
    let first = seen.end;
    // Input sent just before letting go still gets there.
    assert!(link.control.resize(100, 30));
    assert!(link.control.input(b"two\n".to_vec()));
    drop(link);

    let mut again = live(&terminals, launch.terminal, Some(first)).await;
    assert_eq!((again.hello.from, again.hello.reset), (first, false));
    assert_eq!((again.hello.cols, again.hello.rows), (100, 30));
    let mut rest = Seen::from(&again);
    rest.until_text(&mut again, "30 100").await;
    assert!(rest.text().contains("got:two"), "{}", rest.text());
    assert!(!rest.text().contains("got:one"), "{}", rest.text());

    // With nothing, a client gets all the holder keeps.
    let mut whole = live(&terminals, launch.terminal, None).await;
    assert_eq!((whole.hello.from, whole.hello.reset), (0, false));
    let mut all = Seen::from(&whole);
    all.until(&mut whole, false, |seen| seen.end >= rest.end)
        .await;
    let text = all.text();
    let one = text.find("got:one").unwrap();
    let two = text.find("got:two").unwrap();
    assert!(one < two, "{text}");
    // Output the program never printed isn't there either.
    let past = live(&terminals, launch.terminal, Some(rest.end + 10)).await;
    assert!(past.hello.reset);
    drop((whole, past));

    assert!(again.control.kill(Duration::from_secs(5)));
    let exit = rest.until_exit(&mut again).await;
    assert_eq!(exit.signal, Some(1), "{exit:?}");
    assert_eq!(exit.output, None);
}

pub(super) async fn a_reader_past_the_ring_starts_over_at_a_whole_line(ctx: Ctx) {
    let line = "0123456789abcdef0123456789abcdef";
    let script = format!("stty -echo; yes {line} | head -c 3145728; echo; echo done; read wait");
    let launch = sh(&ctx, &script, false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    // This reader may fall behind the ring itself, which shows as a jump.
    seen.until(&mut link, true, |seen| seen.bytes.ends_with(b"done\r\n"))
        .await;
    let end = seen.end;
    assert!(end > 3 * 1024 * 1024, "{end}");

    let mut behind = live(&terminals, launch.terminal, Some(0)).await;
    assert!(behind.hello.reset);
    let from = behind.hello.from;
    assert!(from > end - 2 * 1024 * 1024 - 1, "{from} of {end}");
    let mut caught = Seen::from(&behind);
    caught
        .until(&mut behind, false, |seen| seen.end == end)
        .await;
    assert!(
        caught.text().starts_with(line),
        "{:?}",
        &caught.text()[..40]
    );
    assert!(caught.bytes.ends_with(b"done\r\n"));

    let at_end = live(&terminals, launch.terminal, Some(end)).await;
    assert_eq!((at_end.hello.from, at_end.hello.reset), (end, false));
    drop((link, behind));
    at_end.control.kill(Duration::from_secs(5));
}

pub(super) async fn a_password_prompt_is_flagged_and_its_answer_kept_nowhere(ctx: Ctx) {
    let canary = format!("canary-{}", TerminalId::generate().0.simple());
    let script = "printf 'Password: '; stty -echo; read secret; stty echo; echo; \
                  echo \"length ${#secret}\"; echo after";
    let launch = sh(&ctx, script, true);
    let mut link = start(&ctx, &launch).await;
    assert!(link.hello.busy, "a run is busy until it ends");
    let mut seen = Seen::from(&link);
    // Typing before the prompt hides it would echo, so wait for the flag.
    seen.until(&mut link, false, |seen| seen.password).await;
    let holder_pid = link.hello.holder_pid;
    assert!(link.control.input(format!("{canary}\n").into_bytes()));
    seen.until(&mut link, false, |seen| !seen.password).await;
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0));
    let output = exit.output.unwrap();
    let lines: Vec<&str> = output.text.lines().collect();
    assert_eq!(
        lines,
        [
            format!("Password:{}", maxhq_term::SECRET_SENT),
            format!("length {}", canary.len()),
            "after".to_owned()
        ],
        "{:?}",
        output.text
    );
    assert!(!seen.text().contains(&canary));
    // The holder lingers, so its arguments and environment can be read.
    assert!(alive(holder_pid));
    assert!(!holds_bytes(&args_and_env(holder_pid), canary.as_bytes()));
    assert_no_file_has(&ctx.root, &canary);
    let exit_file = fs::read_to_string(terminal_dir(&ctx, launch.terminal).join("exit.json"));
    assert!(exit_file.unwrap().contains("🔒 sent"));
}

pub(super) async fn the_title_and_folder_come_from_the_shell(ctx: Ctx) {
    let script = "stty -echo; printf '\\033]0;my title\\007'; \
                  printf '\\033]7;file://devbox/tmp/my%%20app\\033\\\\'; echo set; read wait";
    let launch = sh(&ctx, script, false);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    seen.until(&mut link, false, |seen| {
        seen.title.is_some() && seen.cwd.is_some()
    })
    .await;
    assert_eq!(seen.title.as_deref(), Some("my title"));
    assert_eq!(seen.cwd.as_deref(), Some("/tmp/my app"));
    let again = live(&terminals(&ctx), launch.terminal, Some(seen.end)).await;
    assert_eq!(again.hello.title.as_deref(), Some("my title"));
    assert_eq!(again.hello.cwd.as_deref(), Some("/tmp/my app"));
    again.control.kill(Duration::from_secs(5));
}

pub(super) async fn a_watch_gets_all_but_the_output(ctx: Ctx) {
    let script = "stty -echo; printf '\\033]0;watched\\007'; echo loud; read line; echo louder";
    let launch = sh(&ctx, script, false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    seen.until_text(&mut link, "loud").await;
    let Reached::Live(mut watch) = terminals.watch(launch.terminal).await.unwrap() else {
        panic!("no watch");
    };
    assert_eq!(watch.hello.title.as_deref(), Some("watched"));
    assert!(link.control.input(b"go\n".to_vec()));
    let mut exit = None;
    while exit.is_none() {
        match tokio::time::timeout(WAIT, watch.events.recv()).await {
            Ok(Some(TermEvent::Output { offset, data })) => {
                panic!("a watch got output at {offset}: {data:?}")
            }
            Ok(Some(TermEvent::Exit(ended))) => exit = Some(ended),
            Ok(Some(_)) => {}
            Ok(None) => panic!("the watch ended without the exit"),
            Err(_) => panic!("no exit within {WAIT:?}"),
        }
    }
    assert_eq!(exit.unwrap().code, Some(0));
    seen.until_exit(&mut link).await;
    assert!(seen.text().contains("louder"), "{}", seen.text());
}

pub(super) async fn interrupt_stops_what_the_shell_runs(ctx: Ctx) {
    let mut launch = sh(&ctx, "", false);
    launch.args = vec!["-i".into()];
    let mut link = start(&ctx, &launch).await;
    assert!(!link.hello.busy);
    let shell = link.hello.child_pid;
    let mut seen = Seen::from(&link);
    seen.until_text(&mut link, "$ ").await;
    assert!(link.control.input(b"sleep 30 && echo slept\n".to_vec()));
    seen.until(&mut link, false, |seen| seen.busy).await;
    assert!(link.control.interrupt());
    seen.until(&mut link, false, |seen| !seen.busy).await;
    assert!(alive(shell), "the interrupt reached the shell");
    // The pty echoes the command, so only a line of its own counts.
    assert!(!seen.text().contains("\nslept"), "{}", seen.text());
    assert!(link.control.input(b"exit 4\n".to_vec()));
    let exit = seen.until_exit(&mut link).await;
    assert_eq!((exit.code, exit.signal), (Some(4), None));
}

pub(super) async fn kill_hangs_up_the_whole_group(ctx: Ctx) {
    let script = "stty -echo; sleep 300 & echo \"pid $!\"; wait";
    let launch = sh(&ctx, script, false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    seen.until(&mut link, false, |seen| seen.text().contains('\n'))
        .await;
    let text = seen.text();
    let sleeper: u32 = text
        .trim()
        .strip_prefix("pid ")
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("no pid in {text:?}"));
    assert!(link.control.kill(Duration::from_secs(5)));
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.signal, Some(1), "{exit:?}");
    eventually("the background sleep ended", || !alive(sleeper)).await;

    // A goodbye after the exit deletes the folder and ends the holder.
    let dir = terminal_dir(&ctx, launch.terminal);
    let holder_pid = link.hello.holder_pid;
    assert!(link.control.goodbye());
    eventually("the folder went", || !dir.exists()).await;
    eventually("the holder ended", || !alive(holder_pid)).await;
    assert!(matches!(
        terminals.connect(launch.terminal, None).await.unwrap(),
        Reached::Missing
    ));
}

pub(super) async fn kill_escalates_to_sigkill(ctx: Ctx) {
    let script = "trap '' HUP; stty -echo; echo ready; while :; do sleep 1; done";
    let launch = sh(&ctx, script, false);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    seen.until_text(&mut link, "ready").await;
    let started = Instant::now();
    assert!(link.control.kill(Duration::from_millis(300)));
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.signal, Some(9), "{exit:?}");
    assert!(started.elapsed() >= Duration::from_millis(300));
}

pub(super) async fn an_ended_terminal_leaves_its_exit_when_nobody_says_goodbye(ctx: Ctx) {
    let launch = sh(&ctx, "echo bye; exit 7", true);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(7));
    assert_eq!(exit.output.as_ref().unwrap().text, "bye");
    // A goodbye before the end means nothing, and here nobody says one.
    drop(link);
    let dir = terminal_dir(&ctx, launch.terminal);
    eventually("the holder ended", || live_holder(&dir).is_none()).await;
    let Reached::Ended(Some(left)) = terminals.connect(launch.terminal, None).await.unwrap() else {
        panic!("no exit left behind");
    };
    assert_eq!(left, exit);
    assert_eq!(terminals.list().unwrap(), [launch.terminal]);
    assert!(terminals.remove(launch.terminal).unwrap());
    assert!(!dir.exists());
    assert!(terminals.list().unwrap().is_empty());
}

pub(super) async fn a_goodbye_before_the_end_changes_nothing(ctx: Ctx) {
    let launch = sh(&ctx, "stty -echo; read line; echo \"got:$line\"", false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    assert!(link.control.goodbye());
    assert!(!terminals.remove(launch.terminal).unwrap());
    assert!(link.control.input(b"still here\n".to_vec()));
    let mut seen = Seen::from(&link);
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0));
    assert!(seen.text().contains("got:still here"));
}

pub(super) async fn a_terminal_holder_refuses_a_client_without_its_key(ctx: Ctx) {
    let launch = sh(&ctx, "stty -echo; read line; echo \"got:$line\"", false);
    let terminals = terminals(&ctx);
    let mut link = start(&ctx, &launch).await;
    let dir = terminal_dir(&ctx, launch.terminal);
    let key_path = dir.join("holder.key");
    let key = fs::read_to_string(&key_path).unwrap();
    let mode = fs::metadata(&key_path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let socket = socket_of(&dir);
    let hello = |terminal: TerminalId, key: &str| json!({"type": "hello", "terminal": terminal, "key": key, "after": null});
    let wrong = "0".repeat(64);
    for shown in [wrong.as_str(), &key[..63], ""] {
        let (answer, mut read, mut write) =
            say_hello(&socket, &hello(launch.terminal, shown)).await;
        assert_eq!(answer["type"], "refused", "{answer}");
        let message = answer["message"].as_str().unwrap();
        assert!(message.contains("isn't this holder's"), "{message}");
        let sneak = json!({"type": "input", "data": "c25lYWtlZCBpbgo="});
        let _ = send_raw(&mut write, &sneak).await;
        assert!(hung_up(&mut read).await, "the holder kept a refused client");
    }
    // No key at all doesn't get a word.
    let (stream_read, mut stream_write) = tokio::net::UnixStream::connect(&socket)
        .await
        .unwrap()
        .into_split();
    send_raw(
        &mut stream_write,
        &json!({"type": "hello", "terminal": launch.terminal}),
    )
    .await
    .unwrap();
    assert!(hung_up(&mut tokio::io::BufReader::new(stream_read)).await);
    let (answer, _, _) = say_hello(&socket, &hello(TerminalId::generate(), &key)).await;
    assert_eq!(answer["type"], "refused", "{answer}");
    assert!(answer["message"]
        .as_str()
        .unwrap()
        .contains("runs terminal"));
    fs::write(&key_path, &wrong).unwrap();
    let Err(err) = terminals.connect(launch.terminal, None).await else {
        panic!("a wrong key got in");
    };
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    fs::write(&key_path, &key).unwrap();

    assert!(link.control.input(b"after\n".to_vec()));
    let mut seen = Seen::from(&link);
    seen.until_exit(&mut link).await;
    assert!(seen.text().contains("got:after"), "{}", seen.text());
    assert!(!seen.text().contains("sneaked"));
}

pub(super) async fn a_run_keeps_what_it_showed(ctx: Ctx) {
    let terminal = TerminalId::generate();
    let command = "printf '\\033[1mbold\\033[0m\\n'; echo \"$TERM $COLORTERM $TERM_PROGRAM\"";
    let mut launch = TermLaunch::run(terminal, ctx.root.clone(), command, 80, 24);
    // The user's shell would read their startup files.
    launch.program = "/bin/sh".into();
    launch.env.extend(quiet_env(&ctx).into_iter().skip(1));
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0));
    let output = exit.output.unwrap();
    assert!(output.text.contains("1mbold\x1b[0m"), "{:?}", output.text);
    assert!(
        output.text.contains("xterm-256color truecolor MaxHQ"),
        "{:?}",
        output.text
    );
    assert!(!output.truncated);

    let full = "printf 'before\\n\\033[?1049hALT\\033[?1049l'; echo after";
    let launch = sh(&ctx, full, true);
    let mut link = start(&ctx, &launch).await;
    let mut seen = Seen::from(&link);
    let output = seen.until_exit(&mut link).await.output.unwrap();
    assert_eq!(
        output.text,
        format!("before\nafter\n{}", maxhq_term::FULL_SCREEN)
    );
}

/// Startup files that run `script`, in a home folder of their own named
/// `name`, and the variables that point `sh` (`ENV`), bash (`HOME`) and
/// zsh (`ZDOTDIR`) at them.
fn startup(ctx: &Ctx, name: &str, script: &str) -> Vec<(OsString, OsString)> {
    let home = ctx.root.join(name);
    fs::create_dir_all(&home).unwrap();
    for file in [".shrc", ".bash_profile", ".zshrc"] {
        fs::write(home.join(file), script).unwrap();
    }
    vec![
        ("HOME".into(), home.clone().into()),
        ("ZDOTDIR".into(), home.clone().into()),
        ("ENV".into(), home.join(".shrc").into()),
    ]
}

/// A `!` run of `command` by `shell`, whose startup runs `script`.
fn run_after_startup(ctx: &Ctx, shell: &str, script: &str, command: &str) -> TermLaunch {
    let terminal = TerminalId::generate();
    let mut launch = TermLaunch::run(terminal, ctx.root.clone(), command, 80, 24);
    launch.program = shell.into();
    let name = format!("home-{}", terminal.0.simple());
    launch.env.extend(startup(ctx, &name, script));
    launch
}

pub(super) async fn a_run_shows_nothing_its_shell_printed_before_the_command(ctx: Ctx) {
    let terminals = terminals(&ctx);
    let banner = "printf '\\033]0;rc title\\007'; echo 'Welcome to devbox'; : > \"$HOME/started\"";
    let shells = ["/bin/sh", "/bin/bash", "/bin/zsh"];
    for shell in shells.into_iter().filter(|shell| Path::new(shell).exists()) {
        let launch = run_after_startup(&ctx, shell, banner, "echo from the command");
        let mut link = start(&ctx, &launch).await;
        assert_eq!(link.hello.from, 0);
        let mut seen = Seen::from(&link);
        let exit = seen.until_exit(&mut link).await;
        assert_eq!(exit.code, Some(0), "{shell}");
        assert_eq!(seen.text(), "from the command\r\n", "{shell}");
        assert_eq!(exit.output.unwrap().text, "from the command", "{shell}");
        assert_eq!(seen.title, None, "{shell}");
        // The startup did run.
        let home = launch.env.iter().find(|(key, _)| key == "HOME").unwrap();
        assert!(Path::new(&home.1).join("started").exists(), "{shell}");
        // A client that comes later starts at the command's first byte too.
        let mut later = live(&terminals, launch.terminal, None).await;
        assert_eq!((later.hello.from, later.hello.reset), (0, false));
        let mut again = Seen::from(&later);
        again.until_exit(&mut later).await;
        assert_eq!(again.text(), seen.text(), "{shell}");
    }
}

pub(super) async fn a_run_whose_startup_asks_for_something_shows_it(ctx: Ctx) {
    let asks = [
        (
            "Update now? [y/n] ",
            "stty -icanon; read -r answer; stty icanon; echo",
        ),
        (
            "Passphrase: ",
            "stty -echo; read -r answer; stty echo; echo",
        ),
    ];
    for (prompt, then) in asks {
        let script = format!("printf '{prompt}'; {then}");
        let launch = run_after_startup(&ctx, "/bin/sh", &script, "echo from the command");
        let mut link = start(&ctx, &launch).await;
        let mut seen = Seen::from(&link);
        seen.until_text(&mut link, prompt).await;
        assert!(link.control.input(b"y\n".to_vec()));
        let exit = seen.until_exit(&mut link).await;
        assert_eq!(exit.code, Some(0), "{prompt}");
        assert!(seen.text().ends_with("from the command\r\n"), "{prompt}");
        let output = exit.output.unwrap().text;
        assert!(output.starts_with(prompt.trim_end()), "{output:?}");
        assert!(output.ends_with("\nfrom the command"), "{output:?}");
    }
}

/// A mark as the holder passes it on.
fn plain(mark: &Mark) -> String {
    match mark {
        Mark::Prompt => "\x1b]133;A\x07".into(),
        Mark::Input => "\x1b]133;B\x07".into(),
        Mark::Command { .. } => "\x1b]133;C\x07".into(),
        Mark::Done { exit: Some(exit) } => format!("\x1b]133;D;{exit}\x07"),
        Mark::Done { exit: None } => "\x1b]133;D\x07".into(),
    }
}

/// Checks each of `marks` is where it says in `bytes`, the output from its
/// start, and no other 133 got through.
fn assert_marks_in_place(bytes: &[u8], marks: &[(u64, Mark)]) {
    for (offset, mark) in marks {
        let rest = &bytes[usize::try_from(*offset).unwrap()..];
        assert!(
            rest.starts_with(plain(mark).as_bytes()),
            "{mark:?} at {offset}: {:?}",
            String::from_utf8_lossy(&rest[..rest.len().min(40)])
        );
    }
    let all = bytes.windows(6).filter(|w| *w == b"\x1b]133;").count();
    assert_eq!(all, marks.len(), "{:?}", String::from_utf8_lossy(bytes));
}

fn commands_of(marks: &[(u64, Mark)]) -> Commands {
    let mut commands = Commands::default();
    for (offset, mark) in marks {
        commands.mark(*offset, Timestamp::now(), mark);
    }
    commands
}

pub(super) async fn a_shell_mark_needs_its_nonce_and_may_come_in_pieces(ctx: Ctx) {
    let nonce = "0123456789abcdef0123456789abcdef";
    // The D mark comes in three reads, the last split inside its ST. The
    // ESC ends it, as it does in the terminal, and the `ESC \` it starts
    // goes on.
    let script = format!(
        "stty -echo; printf 'one\\033]133;A;maxhq={nonce}\\007two'; \
         printf '\\033]133;B;maxhq=wrong\\007\\033]133;C\\007\\033]133;B;maxhq={nonce}0\\007'; \
         printf '\\033]7;file://devbox/tmp/x\\007\\033]13'; sleep 0.3; \
         printf '3;D;7;maxhq={nonce}\\033'; sleep 0.3; \
         printf '\\\\three\\033]133;C;maxhq={nonce}\\007'; read wait"
    );
    let terminals = terminals(&ctx);
    for nonce in [Some(nonce), None] {
        let mut launch = sh(&ctx, &script, false);
        // sh gets nothing of MaxHQ's.
        assert!(!terminals.integrate(&mut launch).unwrap());
        launch.nonce = nonce.map(str::to_owned);
        let mut link = start(&ctx, &launch).await;
        let Reached::Live(mut watch) = terminals.watch(launch.terminal).await.unwrap() else {
            panic!("no watch");
        };
        let osc7 = "\x1b]7;file://devbox/tmp/x\x07";
        let expected = match nonce {
            Some(_) => {
                format!("one\x1b]133;A\x07two{osc7}\x1b]133;D;7\x07\x1b\\three\x1b]133;C\x07")
            }
            None => format!("onetwo{osc7}\x1b\\three"),
        };
        let mut seen = Seen::from(&link);
        seen.until(&mut link, false, |seen| seen.bytes.len() >= expected.len())
            .await;
        assert_eq!(seen.text(), expected);
        assert!(link.control.input(b"\n".to_vec()));
        seen.until_exit(&mut link).await;
        // A link for the output gets no marks.
        assert_eq!(seen.marks, []);
        let mut watched = Seen::from(&watch);
        watched.until_exit(&mut watch).await;
        let at = |text: &str| u64::try_from(expected.find(text).unwrap()).unwrap();
        let marks = match nonce {
            Some(_) => vec![
                (3, Mark::Prompt),
                (at("\x1b]133;D"), Mark::Done { exit: Some(7) }),
                (
                    at("\x1b]133;C"),
                    Mark::Command {
                        cwd: Some("/tmp/x".into()),
                    },
                ),
            ],
            None => Vec::new(),
        };
        assert_eq!(watched.marks, marks);
        assert_marks_in_place(&seen.bytes, &watched.marks);
        // One that comes later gets them all again.
        let Reached::Live(mut later) = terminals.watch(launch.terminal).await.unwrap() else {
            panic!("no second watch");
        };
        let mut again = Seen::from(&later);
        again.until_exit(&mut later).await;
        assert_eq!(again.marks, marks);
    }
}

/// Every program called `name` on the PATH, once each.
fn on_path(name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut real = Vec::new();
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = dir.join(name);
        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        if path.is_file() && !real.contains(&canonical) {
            real.push(canonical);
            found.push(path);
        }
    }
    if found.is_empty() {
        eprintln!("note: no {name} on the PATH, so its shell integration goes untested");
    }
    found
}

pub(super) async fn zsh_bash_and_fish_mark_their_commands(ctx: Ctx) {
    let shells: Vec<_> = ["zsh", "bash", "fish"]
        .into_iter()
        .flat_map(on_path)
        .collect();
    for program in &shells {
        a_shell_marks_its_commands(&ctx, program).await;
    }
    // One folder for them all, and nothing else in it, history included.
    let folders: Vec<_> = (fs::read_dir(&ctx.dirs.sessions).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("shell-")
        })
        .collect();
    assert_eq!(folders.len(), usize::from(!shells.is_empty()));
    for folder in folders {
        let mut names: Vec<_> = (fs::read_dir(folder).unwrap())
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        let all = [
            ".zlogin",
            ".zprofile",
            ".zshenv",
            ".zshrc",
            "bash.sh",
            "fish.fish",
        ];
        assert_eq!(names, all);
    }
}

/// Waits until the shell has run `done` commands and shows its prompt for
/// the next.
async fn ready(watch: &mut TermLink, watched: &mut Seen, done: usize) {
    watched
        .until(watch, false, |seen| {
            let commands = commands_of(&seen.marks);
            let list = commands.list();
            list.len() == done + 1
                && list.iter().take(done).all(|command| command.end.is_some())
                && list.back().is_some_and(|last| last.input.is_some())
        })
        .await;
}

async fn a_shell_marks_its_commands(ctx: &Ctx, program: &Path) {
    let shell = program.file_name().unwrap().to_str().unwrap();
    let shown = program.display();
    let terminals = terminals(ctx);
    let home = ctx.root.join(format!("home-{shell}"));
    let zdot = home.join("zdot");
    let config = home.join("config");
    fs::create_dir_all(&zdot).unwrap();
    fs::create_dir_all(config.join("fish")).unwrap();
    fs::create_dir_all(ctx.root.join("a b%c")).unwrap();
    // Each startup file says it ran. The prompt is the user's own, and so
    // are hooks at the prompt and before a command, which still run.
    let fish = "set -gx RC config\n\
                function fish_prompt; printf 'ready> '; end\n\
                function hooked --on-event fish_prompt; set -gx HOOKED yes; end\n\
                function trapped --on-event fish_preexec; set -gx TRAPPED yes; end\n";
    for (path, text) in [
        (zdot.join(".zshenv"), "export RC=env"),
        (zdot.join(".zprofile"), "RC=${RC}:profile"),
        (
            zdot.join(".zshrc"),
            "RC=${RC}:rc; PS1='ready> '\n\
             precmd() { export HOOKED=yes }\n\
             preexec() { export TRAPPED=yes }",
        ),
        (zdot.join(".zlogin"), "RC=${RC}:login"),
        (
            home.join(".bash_profile"),
            "export RC=profile; PS1='ready> '\n\
             PROMPT_COMMAND='export HOOKED=yes'\n\
             trap 'export TRAPPED=yes' DEBUG",
        ),
        (home.join(".bashrc"), "export RC=bashrc"),
        (config.join("fish/config.fish"), fish),
    ] {
        fs::write(path, text).unwrap();
    }
    let rc = match shell {
        "zsh" => "env:profile:rc:login",
        "bash" => "profile",
        _ => "config",
    };
    // Wide, so no line the test types wraps.
    let mut launch = TermLaunch::shell(TerminalId::generate(), ctx.root.clone(), 200, 24);
    launch.program = program.into();
    launch.env.extend(
        [
            ("HOME", home.clone()),
            ("ZDOTDIR", zdot.clone()),
            ("XDG_CONFIG_HOME", config),
            ("XDG_DATA_HOME", home.join("data")),
            ("XDG_CACHE_HOME", home.join("cache")),
        ]
        .map(|(key, value)| (key.into(), value.into())),
    );
    assert!(terminals.integrate(&mut launch).unwrap(), "{shown}");
    let nonce = launch.nonce.clone().unwrap();
    let mut link = start(ctx, &launch).await;
    let Reached::Live(mut watch) = terminals.watch(launch.terminal).await.unwrap() else {
        panic!("no watch for {shown}");
    };
    let mut watched = Seen::from(&watch);
    let typed = [
        "sh -c 'echo \"rc:$RC:$ZDOTDIR:${MAXHQ_SHELL_NONCE-gone}:$HOOKED:$TRAPPED\"'",
        "cd 'a b%c'",
        "env printf '\\033]133;D;1;maxhq=wrong\\007\\033]133;A\\007'; sh -c 'exit 3'",
    ];
    for (done, line) in typed.iter().enumerate() {
        ready(&mut watch, &mut watched, done).await;
        assert!(link.control.input(format!("{line}\n").into_bytes()));
    }
    ready(&mut watch, &mut watched, typed.len()).await;
    assert!(link.control.input(b"exit 0\n".to_vec()));
    let mut seen = Seen::from(&link);
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0), "{shown}: {seen:?}");
    watched.until_exit(&mut watch).await;

    assert_marks_in_place(&seen.bytes, &watched.marks);
    assert!(!holds_bytes(&seen.bytes, nonce.as_bytes()), "{shown}");
    let commands = commands_of(&watched.marks);
    let list: Vec<_> = commands.list().iter().take(typed.len()).collect();
    let exits: Vec<_> = list.iter().map(|command| command.exit).collect();
    assert_eq!(exits, [Some(0), Some(0), Some(3)], "{shown}: {watched:?}");
    let text = |from: Option<u64>, to: Option<u64>| {
        let range = usize::try_from(from.unwrap()).unwrap()..usize::try_from(to.unwrap()).unwrap();
        String::from_utf8_lossy(&seen.bytes[range]).into_owned()
    };
    let output = text(list[0].start, list[0].end);
    let rc_line = format!("rc:{rc}:{}:gone:yes:yes", zdot.display());
    assert!(output.contains(&rc_line), "{shown}: {output:?}");
    // What the shell echoed between its prompt and the command is the
    // command. fish colors it as you type.
    if shell != "fish" {
        for (command, line) in list.iter().zip(typed) {
            let echoed = text(command.input, command.start);
            assert!(echoed.contains(line), "{shown}: {echoed:?}");
        }
    }
    let root = ctx.root.file_name().unwrap();
    let cwd = |command: &ShellCommand| PathBuf::from(command.cwd.clone().unwrap());
    assert!(cwd(list[0]).ends_with(root), "{shown}: {list:?}");
    assert!(
        cwd(list[2]).ends_with(Path::new(root).join("a b%c")),
        "{shown}"
    );
    assert!(seen.cwd.unwrap().ends_with("/a b%c"), "{shown}");
}

pub(super) async fn a_terminal_outlives_its_spawner(ctx: Ctx) {
    let terminal = TerminalId::generate();
    let mut command = Command::new(exe());
    command
        .arg("__spawner")
        .arg("--terminal")
        .arg(&ctx.dirs.sessions)
        .arg(&ctx.dirs.sockets)
        .arg(terminal.to_string())
        .arg(&ctx.root)
        .env(DETACH_ENV, "setsid")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .process_group(0);
    let mut spawner = command.spawn().unwrap();
    let stdout = spawner.stdout.take().unwrap();
    let said = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).map(|_| line)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(said.trim(), "spawned");
    let spawner_pid = i32::try_from(spawner.id()).unwrap();
    rustix::process::kill_process_group(Pid::from_raw(spawner_pid).unwrap(), Signal::KILL).unwrap();
    spawner.wait().unwrap();

    let mut link = live(&terminals(&ctx), terminal, None).await;
    let holder = super::pid(link.hello.holder_pid);
    let child = super::pid(link.hello.child_pid);
    if let Ok(sid) = rustix::process::getsid(Some(holder)) {
        assert_eq!(sid, holder);
    }
    // The program leads a session of its own, with the pty as its terminal.
    if let Ok(sid) = rustix::process::getsid(Some(child)) {
        assert_eq!(sid, child);
    }
    let mut seen = Seen::from(&link);
    assert!(link.control.input(b"still here\n".to_vec()));
    let exit = seen.until_exit(&mut link).await;
    assert_eq!(exit.code, Some(0));
    assert!(seen.text().contains("got:still here"), "{}", seen.text());
}

/// `__spawner --terminal`: starts a terminal holder, says so, and waits to
/// be killed.
pub(super) fn spawner(sessions: &Path, sockets: &Path, terminal: &str, cwd: &Path) -> ExitCode {
    let terminals = Terminals::new(Dirs {
        sessions: sessions.into(),
        sockets: sockets.into(),
    })
    .with_holder(exe(), vec!["__hold".into()])
    .with_linger(LINGER);
    let launch = TermLaunch {
        terminal: terminal.parse().unwrap(),
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "stty -echo; read line; echo \"got:$line\"".into(),
        ],
        env: Vec::new(),
        cwd: cwd.into(),
        cols: 80,
        rows: 24,
        run: false,
        nonce: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if let Err(err) = runtime.block_on(terminals.spawn(&launch)) {
        eprintln!("spawn failed: {err}");
        return ExitCode::FAILURE;
    }
    println!("spawned");
    let _ = io::stdout().flush();
    thread::sleep(Duration::from_secs(60));
    ExitCode::SUCCESS
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The check against small programs this test builds and signs ad hoc,
//! which carry an identifier and maybe the hardened runtime, but no team.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use security_framework::os::macos::code_signing::SecRequirement;

use super::*;
use crate::testing::TempDir;

/// The identifier the test programs that stand in for the app carry.
const STAND_IN: &str = "com.example.maxhq-stand-in";

/// With `hold`, says so on stdout and waits for stdin to close. With a
/// socket path, connects, reads a byte, and then says so and waits, or with
/// a program too, runs it in its place with `hold`, the socket still open.
const HELPER: &str = r#"
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int hold(const char *mark) {
    char c;
    write(1, mark, 1);
    while (read(0, &c, 1) > 0) {}
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "hold") == 0) return hold("h");
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof addr);
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, argv[1], sizeof addr.sun_path - 1);
    if (connect(fd, (struct sockaddr *)&addr, sizeof addr) != 0) return 1;
    char c;
    if (read(fd, &c, 1) != 1) return 2;
    if (argc == 3) {
        execl(argv[2], argv[2], "hold", (char *)0);
        return 3;
    }
    return hold("c");
}
"#;

/// The test programs, each signed its own way, and a socket they connect
/// to.
struct Rig {
    dir: TempDir,
    listener: UnixListener,
}

impl Rig {
    fn new() -> Self {
        let dir = TempDir::new();
        let source = dir.path().join("helper.c");
        std::fs::write(&source, HELPER).unwrap();
        run(Command::new("cc")
            .arg("-o")
            .arg(dir.path().join("helper"))
            .arg(&source));
        let listener = UnixListener::bind(dir.path().join("s")).unwrap();
        Self { dir, listener }
    }

    /// A copy of the helper signed with `identifier`, and with the hardened
    /// runtime when `runtime`.
    fn signed(&self, name: &str, identifier: &str, runtime: bool) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::copy(self.dir.path().join("helper"), &path).unwrap();
        let mut sign = Command::new("codesign");
        sign.args(["--force", "--sign", "-", "--identifier", identifier]);
        if runtime {
            sign.args(["--options", "runtime"]);
        }
        run(sign.arg(&path));
        path
    }

    /// Runs `program` against the socket, then `then` in its place when
    /// given. Answers the connection, the program, and the audit token as
    /// the connection was accepted, before the program read its byte.
    fn connect(&self, program: &Path, then: Option<&Path>) -> (UnixStream, Running, Option<Token>) {
        let mut command = Command::new(program);
        command.arg(self.dir.path().join("s"));
        if let Some(then) = then {
            command.arg(then);
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut running = Running(Some(child));
        let (mut stream, _) = self.listener.accept().unwrap();
        let accepted = token(&stream);
        stream.write_all(b"x").unwrap();
        running.wait_for_mark();
        (stream, running, accepted)
    }
}

/// A test program, told to end when dropped.
struct Running(Option<Child>);

impl Running {
    /// Waits until the program says it's holding.
    fn wait_for_mark(&mut self) {
        let mut mark = [0u8; 1];
        let child = self.0.as_mut().unwrap();
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut mark)
            .unwrap();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // Closing its stdin ends it.
            drop(child.stdin.take());
            let _ = child.wait();
        }
    }
}

fn run(command: &mut Command) {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn policy(identifier: &str, runtime: bool) -> Policy {
    let requirement = format!("identifier \"{identifier}\"");
    Policy {
        signer: requirement.clone(),
        requirement,
        runtime,
    }
}

#[test]
fn the_requirement_names_apple_the_app_and_the_team() {
    let app = Policy::app("ABCDE12345");
    assert_eq!(
        app.requirement,
        "anchor apple generic and identifier \"com.maxagenthq.maxhq\" and certificate \
         leaf[subject.OU] = \"ABCDE12345\""
    );
    assert!(app.runtime);
    assert_eq!(
        app.signer,
        "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\""
    );
    for policy in [app, Policy::nobody()] {
        policy.requirement.parse::<SecRequirement>().unwrap();
        policy.signer.parse::<SecRequirement>().unwrap();
    }
}

#[tokio::test]
async fn a_maxd_without_a_team_has_the_check_off() {
    // Tests run ad hoc signed, as a dev build does.
    assert_eq!(mac::own_team().unwrap(), None);
    assert!(Policy::for_this_maxd().is_none());
    assert_eq!(check(None, None).await, Signature::Off);
}

#[test]
fn the_program_passes_by_its_signature() {
    let rig = Rig::new();
    let app = rig.signed("app", STAND_IN, true);
    let (stream, _running, accepted) = rig.connect(&app, None);
    let now = token(&stream);
    assert!(accepted.is_some());
    assert_eq!(accepted, now);
    assert_eq!(
        judge(accepted, now, &policy(STAND_IN, true)),
        Signature::Passed
    );
    assert_eq!(
        judge(accepted, now, &policy("com.example.other", false)),
        Signature::Failed
    );
    assert_eq!(judge(None, now, &policy(STAND_IN, true)), Signature::Failed);
    assert_eq!(
        judge(accepted, None, &policy(STAND_IN, true)),
        Signature::Failed
    );
}

#[test]
fn the_app_needs_the_hardened_runtime() {
    let rig = Rig::new();
    let plain = rig.signed("plain", STAND_IN, false);
    let (stream, _running, accepted) = rig.connect(&plain, None);
    let now = token(&stream);
    assert_eq!(
        judge(accepted, now, &policy(STAND_IN, true)),
        Signature::Failed
    );
    assert_eq!(
        judge(accepted, now, &policy(STAND_IN, false)),
        Signature::Passed
    );
}

#[test]
fn a_program_that_connects_and_then_runs_the_app_fails() {
    let rig = Rig::new();
    let other = rig.signed("other", "com.example.other", true);
    let app = rig.signed("app", STAND_IN, true);
    let stand_in = policy(STAND_IN, true);
    let (stream, _running, accepted) = rig.connect(&other, Some(&app));
    let now = token(&stream);
    assert_ne!(accepted, now, "running another program changes the token");
    // The app holds the socket now, so a check of the token now alone
    // would pass.
    assert!(passes(&now.unwrap(), &stand_in));
    assert_eq!(judge(accepted, now, &stand_in), Signature::Failed);
    // The token from before names a program that's gone.
    assert_eq!(judge(accepted, accepted, &stand_in), Signature::Failed);
}

#[test]
fn the_card_names_the_program_and_whether_your_certificate_signed_it() {
    let rig = Rig::new();
    let app = rig.signed("app", STAND_IN, false);
    let (_stream, _running, accepted) = rig.connect(&app, None);
    let path = Some(app.canonicalize().unwrap());

    let yours = describe(accepted, Some(&policy(STAND_IN, true)));
    assert_eq!(yours.path, path);
    assert_eq!(yours.signed, Signed::Yours);
    let theirs = describe(accepted, Some(&policy("com.example.other", true)));
    assert_eq!(theirs.path, path);
    assert_eq!(theirs.signed, Signed::NotYours);
    // A maxd with no team can't say.
    assert_eq!(describe(accepted, None).signed, Signed::Unchecked);
    // Nor can anyone without a token.
    let unknown = describe(None, Some(&policy(STAND_IN, true)));
    assert_eq!(
        unknown,
        Program {
            path: None,
            signed: Signed::NotYours
        }
    );
}

// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

//! The text of the documents you attach, ported from MIT-licensed code, see
//! THIRD_PARTY_NOTICES. A Word file, a spreadsheet, a slide deck, an RTF
//! file, an EPUB or a notebook gets its text saved beside it as
//! `<name>.txt`, so an agent without tools for the format can still read
//! it. The reading runs in a child `maxd`, so a document that takes too
//! long or too much memory can't hold up or take down the daemon. That code
//! tries a failed document again later; here a document is read once, as it
//! lands. A daemon running as root, as the guest in a container does, reads
//! as the user whose documents they are, or as nobody when it can't tell
//! who that is, and never as root.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

mod extract;
#[cfg(unix)]
mod root;
mod rtf;
mod xml;
mod zip;

pub use extract::{strip_vt, EXTENSIONS};

/// Windows has no root for a daemon to give up, so the child reads as
/// `maxd`'s own user.
#[cfg(not(unix))]
mod root {
    pub fn give_up(_command: &mut std::process::Command, _owner: Option<&str>) {}
}

/// The hidden subcommand the child runs.
pub const SUBCOMMAND: &str = "__document-text";

/// How long the child may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How large a document can be and still be read.
const INPUT_CAP: u64 = 16 * 1024 * 1024;

/// How much text the child may give back.
const OUTPUT_CAP: u64 = 16 * 1024 * 1024;

/// How much of the child's error output is kept for the log.
const ERROR_CAP: u64 = 4096;

/// How often a running child is checked on.
const POLL: Duration = Duration::from_millis(10);

/// The exit code for a document that can't be read this way, which won't
/// change if it's tried again.
const REJECTED: u8 = 2;

/// Whether the file's extension is one with text to read out.
pub fn wants(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Where a document's text goes: its path with `.txt` after it.
pub fn text_path(path: &Path) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(".txt");
    PathBuf::from(text)
}

/// What runs the child that reads a document.
#[derive(Clone, Debug)]
enum Program {
    /// This `maxd`, with [`SUBCOMMAND`].
    Maxd,
    /// Another program and its first arguments, for tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Other(PathBuf, Vec<OsString>),
    /// Nothing, so no text is read.
    Off,
}

/// Reads documents' text in a child process and saves it beside them.
#[derive(Clone, Debug)]
pub struct Extractor {
    program: Program,
    /// How long the child may take.
    timeout: Duration,
    /// The user whose documents these are, for a daemon running as root.
    owner: Option<String>,
}

impl Default for Extractor {
    /// This `maxd`. Unit tests run as a test binary that has no such
    /// subcommand, so there it reads nothing.
    fn default() -> Self {
        let program = if cfg!(test) {
            Program::Off
        } else {
            Program::Maxd
        };
        Self {
            program,
            timeout: TIMEOUT,
            owner: None,
        }
    }
}

impl Extractor {
    /// Runs `program` with `args`, then the document's path, in place of
    /// `maxd`.
    #[cfg(test)]
    pub fn with(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: Program::Other(program.into(), args.iter().map(OsString::from).collect()),
            timeout: TIMEOUT,
            owner: None,
        }
    }

    /// The same, giving the child `timeout` in place of [`TIMEOUT`].
    #[cfg(test)]
    pub fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// The same, reading as `owner` while this process runs as root.
    pub fn owned_by(self, owner: Option<String>) -> Self {
        Self { owner, ..self }
    }

    /// Saves the text of the document at `path` beside it, unless it's
    /// there already, and gives where it is. A file of another kind, too
    /// large a document, or one that can't be read gets none.
    pub fn save(&self, path: &Path) -> Option<PathBuf> {
        if !wants(path) {
            return None;
        }
        let text = text_path(path);
        if fs::symlink_metadata(&text).is_ok_and(|meta| meta.is_file()) {
            return Some(text);
        }
        let meta = fs::symlink_metadata(path).ok()?;
        if !meta.is_file() || meta.len() > INPUT_CAP {
            return None;
        }
        let read = match self.read(path) {
            Ok(Some(read)) => read,
            Ok(None) => return None,
            Err(err) => {
                tracing::debug!(path = %path.display(), "couldn't read a document's text: {err}");
                return None;
            }
        };
        // The text lands whole or not at all, so an agent never reads half.
        let mut part = text.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let _ = fs::remove_file(&part);
        let written =
            crate::sidecar::write_readable(&part, &read).and_then(|()| fs::rename(&part, &text));
        if let Err(err) = written {
            let _ = fs::remove_file(&part);
            tracing::debug!(path = %path.display(), "couldn't save a document's text: {err}");
            return None;
        }
        Some(text)
    }

    /// The text the child reads out of `path`, or none when it can't.
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        let mut command = match &self.program {
            Program::Off => return Ok(None),
            Program::Maxd => {
                let mut command = Command::new(std::env::current_exe()?);
                command.arg(SUBCOMMAND);
                command
            }
            Program::Other(program, args) => {
                let mut command = Command::new(program);
                command.args(args);
                command
            }
        };
        root::give_up(&mut command, self.owner.as_deref());
        let mut child = command
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let over = Arc::new(AtomicBool::new(false));
        let out = drain(child.stdout.take(), OUTPUT_CAP, Some(over.clone()));
        let err = drain(child.stderr.take(), ERROR_CAP, None);
        let status = wait(&mut child, self.timeout, &over)?;
        let out = out.join().unwrap_or_default();
        let err = err.join().unwrap_or_default();
        if over.load(Ordering::Relaxed) {
            tracing::debug!(path = %path.display(), "a document's text is too long to keep");
            return Ok(None);
        }
        let Some(status) = status else {
            tracing::debug!(path = %path.display(), "a document took too long to read");
            return Ok(None);
        };
        if !status.success() {
            let why = String::from_utf8_lossy(&err);
            let why = why.trim();
            if status.code() == Some(i32::from(REJECTED)) {
                tracing::debug!(path = %path.display(), "a document has no text to read: {why}");
            } else {
                tracing::debug!(path = %path.display(), %status, "reading a document failed: {why}");
            }
            return Ok(None);
        }
        Ok(Some(out))
    }
}

/// Reads up to `cap` bytes from a child's pipe on a thread of its own.
/// With `over`, more than that sets it and stops reading. Without, the
/// rest is thrown away, so the child never waits on a full pipe.
fn drain<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: u64,
    over: Option<Arc<AtomicBool>>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(pipe) = pipe else {
            return kept;
        };
        let mut pipe = pipe.take(cap + 1);
        let _ = pipe.read_to_end(&mut kept);
        if u64::try_from(kept.len()).unwrap_or(u64::MAX) > cap {
            match over {
                Some(over) => over.store(true, Ordering::Relaxed),
                None => {
                    kept.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
                    let _ = io::copy(&mut pipe.into_inner(), &mut io::sink());
                }
            }
        }
        kept
    })
}

/// Waits for the child until `timeout`, or until `over` says it printed
/// too much, and ends it then. Gives how it exited, or none when it was
/// ended.
fn wait(child: &mut Child, timeout: Duration, over: &AtomicBool) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline || over.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

/// `maxd __document-text <file>`: prints the document's text. It exits
/// with 2 when the document can't be read this way, and 1 when reading
/// the file failed.
pub fn run(args: &[OsString]) -> ExitCode {
    let [path] = args else {
        eprintln!("usage: maxd {SUBCOMMAND} <file>");
        return ExitCode::from(64);
    };
    match extract::extract(Path::new(path)) {
        Ok(text) => {
            let mut out = io::stdout().lock();
            match io::Write::write_all(&mut out, text.as_bytes())
                .and_then(|()| io::Write::flush(&mut out))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("couldn't write the text: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(extract::Failure::Rejected(why)) => {
            eprintln!("{why}");
            ExitCode::from(REJECTED)
        }
        Err(extract::Failure::Io(err)) => {
            eprintln!("couldn't read {}: {err}", Path::new(path).display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn sh(script: &str) -> Extractor {
        Extractor::with("/bin/sh", &["-c", script, "sh"])
    }

    #[test]
    fn saves_what_the_child_prints_beside_the_document() {
        let dir = TempDir::new();
        let doc = dir.path().join("notes.docx");
        fs::write(&doc, "words").unwrap();
        let saved = sh("printf 'text of %s' \"$(basename \"$1\")\"").save(&doc);
        assert_eq!(
            saved.as_deref(),
            Some(dir.path().join("notes.docx.txt").as_path())
        );
        assert_eq!(
            fs::read_to_string(saved.unwrap()).unwrap(),
            "text of notes.docx"
        );
        assert!(!dir.path().join("notes.docx.txt.part").exists());
    }

    #[test]
    fn keeps_text_saved_already() {
        let dir = TempDir::new();
        let doc = dir.path().join("a.ODT");
        fs::write(&doc, "words").unwrap();
        fs::write(dir.path().join("a.ODT.txt"), "kept").unwrap();
        assert!(sh("exit 1").save(&doc).is_some());
        assert_eq!(
            fs::read_to_string(dir.path().join("a.ODT.txt")).unwrap(),
            "kept"
        );
    }

    #[test]
    fn saves_nothing_when_the_child_fails_or_the_file_is_another_kind() {
        let dir = TempDir::new();
        let doc = dir.path().join("a.xlsx");
        fs::write(&doc, "words").unwrap();
        for script in ["echo half; exit 2", "echo half; exit 1"] {
            assert_eq!(sh(script).save(&doc), None, "{script}");
            assert!(!text_path(&doc).exists(), "{script}");
        }
        let other = dir.path().join("a.md");
        fs::write(&other, "words").unwrap();
        assert_eq!(sh("echo text").save(&other), None);
        assert_eq!(Extractor::default().save(&doc), None);
    }

    #[test]
    fn ends_a_child_that_prints_too_much() {
        let dir = TempDir::new();
        let doc = dir.path().join("a.rtf");
        fs::write(&doc, "words").unwrap();
        let started = Instant::now();
        assert_eq!(sh("yes").save(&doc), None);
        assert!(started.elapsed() < TIMEOUT);
        assert!(!text_path(&doc).exists());
    }

    #[test]
    fn ends_a_child_that_takes_too_long() {
        let dir = TempDir::new();
        let doc = dir.path().join("a.epub");
        fs::write(&doc, "words").unwrap();
        let slow = sh("echo early; exec sleep 30").timeout(Duration::from_millis(200));
        let started = Instant::now();
        assert_eq!(slow.save(&doc), None);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!text_path(&doc).exists());
    }

    #[test]
    fn leaves_a_document_past_the_input_cap_alone() {
        let dir = TempDir::new();
        let doc = dir.path().join("big.pptx");
        let file = fs::File::create(&doc).unwrap();
        file.set_len(INPUT_CAP + 1).unwrap();
        assert_eq!(sh("echo text").save(&doc), None);
    }

    #[test]
    fn knows_the_extensions_with_text() {
        assert!(wants(Path::new("/a/b.DOCX")));
        assert!(wants(Path::new("/a/b.ipynb")));
        assert!(!wants(Path::new("/a/b.pdf")));
        assert!(!wants(Path::new("/a/docx")));
        assert_eq!(
            text_path(Path::new("/a/b.epub")),
            Path::new("/a/b.epub.txt")
        );
    }
}

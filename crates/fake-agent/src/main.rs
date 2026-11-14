// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `fake-agent`, which writes the same terminal output every run. Ctrl-C
//! stops it, like `yes`.

use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use maxhq_fake_agent::cast::{self, EventKind};
use maxhq_fake_agent::load;
use maxhq_fake_agent::playback::{self, Playback};
use serde_json::json;

const USAGE: &str = "\
usage:
  fake-agent replay FILE [--speed X] [--idle-limit SECS] [--loop]
      Plays an asciicast v2 or v3 recording with its original timing.
  fake-agent info FILE
      Prints a recording's size, length and event counts as JSON.
  fake-agent flood [--bytes SIZE] [--line TEXT]
      Writes TEXT and a newline until SIZE bytes are out, 200M by default.
      SIZE takes a K, M or G suffix.
  fake-agent spinner [--hz N] [--cols N] [--seconds SECS]
      Rewrites one full line N times a second, 10 by default, 200 wide.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        // A reader that went away, such as `head`, just ends the run.
        Err(Failure::Io(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("fake-agent: {message}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(failure) => {
            eprintln!("fake-agent: {failure}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{path}: {source}")]
    Cast {
        path: String,
        source: cast::CastError,
    },
}

fn usage(message: impl Into<String>) -> Failure {
    Failure::Usage(message.into())
}

/// The arguments after the subcommand. Each lookup takes what it finds out
/// of `rest`, so whatever is left at the end is a mistake.
struct Flags {
    rest: Vec<String>,
}

impl Flags {
    /// Takes the value of `--name`, if given.
    fn value(&mut self, name: &str) -> Result<Option<String>, Failure> {
        let Some(at) = self.rest.iter().position(|arg| arg == name) else {
            return Ok(None);
        };
        if at + 1 == self.rest.len() {
            return Err(usage(format!("{name} needs a value")));
        }
        Ok(self.rest.drain(at..at + 2).nth(1))
    }

    fn parsed<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>, Failure> {
        self.value(name)?
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| usage(format!("{name} can't be {value:?}")))
            })
            .transpose()
    }

    /// Takes `--name` as a number of seconds above zero.
    fn seconds(&mut self, name: &str) -> Result<Option<Duration>, Failure> {
        let Some(secs) = self.parsed::<f64>(name)? else {
            return Ok(None);
        };
        if !(secs.is_finite() && secs > 0.0) {
            return Err(usage(format!("{name} must be above zero")));
        }
        Duration::try_from_secs_f64(secs)
            .map(Some)
            .map_err(|_| usage(format!("{name} is too long")))
    }

    fn switch(&mut self, name: &str) -> bool {
        let at = self.rest.iter().position(|arg| arg == name);
        if let Some(at) = at {
            self.rest.remove(at);
        }
        at.is_some()
    }

    /// The one positional argument, once every flag is taken.
    fn positional(mut self, what: &str) -> Result<String, Failure> {
        match self.rest.len() {
            0 => Err(usage(format!("missing {what}"))),
            1 if !self.rest[0].starts_with("--") => Ok(self.rest.remove(0)),
            _ => Err(usage(format!("unexpected {:?}", self.rest[0]))),
        }
    }

    fn done(self) -> Result<(), Failure> {
        match self.rest.first() {
            None => Ok(()),
            Some(extra) => Err(usage(format!("unexpected {extra:?}"))),
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, Failure> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| usage("missing a command"))?;
    let mut flags = Flags {
        rest: rest.to_vec(),
    };
    match command.as_str() {
        "replay" => {
            let speed = flags.parsed::<f64>("--speed")?.unwrap_or(1.0);
            if !(speed.is_finite() && speed > 0.0) {
                return Err(usage("--speed must be above zero"));
            }
            let idle_limit = flags.seconds("--idle-limit")?;
            let repeat = flags.switch("--loop");
            let path = flags.positional("FILE")?;
            replay(&path, Playback { speed, idle_limit }, repeat)
        }
        "info" => info(&flags.positional("FILE")?),
        "flood" => {
            let bytes = match flags.value("--bytes")? {
                Some(size) => {
                    parse_size(&size).ok_or_else(|| usage(format!("--bytes can't be {size:?}")))?
                }
                None => 200 * 1024 * 1024,
            };
            let line = flags.value("--line")?.unwrap_or_else(|| "y".into());
            flags.done()?;
            flood(&line, bytes)
        }
        "spinner" => {
            let hz = flags.parsed::<f64>("--hz")?.unwrap_or(10.0);
            if !(hz.is_finite() && hz > 0.0) {
                return Err(usage("--hz must be above zero"));
            }
            let period =
                Duration::try_from_secs_f64(1.0 / hz).map_err(|_| usage("--hz is too low"))?;
            let cols = flags.parsed::<usize>("--cols")?.unwrap_or(200);
            let end = flags.seconds("--seconds")?;
            flags.done()?;
            spinner(period, cols, end)
        }
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(usage(format!("unknown command {other:?}"))),
    }
}

fn load(path: &str) -> Result<cast::Recording, Failure> {
    let text = std::fs::read_to_string(path)?;
    cast::parse(&text).map_err(|source| Failure::Cast {
        path: path.into(),
        source,
    })
}

fn replay(path: &str, playback: Playback, repeat: bool) -> Result<ExitCode, Failure> {
    let recording = load(path)?;
    // Each lap would take no time and print nothing, so --loop would spin.
    if repeat && recording.output_bytes() == 0 {
        return Err(usage("--loop needs a recording that prints something"));
    }
    let times = playback::schedule(&recording, playback);
    let mut out = io::stdout().lock();
    loop {
        // Sleeping to a deadline from the start keeps rounding from adding up.
        // A deadline past what Instant holds never comes.
        let start = Instant::now();
        for (event, at) in recording.events.iter().zip(&times) {
            thread::sleep(start.checked_add(*at).map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            }));
            match &event.kind {
                EventKind::Output(data) => {
                    out.write_all(data.as_bytes())?;
                    out.flush()?;
                }
                EventKind::Exit(status) if !repeat => {
                    return Ok(ExitCode::from(u8::try_from(*status).unwrap_or(1)));
                }
                _ => {}
            }
        }
        if !repeat {
            return Ok(ExitCode::SUCCESS);
        }
    }
}

fn info(path: &str) -> Result<ExitCode, Failure> {
    let recording = load(path)?;
    let count = |wanted: fn(&EventKind) -> bool| {
        recording.events.iter().filter(|e| wanted(&e.kind)).count()
    };
    let summary = json!({
        "version": recording.version,
        "cols": recording.cols,
        "rows": recording.rows,
        "duration_secs": recording.duration().as_secs_f64(),
        "idle_time_limit_secs": recording.idle_time_limit.map(|limit| limit.as_secs_f64()),
        "output_bytes": recording.output_bytes(),
        "events": {
            "output": count(|kind| matches!(kind, EventKind::Output(_))),
            "input": count(|kind| matches!(kind, EventKind::Input(_))),
            "resize": count(|kind| matches!(kind, EventKind::Resize { .. })),
            "marker": count(|kind| matches!(kind, EventKind::Marker(_))),
            "exit": count(|kind| matches!(kind, EventKind::Exit(_))),
        },
    });
    println!("{summary:#}");
    Ok(ExitCode::SUCCESS)
}

fn flood(line: &str, total: usize) -> Result<ExitCode, Failure> {
    let block = load::flood_block(line, 64 * 1024);
    let mut out = io::stdout().lock();
    let mut left = total;
    while left > 0 {
        let take = left.min(block.len());
        out.write_all(&block[..take])?;
        left -= take;
    }
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn spinner(period: Duration, cols: usize, end: Option<Duration>) -> Result<ExitCode, Failure> {
    let mut out = io::stdout().lock();
    let start = Instant::now();
    for tick in 0u64.. {
        let elapsed = start.elapsed();
        if end.is_some_and(|end| elapsed >= end) {
            break;
        }
        out.write_all(load::spinner_line(tick, cols, elapsed.as_secs_f64()).as_bytes())?;
        out.flush()?;
        // A slow spinner can put its next line past what a Duration holds,
        // and it stops at the end rather than waiting for that line.
        let next = Duration::try_from_secs_f64(period.as_secs_f64() * (tick + 1) as f64)
            .unwrap_or(Duration::MAX);
        let wake = end.map_or(next, |end| next.min(end));
        if let Some(wait) = wake.checked_sub(start.elapsed()) {
            thread::sleep(wait);
        }
    }
    out.write_all(b"\r\n")?;
    Ok(ExitCode::SUCCESS)
}

/// Reads sizes such as `4096`, `64K`, `200M` or `1G`, in powers of 1024.
fn parse_size(text: &str) -> Option<usize> {
    let (digits, shift) = match text.as_bytes().last()? {
        b'K' | b'k' => (&text[..text.len() - 1], 10),
        b'M' | b'm' => (&text[..text.len() - 1], 20),
        b'G' | b'g' => (&text[..text.len() - 1], 30),
        _ => (text, 0),
    };
    digits.parse::<usize>().ok()?.checked_mul(1 << shift)
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn sizes_take_binary_suffixes() {
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("64K"), Some(65_536));
        assert_eq!(parse_size("200m"), Some(200 * 1024 * 1024));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("M"), None);
        assert_eq!(parse_size("ten"), None);
    }
}

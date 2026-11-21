// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminal channel. Each window gets one, which carries the output of
//! every terminal it attached as records: `[number u32][offset u64][bytes]`,
//! big-endian, where the number is the window's own for the terminal and
//! the offset is where the bytes start in the terminal's output. A record
//! with no bytes is a reset: what the window shows of the terminal is gone,
//! and its output starts over at the offset.
//!
//! Output waits at most [`GAP`], so a burst goes as a few long records
//! instead of many short ones. The first output after a quiet spell goes at
//! once. Output that carries straight on from the record waiting joins it,
//! up to [`RECORD`] bytes. One thread sends every record, so each window
//! gets a terminal's records in order.
//!
//! Windows that attach the same terminal share its attachment on the link,
//! which detaches it once the last window lets go. Output a window has
//! already, as when another window asks for a terminal from further back,
//! doesn't go to it again.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use maxhq_client::TerminalSink;
use maxhq_protocol::ids::{LinkId, TerminalId};

use crate::pace::{lock, Outbox};
use crate::timeline::Sink;

/// The longest output waits to join a record, so typing still echoes
/// within a frame.
pub const GAP: Duration = Duration::from_millis(4);

/// The most bytes one record carries.
pub const RECORD: usize = 256 * 1024;

/// A record's number and offset, before its bytes.
const HEADER: usize = 12;

/// Where a window's records go: its channel in the desktop app, and a
/// recorder in tests. Each record goes as a message of its own.
pub type RecordSink = dyn Sink<Vec<u8>>;

/// A terminal no window has attached now, with the link to detach it on.
pub type Detach = (TerminalId, LinkId);

/// Every window's terminal channel, with the thread that sends their
/// records. Clones share it, and the thread stops once the last one drops.
#[derive(Clone)]
pub struct Terminals(Arc<Owner>);

/// Stops the thread when the last [`Terminals`] drops.
struct Owner(Arc<Shared>);

/// What the thread shares with the callers and the links.
#[derive(Default)]
struct Shared {
    hub: Mutex<Hub>,
    /// Wakes the thread when something new waits to go out.
    wake: Condvar,
}

impl Drop for Owner {
    fn drop(&mut self) {
        lock(&self.0.hub).stop = true;
        self.0.wake.notify_all();
    }
}

impl Terminals {
    /// Starts the thread that sends records.
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let sender = Arc::clone(&shared);
        if let Err(error) = thread::Builder::new()
            .name("terminals".to_owned())
            .spawn(move || send_records(&sender))
        {
            tracing::error!(%error, "couldn't start the terminal thread, so no terminal shows");
        }
        Self(Arc::new(Owner(shared)))
    }

    /// Has a window attach a terminal that comes through `link`, from byte
    /// `after` of its output on. Gives the window's number for it, and the
    /// terminals no window has attached now, which the window had attached
    /// through the channel it had before it reloaded.
    pub fn attach(
        &self,
        window: &str,
        sink: Arc<RecordSink>,
        terminal: TerminalId,
        after: Option<u64>,
        link: LinkId,
    ) -> (u32, Vec<Detach>) {
        self.hub().attach(window, sink, terminal, after, link)
    }

    /// Takes a window's attachment of a terminal back. Gives the link to
    /// detach it on once no window has it attached.
    pub fn detach(&self, window: &str, terminal: TerminalId) -> Option<LinkId> {
        self.hub().detach(window, terminal)
    }

    /// Forgets a window that closed or reloaded. Gives the terminals no
    /// window has attached now.
    pub fn release(&self, window: &str) -> Vec<Detach> {
        self.hub().release(window)
    }

    /// The link a terminal comes through, if a window has it attached.
    pub fn link_of(&self, terminal: TerminalId) -> Option<LinkId> {
        self.hub().links.get(&terminal).copied()
    }

    /// Where the links hand the output of the terminals they attached.
    pub fn feed(&self) -> Arc<dyn TerminalSink> {
        Arc::new(Feed(Arc::clone(&self.0 .0)))
    }

    fn hub(&self) -> MutexGuard<'_, Hub> {
        lock(&self.0 .0.hub)
    }
}

/// The hub as the links see it. It doesn't keep the thread going.
struct Feed(Arc<Shared>);

impl TerminalSink for Feed {
    fn output(&self, terminal: TerminalId, offset: u64, bytes: &[u8]) {
        if lock(&self.0.hub).output(terminal, offset, bytes, Instant::now()) {
            self.0.wake.notify_one();
        }
    }

    fn reset(&self, terminal: TerminalId, from: u64) {
        if lock(&self.0.hub).reset(terminal, from, Instant::now()) {
            self.0.wake.notify_one();
        }
    }
}

/// Sends each window's records once they're due, until the hub stops.
fn send_records(shared: &Shared) {
    let mut hub = lock(&shared.hub);
    loop {
        if hub.stop {
            return;
        }
        let now = Instant::now();
        let due = hub.take_due(now);
        if !due.is_empty() {
            // Sending happens outside the lock, so the links never wait on
            // a page. Only this thread sends, which keeps the order.
            drop(hub);
            for (window, sink, records) in due {
                if !sink.send(records) {
                    tracing::debug!(%window, "a window's terminal channel stopped taking records");
                }
            }
            hub = lock(&shared.hub);
            continue;
        }
        hub = match hub.next_due() {
            Some(at) => {
                let wait = at.saturating_duration_since(now);
                shared
                    .wake
                    .wait_timeout(hub, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .wake
                .wait(hub)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// Which windows attached which terminals, and what waits to go to each.
/// It takes the time as an argument, so tests drive it with made-up times.
#[derive(Default)]
struct Hub {
    windows: BTreeMap<String, Window>,
    /// The link each terminal a window attached comes through.
    links: HashMap<TerminalId, LinkId>,
    /// Tells the thread to end.
    stop: bool,
}

/// One window's channel.
struct Window {
    sink: Arc<RecordSink>,
    /// The window's number for each terminal it attached since its page
    /// loaded. A terminal keeps its number, so a late record for it is
    /// never taken for another's.
    numbers: HashMap<TerminalId, u32>,
    /// The terminals it has attached now.
    attached: HashMap<TerminalId, Watch>,
}

/// One terminal a window attached.
struct Watch {
    number: u32,
    /// Where the output the window has ends, so output it has already
    /// doesn't go again. None until the first comes.
    end: Option<u64>,
    records: Outbox<Vec<u8>>,
}

/// The records due for one window.
type Due = (String, Arc<RecordSink>, Vec<Vec<u8>>);

impl Hub {
    fn attach(
        &mut self,
        label: &str,
        sink: Arc<RecordSink>,
        terminal: TerminalId,
        after: Option<u64>,
        link: LinkId,
    ) -> (u32, Vec<Detach>) {
        // A window that reloaded has a new channel, and what it attached
        // before is gone.
        let mut released = if self
            .windows
            .get(label)
            .is_some_and(|window| window.sink.id() != sink.id())
        {
            self.release(label)
        } else {
            Vec::new()
        };
        let window = self
            .windows
            .entry(label.to_owned())
            .or_insert_with(|| Window {
                sink,
                numbers: HashMap::new(),
                attached: HashMap::new(),
            });
        let next = u32::try_from(window.numbers.len() + 1).unwrap_or(u32::MAX);
        let number = *window.numbers.entry(terminal).or_insert(next);
        // Attaching again starts over from `after`, so what waited from
        // before goes nowhere.
        window.attached.insert(
            terminal,
            Watch {
                number,
                end: after,
                records: Outbox::every(GAP),
            },
        );
        self.links.insert(terminal, link);
        // The new page attaching what the old one had keeps it attached.
        released.retain(|&detach| detach != (terminal, link));
        (number, released)
    }

    fn detach(&mut self, label: &str, terminal: TerminalId) -> Option<LinkId> {
        let window = self.windows.get_mut(label)?;
        window.attached.remove(&terminal)?;
        self.unwatched(terminal)
    }

    fn release(&mut self, label: &str) -> Vec<Detach> {
        let Some(window) = self.windows.remove(label) else {
            return Vec::new();
        };
        window
            .attached
            .into_keys()
            .filter_map(|terminal| Some((terminal, self.unwatched(terminal)?)))
            .collect()
    }

    /// Forgets a terminal's link once no window has it attached, and gives
    /// the link.
    fn unwatched(&mut self, terminal: TerminalId) -> Option<LinkId> {
        let watched = self
            .windows
            .values()
            .any(|window| window.attached.contains_key(&terminal));
        if watched {
            None
        } else {
            self.links.remove(&terminal)
        }
    }

    /// Queues output for every window that attached the terminal, and says
    /// whether any did.
    fn output(&mut self, terminal: TerminalId, offset: u64, bytes: &[u8], now: Instant) -> bool {
        let mut queued = false;
        for window in self.windows.values_mut() {
            if let Some(watch) = window.attached.get_mut(&terminal) {
                queued |= watch.push(offset, bytes, now);
            }
        }
        queued
    }

    /// Queues a reset for every window whose output ends before `from`.
    fn reset(&mut self, terminal: TerminalId, from: u64, now: Instant) -> bool {
        let mut queued = false;
        for window in self.windows.values_mut() {
            if let Some(watch) = window.attached.get_mut(&terminal) {
                queued |= watch.reset(from, now);
            }
        }
        queued
    }

    /// Takes every record that's due by `now`, a window's together.
    fn take_due(&mut self, now: Instant) -> Vec<Due> {
        self.windows
            .iter_mut()
            .filter_map(|(label, window)| {
                let records: Vec<Vec<u8>> = window
                    .attached
                    .values_mut()
                    .filter(|watch| watch.records.due().is_some_and(|due| due <= now))
                    .flat_map(|watch| watch.records.take(now))
                    .collect();
                (!records.is_empty()).then(|| (label.clone(), Arc::clone(&window.sink), records))
            })
            .collect()
    }

    /// When the next record is due, if anything waits.
    fn next_due(&self) -> Option<Instant> {
        self.windows
            .values()
            .flat_map(|window| window.attached.values())
            .filter_map(|watch| watch.records.due())
            .min()
    }
}

impl Watch {
    /// Queues the part of some output the window doesn't have yet. Says
    /// whether anything was left to queue. Output that would end past the
    /// largest offset is dropped.
    fn push(&mut self, offset: u64, bytes: &[u8], now: Instant) -> bool {
        let Some(end) = offset.checked_add(bytes.len() as u64) else {
            return false;
        };
        let skip = self.end.map_or(0, |end| end.saturating_sub(offset));
        let rest = usize::try_from(skip)
            .ok()
            .and_then(|skip| bytes.get(skip..));
        let Some(bytes) = rest.filter(|rest| !rest.is_empty()) else {
            return false;
        };
        // Below `end`, which fits.
        let offset = offset + skip;
        self.end = Some(end);
        let waiting = self.records.waiting(now);
        match waiting.last_mut() {
            Some(last) if follows(last, offset) && last.len() + bytes.len() <= RECORD => {
                last.extend_from_slice(bytes);
            }
            _ => waiting.push(record(self.number, offset, bytes)),
        }
        true
    }

    /// Queues a reset, unless the window has what follows `from` already.
    fn reset(&mut self, from: u64, now: Instant) -> bool {
        if self.end.is_some_and(|end| end >= from) {
            return false;
        }
        self.end = Some(from);
        self.records.push(record(self.number, from, &[]), now);
        true
    }
}

/// A record of `bytes` from `offset` in the window's terminal `number`.
pub fn record(number: u32, offset: u64, bytes: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(HEADER + bytes.len());
    record.extend_from_slice(&number.to_be_bytes());
    record.extend_from_slice(&offset.to_be_bytes());
    record.extend_from_slice(bytes);
    record
}

/// Whether bytes from `offset` carry straight on from `record`, which
/// isn't a reset.
fn follows(record: &[u8], offset: u64) -> bool {
    let Some(start) = record.get(4..HEADER) else {
        return false;
    };
    let start = u64::from_be_bytes(start.try_into().unwrap_or_default());
    record.len() > HEADER && start.checked_add((record.len() - HEADER) as u64) == Some(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::tests::Recorder;

    fn terminal(n: u8) -> TerminalId {
        format!("0199a7c2-0000-7000-8000-0000000000{n:02x}")
            .parse()
            .unwrap()
    }

    fn link(name: &str) -> LinkId {
        LinkId::derive(name)
    }

    fn ms(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// Takes what's due at `now` and hands it to the sinks.
    fn send(hub: &mut Hub, now: Instant) {
        for (_, sink, records) in hub.take_due(now) {
            sink.send(records);
        }
    }

    #[test]
    fn a_record_carries_the_number_offset_and_bytes() {
        assert_eq!(
            record(3, 0x0102, b"hi"),
            [0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 1, 2, b'h', b'i']
        );
        // A reset has no bytes.
        assert_eq!(record(1, 7, &[]).len(), HEADER);
        assert!(follows(&record(1, 10, b"abc"), 13));
        assert!(!follows(&record(1, 10, b"abc"), 14));
        assert!(!follows(&record(1, 10, &[]), 10));
    }

    #[test]
    fn output_goes_at_once_then_joins_for_a_gap() {
        let mut hub = Hub::default();
        let start = Instant::now();
        let main = Recorder::<Vec<u8>>::new(1);
        let one = terminal(1);
        let (number, released) = hub.attach("main", main.clone(), one, None, link("devbox"));
        assert_eq!((number, released), (1, Vec::new()));

        // The first output after a quiet spell is due at once.
        assert!(hub.output(one, 0, b"$ ", start));
        assert_eq!(hub.next_due(), Some(start));
        send(&mut hub, start);
        // What follows within the gap joins one record, due a gap later.
        hub.output(one, 2, b"l", ms(start, 1));
        hub.output(one, 3, b"s", ms(start, 2));
        assert_eq!(hub.next_due(), Some(ms(start, 4)));
        send(&mut hub, ms(start, 3));
        assert_eq!(main.messages(), vec![record(1, 0, b"$ ")]);
        send(&mut hub, ms(start, 4));
        assert_eq!(
            main.messages(),
            vec![record(1, 0, b"$ "), record(1, 2, b"ls")]
        );
        assert_eq!(hub.next_due(), None);
    }

    #[test]
    fn a_gap_in_the_offsets_or_a_full_record_starts_another() {
        let mut hub = Hub::default();
        let start = Instant::now();
        let main = Recorder::<Vec<u8>>::new(1);
        let one = terminal(1);
        hub.attach("main", main.clone(), one, None, link("devbox"));
        hub.output(one, 0, b"a", start);
        send(&mut hub, start);

        hub.output(one, 1, b"b", ms(start, 1));
        hub.output(one, 10, b"c", ms(start, 1));
        let big = vec![b'x'; RECORD - HEADER];
        hub.output(one, 11, &big, ms(start, 2));
        hub.output(one, 11 + big.len() as u64, b"y", ms(start, 2));
        send(&mut hub, ms(start, 4));
        let mut full = record(1, 10, b"c");
        full.extend_from_slice(&big);
        assert_eq!(
            main.messages()[1..],
            [
                record(1, 1, b"b"),
                record(1, 10, b"c"),
                record(1, 11, &big),
                record(1, 11 + big.len() as u64, b"y"),
            ],
            "{} bytes would pass the cap",
            full.len()
        );
        assert!(main.messages().iter().all(|record| record.len() <= RECORD));
    }

    #[test]
    fn output_ending_past_the_largest_offset_is_dropped() {
        let mut hub = Hub::default();
        let start = Instant::now();
        let main = Recorder::<Vec<u8>>::new(1);
        let one = terminal(1);
        hub.attach("main", main.clone(), one, None, link("devbox"));
        assert!(!hub.output(one, u64::MAX, b"xy", start));
        assert!(!hub.output(one, u64::MAX - 1, b"abc", start));
        assert_eq!(hub.next_due(), None);
        // Ending right at the largest offset still fits, and what goes on
        // past it is dropped too.
        assert!(hub.output(one, u64::MAX - 2, b"ok", start));
        assert!(!hub.output(one, u64::MAX, b"!", ms(start, 1)));
        send(&mut hub, ms(start, 10));
        assert_eq!(main.messages(), vec![record(1, u64::MAX - 2, b"ok")]);
        assert!(!follows(&record(1, u64::MAX - 1, b"ab"), 0));
    }

    #[test]
    fn every_window_gets_the_output_but_none_gets_it_twice() {
        let mut hub = Hub::default();
        let start = Instant::now();
        let (main, other) = (Recorder::<Vec<u8>>::new(1), Recorder::<Vec<u8>>::new(2));
        let (one, two) = (terminal(1), terminal(2));
        hub.attach("main", main.clone(), two, None, link("devbox"));
        let (number, _) = hub.attach("main", main.clone(), one, None, link("devbox"));
        assert_eq!(number, 2);
        hub.output(one, 0, b"hello", start);
        send(&mut hub, start);

        // Another window asks for everything again, which the link asks
        // `maxd` for. The first window has the start already.
        let (number, _) = hub.attach("other", other.clone(), one, None, link("devbox"));
        assert_eq!(number, 1);
        hub.output(one, 0, b"hello world", ms(start, 10));
        send(&mut hub, ms(start, 10));
        assert_eq!(
            main.messages(),
            vec![record(2, 0, b"hello"), record(2, 5, b" world")]
        );
        assert_eq!(other.messages(), vec![record(1, 0, b"hello world")]);

        // A reset reaches only a window that has a hole before `from`.
        hub.detach("other", one);
        hub.attach("other", other.clone(), one, Some(4), link("devbox"));
        assert!(hub.reset(one, 11, ms(start, 20)));
        assert!(!hub.reset(one, 11, ms(start, 20)), "both have it now");
        send(&mut hub, ms(start, 20));
        assert_eq!(main.messages().len(), 2);
        assert_eq!(other.messages()[1..], [record(1, 11, &[])]);
    }

    #[test]
    fn the_link_detaches_once_the_last_window_lets_go() {
        let mut hub = Hub::default();
        let (main, other) = (Recorder::<Vec<u8>>::new(1), Recorder::<Vec<u8>>::new(2));
        let (one, two) = (terminal(1), terminal(2));
        let nl = link("devbox");
        hub.attach("main", main.clone(), one, None, nl);
        hub.attach("other", other.clone(), one, None, nl);
        hub.attach("other", other.clone(), two, None, nl);
        assert_eq!(hub.detach("main", one), None, "the other window has it");
        assert_eq!(hub.detach("main", one), None, "detached already");
        assert_eq!(hub.links.get(&one), Some(&nl));
        let mut released = hub.release("other");
        released.sort();
        let mut both = vec![(one, nl), (two, nl)];
        both.sort();
        assert_eq!(released, both);
        assert!(hub.links.is_empty());
        assert!(hub.release("other").is_empty());

        // Attaching again keeps the window's number for the terminal.
        hub.attach("main", main.clone(), two, None, nl);
        let (number, _) = hub.attach("main", main.clone(), one, None, nl);
        assert_eq!(number, 1);
        assert_eq!(hub.detach("main", one), Some(nl));
    }

    #[test]
    fn a_reloaded_window_drops_what_its_old_page_attached() {
        let mut hub = Hub::default();
        let start = Instant::now();
        let (old, new) = (Recorder::<Vec<u8>>::new(1), Recorder::<Vec<u8>>::new(2));
        let (one, two) = (terminal(1), terminal(2));
        let nl = link("devbox");
        hub.attach("main", old.clone(), one, None, nl);
        hub.attach("main", old.clone(), two, None, nl);
        // The same channel again keeps everything.
        let (number, released) = hub.attach("main", old.clone(), two, Some(3), nl);
        assert_eq!((number, released), (2, Vec::new()));

        let (number, released) = hub.attach("main", new.clone(), two, None, nl);
        assert_eq!(number, 1, "the new page numbers afresh");
        assert_eq!(released, vec![(one, nl)]);
        hub.output(one, 0, b"gone", start);
        hub.output(two, 0, b"kept", start);
        send(&mut hub, start);
        assert!(old.messages().is_empty());
        assert_eq!(new.messages(), vec![record(1, 0, b"kept")]);
    }

    #[test]
    fn the_thread_sends_through_the_feed() {
        let terminals = Terminals::start();
        let main = Recorder::<Vec<u8>>::new(1);
        let one = terminal(1);
        terminals.attach("main", main.clone(), one, None, link("devbox"));
        let feed = terminals.feed();
        feed.output(one, 0, b"$ ");
        feed.reset(one, 100);
        let deadline = Instant::now() + Duration::from_secs(5);
        while main.messages().len() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            main.messages(),
            vec![record(1, 0, b"$ "), record(1, 100, &[])]
        );
        assert_eq!(terminals.release("main"), vec![(one, link("devbox"))]);
    }
}

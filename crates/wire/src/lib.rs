// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The writer each MaxHQ connection sends its frames through.
//!
//! One link carries control messages, timeline updates, terminal and tunnel
//! bytes and file data at once. [`spawn`] starts a task that owns the write
//! side and keeps three queues, one per [`Queue`]. It writes one whole frame
//! at a time from the first queue that has one, so a keystroke or an
//! approval waits behind at most one frame of file data rather than
//! megabytes of it.
//!
//! Each queue holds a set number of bytes. A sender that finds its queue
//! full waits, so a slow reader slows the code that produces for it instead
//! of filling memory.
//!
//! [`Window`] is the credit window a receiver grants for one transfer. The
//! writer can't reorder bytes that have already left it, so the window is
//! what bounds how much file data sits in the pipes between the two ends.

#![warn(missing_docs)]

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

/// Which queue a frame waits in. The writer empties `Control` before it
/// looks at `Timeline`, and `Timeline` before `Bulk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Queue {
    /// Control messages and terminal input: pings, actions and their
    /// results, answers, errors and transfer credit.
    Control,
    /// Snapshots, deltas and timeline updates, and terminal output.
    Timeline,
    /// Transfer chunks, and tunnel messages with the bytes they carry. A
    /// tunnel's messages and bytes share the queue, so a stream's close
    /// never passes its bytes.
    Bulk,
}

impl Queue {
    /// How many bytes may wait in the queue before a sender waits. A frame
    /// larger than this still goes, alone.
    pub const fn budget(self) -> usize {
        match self {
            Queue::Control | Queue::Timeline => 4 * 1024 * 1024,
            Queue::Bulk => 1024 * 1024,
        }
    }

    const fn index(self) -> usize {
        match self {
            Queue::Control => 0,
            Queue::Timeline => 1,
            Queue::Bulk => 2,
        }
    }
}

/// The writer has stopped, because the link failed or every sender closed
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the connection's writer has stopped")
    }
}

impl std::error::Error for Closed {}

/// Why [`Sender::try_send`] didn't queue a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrySendError {
    /// The queue holds its budget already.
    Full,
    /// The writer has stopped.
    Closed,
}

impl std::fmt::Display for TrySendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrySendError::Full => f.write_str("the queue is full"),
            TrySendError::Closed => Closed.fmt(f),
        }
    }
}

impl std::error::Error for TrySendError {}

/// Queues frames for one connection's writer. Clones share the writer,
/// which stops once every clone is gone or one calls [`Sender::close`].
#[derive(Clone)]
pub struct Sender {
    shared: Arc<Shared>,
}

struct Shared {
    lanes: [Lane; 3],
    signal: Arc<Signal>,
}

struct Lane {
    queue: mpsc::UnboundedSender<Item>,
    room: Arc<Semaphore>,
}

/// What the writer and the senders both see.
struct Signal {
    closed: AtomicBool,
    wake: Notify,
}

struct Item {
    frame: Vec<u8>,
    _room: OwnedSemaphorePermit,
}

impl std::fmt::Debug for Sender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Sender {
    /// Queues one encoded frame, of any kind, waiting while the queue is
    /// full. Several frames back to back in one buffer go out together, so
    /// pass file data one chunk at a time.
    pub async fn send(&self, queue: Queue, frame: Vec<u8>) -> Result<(), Closed> {
        if self.is_closed() {
            return Err(Closed);
        }
        let lane = &self.shared.lanes[queue.index()];
        let room = lane
            .room
            .clone()
            .acquire_many_owned(permits(queue, frame.len()))
            .await
            .map_err(|_| Closed)?;
        lane.queue
            .send(Item { frame, _room: room })
            .map_err(|_| Closed)
    }

    /// Queues one encoded frame now, or says why it can't.
    pub fn try_send(&self, queue: Queue, frame: Vec<u8>) -> Result<(), TrySendError> {
        if self.is_closed() {
            return Err(TrySendError::Closed);
        }
        let lane = &self.shared.lanes[queue.index()];
        let room = lane
            .room
            .clone()
            .try_acquire_many_owned(permits(queue, frame.len()))
            .map_err(|err| match err {
                tokio::sync::TryAcquireError::NoPermits => TrySendError::Full,
                tokio::sync::TryAcquireError::Closed => TrySendError::Closed,
            })?;
        lane.queue
            .send(Item { frame, _room: room })
            .map_err(|_| TrySendError::Closed)
    }

    /// Stops the writer once it has written what's queued. Sends after this
    /// fail, for every clone.
    pub fn close(&self) {
        self.shared.signal.closed.store(true, Ordering::Release);
        self.shared.signal.wake.notify_one();
    }

    /// Whether the writer has stopped or been told to.
    pub fn is_closed(&self) -> bool {
        self.shared.signal.closed.load(Ordering::Acquire)
    }
}

/// Permits a frame takes from its queue: one per byte, up to the budget.
fn permits(queue: Queue, len: usize) -> u32 {
    u32::try_from(len.min(queue.budget())).unwrap_or(u32::MAX)
}

/// Starts the writer for one connection on the current runtime. The task
/// ends when every sender is gone or closed, after it has written what was
/// queued and shut the write side, or as soon as a write fails.
pub fn spawn<W>(writer: W) -> (Sender, JoinHandle<io::Result<()>>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let signal = Arc::new(Signal {
        closed: AtomicBool::new(false),
        wake: Notify::new(),
    });
    let mut senders = Vec::with_capacity(3);
    let mut receivers = Vec::with_capacity(3);
    let mut rooms = Vec::with_capacity(3);
    for queue in [Queue::Control, Queue::Timeline, Queue::Bulk] {
        let (tx, rx) = mpsc::unbounded_channel();
        let room = Arc::new(Semaphore::new(queue.budget()));
        senders.push(Lane {
            queue: tx,
            room: room.clone(),
        });
        receivers.push(rx);
        rooms.push(room);
    }
    let lanes: [Lane; 3] = match senders.try_into() {
        Ok(lanes) => lanes,
        Err(_) => unreachable!("three queues"),
    };
    let receivers: [mpsc::UnboundedReceiver<Item>; 3] = match receivers.try_into() {
        Ok(receivers) => receivers,
        Err(_) => unreachable!("three queues"),
    };
    let stop = Stop {
        rooms,
        signal: signal.clone(),
    };
    let task = tokio::spawn(write(writer, receivers, signal.clone(), stop));
    let sender = Sender {
        shared: Arc::new(Shared { lanes, signal }),
    };
    (sender, task)
}

/// Marks the writer stopped and wakes every waiting sender, however the
/// task ends.
struct Stop {
    rooms: Vec<Arc<Semaphore>>,
    signal: Arc<Signal>,
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.signal.closed.store(true, Ordering::Release);
        for room in &self.rooms {
            room.close();
        }
    }
}

/// Bytes the writer collects before it hands them to the link, when frames
/// come faster than one write each.
const BUFFER: usize = 64 * 1024;

async fn write<W>(
    writer: W,
    mut lanes: [mpsc::UnboundedReceiver<Item>; 3],
    signal: Arc<Signal>,
    _stop: Stop,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut out = BufWriter::with_capacity(BUFFER, writer);
    loop {
        let item = match next(&mut lanes) {
            Some(item) => item,
            None => {
                out.flush().await?;
                let gone = lanes.iter().all(|lane| lane.is_closed() && lane.is_empty());
                if gone || signal.closed.load(Ordering::Acquire) {
                    break;
                }
                let [control, timeline, bulk] = &mut lanes;
                let item = tokio::select! {
                    biased;
                    item = control.recv() => item,
                    item = timeline.recv() => item,
                    item = bulk.recv() => item,
                    () = signal.wake.notified() => None,
                };
                // Nothing yet means a lane closed or a sender closed the
                // writer, which the next pass looks at.
                match item {
                    Some(item) => item,
                    None => continue,
                }
            }
        };
        out.write_all(&item.frame).await?;
    }
    out.shutdown().await
}

/// The next frame from the first queue that has one.
fn next(lanes: &mut [mpsc::UnboundedReceiver<Item>; 3]) -> Option<Item> {
    lanes.iter_mut().find_map(|lane| lane.try_recv().ok())
}

/// The credit window a receiver grants for one transfer.
///
/// It starts at [`Window::MIN`]. The receiver measures round trips, from a
/// grant going out to the bytes it allowed arriving. The quickest one seen
/// stands for the link's idle round trip, and whatever a round trip takes
/// past it is time the bytes spent queued. While that queueing is at most
/// half of [`Window::SLACK`] the window doubles. Up to the slack it grows by
/// a quarter, and past the slack it halves. It never leaves
/// [`Window::MIN`] to [`Window::MAX`].
///
/// Once bytes queue, the link is full and moves about `size / round_trip`,
/// so each byte the window adds queues for another `round_trip / size`.
/// Growth then stops where the bytes it adds would take the queueing past
/// the slack. A round trip times bytes that waited behind earlier grants,
/// so the growth after one round trip doesn't show in the next, and it
/// counts against that room until a round trip has measured it.
///
/// Every byte the window allows can sit in the pipes ahead of a ping, so
/// the queueing it measures is the delay a control message sees too. On a
/// link that moves 20 MB/s, 512 KiB is already 26 ms of data, so the window
/// stays under 1 MiB there. On a quick link where nothing queues it reaches
/// its most within four round trips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    size: u64,
    idle: Option<Duration>,
    /// Bytes the last round trip added that no round trip has measured yet.
    unmeasured: u64,
}

impl Window {
    /// Where a window starts, and the least it shrinks to. At a 40 Mbit/s
    /// uplink that's about 100 ms of file data ahead of a keystroke.
    pub const MIN: u64 = 512 * 1024;
    /// The most it grows to.
    pub const MAX: u64 = 8 * 1024 * 1024;
    /// How long bytes may queue, past the idle round trip, before the
    /// window shrinks. Up to half of it the window doubles, and from half
    /// to all of it the window grows by a quarter.
    pub const SLACK: Duration = Duration::from_millis(20);

    /// A window at its smallest.
    pub const fn new() -> Self {
        Self {
            size: Self::MIN,
            idle: None,
            unmeasured: 0,
        }
    }

    /// Bytes the sender may have on the way.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// The quickest round trip seen, which stands for the link's idle one.
    pub const fn idle(&self) -> Option<Duration> {
        self.idle
    }

    /// Grows or shrinks the window after one measured round trip.
    pub fn observe(&mut self, round_trip: Duration) {
        let idle = self.idle.map_or(round_trip, |idle| idle.min(round_trip));
        self.idle = Some(idle);
        let queued = round_trip.saturating_sub(idle);
        let size = if queued > Self::SLACK {
            self.size / 2
        } else {
            let step = if queued <= Self::SLACK / 2 {
                self.size
            } else {
                self.size / 4
            };
            self.size
                .saturating_add(step.min(self.room(queued, round_trip)))
        };
        let size = size.clamp(Self::MIN, Self::MAX);
        self.unmeasured = size.saturating_sub(self.size);
        self.size = size;
    }

    /// How many bytes the window may add before they'd queue past the
    /// slack. While nothing queues the link has room to spare, so there's
    /// no limit.
    fn room(&self, queued: Duration, round_trip: Duration) -> u64 {
        if queued.is_zero() {
            return u64::MAX;
        }
        let measured = u128::from(self.size.saturating_sub(self.unmeasured));
        let left = Self::SLACK.saturating_sub(queued).as_nanos();
        let room = measured * left / round_trip.as_nanos();
        u64::try_from(room)
            .unwrap_or(u64::MAX)
            .saturating_sub(self.unmeasured)
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, DuplexStream};

    const WAIT: Duration = Duration::from_secs(5);

    /// A frame the tests can tell apart: a length, a tag and filler.
    fn frame(tag: u8, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len + 5);
        out.extend_from_slice(&u32::try_from(len + 1).unwrap().to_be_bytes());
        out.push(tag);
        out.resize(len + 5, tag);
        out
    }

    /// Reads frames until the stream ends, giving each one's tag.
    async fn tags(mut stream: DuplexStream) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut len = [0u8; 4];
            if stream.read_exact(&mut len).await.is_err() {
                return out;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut body).await.unwrap();
            assert!(
                body.iter().all(|byte| *byte == body[0]),
                "a frame was split"
            );
            out.push(body[0]);
        }
    }

    #[tokio::test]
    async fn control_overtakes_bulk_that_is_waiting() {
        // A pipe much smaller than one chunk keeps the writer busy.
        let (near, far) = tokio::io::duplex(4 * 1024);
        let (sender, task) = spawn(near);
        let bulk = sender.clone();
        let filler = tokio::spawn(async move {
            for _ in 0..8 {
                bulk.send(Queue::Bulk, frame(b'b', 256 * 1024))
                    .await
                    .unwrap();
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        sender.send(Queue::Control, frame(b'c', 40)).await.unwrap();
        sender.send(Queue::Timeline, frame(b't', 40)).await.unwrap();
        let reader = tokio::spawn(tags(far));
        tokio::time::timeout(WAIT, filler).await.unwrap().unwrap();
        drop(sender);
        let tags = tokio::time::timeout(WAIT, reader).await.unwrap().unwrap();
        assert_eq!(tags.len(), 10);
        let control = tags.iter().position(|tag| *tag == b'c').unwrap();
        let timeline = tags.iter().position(|tag| *tag == b't').unwrap();
        assert!(
            control <= 1,
            "control came after {control} chunks: {tags:?}"
        );
        assert_eq!(timeline, control + 1, "{tags:?}");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn frames_in_one_queue_keep_their_order_whatever_their_kind() {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let (sender, task) = spawn(near);
        let reader = tokio::spawn(tags(far));
        for tag in 1..=50u8 {
            let len = usize::from(tag) * 997;
            sender.send(Queue::Timeline, frame(tag, len)).await.unwrap();
        }
        drop(sender);
        let tags = tokio::time::timeout(WAIT, reader).await.unwrap().unwrap();
        assert_eq!(tags, (1..=50).collect::<Vec<_>>());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_full_queue_makes_the_sender_wait() {
        // Nobody reads, so the queue fills and stays full.
        let (near, _far) = tokio::io::duplex(1024);
        let (sender, _task) = spawn(near);
        let mut waited = false;
        for _ in 0..16 {
            let send = sender.send(Queue::Bulk, frame(b'b', 128 * 1024));
            if tokio::time::timeout(Duration::from_millis(100), send)
                .await
                .is_err()
            {
                waited = true;
                break;
            }
        }
        assert!(waited, "sixteen chunks fit in a 1 MiB queue");
        assert_eq!(
            sender.try_send(Queue::Bulk, frame(b'b', 128 * 1024)),
            Err(TrySendError::Full)
        );
        // Other queues still take frames.
        sender.try_send(Queue::Control, frame(b'c', 10)).unwrap();
    }

    #[tokio::test]
    async fn the_writer_stops_when_every_sender_is_gone() {
        let (near, far) = tokio::io::duplex(1024);
        let (sender, task) = spawn(near);
        let other = sender.clone();
        let reader = tokio::spawn(tags(far));
        other.send(Queue::Control, frame(b'c', 3)).await.unwrap();
        drop(sender);
        drop(other);
        tokio::time::timeout(WAIT, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reader.await.unwrap(), vec![b'c']);
    }

    #[tokio::test]
    async fn close_writes_what_is_queued_then_refuses_more() {
        let (near, far) = tokio::io::duplex(1024);
        let (sender, task) = spawn(near);
        let keep = sender.clone();
        for _ in 0..4 {
            sender.send(Queue::Bulk, frame(b'b', 4000)).await.unwrap();
        }
        let reader = tokio::spawn(tags(far));
        sender.close();
        assert_eq!(keep.send(Queue::Control, frame(b'c', 1)).await, Err(Closed));
        tokio::time::timeout(WAIT, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reader.await.unwrap(), vec![b'b'; 4]);
        assert!(keep.is_closed());
    }

    #[tokio::test]
    async fn a_failed_write_stops_the_writer_and_wakes_senders() {
        let (near, far) = tokio::io::duplex(1024);
        let (sender, task) = spawn(near);
        drop(far);
        let mut failed = false;
        for _ in 0..32 {
            let send = sender.send(Queue::Bulk, frame(b'b', 128 * 1024));
            if tokio::time::timeout(WAIT, send).await.unwrap().is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
        assert!(tokio::time::timeout(WAIT, task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(
            sender.try_send(Queue::Control, frame(b'c', 1)),
            Err(TrySendError::Closed)
        );
    }

    #[test]
    fn the_window_doubles_while_little_queues_and_grows_by_a_quarter_nearer_the_slack() {
        let ms = Duration::from_millis;
        let mut window = Window::new();
        assert_eq!(window.size(), Window::MIN);
        // Nothing queues, so it doubles.
        for _ in 0..3 {
            window.observe(ms(1));
        }
        assert_eq!(window.size(), 8 * Window::MIN);
        window.observe(ms(31));
        assert_eq!(window.size(), 4 * Window::MIN);
        // 4 ms of queueing is under half the slack, so it doubles again.
        window.observe(ms(5));
        assert_eq!(window.size(), 8 * Window::MIN);
        window.observe(ms(31));
        assert_eq!(window.size(), 4 * Window::MIN);
        // 12 ms is past half, so it grows by a quarter, twice.
        window.observe(ms(13));
        assert_eq!(window.size(), 5 * Window::MIN);
        window.observe(ms(13));
        assert_eq!(window.size(), 5 * Window::MIN + 5 * Window::MIN / 4);
        assert_eq!(window.idle(), Some(ms(1)));
    }

    #[test]
    fn the_window_halves_past_the_slack_and_stops_at_its_least() {
        let ms = Duration::from_millis;
        let mut window = Window::new();
        for _ in 0..4 {
            window.observe(ms(2));
        }
        assert_eq!(window.size(), Window::MAX);
        window.observe(ms(23));
        assert_eq!(window.size(), Window::MAX / 2);
        // Exactly the slack leaves no room to grow, but doesn't shrink it.
        window.observe(ms(22));
        assert_eq!(window.size(), Window::MAX / 2);
        for _ in 0..10 {
            window.observe(ms(300));
        }
        assert_eq!(window.size(), Window::MIN);
        window.observe(ms(2));
        assert_eq!(window.size(), Window::MIN * 2);
    }

    #[test]
    fn growth_stops_where_the_bytes_it_adds_would_queue_past_the_slack() {
        let ms = Duration::from_millis;
        let mut window = Window::new();
        window.observe(ms(20));
        assert_eq!(window.size(), 2 * Window::MIN);
        // 5 ms of queueing behind 512 KiB at 25 ms leaves room for 60% of
        // it, about 307 KiB. The doubling no round trip has measured yet
        // already took more than that, so the window holds.
        window.observe(ms(25));
        assert_eq!(window.size(), 2 * Window::MIN);
        // Now 1 MiB is measured, with room for 60% more.
        window.observe(ms(25));
        assert_eq!(window.size(), 1_048_576 + 629_145);
        // 16 ms leaves room for a ninth, which the last growth took.
        window.observe(ms(36));
        assert_eq!(window.size(), 1_677_721);
        window.observe(ms(36));
        assert_eq!(window.size(), 1_677_721 + 1_677_721 / 9);
    }

    /// The A-14 runs on devbox: bytes a second through the link and its
    /// idle round trip in microseconds.
    const DEVBOX: [(u64, u64); 3] = [
        (18_600_000, 5_600),
        (19_000_000, 5_700),
        (24_100_000, 6_000),
    ];

    /// When a simulated upload has settled, past the window's first growth.
    const SETTLED: Duration = Duration::from_secs(1);

    #[test]
    fn on_a_link_like_devbox_the_window_keeps_queueing_short() {
        for (rate, base) in DEVBOX {
            let link = Link {
                rate,
                base: Duration::from_micros(base),
                jitter: Duration::ZERO,
            };
            let run = upload(&link, 200_000_000);
            assert!(run.rate >= rate * 99 / 100, "{rate}: moved {}", run.rate);
            let settled: Vec<&Round> = run.rounds_after(SETTLED).collect();
            assert!(settled.len() > 100, "{rate}: {} round trips", settled.len());
            for round in settled {
                assert!(
                    round.queued <= Duration::from_millis(25),
                    "{rate}: {:?} queued at {:?}",
                    round.queued,
                    round.at
                );
                assert!(
                    round.size <= 2 * Window::MIN,
                    "{rate}: {} at {:?}",
                    round.size,
                    round.at
                );
            }
            let (median, p95) = (run.wait(SETTLED, 50), run.wait(SETTLED, 95));
            assert!(median <= Duration::from_millis(30), "{rate}: {median:?}");
            assert!(p95 <= Duration::from_millis(40), "{rate}: {p95:?}");
        }
    }

    #[test]
    fn jitter_on_a_link_like_devbox_does_not_swing_the_window_to_its_most() {
        for (rate, base) in DEVBOX {
            let link = Link {
                rate,
                base: Duration::from_micros(base),
                jitter: Duration::from_millis(4),
            };
            let run = upload(&link, 200_000_000);
            assert!(run.rate >= rate * 99 / 100, "{rate}: moved {}", run.rate);
            let most = run.rounds_after(SETTLED).map(|round| round.size).max();
            assert!(most <= Some(Window::MAX / 4), "{rate}: {most:?}");
            let p95 = run.wait(SETTLED, 95);
            assert!(p95 <= Duration::from_millis(40), "{rate}: {p95:?}");
        }
    }

    #[test]
    fn on_a_quick_idle_link_the_window_still_grows_to_its_most() {
        let link = Link {
            rate: 4_000_000_000,
            base: Duration::from_millis(3),
            jitter: Duration::ZERO,
        };
        let run = upload(&link, 200_000_000);
        assert!(run
            .rounds
            .iter()
            .all(|round| round.queued < Duration::from_millis(1)));
        let sizes: Vec<u64> = run.rounds.iter().map(|round| round.size).collect();
        assert_eq!(sizes[..4], [2, 4, 8, 16].map(|times| times * Window::MIN));
        assert!(sizes[3..].iter().all(|&size| size == Window::MAX));
        // The window sets the pace here, and moves 80% of its most each
        // idle round trip.
        let most = Window::MAX * 1000 / 3;
        assert!(run.rate >= most * 4 / 5, "moved {}", run.rate);
    }

    /// A link where everything between the two ends drains through one
    /// queue at a set rate, as the SSH pipe, SSH's channel and the TCP
    /// buffers on the way to devbox do. Every granted byte waits there
    /// ahead of a ping. On the real link the writer still holds some of
    /// them and lets a ping pass, so a ping there waits less.
    struct Link {
        /// Bytes a second through the slowest hop.
        rate: u64,
        /// The round trip with nothing queued, half each way.
        base: Duration,
        /// The most a chunk lands late by.
        jitter: Duration,
    }

    impl Link {
        /// How long `len` bytes take through the slowest hop.
        fn pass(&self, len: u64) -> Duration {
            Duration::from_nanos(len * 1_000_000_000 / self.rate)
        }
    }

    /// Delays up to a most, spread evenly, the same on every run.
    struct Jitter {
        state: u64,
        most: u64,
    }

    impl Jitter {
        fn next(&mut self) -> Duration {
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            Duration::from_nanos((self.most * (self.state >> 32)) >> 32)
        }
    }

    /// One round trip the receiver measured.
    struct Round {
        /// When it ended.
        at: Duration,
        /// How long it took past the idle round trip.
        queued: Duration,
        /// The window after it.
        size: u64,
    }

    /// What one simulated upload saw.
    struct Run {
        rounds: Vec<Round>,
        /// How long a ping sent every 10 ms would wait behind file data,
        /// with when it went.
        waits: Vec<(Duration, Duration)>,
        /// Bytes a second the upload moved.
        rate: u64,
    }

    impl Run {
        fn rounds_after(&self, from: Duration) -> impl Iterator<Item = &Round> {
            self.rounds.iter().filter(move |round| round.at >= from)
        }

        /// The wait of pings sent from `from` on that `percent` of them
        /// stayed within.
        fn wait(&self, from: Duration, percent: usize) -> Duration {
            let mut waits: Vec<Duration> = self
                .waits
                .iter()
                .filter(|(at, _)| *at >= from)
                .map(|&(_, wait)| wait)
                .collect();
            waits.sort();
            waits[(waits.len() - 1) * percent / 100]
        }
    }

    /// The receiving end of a simulated upload. It grants and measures as
    /// `maxhq_transfer`'s `Receiver` and `Grant` do.
    struct Receiver {
        window: Window,
        total: u64,
        written: u64,
        end: u64,
        sent: Option<u64>,
        probe: Option<(u64, Duration)>,
    }

    impl Receiver {
        /// Credit worth sending: the end moved by this much or reached the
        /// total.
        const STEP: u64 = 64 * 1024;

        fn new(total: u64) -> Self {
            Self {
                window: Window::new(),
                total,
                written: 0,
                end: 0,
                sent: None,
                probe: None,
            }
        }

        /// Extends the grant, and gives the end to send if it's worth it.
        fn credit(&mut self, now: Duration) -> Option<u64> {
            let end = (self.written + self.window.size()).min(self.total);
            if end > self.end {
                if self.probe.is_none() {
                    self.probe = Some((self.end, now));
                }
                self.end = end;
            }
            let due = self.sent.is_none_or(|sent| {
                self.end - sent >= Self::STEP || (self.end == self.total && sent < self.total)
            });
            if !due {
                return None;
            }
            self.sent = Some(self.end);
            Some(self.end)
        }

        /// Notes that bytes up to `end` arrived, and gives the round trip
        /// that measured, if one did.
        fn arrived(&mut self, end: u64, now: Duration) -> Option<Round> {
            self.written = end;
            let (_, sent) = self.probe.filter(|&(mark, _)| end > mark)?;
            self.probe = None;
            let round_trip = now - sent;
            self.window.observe(round_trip);
            Some(Round {
                at: now,
                queued: round_trip.saturating_sub(self.window.idle()?),
                size: self.window.size(),
            })
        }
    }

    /// Uploads `total` bytes over `link`. The sender sends all the credit
    /// allows at once, in chunks of 256 KiB, as `maxhq_transfer` does.
    fn upload(link: &Link, total: u64) -> Run {
        const CHUNK: u64 = 256 * 1024;
        const PING_EVERY: Duration = Duration::from_millis(10);
        let one_way = link.base / 2;
        let mut jitter = Jitter {
            state: 1,
            most: u64::try_from(link.jitter.as_nanos()).unwrap(),
        };
        let mut receiver = Receiver::new(total);
        let mut run = Run {
            rounds: Vec::new(),
            waits: Vec::new(),
            rate: 0,
        };
        // Grants and chunks on the way, each with when it lands.
        let mut grants = VecDeque::new();
        let mut chunks = VecDeque::new();
        // The end the sender knows of and the bytes it has sent.
        let (mut allowed, mut sent) = (0u64, 0u64);
        // When the slowest hop next stands empty, when the last chunk
        // lands and when the next ping goes.
        let (mut busy, mut landed, mut ping) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
        let mut now = Duration::ZERO;
        grants.extend(receiver.credit(now).map(|end| (now + one_way, end)));
        while receiver.written < total {
            let chunk = chunks.front().map(|&(at, _)| at);
            let grant = grants.front().map(|&(at, _)| at);
            now = [chunk, grant]
                .into_iter()
                .flatten()
                .fold(ping, Duration::min);
            if chunk == Some(now) {
                let (_, end) = chunks.pop_front().unwrap();
                run.rounds.extend(receiver.arrived(end, now));
                grants.extend(receiver.credit(now).map(|end| (now + one_way, end)));
            } else if grant == Some(now) {
                let (_, end) = grants.pop_front().unwrap();
                allowed = allowed.max(end);
                while sent < allowed {
                    let len = (allowed - sent).min(CHUNK);
                    sent += len;
                    busy = busy.max(now) + link.pass(len);
                    // The bytes keep their order, so a late chunk holds
                    // up the ones behind it.
                    landed = landed.max(busy + one_way + jitter.next());
                    chunks.push_back((landed, sent));
                }
            } else {
                run.waits.push((now, busy.saturating_sub(now)));
                ping += PING_EVERY;
            }
        }
        run.rate = u64::try_from(u128::from(total) * 1_000_000_000 / now.as_nanos()).unwrap();
        run
    }
}

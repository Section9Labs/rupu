//! `TranscriptTail` — polling consumer of a transcript JSONL file, yielding
//! parsed [`rupu_transcript::Event`] values as a [`Stream`].
//!
//! This mirrors [`rupu_orchestrator::executor::FileTailRunSource`] but parses
//! the transcript Event type (`rupu_transcript::Event`) rather than the
//! orchestrator's step-level event. The transcript file and the orchestrator's
//! `events.jsonl` are different JSONL schemas, so they need separate tailers.
//!
//! Tailing is poll-based and single-owner: one spawned task owns a
//! [`rupu_transcript::JsonlCursor`] and, every 250 ms, reads only the bytes
//! appended since the previous tick (the first tick drains the whole backlog
//! from offset 0, so there is no second drainer to race). The cursor holds
//! back a partially written trailing line until its newline lands, decodes
//! UTF-8 per whole line (a write boundary inside a multi-byte character can't
//! drop a chunk), and restarts from 0 if the file shrinks or is replaced. A
//! file that does not exist yet simply yields nothing until it appears; the
//! task exits once the consumer drops the stream. (We deliberately do NOT use
//! a `notify` filesystem watcher — the macOS kqueue backend it's pinned to
//! panics a background thread on teardown when the stream is dropped, and the
//! 250 ms poll already covers append-tailing reliably.)

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use rupu_transcript::{Event, JsonlCursor};
use tokio::sync::mpsc;

/// Live tail of a transcript JSONL file. Each newly-appended, parseable line
/// becomes one [`Event`] on the stream. Lines that fail to parse are skipped
/// (they don't terminate the stream).
pub struct TranscriptTail {
    rx: mpsc::Receiver<Event>,
}

impl TranscriptTail {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(256);
        let path_buf: PathBuf = path.to_path_buf();

        tokio::spawn(async move {
            let mut cursor = JsonlCursor::new();
            loop {
                // Nobody is listening any more (including the case where the
                // file never appeared): stop polling instead of spinning for
                // the rest of the process.
                if tx.is_closed() {
                    return;
                }
                // Blocking read of only the appended bytes; bounded work per
                // tick. A drain I/O error is treated like "no data this
                // tick" — the next tick retries from the same offset.
                let p = path_buf.clone();
                let mut c = std::mem::take(&mut cursor);
                match tokio::task::spawn_blocking(move || {
                    let mut lines = Vec::new();
                    let _ = c.drain_with(&p, || {}, |l| lines.push(l.to_string()));
                    (c, lines)
                })
                .await
                {
                    Ok((c, lines)) => {
                        cursor = c;
                        for line in lines {
                            if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                                if tx.send(ev).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Only reachable if the blocking closure panicked; the
                        // cursor was lost with it, so restart from offset 0.
                        tracing::warn!(error = %e, "transcript tail drain failed; restarting cursor");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });

        Ok(Self { rx })
    }
}

impl Stream for TranscriptTail {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}

/// One line of a [`NumberedTail`]: its 1-based position among the file's
/// non-blank lines (`id`), the line parsed as `E` (`None` when it does not
/// parse), and whether it lies past the resume point (`send`) — a line the
/// client already has is still reported, unsent, so a caller can see what
/// it was (a terminal event, say).
#[derive(Debug, Clone, PartialEq)]
pub struct NumberedLine<E> {
    pub id: u64,
    pub event: Option<E>,
    pub send: bool,
}

/// Live tail of a JSONL file, numbering every non-blank line — parseable or
/// not, recognised or not — so ids are positions in the file and stay stable
/// across reconnects and rupu versions. Lines up to `after` (a client's
/// `Last-Event-ID`) come back with `send: false`.
///
/// When the file shrinks or is replaced (a mirror cache truncated for a
/// byte-zero replay, a terminal pull renaming a fresh copy over it), the
/// numbering restarts at 1 — and nothing at or before the highest id already
/// sent on this stream is sent again: the rewrite carries the same history.
pub struct NumberedTail<E> {
    rx: mpsc::Receiver<NumberedLine<E>>,
}

impl<E: serde::de::DeserializeOwned + Send + 'static> NumberedTail<E> {
    pub fn open(path: &Path, after: u64) -> Self {
        let (tx, rx) = mpsc::channel::<NumberedLine<E>>(256);
        let path_buf: PathBuf = path.to_path_buf();
        tokio::spawn(async move {
            let mut cursor = JsonlCursor::new();
            let (mut seq, mut floor, mut sent_max) = (0u64, after, 0u64);
            loop {
                if tx.is_closed() {
                    return;
                }
                let p = path_buf.clone();
                let mut c = std::mem::take(&mut cursor);
                let drained = tokio::task::spawn_blocking(move || {
                    let mut lines = Vec::new();
                    let mut reset = false;
                    let _ = c.drain_with(&p, || reset = true, |l| lines.push(l.to_string()));
                    (c, reset, lines)
                })
                .await;
                match drained {
                    Ok((c, reset, lines)) => {
                        cursor = c;
                        if reset {
                            seq = 0;
                            floor = floor.max(sent_max);
                        }
                        for line in lines {
                            seq += 1;
                            let send = seq > floor;
                            if send {
                                sent_max = seq;
                            }
                            let item = NumberedLine {
                                id: seq,
                                event: serde_json::from_str::<E>(&line).ok(),
                                send,
                            };
                            if tx.send(item).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        // The blocking closure panicked and the cursor went
                        // with it: the next drain restarts from offset 0,
                        // which is a reset.
                        tracing::warn!(error = %e, "jsonl tail drain failed; restarting cursor");
                        seq = 0;
                        floor = floor.max(sent_max);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
        Self { rx }
    }
}

impl<E> Stream for NumberedTail<E> {
    type Item = NumberedLine<E>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

/// The lines of the JSONL file at `path` past line `after`, numbered exactly
/// as [`NumberedTail`] numbers them (every non-blank line from 1), at most
/// `max` of them, all `send: true`. Lines up to `after` are counted, never
/// parsed. Also returns the id to resume after: the last line returned when
/// `max` cut the read short, else the file's last line (`after` when the
/// file has no line past it). A missing file has no lines. Blocking.
pub fn read_numbered<E: serde::de::DeserializeOwned>(
    path: &Path,
    after: u64,
    max: usize,
) -> std::io::Result<(Vec<NumberedLine<E>>, u64)> {
    let (mut out, mut seq, mut capped) = (Vec::new(), 0u64, false);
    JsonlCursor::new().drain_with(
        path,
        || {},
        |l| {
            seq += 1;
            if seq <= after || capped {
                return;
            }
            if out.len() >= max {
                capped = true;
                return;
            }
            out.push(NumberedLine {
                id: seq,
                event: serde_json::from_str::<E>(l).ok(),
                send: true,
            });
        },
    )?;
    let last = match (capped, out.last()) {
        (true, Some(l)) => l.id,
        (true, None) => after,
        (false, _) => seq.max(after),
    };
    Ok((out, last))
}

/// The id of the last line of the JSONL file at `path`, numbered as
/// [`NumberedTail`] and [`read_numbered`] number it (0 for a missing or
/// empty file): where a tail that wants only new lines starts. Blocking.
pub fn count_numbered(path: &Path) -> std::io::Result<u64> {
    let mut seq = 0u64;
    JsonlCursor::new().drain_with(path, || {}, |_| seq += 1)?;
    Ok(seq)
}

/// One item of a resumable transcript stream ([`sequenced`]).
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptFrame {
    /// A transcript event and its line number in the file (1-based).
    Event { id: u64, event: Box<Event> },
    /// The transcript reached its `run_complete`; the stream ends after this.
    End,
}

/// A transcript's events, numbered by line ([`NumberedTail`]) and resumed
/// after line `after`. After a `run_complete` one [`TranscriptFrame::End`]
/// follows and the stream ends — each run writes its own transcript and
/// nothing after `run_complete`. A resume point at or past it gets the end
/// at once.
pub fn sequenced(path: &Path, after: u64) -> impl Stream<Item = TranscriptFrame> {
    use futures_util::StreamExt as _;
    #[derive(Clone, Copy, PartialEq)]
    enum Phase {
        Tailing,
        EndNext,
        Done,
    }
    let lines = Box::pin(NumberedTail::<Event>::open(path, after));
    futures_util::stream::unfold((lines, Phase::Tailing), |(mut lines, phase)| async move {
        match phase {
            Phase::Done => return None,
            Phase::EndNext => return Some((TranscriptFrame::End, (lines, Phase::Done))),
            Phase::Tailing => {}
        }
        loop {
            let line = lines.next().await?;
            let Some(ev) = line.event else { continue };
            let last = matches!(ev, Event::RunComplete { .. });
            if !line.send {
                if last {
                    return Some((TranscriptFrame::End, (lines, Phase::Done)));
                }
                continue;
            }
            let next = if last { Phase::EndNext } else { Phase::Tailing };
            let frame = TranscriptFrame::Event {
                id: line.id,
                event: Box::new(ev),
            };
            return Some((frame, (lines, next)));
        }
    })
}

#[cfg(test)]
mod numbered_tests {
    use super::*;
    use futures_util::StreamExt as _;

    /// `read_numbered` numbers like the tail (blank lines don't count,
    /// unparseable ones do), skips up to `after`, and stops at `max` with
    /// the last returned id to resume from.
    #[tokio::test]
    async fn read_numbered_numbers_like_the_tail_and_resumes_past_a_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, "1\n\n  \nnot json\n3\n4\npartial").unwrap();
        let ids =
            |v: &Vec<NumberedLine<u32>>| v.iter().map(|l| (l.id, l.event)).collect::<Vec<_>>();
        let (all, last) = read_numbered::<u32>(&p, 0, 1000).unwrap();
        assert_eq!(
            ids(&all),
            vec![(1, Some(1)), (2, None), (3, Some(3)), (4, Some(4))]
        );
        assert_eq!(last, 4);
        let mut tail = NumberedTail::<u32>::open(&p, 0);
        for want in &all {
            assert_eq!(next(&mut tail).await, *want);
        }
        let (some, last) = read_numbered::<u32>(&p, 1, 2).unwrap();
        assert_eq!(ids(&some), vec![(2, None), (3, Some(3))]);
        assert_eq!(last, 3);
        let (rest, last) = read_numbered::<u32>(&p, last, 2).unwrap();
        assert_eq!(ids(&rest), vec![(4, Some(4))]);
        assert_eq!(last, 4);
        // The count ends where the numbering does (the partial line is not
        // a line yet); a missing file has none.
        assert_eq!(count_numbered(&p).unwrap(), 4);
        assert_eq!(count_numbered(&dir.path().join("nope.jsonl")).unwrap(), 0);
        let (none, last) = read_numbered::<u32>(&p, 9, 2).unwrap();
        assert!(none.is_empty());
        assert_eq!(last, 9);
        let (none, last) = read_numbered::<u32>(&dir.path().join("missing"), 0, 2).unwrap();
        assert!(none.is_empty());
        assert_eq!(last, 0);
    }

    async fn next<E>(t: &mut NumberedTail<E>) -> NumberedLine<E> {
        tokio::time::timeout(std::time::Duration::from_secs(3), t.next())
            .await
            .expect("a line")
            .expect("open")
    }

    /// Every non-blank line is numbered (unparseable ones too, unsent past
    /// the resume point only), and a rewritten file never resends what this
    /// stream already sent.
    #[tokio::test]
    async fn numbers_every_line_and_survives_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, "1\nnot json\n3\n").unwrap();
        let mut t = NumberedTail::<u32>::open(&p, 1);
        let a = next(&mut t).await;
        assert_eq!((a.id, a.event, a.send), (1, Some(1), false));
        let b = next(&mut t).await;
        assert_eq!((b.id, b.event, b.send), (2, None, true));
        let c = next(&mut t).await;
        assert_eq!((c.id, c.event, c.send), (3, Some(3), true));

        // Replaced by a shorter file with the same history plus one line: the
        // numbering restarts, nothing already sent is sent again.
        std::fs::write(&p, "1\nx\n").unwrap();
        let d = next(&mut t).await;
        assert_eq!((d.id, d.send), (1, false));
        let e = next(&mut t).await;
        assert_eq!((e.id, e.send), (2, false));
        std::fs::write(&p, "1\nx\n3\n4\n").unwrap();
        let mut sent = Vec::new();
        while sent.is_empty() {
            let l = next(&mut t).await;
            if l.send {
                sent.push((l.id, l.event));
            }
        }
        assert_eq!(sent, vec![(4, Some(4))]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use futures_util::StreamExt as _;
    use rupu_transcript::RunMode;
    use std::time::Duration;

    /// Build N transcript `AssistantMessage` events as JSONL bytes.
    fn make_jsonl_events(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..n {
            let ev = Event::AssistantMessage {
                content: format!("msg {i}"),
                thinking: None,
            };
            let mut line = serde_json::to_vec(&ev).unwrap();
            line.push(b'\n');
            out.extend_from_slice(&line);
        }
        out
    }

    /// Writes `n` events to a file BEFORE `TranscriptTail::open` so the first
    /// drain sees a multi-line backlog. Asserts EXACTLY `n` events arrive (no
    /// duplicates) within a generous timeout.
    #[tokio::test]
    async fn no_duplicate_events_on_backlog_open() {
        const N: usize = 3;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");

        // Write the full backlog BEFORE opening the tail.
        std::fs::write(&path, make_jsonl_events(N)).unwrap();

        let mut tail = TranscriptTail::open(&path).await.unwrap();

        // Collect events until we have N or until 2 s elapse.
        let collected: Vec<Event> = tokio::time::timeout(Duration::from_secs(2), async {
            let mut v = Vec::new();
            while v.len() < N {
                match tail.next().await {
                    Some(ev) => v.push(ev),
                    None => break,
                }
            }
            v
        })
        .await
        .unwrap_or_default();

        // Exactly N events — no duplicates.
        assert_eq!(
            collected.len(),
            N,
            "expected {N} events, got {}: {collected:?}",
            collected.len()
        );
        // Each message is distinct (content matches the emitted index).
        for (i, ev) in collected.iter().enumerate() {
            match ev {
                Event::AssistantMessage { content, .. } => {
                    assert_eq!(content, &format!("msg {i}"));
                }
                other => panic!("unexpected event at index {i}: {other:?}"),
            }
        }
    }

    /// Smoke test: open the tail with no pre-existing file, append one event
    /// after opening, and confirm it arrives on the stream.
    #[tokio::test]
    async fn stream_receives_appended_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");

        // File does not exist yet.
        let mut tail = TranscriptTail::open(&path).await.unwrap();

        // Write one event after the tail is open.
        let ev = Event::RunStart {
            run_id: "r1".into(),
            workspace_id: "ws1".into(),
            agent: "agent".into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 6, 16, 12, 0, 0).unwrap(),
            mode: RunMode::Ask,
            schema: None,
            system_prompt: None,
            codename: None,
            customer: None,
            launch_skills: Vec::new(),
            launch_preload_skills: Vec::new(),
            launch_args: Vec::new(),
        };
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        std::fs::write(&path, &line).unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), tail.next())
            .await
            .expect("timed out waiting for event")
            .expect("stream closed");

        assert_eq!(got, ev);
    }
    fn usage_line(model: &str) -> String {
        serde_json::to_string(&Event::Usage {
            provider: "anthropic".into(),
            model: model.into(),
            served_model: None,
            input_tokens: 1,
            output_tokens: 1,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: None,
        })
        .unwrap()
            + "\n"
    }

    #[tokio::test]
    async fn partial_line_is_delivered_once_completed_never_dropped() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let full = usage_line("m-partial");
        let (head, tail_part) = full.split_at(full.len() / 2);
        std::fs::write(&p, head).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(tail_part.as_bytes()).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(3), tail.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Event::Usage { ref model, .. } if model == "m-partial"));
    }

    #[tokio::test]
    async fn large_backlog_is_emitted_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let body: String = (0..5000).map(|i| usage_line(&format!("m{i}"))).collect();
        std::fs::write(&p, body).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        let mut n = 0;
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(800), tail.next()).await
        {
            n += 1;
        }
        assert_eq!(n, 5000);
    }

    #[tokio::test]
    async fn utf8_split_does_not_drop_chunk() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let line = serde_json::to_string(&Event::AssistantMessage {
            content: "build \u{2713} passed".into(),
            thinking: None,
        })
        .unwrap()
            + "\n";
        // serde_json emits the check mark as raw UTF-8 (E2 9C 93); cut inside it.
        let bytes = line.as_bytes();
        let mark = bytes
            .windows(3)
            .position(|w| w == [0xE2, 0x9C, 0x93])
            .unwrap();
        let (head, rest) = bytes.split_at(mark + 1);
        std::fs::write(&p, head).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(rest).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(3), tail.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, Event::AssistantMessage { ref content, .. } if content.contains('\u{2713}'))
        );
    }
}

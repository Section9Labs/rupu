//! Bounded reverse physical JSONL traversal. The caller owns authorization,
//! regular-file validation and before/after snapshot checks. No payload bytes
//! are retained in a cursor; oversized lines advance in discard mode.
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const MAX_RECORD_BYTES: usize = 256 * 1024;
pub(crate) const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_LINES: u64 = 20_000;
const CHUNK_BYTES: usize = 4096;
const RECORD_RESERVE: u64 = (MAX_RECORD_BYTES + CHUNK_BYTES) as u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReverseState {
    /// Exclusive end of the unread file prefix. Decreases monotonically.
    pub(crate) offset: u64,
    boundary: Boundary,
    pending_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Boundary {
    Tail,
    Record,
    DiscardOversized,
    DiscardTail,
    End,
}

impl ReverseState {
    pub(crate) fn new(file_len: u64) -> Self {
        Self {
            offset: file_len,
            boundary: Boundary::Tail,
            pending_bytes: 0,
        }
    }

    pub(crate) fn pending_oversized(&self) -> bool {
        self.boundary == Boundary::DiscardOversized
    }

    pub(crate) fn finished(&self) -> bool {
        self.boundary == Boundary::End
    }
}

/// Counts actual reads, including bytes preceding a discovered delimiter that
/// a later call may read again. Rollback of a result does not refund work.
#[derive(Debug)]
pub(crate) struct ScanBudget {
    pub(crate) bytes_read: u64,
    pub(crate) physical_lines: u64,
    byte_limit: u64,
    line_limit: u64,
}

impl Default for ScanBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanBudget {
    pub(crate) fn new() -> Self {
        Self {
            bytes_read: 0,
            physical_lines: 0,
            byte_limit: MAX_SCAN_BYTES,
            line_limit: MAX_LINES,
        }
    }

    fn remaining(&self) -> u64 {
        self.byte_limit.saturating_sub(self.bytes_read)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScanItem {
    /// UTF-8/JSON parsing belongs to the caller. CRLF retains its final CR.
    Record {
        byte_start: u64,
        bytes: Vec<u8>,
    },
    Blank {
        byte_start: u64,
    },
    Oversized {
        byte_start: u64,
        bytes: u64,
    },
    /// A final fragment without a newline is not a committed JSONL record.
    UnterminatedTail {
        byte_start: u64,
        bytes: u64,
    },
    BudgetStop,
    End,
}

fn check_cancelled(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "event scan cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Unbuffered bounded reads: the budget includes every successful read, even
/// when the file subsequently changes or a read fails. Never read ahead.
fn read_at(
    file: &mut File,
    offset: u64,
    bytes: &mut [u8],
    budget: &mut ScanBudget,
    cancelled: &AtomicBool,
) -> io::Result<()> {
    if bytes.len() as u64 > budget.remaining() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "event scan read exceeds budget",
        ));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut done = 0;
    while done < bytes.len() {
        check_cancelled(cancelled)?;
        match file.read(&mut bytes[done..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "event file changed during scan",
                ))
            }
            Ok(count) => {
                done += count;
                budget.bytes_read += count as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => check_cancelled(cancelled)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Return one physical record or skip. A caller may clone and restore `state`
/// to defer an unreturned match. Keep the budget charged. On errors discard the
/// page; ordinary record assembly is intentionally not serialized.
pub(crate) fn next(
    file: &mut File,
    state: &mut ReverseState,
    budget: &mut ScanBudget,
    cancelled: &AtomicBool,
) -> io::Result<ScanItem> {
    check_cancelled(cancelled)?;
    if state.finished() {
        return Ok(ScanItem::End);
    }
    if budget.physical_lines >= budget.line_limit {
        return Ok(ScanItem::BudgetStop);
    }

    if state.boundary == Boundary::Tail {
        if state.offset == 0 {
            state.boundary = Boundary::End;
            return Ok(ScanItem::End);
        }
        if budget.remaining() == 0 {
            return Ok(ScanItem::BudgetStop);
        }
        let mut last = [0];
        read_at(file, state.offset - 1, &mut last, budget, cancelled)?;
        state.offset -= 1;
        if last[0] == b'\n' {
            state.boundary = Boundary::Record;
        } else {
            state.boundary = Boundary::DiscardTail;
            state.pending_bytes = 1;
        }
    }

    // An ordinary line <= MAX_RECORD_BYTES can finish under this reserve,
    // including its delimiter chunk. Larger lines become persistent discard
    // state before the budget can expire. No raw bytes enter the cursor.
    if state.boundary == Boundary::Record && state.offset > 0 && budget.remaining() < RECORD_RESERVE
    {
        return Ok(ScanItem::BudgetStop);
    }

    let mut reversed = Vec::new();
    let mut chunk = [0u8; CHUNK_BYTES];
    loop {
        check_cancelled(cancelled)?;
        if state.offset == 0 {
            return Ok(finish(state, budget, reversed, 0, true));
        }
        if budget.remaining() == 0 {
            // RECORD_RESERVE guarantees we only reach this with discard state.
            debug_assert!(state.boundary != Boundary::Record);
            return Ok(ScanItem::BudgetStop);
        }
        let count = state.offset.min(CHUNK_BYTES as u64).min(budget.remaining()) as usize;
        let start = state.offset - count as u64;
        read_at(file, start, &mut chunk[..count], budget, cancelled)?;
        let delimiter = chunk[..count].iter().rposition(|byte| *byte == b'\n');
        let payload_start = delimiter.map_or(0, |index| index + 1);
        let payload = &chunk[payload_start..count];
        state.pending_bytes += payload.len() as u64;
        if state.boundary == Boundary::Record {
            if state.pending_bytes > MAX_RECORD_BYTES as u64 {
                state.boundary = Boundary::DiscardOversized;
                reversed.clear();
            } else {
                reversed.extend(payload.iter().rev());
            }
        }
        if let Some(index) = delimiter {
            // The delimiter also terminates the next older record; exclude it
            // from the next scan, exactly as the initial final newline.
            state.offset = start + index as u64;
            let byte_start = state.offset + 1;
            return Ok(finish(state, budget, reversed, byte_start, false));
        }
        state.offset = start;
    }
}

fn finish(
    state: &mut ReverseState,
    budget: &mut ScanBudget,
    mut reversed: Vec<u8>,
    byte_start: u64,
    at_start: bool,
) -> ScanItem {
    let kind = state.boundary;
    let bytes = state.pending_bytes;
    state.pending_bytes = 0;
    state.boundary = if at_start {
        Boundary::End
    } else {
        Boundary::Record
    };
    budget.physical_lines += 1;
    match kind {
        Boundary::DiscardTail => ScanItem::UnterminatedTail { byte_start, bytes },
        Boundary::DiscardOversized => ScanItem::Oversized { byte_start, bytes },
        Boundary::Record => {
            reversed.reverse();
            if reversed.iter().all(u8::is_ascii_whitespace) {
                ScanItem::Blank { byte_start }
            } else {
                ScanItem::Record {
                    byte_start,
                    bytes: reversed,
                }
            }
        }
        Boundary::Tail | Boundary::End => unreachable!("finish needs a record boundary"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(bytes: &[u8]) -> (tempfile::NamedTempFile, ReverseState) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        (file, ReverseState::new(bytes.len() as u64))
    }

    fn collect(bytes: &[u8]) -> Vec<ScanItem> {
        let (mut file, mut state) = fixture(bytes);
        let mut budget = ScanBudget::new();
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        loop {
            let item = next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap();
            match item {
                ScanItem::End => break,
                ScanItem::BudgetStop => panic!("small fixture must fit"),
                item => out.push(item),
            }
        }
        assert_eq!(budget.physical_lines, out.len() as u64);
        out
    }

    #[test]
    fn reverse_offsets_crlf_blank_and_unterminated_tail() {
        assert_eq!(
            collect(b"a\r\n\n b\npartial"),
            vec![
                ScanItem::UnterminatedTail {
                    byte_start: 7,
                    bytes: 7
                },
                ScanItem::Record {
                    byte_start: 4,
                    bytes: b" b".to_vec()
                },
                ScanItem::Blank { byte_start: 3 },
                ScanItem::Record {
                    byte_start: 0,
                    bytes: b"a\r".to_vec()
                },
            ]
        );
        assert!(collect(b"").is_empty());
        assert_eq!(collect(b"\n"), vec![ScanItem::Blank { byte_start: 0 }]);
        assert_eq!(
            collect(b"x"),
            vec![ScanItem::UnterminatedTail {
                byte_start: 0,
                bytes: 1
            }]
        );
        assert_eq!(
            collect(b"\n\n"),
            vec![
                ScanItem::Blank { byte_start: 1 },
                ScanItem::Blank { byte_start: 0 }
            ]
        );
    }

    #[test]
    fn maximum_record_is_preserved_and_one_extra_byte_is_skipped() {
        for size in [MAX_RECORD_BYTES, MAX_RECORD_BYTES + 1] {
            let mut bytes = vec![b'x'; size];
            bytes.push(b'\n');
            let result = collect(&bytes);
            if size == MAX_RECORD_BYTES {
                assert_eq!(
                    result,
                    vec![ScanItem::Record {
                        byte_start: 0,
                        bytes: vec![b'x'; size]
                    }]
                );
            } else {
                assert_eq!(
                    result,
                    vec![ScanItem::Oversized {
                        byte_start: 0,
                        bytes: size as u64
                    }]
                );
            }
        }
    }

    #[test]
    fn giant_record_advances_across_serialized_pages_without_payload() {
        let size = MAX_SCAN_BYTES as usize + MAX_RECORD_BYTES;
        let mut bytes = b"older\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', size));
        bytes.extend_from_slice(b"\nnewer\n");
        let (mut file, mut state) = fixture(&bytes);
        let cancel = AtomicBool::new(false);
        let mut first = ScanBudget::new();
        assert!(matches!(
            next(file.as_file_mut(), &mut state, &mut first, &cancel).unwrap(),
            ScanItem::Record { .. }
        ));
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut first, &cancel).unwrap(),
            ScanItem::BudgetStop
        );
        assert_eq!(first.bytes_read, MAX_SCAN_BYTES);
        assert!(state.pending_oversized());
        assert!(state.offset < size as u64);
        let token = serde_json::to_vec(&state).unwrap();
        assert!(token.len() < 200, "no raw line in continuation");
        state = serde_json::from_slice(&token).unwrap();
        let mut second = ScanBudget::new();
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut second, &cancel).unwrap(),
            ScanItem::Oversized {
                byte_start: 6,
                bytes: size as u64
            }
        );
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut second, &cancel).unwrap(),
            ScanItem::Record {
                byte_start: 0,
                bytes: b"older".to_vec()
            }
        );
        assert!(state.finished());
        assert_eq!(first.physical_lines + second.physical_lines, 3);
    }

    #[test]
    fn giant_unterminated_tail_advances_then_reaches_older_record() {
        let size = MAX_SCAN_BYTES as usize + 17;
        let mut bytes = b"older\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', size));
        let (mut file, mut state) = fixture(&bytes);
        let cancel = AtomicBool::new(false);
        let mut budget = ScanBudget::new();
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::BudgetStop
        );
        assert_eq!(budget.bytes_read, MAX_SCAN_BYTES);
        assert!(!state.pending_oversized());
        let prior_offset = state.offset;
        let mut budget = ScanBudget::new();
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::UnterminatedTail {
                byte_start: 6,
                bytes: size as u64
            }
        );
        assert!(state.offset < prior_offset);
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::Record {
                byte_start: 0,
                bytes: b"older".to_vec()
            }
        );
    }

    #[test]
    fn rollback_replays_a_match_without_refunding_read_work() {
        let (mut file, mut state) = fixture(b"old\nmatch\n");
        let cancel = AtomicBool::new(false);
        let mut budget = ScanBudget::new();
        let before = state.clone();
        let item = next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap();
        let read = budget.bytes_read;
        state = before;
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            item
        );
        assert!(budget.bytes_read > read);
        assert_eq!(budget.physical_lines, 2);
    }

    #[test]
    fn exhausted_line_budget_and_record_reserve_do_not_consume_records() {
        let (mut file, mut state) = fixture(b"old\nnew\n");
        let cancel = AtomicBool::new(false);
        let mut budget = ScanBudget {
            bytes_read: 0,
            physical_lines: 0,
            byte_limit: 20,
            line_limit: 1,
        };
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::BudgetStop
        );
        assert_eq!(state.offset, 7, "only final delimiter consumed");
        assert_eq!(budget.bytes_read, 1);
        budget = ScanBudget {
            bytes_read: 0,
            physical_lines: 0,
            byte_limit: MAX_SCAN_BYTES,
            line_limit: 1,
        };
        assert!(matches!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::Record { .. }
        ));
        let before = state.clone();
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::BudgetStop
        );
        assert_eq!(state, before);
    }

    #[test]
    fn raw_invalid_json_and_utf8_are_returned_for_caller_classification() {
        assert_eq!(
            collect(b"{broken}\n\xff\n"),
            vec![
                ScanItem::Record {
                    byte_start: 9,
                    bytes: vec![0xff]
                },
                ScanItem::Record {
                    byte_start: 0,
                    bytes: b"{broken}".to_vec()
                },
            ]
        );
    }

    #[test]
    fn sparse_consumer_can_continue_after_a_page_with_no_matches() {
        let (mut file, mut state) = fixture(b"match\nnoise\nnoise\n");
        let cancel = AtomicBool::new(false);
        let mut budget = ScanBudget {
            bytes_read: 0,
            physical_lines: 0,
            byte_limit: MAX_SCAN_BYTES,
            line_limit: 2,
        };
        let mut matches = Vec::new();
        loop {
            match next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap() {
                ScanItem::Record { byte_start, bytes } if bytes == b"match" => {
                    matches.push(byte_start)
                }
                ScanItem::BudgetStop => break,
                ScanItem::End => panic!("older record still remains"),
                _ => {}
            }
        }
        assert!(matches.is_empty());
        assert!(!state.finished());
        let offset = state.offset;
        let mut budget = ScanBudget::new();
        assert_eq!(
            next(file.as_file_mut(), &mut state, &mut budget, &cancel).unwrap(),
            ScanItem::Record {
                byte_start: 0,
                bytes: b"match".to_vec()
            }
        );
        assert!(state.offset < offset);
        assert!(state.finished());
    }

    #[test]
    fn cancellation_does_not_read_or_advance() {
        let (mut file, mut state) = fixture(b"record\n");
        let before = state.clone();
        let mut budget = ScanBudget::new();
        let error = next(
            file.as_file_mut(),
            &mut state,
            &mut budget,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(state, before);
        assert_eq!(budget.bytes_read, 0);
    }
}

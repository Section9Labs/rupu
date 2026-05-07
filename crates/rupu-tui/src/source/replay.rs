use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rupu_transcript::Event;

use super::{EventSource, SourceEvent};

/// Pace-controlled iterator over a scripted (step_id, event) sequence.
/// `pace_us` = microseconds between events. `wait()` releases the next
/// event when its scheduled instant has passed.
pub struct ReplaySource {
    queue: VecDeque<(String, Event)>,
    pace: Duration,
    next_at: Instant,
}

impl ReplaySource {
    pub fn new(scripted: Vec<(String, Event)>, pace_us: u64) -> Self {
        Self {
            queue: scripted.into(),
            pace: Duration::from_micros(pace_us),
            next_at: Instant::now() + Duration::from_micros(pace_us),
        }
    }
}

impl EventSource for ReplaySource {
    fn poll(&mut self) -> Vec<SourceEvent> {
        let now = Instant::now();
        let mut out = Vec::new();
        while now >= self.next_at {
            let Some((step_id, event)) = self.queue.pop_front() else {
                break;
            };
            out.push(SourceEvent::StepEvent { step_id, event });
            self.next_at += self.pace;
        }
        out
    }

    fn wait(&mut self, dur: Duration) -> Option<SourceEvent> {
        let now = Instant::now();
        let until = now + dur;
        if self.next_at > until {
            return None;
        }
        let sleep_for = self.next_at.saturating_duration_since(now);
        std::thread::sleep(sleep_for);
        let (step_id, event) = self.queue.pop_front()?;
        self.next_at += self.pace;
        Some(SourceEvent::StepEvent { step_id, event })
    }

    fn is_drained(&self) -> bool {
        self.queue.is_empty()
    }
}

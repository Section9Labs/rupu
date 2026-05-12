//! EventSink trait + FanOutSink for delivering events to multiple
//! consumers (in-memory broadcast + on-disk JSONL).

use std::sync::Arc;

use crate::executor::Event;

pub trait EventSink: Send + Sync {
    fn emit(&self, run_id: &str, ev: &Event);
}

/// Fan-out wrapper: holds a vec of sinks and forwards every emit to
/// each. The runner uses one of these per run so it doesn't need to
/// know how many sinks are attached.
pub struct FanOutSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl FanOutSink {
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }

    pub fn push(&mut self, sink: Arc<dyn EventSink>) {
        self.sinks.push(sink);
    }
}

impl EventSink for FanOutSink {
    fn emit(&self, run_id: &str, ev: &Event) {
        for sink in &self.sinks {
            sink.emit(run_id, ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CountingSink {
        count: Mutex<usize>,
    }

    impl EventSink for CountingSink {
        fn emit(&self, _run_id: &str, _ev: &Event) {
            *self.count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn fan_out_delivers_to_every_sink() {
        let a = Arc::new(CountingSink::default());
        let b = Arc::new(CountingSink::default());
        let fan = FanOutSink::new(vec![
            a.clone() as Arc<dyn EventSink>,
            b.clone() as Arc<dyn EventSink>,
        ]);

        let ev = Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: crate::runs::StepKind::Linear,
            agent: None,
        };
        fan.emit("r", &ev);
        fan.emit("r", &ev);

        assert_eq!(*a.count.lock().unwrap(), 2);
        assert_eq!(*b.count.lock().unwrap(), 2);
    }
}

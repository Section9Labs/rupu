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

/// The parent run's event sink as the in-process dispatcher's
/// [`DispatchEvents`](rupu_runtime::dispatch::DispatchEvents) port: a child
/// becomes `DispatchStarted` / `DispatchCompleted` on the parent's run, so
/// the live view renders it as a node under the active step.
pub struct DispatchEventSink(pub Arc<dyn EventSink>);

impl rupu_runtime::dispatch::DispatchEvents for DispatchEventSink {
    fn started(&self, parent_run_id: &str, child: &rupu_runtime::dispatch::DispatchedChild) {
        self.0.emit(
            parent_run_id,
            &Event::DispatchStarted {
                run_id: parent_run_id.to_string(),
                sub_run_id: child.sub_run_id.clone(),
                agent: Some(child.agent.clone()),
                transcript_path: child.transcript_path.clone(),
                codename: child.codename.clone(),
                provider: Some(child.provider.clone()),
                model: Some(child.model.clone()),
            },
        );
    }

    fn completed(&self, parent_run_id: &str, done: &rupu_runtime::dispatch::DispatchDone) {
        self.0.emit(
            parent_run_id,
            &Event::DispatchCompleted {
                run_id: parent_run_id.to_string(),
                sub_run_id: done.sub_run_id.clone(),
                success: done.success,
                tokens_in: done.tokens_in,
                tokens_out: done.tokens_out,
                cause: done.cause.clone(),
            },
        );
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
            host: None,
            codename: None,
        };
        fan.emit("r", &ev);
        fan.emit("r", &ev);

        assert_eq!(*a.count.lock().unwrap(), 2);
        assert_eq!(*b.count.lock().unwrap(), 2);
    }
}

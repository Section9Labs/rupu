//! InMemorySink — wraps a tokio::sync::broadcast::Sender so the
//! executor can fan events to live subscribers (e.g. the rupu.app
//! Graph view). Non-blocking emit, drops on no-subscribers.

use tokio::sync::broadcast;

use crate::executor::sink::EventSink;
use crate::executor::Event;

pub struct InMemorySink {
    tx: broadcast::Sender<Event>,
}

impl InMemorySink {
    pub fn with_capacity(cap: usize) -> Self {
        let (tx, _) = broadcast::channel(cap);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl EventSink for InMemorySink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        // send() returns Err when there are no live receivers; that's
        // expected (the run started before anyone subscribed) and we
        // deliberately drop. The on-disk JsonlSink is the durable copy.
        let _ = self.tx.send(ev.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::sink::EventSink;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[tokio::test]
    async fn two_subscribers_both_receive_the_same_event() {
        let sink = InMemorySink::with_capacity(16);
        let mut a = sink.subscribe();
        let mut b = sink.subscribe();
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        let ev_a = a.recv().await.expect("a recv");
        let ev_b = b.recv().await.expect("b recv");
        assert_eq!(ev_a.run_id(), "r");
        assert_eq!(ev_b.run_id(), "r");
    }

    #[tokio::test]
    async fn no_subscribers_drops_silently() {
        let sink = InMemorySink::with_capacity(16);
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
    }
}

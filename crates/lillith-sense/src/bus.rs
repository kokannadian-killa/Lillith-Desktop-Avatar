//! The event bus: a tokio broadcast channel of [`Event`]s.

use lillith_core::Event;
use tokio::sync::broadcast;

/// Cloneable handle to the shared event bus.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publishes an event; dropped silently if nobody listens yet.
    pub fn publish(&self, ev: Event) {
        tracing::debug!(event = %ev.name, "publish");
        let _ = self.tx.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_subscriber() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        bus.publish(Event::new("battery.low"));
        assert_eq!(rx.recv().await.unwrap().name, "battery.low");
    }
}

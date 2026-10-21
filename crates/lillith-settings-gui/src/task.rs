//! A one-shot background job for blocking calls (ping, test chat,
//! `ollama create`, the file dialog) so the UI thread never blocks.

use std::sync::mpsc::{channel, Receiver};

pub struct Task<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Task<T> {
    /// Runs `f` on a new thread and repaints `ctx` when it's done.
    pub fn spawn(ctx: &egui::Context, name: &str, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name(format!("lillith-settings-{name}"))
            .spawn(move || {
                let _ = tx.send(f());
                ctx.request_repaint();
            });
        Self { rx }
    }

    /// The result, once (`None` while running).
    pub fn poll(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

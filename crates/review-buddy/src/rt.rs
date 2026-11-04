//! Background work for an immediate-mode UI: run blocking work (git, gh, claude, HTTP)
//! on a worker thread and hand the UI thread a closure that applies the result.

use std::sync::mpsc::{channel, Receiver, Sender};

use crate::app::AppState;

/// A state change computed off the UI thread, applied on it.
pub type Job = Box<dyn FnOnce(&mut AppState) + Send>;

#[derive(Clone)]
pub struct Spawner {
    tx: Sender<Job>,
    ctx: Option<egui::Context>,
    /// Tests: run work on the calling thread so results are deterministic.
    inline: bool,
}

impl Spawner {
    pub fn new(ctx: Option<egui::Context>) -> (Spawner, Receiver<Job>) {
        let (tx, rx) = channel();
        (Spawner { tx, ctx, inline: false }, rx)
    }

    /// A spawner that runs work synchronously; jobs still queue until drained.
    #[allow(dead_code)]
    pub fn inline() -> (Spawner, Receiver<Job>) {
        let (tx, rx) = channel();
        (Spawner { tx, ctx: None, inline: true }, rx)
    }

    /// Runs `work` on a new thread; the job it returns is applied on the UI thread.
    pub fn spawn<F>(&self, work: F)
    where
        F: FnOnce() -> Job + Send + 'static,
    {
        if self.inline {
            let job = work();
            self.post(job);
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let job = work();
            me.post(job);
        });
    }

    /// Queues a job for the UI thread (callable from any thread).
    pub fn post(&self, job: Job) {
        let _ = self.tx.send(job);
        self.repaint();
    }

    pub fn repaint(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

/// Builds a `Job` from a closure with less boxing noise.
pub fn job(f: impl FnOnce(&mut AppState) + Send + 'static) -> Job {
    Box::new(f)
}

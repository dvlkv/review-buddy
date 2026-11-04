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
}

impl Spawner {
    pub fn new(ctx: Option<egui::Context>) -> (Spawner, Receiver<Job>) {
        let (tx, rx) = channel();
        (Spawner { tx, ctx }, rx)
    }

    /// Runs `work` on a new thread; the job it returns is applied on the UI thread.
    pub fn spawn<F>(&self, work: F)
    where
        F: FnOnce() -> Job + Send + 'static,
    {
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

    /// Applies `job` after `delay` (timers, debounces, retries).
    pub fn after(&self, delay: std::time::Duration, job: Job) {
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            me.post(job);
        });
    }
}

/// Builds a `Job` from a closure with less boxing noise.
pub fn job(f: impl FnOnce(&mut AppState) + Send + 'static) -> Job {
    Box::new(f)
}

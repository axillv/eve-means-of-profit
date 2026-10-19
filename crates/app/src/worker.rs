//! The one background thread. The UI sends jobs and reads results each frame. A job reports back
//! over a channel of its own, and asks the UI to repaint after each message.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use egui::Context;
use profit_sde::Fetch;

/// Work for the worker thread. It runs once, then the worker asks the UI to repaint.
pub type Job = Box<dyn FnOnce() + Send>;

/// The network, shared by the jobs. Only the worker thread locks it, so a download never blocks
/// the UI.
pub type SharedFetch = Arc<Mutex<Box<dyn Fetch + Send>>>;

pub struct Worker {
    jobs: Sender<Job>,
    fetch: SharedFetch,
}

impl Worker {
    /// Starts the thread. It runs jobs in the order they were given, until the worker is dropped.
    pub fn spawn(ctx: Context, fetch: Box<dyn Fetch + Send>) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        thread::spawn(move || {
            for job in inbox {
                job();
                ctx.request_repaint();
            }
        });
        Self {
            jobs,
            fetch: Arc::new(Mutex::new(fetch)),
        }
    }

    /// Queues a job. A worker whose thread has stopped (a job panicked) takes no more jobs.
    pub fn run(&self, job: Job) {
        drop(self.jobs.send(job));
    }

    /// The network for a job to lock on the worker thread.
    pub fn fetch(&self) -> SharedFetch {
        Arc::clone(&self.fetch)
    }
}

/// Locks the network. A lock poisoned by a panicked job is still used.
pub fn lock(fetch: &SharedFetch) -> MutexGuard<'_, Box<dyn Fetch + Send>> {
    fetch.lock().unwrap_or_else(PoisonError::into_inner)
}

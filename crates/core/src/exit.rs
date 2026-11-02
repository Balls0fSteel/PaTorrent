//! A session closing in the background (D47): what the engine is waiting for, so that a GUI can
//! show it instead of looking hung, and the "Quit now" that cuts the waits short where that is
//! safe.
//!
//! [`Session::begin_shutdown`](crate::Session::begin_shutdown) returns an [`Exit`]. The engine
//! goes through the [`ExitPhase`]s in order. [`Exit::hurry`] cancels the moves in flight (their
//! copies are deleted, the files stay where they were, the torrents keep their old folder) and
//! skips the wait for the trackers; the resume data is saved in any case. Once the phase is
//! [`ExitPhase::Closing`] what is left is writing the downloaded blocks still queued (at most
//! about `max_queued_disk_bytes`) and closing files: a GUI may stop waiting then
//! ([`Exit::abandon`]). The blocks not written are not in the resume data, so they are only
//! downloaded again; what was written is in the operating system's hands and survives the
//! process.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use crate::session::Shared;

/// The steps of a shutdown, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitPhase {
    /// Stopping the torrents (quick).
    #[default]
    Stopping,
    /// Waiting for moves and renames in flight; [`Exit::hurry`] cancels the moves.
    Moving,
    /// Saving the resume data. Never cut short.
    Saving,
    /// Up to `stop_tracker_timeout` for the "stopped" announces; [`Exit::hurry`] skips it.
    Trackers,
    /// The disk threads write what is queued and close the files.
    Closing,
}

/// A torrent whose files are being moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitMove {
    pub name: String,
    /// The folder the files are in, and stay in if the move is cancelled.
    pub from: String,
    pub to: String,
    /// Bytes moved of `total` (a file renamed on the same volume counts whole at once).
    pub done: u64,
    pub total: u64,
}

/// What a closing session is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExitStatus {
    pub phase: ExitPhase,
    /// In [`ExitPhase::Moving`]: the moves still running.
    pub moves: Vec<ExitMove>,
    /// Downloaded bytes handed to the disk threads and not written yet.
    pub unwritten: u64,
    /// [`Exit::hurry`] was called.
    pub hurried: bool,
}

/// The engine's side, in the session's shared state.
#[derive(Default)]
pub(crate) struct ExitState {
    status: Mutex<ExitStatus>,
    hurry: AtomicBool,
    /// The disk threads' count of queued write bytes, once the engine has them.
    unwritten: OnceLock<Arc<AtomicU64>>,
}

impl ExitState {
    pub(crate) fn set_phase(&self, phase: ExitPhase) {
        let mut s = self.lock();
        s.phase = phase;
        if phase != ExitPhase::Moving {
            s.moves.clear();
        }
    }

    pub(crate) fn set_moves(&self, moves: Vec<ExitMove>) {
        self.lock().moves = moves;
    }

    pub(crate) fn hurried(&self) -> bool {
        self.hurry.load(Ordering::Relaxed)
    }

    pub(crate) fn watch_unwritten(&self, counter: Arc<AtomicU64>) {
        let _ = self.unwritten.set(counter);
    }

    fn status(&self) -> ExitStatus {
        let mut s = self.lock().clone();
        s.unwritten = self.unwritten.get().map_or(0, |c| c.load(Ordering::Relaxed));
        s.hurried = self.hurried();
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ExitStatus> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A session shutting down on its own thread. Dropping it waits for the end, as
/// [`Exit::wait`].
pub struct Exit {
    pub(crate) shared: Arc<Shared>,
    pub(crate) thread: Option<JoinHandle<()>>,
}

impl Exit {
    /// What the engine is doing now.
    pub fn status(&self) -> ExitStatus {
        self.shared.exit.status()
    }

    /// The engine thread has ended: everything is saved and closed.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// "Quit now": cancel the moves and skip the trackers' wait (see the module documentation).
    pub fn hurry(&self) {
        self.shared.exit.hurry.store(true, Ordering::Relaxed);
        (self.shared.wake)();
    }

    /// Waits for the engine thread to end.
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Stops following the shutdown: the engine thread goes on and ends with the process. Only
    /// sound from [`ExitPhase::Closing`] on, when the resume data is saved.
    pub fn abandon(mut self) {
        self.thread = None;
    }
}

impl Drop for Exit {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

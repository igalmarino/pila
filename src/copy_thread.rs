//! The copy thread: writes a capture's read-back pixels into memory that may
//! be new (a client's shm buffer, a screenshot's), off the event loop.
//!
//! The first write into such memory faults its pages in: a 1920x1080 frame
//! held the event loop 9 ms on bare metal, for every screenshot and the
//! first frame of every share into shm. Mapping the read-back stays on the
//! event loop (it needs the GL context, and takes microseconds); the write
//! comes here (`capture::Write::OffLoop`), one job after another, and its
//! result goes back to the event loop, in the same order, where the copy's
//! continuation runs (`Pila::when_copied`).
//!
//! A job reads the read-back through its GL mapping, which stays mapped as
//! long as the job holds it; the renderer outlives the thread: `Captures`,
//! which owns it and joins it when dropped, is a field of `Pila` declared
//! before `backend`, so dropped before it.

use std::{collections::VecDeque, sync::mpsc, thread::JoinHandle};

use smithay::reexports::calloop::{
    channel::{self, Event},
    RegistrationToken,
};
use tracing::{trace, warn};

use crate::{capture::CopyError, Pila};

/// The rest of a copy, for the copy thread.
pub(crate) type Job = Box<dyn FnOnce() -> Result<(), CopyError> + Send>;

/// What runs on the event loop once a job is done.
type Then = Box<dyn FnOnce(&mut Pila, Result<(), CopyError>)>;

/// The copy thread, started at the first job (`Captures::copy_thread`).
#[derive(Default)]
pub struct CopyThread {
    /// Where jobs go; `None` until the thread is started, or once it's gone.
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    /// The event loop's source of the thread's results.
    results: Option<RegistrationToken>,
    /// The continuations of the jobs sent, in their order: the thread
    /// answers in that order.
    waiting: VecDeque<Then>,
    /// Jobs written on the thread, for the state (`copies_off_loop`).
    pub written: u64,
}

impl Drop for CopyThread {
    /// Let the thread finish the jobs it has, and wait for it: they read
    /// GL mappings, which must not outlive the renderer.
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Pila {
    /// Run `job` on the copy thread, then `then` on the event loop with its
    /// result; on the event loop right away if the thread can't be started.
    pub(crate) fn write_off_loop(
        &mut self,
        job: Job,
        then: impl FnOnce(&mut Pila, Result<(), CopyError>) + 'static,
    ) {
        if self.capture.copy_thread.jobs.is_none() {
            if let Err(err) = self.start_copy_thread() {
                warn!(err, "capture: no copy thread; copying on the event loop");
                return then(self, job());
            }
        }
        let thread = &mut self.capture.copy_thread;
        let Some(jobs) = &thread.jobs else {
            return then(self, job());
        };
        match jobs.send(job) {
            Ok(()) => thread.waiting.push_back(Box::new(then)),
            // Gone since: its `Closed` fails what it left.
            Err(mpsc::SendError(job)) => then(self, job()),
        }
    }

    fn start_copy_thread(&mut self) -> Result<(), String> {
        let (jobs, received) = mpsc::channel::<Job>();
        let (results, channel) = channel::channel();
        let token = self
            .loop_handle
            .insert_source(channel, |event, _, state| match event {
                Event::Msg(result) => state.copy_written(result),
                Event::Closed => state.copy_thread_gone(),
            })
            .map_err(|err| err.to_string())?;
        let spawned = std::thread::Builder::new()
            .name("pila-copy".into())
            .spawn(move || {
                for job in received {
                    if results.send(job()).is_err() {
                        break;
                    }
                }
            });
        match spawned {
            Ok(thread) => {
                trace!("capture: copy thread started");
                let copy_thread = &mut self.capture.copy_thread;
                copy_thread.jobs = Some(jobs);
                copy_thread.thread = Some(thread);
                copy_thread.results = Some(token);
                Ok(())
            }
            Err(err) => {
                self.loop_handle.remove(token);
                Err(err.to_string())
            }
        }
    }

    /// The copy thread finished a job: its continuation, the first waiting.
    fn copy_written(&mut self, result: Result<(), CopyError>) {
        let copy_thread = &mut self.capture.copy_thread;
        let Some(then) = copy_thread.waiting.pop_front() else {
            warn!("capture: a copy done that nothing waited for");
            return;
        };
        copy_thread.written += 1;
        then(self, result);
    }

    /// The copy thread ended (it panicked: the panic is logged): fail the
    /// copies it left; the next one starts another.
    fn copy_thread_gone(&mut self) {
        let copy_thread = &mut self.capture.copy_thread;
        copy_thread.jobs = None;
        if let Some(thread) = copy_thread.thread.take() {
            let _ = thread.join();
        }
        if let Some(token) = copy_thread.results.take() {
            self.loop_handle.remove(token);
        }
        let copy_thread = &mut self.capture.copy_thread;
        let waiting = std::mem::take(&mut copy_thread.waiting);
        if !waiting.is_empty() {
            warn!(copies = waiting.len(), "capture: the copy thread ended");
        }
        for then in waiting {
            then(self, Err(CopyError::Render("the copy thread ended".into())));
        }
    }
}

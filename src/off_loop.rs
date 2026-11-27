//! Work done off the event loop: on a thread of its own, its result handed
//! back to the loop through a channel. Reading files, drawing a panel's
//! text and asking X or D-Bus take milliseconds, which a frame can't wait
//! for.

use smithay::reexports::calloop::channel::{self, Event};
use tracing::warn;

use crate::Pila;

impl Pila {
    /// Run `work` on a thread named `thread`, then `done` on the event loop
    /// with what it gave. Whether both were started (a failure is logged,
    /// and `done` never runs then).
    pub(crate) fn off_loop<T: Send + 'static>(
        &self,
        thread: &str,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Pila, T) + 'static,
    ) -> bool {
        let (sender, results) = channel::channel();
        let spawned = std::thread::Builder::new()
            .name(thread.into())
            .spawn(move || {
                // The loop may have gone (Pila quitting): nobody to tell.
                let _ = sender.send(work());
            });
        if let Err(err) = spawned {
            warn!(?err, thread, "can't start a thread");
            return false;
        }
        // One message, then the channel closes.
        let mut done = Some(done);
        let inserted = self
            .loop_handle
            .insert_source(results, move |event, _, state| {
                if let (Event::Msg(result), Some(done)) = (event, done.take()) {
                    done(state, result);
                }
            });
        if let Err(err) = inserted {
            warn!(error = ?err.error, thread, "can't wait for a thread");
            return false;
        }
        true
    }
}

//! Locks that can't panic. A mutex is poisoned only when a thread panicked
//! holding it; Pila's are used on the event-loop thread, where a panic ends
//! Pila first, so poisoning never shows. Taking the value anyway
//! (`PoisonError::into_inner`) makes every lock infallible instead of a
//! `lock().unwrap()` resting on that.

use std::sync::{Mutex, MutexGuard, PoisonError};

pub trait Locked<T> {
    /// `lock`, taking the value even from a poisoned mutex.
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

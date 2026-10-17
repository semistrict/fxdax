// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Channels with many senders and one receiver: a queue under the crate's `Mutex`, which the
//! receiver waits on with its `Condvar`.

use super::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use alloc_crate::collections::VecDeque;
use core::fmt;
use core::time::Duration;

struct State<T> {
    queue: VecDeque<T>,
    senders: usize,
    receiver: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Creates an unbounded channel.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State { queue: VecDeque::new(), senders: 1, receiver: true }),
        ready: Condvar::new(),
    });
    (Sender { shared: shared.clone() }, Receiver { shared })
}

/// The sending half of a channel.
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Sender<T> {
    /// Queues `value`, unless the receiver is gone.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut state = self.shared.lock();
        if !state.receiver {
            return Err(SendError(value));
        }
        state.queue.push_back(value);
        drop(state);
        self.shared.ready.notify_one();
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().senders += 1;
        Self { shared: self.shared.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // Under the lock, so that a receiver that saw a sender left is waiting when notified.
        self.shared.lock().senders -= 1;
        self.shared.ready.notify_all();
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sender { .. }")
    }
}

/// The receiving half of a channel.
pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Receiver<T> {
    /// Waits for a value, until every sender is gone.
    pub fn recv(&self) -> Result<T, RecvError> {
        self.recv_timeout(Duration::MAX).map_err(|_| RecvError)
    }

    /// Takes a value, if one is queued.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut state = self.shared.lock();
        match state.queue.pop_front() {
            Some(value) => Ok(value),
            None if state.senders == 0 => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }

    /// Waits up to `timeout` for a value, until every sender is gone.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let start = crate::time::Instant::now();
        let mut state = self.shared.lock();
        loop {
            if let Some(value) = state.queue.pop_front() {
                return Ok(value);
            }
            if state.senders == 0 {
                return Err(RecvTimeoutError::Disconnected);
            }
            let Some(left) = timeout.checked_sub(start.elapsed()) else {
                return Err(RecvTimeoutError::Timeout);
            };
            state = self
                .shared
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The values received until every sender is gone.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { receiver: self }
    }

    /// The values already queued.
    pub fn try_iter(&self) -> TryIter<'_, T> {
        TryIter { receiver: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.receiver = false;
        // Values queued now will never be received.
        let queue = core::mem::take(&mut state.queue);
        drop(state);
        drop(queue);
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Receiver { .. }")
    }
}

/// An iterator over the values received until every sender is gone.
pub struct Iter<'a, T> {
    receiver: &'a Receiver<T>,
}

impl<T> Iterator for Iter<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.receiver.recv().ok()
    }
}

/// An iterator over the values already queued.
pub struct TryIter<'a, T> {
    receiver: &'a Receiver<T>,
}

impl<T> Iterator for TryIter<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.receiver.try_recv().ok()
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

/// The value of a send whose receiver is gone.
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SendError { .. }")
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sending on a closed channel")
    }
}

impl<T> core::error::Error for SendError<T> {}

/// Every sender is gone.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("receiving on a closed channel")
    }
}

impl core::error::Error for RecvError {}

/// Why `try_recv` returned no value.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TryRecvError::Empty => "receiving on an empty channel",
            TryRecvError::Disconnected => "receiving on a closed channel",
        })
    }
}

impl core::error::Error for TryRecvError {}

/// Why `recv_timeout` returned no value.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum RecvTimeoutError {
    Timeout,
    Disconnected,
}

impl fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RecvTimeoutError::Timeout => "timed out waiting on channel",
            RecvTimeoutError::Disconnected => "channel is empty and sending half is closed",
        })
    }
}

impl core::error::Error for RecvTimeoutError {}

//! Pausing an image's writes, so that a copy of it is the image as it was at one time.

use event_listener::Event;
use std::sync::{Condvar, Mutex};

/// The writes to an image, which copying it pauses.
#[derive(Debug, Default)]
pub(crate) struct WriteGate {
    state: Mutex<State>,
    /// Notified when writes resume.
    resumed: Event,
    /// Notified when the last write in progress ends.
    idle: Condvar,
}

#[derive(Debug, Default)]
struct State {
    /// The writes in progress.
    writing: usize,
    /// The pauses in effect.
    pauses: usize,
}

impl WriteGate {
    /// Waits until writes aren't paused, and returns a guard that holds off pauses until the write
    /// ends.
    pub async fn write(&self) -> Writing<'_> {
        loop {
            let resumed = {
                let mut state = self.state.lock().unwrap();
                if state.pauses == 0 {
                    state.writing += 1;
                    return Writing(self);
                }
                self.resumed.listen()
            };
            resumed.await;
        }
    }

    /// Pauses writes once those in progress end, blocking the calling thread until they do, and
    /// returns a guard that resumes them when dropped.
    pub fn pause(&self) -> Paused<'_> {
        let mut state = self.state.lock().unwrap();
        state.pauses += 1;
        while state.writing > 0 {
            state = self.idle.wait(state).unwrap();
        }
        Paused(self)
    }
}

/// A write in progress.
pub(crate) struct Writing<'a>(&'a WriteGate);

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.writing -= 1;
        if state.writing == 0 {
            self.0.idle.notify_all();
        }
    }
}

/// Writes paused.
pub(crate) struct Paused<'a>(&'a WriteGate);

impl Drop for Paused<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.pauses -= 1;
        if state.pauses == 0 {
            self.0.resumed.notify(usize::MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WriteGate;
    use futures::FutureExt;
    use std::sync::Arc;

    #[test]
    fn pause_waits_for_writes_and_holds_off_new_ones() {
        let gate = Arc::new(WriteGate::default());
        let writing = gate.write().now_or_never().expect("writes aren't paused");
        let pausing = std::thread::spawn({
            let gate = gate.clone();
            move || {
                let _paused = gate.pause();
            }
        });
        // The pause waits for the write in progress, and holds off writes meanwhile.
        while gate.state.lock().unwrap().pauses == 0 {
            std::thread::yield_now();
        }
        assert!(!pausing.is_finished());
        assert!(gate.write().now_or_never().is_none());
        drop(writing);
        pausing.join().unwrap();
        // Once the pause ends, writes go ahead.
        assert!(gate.write().now_or_never().is_some());
    }

    #[test]
    fn writes_resume_when_the_pause_ends() {
        let gate = WriteGate::default();
        let paused = gate.pause();
        let mut write = Box::pin(gate.write());
        assert!((&mut write).now_or_never().is_none());
        drop(paused);
        futures::executor::block_on(write);
    }
}

//! Wakers of the futures waiting on something, alongside the threads blocked on it.
//!
//! Zircon has only threads to wait. Here a wait may also be a future, whose task registers its
//! waker where a thread would block, and is woken where a thread would be. This is a deviation of
//! the port's: it lets a host process wait on the kernel without holding a thread per wait.

use std::task::Waker;

/// The wakers waiting on something, each under the key of the future that registered it. The
/// owner keeps it under the lock that guards what is waited on, so that a waker registered after
/// checking that state cannot miss the change that wakes it.
#[derive(Default)]
pub struct Wakers {
    next_key: u64,
    wakers: Vec<(u64, Waker)>,
}

impl Wakers {
    pub const fn new() -> Self {
        Self { next_key: 0, wakers: Vec::new() }
    }

    /// Registers `waker` for the future whose key is `key`, replacing the one it registered
    /// before, and gives the future a key if it has none.
    pub fn register(&mut self, key: &mut Option<u64>, waker: &Waker) {
        if let Some(key) = *key
            && let Some((_, registered)) = self.wakers.iter_mut().find(|(k, _)| *k == key)
        {
            if !registered.will_wake(waker) {
                registered.clone_from(waker);
            }
            return;
        }
        let new_key = self.next_key;
        self.next_key += 1;
        *key = Some(new_key);
        self.wakers.push((new_key, waker.clone()));
    }

    /// Forgets the waker of the future whose key is `key`, if it has one.
    pub fn remove(&mut self, key: &mut Option<u64>) {
        if let Some(key) = key.take() {
            self.wakers.retain(|(k, _)| *k != key);
        }
    }

    /// Takes every waker, to be woken once the lock guarding them is released. Their futures
    /// register again if they still have to wait.
    #[must_use = "the wakers must be woken"]
    pub fn take(&mut self) -> WakeAll {
        WakeAll(std::mem::take(&mut self.wakers))
    }

    pub fn is_empty(&self) -> bool {
        self.wakers.is_empty()
    }
}

/// Wakers taken to be woken, which they are when this is dropped.
pub struct WakeAll(Vec<(u64, Waker)>);

impl Drop for WakeAll {
    fn drop(&mut self) {
        for (_, waker) in self.0.drain(..) {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    struct CountingWaker(AtomicUsize);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn registering_again_replaces_the_waker() {
        let (first, second) = (
            Arc::new(CountingWaker(AtomicUsize::new(0))),
            Arc::new(CountingWaker(AtomicUsize::new(0))),
        );
        let mut wakers = Wakers::new();
        let mut key = None;
        wakers.register(&mut key, &Waker::from(first.clone()));
        wakers.register(&mut key, &Waker::from(second.clone()));
        drop(wakers.take());
        assert_eq!(first.0.load(Ordering::SeqCst), 0);
        assert_eq!(second.0.load(Ordering::SeqCst), 1);
        assert!(wakers.is_empty());
    }

    #[test]
    fn removed_wakers_are_not_woken() {
        let waker = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let mut wakers = Wakers::new();
        let (mut kept, mut removed) = (None, None);
        wakers.register(&mut kept, &Waker::from(waker.clone()));
        wakers.register(&mut removed, &Waker::from(waker.clone()));
        wakers.remove(&mut removed);
        assert_eq!(removed, None);
        drop(wakers.take());
        assert_eq!(waker.0.load(Ordering::SeqCst), 1);
    }
}

// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: asking guests to seal (TASK-153.15).
//!
//! An open extent's pages, which a window maps writably, are pinned until its guest seals it:
//! nothing in the daemon can evict them, nor seal them (`crate::cache`). So a guest's writable
//! `MAP` past the budget of pinned pages is refused (`ENOMEM`), which tells it to seal and try
//! again (TASK-150.20). But the budget is shared, its arena's and every arena's together: another
//! guest's open pages, one idle, say, holding them for as long as it doesn't sync, leave the one
//! refused nothing its own seals make room in, and it fails `ENOMEM`.
//!
//! So a writable `MAP` refused waits for room ([`made_room`]), as it can: the daemon asks the
//! guests holding the most open pages to seal ([`wire::SEAL`]), its tenant's first, as many as
//! hold what the `MAP` brings in, and waits for their answers ([`wire::SEALED`]), holding no
//! thread, then tries once more. Refused again, it says `ENOMEM`, and its guest seals, as before.
//!
//! - **No deadlock.** A guest's seal may wait for its own `MAP`, which a seal can't overtake in the
//!   guest's gate. So a connection whose guest is asked to seal never waits for others' seals: it
//!   is refused at once, and seals. Each connection that waits asks, or finds asked, others under
//!   one lock, before any of them waits: those it waits for were asked before it began to, and
//!   began to wait, if at all, before they were asked; so no cycle of waits closes.
//! - **Bounded.** A guest that doesn't answer, paused or hung, is waited for [`SEAL_WAIT`] at most,
//!   and asked no more until it answers: no other guest waits for it again meanwhile.
//! - **One ask at a time.** A guest asked and yet to answer isn't asked again: its answer answers
//!   every waiter.

use super::server::Connection;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How long a writable `MAP` refused waits at most for the guests asked to seal to answer.
pub(super) const SEAL_WAIT: Duration = Duration::from_secs(10);

/// The connections that write a store, whose guests may hold open pages: what [`made_room`] asks
/// from, under its lock, which every ask is made under.
static WRITERS: Mutex<Vec<Weak<Connection>>> = Mutex::new(Vec::new());

/// The connection `writer`, which has opened a store to write it, may be asked to seal from now
/// on: until it goes.
pub(super) fn writing(writer: Weak<Connection>) {
    WRITERS.lock().unwrap().push(writer);
}

/// What a connection's guest was asked to seal, and has answered.
#[derive(Default)]
pub(super) struct Seals {
    state: Mutex<State>,
    /// Notified as the guest answers, and as the connection goes.
    answered: event_listener::Event,
}

#[derive(Default)]
struct State {
    /// The number of the latest ask, and of the latest answered: the guest is asked while they
    /// differ.
    asked: u64,
    answered: u64,
    /// When the oldest ask unanswered was made.
    since: Option<Instant>,
    /// Whether the connection went: none of its asks is waited for any more.
    gone: bool,
}

impl Seals {
    /// Whether the guest is asked to seal, and has yet to answer.
    fn is_asked(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.asked != state.answered
    }

    /// Whether an ask has gone unanswered past [`SEAL_WAIT`], or the connection went: it isn't
    /// waited for again until it answers.
    fn is_unresponsive(&self, now: Instant) -> bool {
        let state = self.state.lock().unwrap();
        state.gone || state.since.is_some_and(|since| now.duration_since(since) >= SEAL_WAIT)
    }

    /// Asks the guest to seal, `tell` telling it the ask's number, unless an ask is unanswered
    /// still: the number of the ask to wait for, or `None` if telling it failed.
    fn ask(&self, now: Instant, tell: impl FnOnce(u64) -> bool) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if state.asked != state.answered {
            return Some(state.asked);
        }
        let number = state.asked + 1;
        if !tell(number) {
            return None;
        }
        state.asked = number;
        state.since = Some(now);
        Some(number)
    }

    /// The guest answered the ask of `number`: it has sealed, and the asks before it are answered
    /// too. A number not asked yet answers no more than was.
    pub(super) fn answered(&self, number: u64) {
        let mut state = self.state.lock().unwrap();
        state.answered = state.answered.max(number.min(state.asked));
        if state.answered == state.asked {
            state.since = None;
        }
        drop(state);
        self.answered.notify(usize::MAX);
    }

    /// The connection went: nothing waits for its guest's answers any more.
    pub(super) fn stop(&self) {
        self.state.lock().unwrap().gone = true;
        self.answered.notify(usize::MAX);
    }

    /// Once the ask of `number` is answered, or the connection went.
    async fn wait(&self, number: u64) {
        loop {
            let answered = self.answered.listen();
            {
                let state = self.state.lock().unwrap();
                if state.gone || state.answered >= number {
                    return;
                }
            }
            answered.await;
        }
    }
}

/// A connection another's `MAP` may ask to seal, as [`choose`] sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Holder {
    /// The bytes its guest holds open ([`crate::ExtentStore::open_bytes`]).
    pub open: u64,
    /// Whether its pages are of the asking connection's arena, its tenant's: those that count
    /// against the arena's budget as well as every arena's.
    pub same_arena: bool,
}

/// Which of `holders` to ask to seal to make room for `bytes` more: those holding any, of the
/// asking connection's arena first, the most first, until those asked hold `bytes`, or all of
/// them if they hold less. In the order to ask them in.
pub(super) fn choose(holders: &[Holder], bytes: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..holders.len()).filter(|at| holders[*at].open > 0).collect();
    order.sort_by_key(|at| (!holders[*at].same_arena, std::cmp::Reverse(holders[*at].open)));
    let mut held = 0;
    order
        .into_iter()
        .take_while(|at| {
            let more = held < bytes.max(1);
            held += holders[*at].open;
            more
        })
        .collect()
}

/// Makes room for `bytes` more open pages, which a writable `MAP` of `asking`'s was refused for:
/// asks the guests of other connections holding open pages to seal ([`choose`]), and waits for
/// their answers, [`SEAL_WAIT`] at most. Whether it asked any, which makes another try worth it;
/// not if `asking`'s own guest is asked to seal, which it would wait for, maybe, in turn (the
/// module docs).
pub(super) async fn made_room(asking: &Arc<Connection>, bytes: u64) -> bool {
    let now = Instant::now();
    let waits: Vec<(Arc<Seals>, u64)> = {
        let mut writers = WRITERS.lock().unwrap();
        writers.retain(|writer| writer.strong_count() > 0);
        if asking.seals().is_asked() {
            return false;
        }
        let arena = asking.arena_id();
        let others: Vec<Arc<Connection>> = (writers.iter())
            .filter_map(Weak::upgrade)
            .filter(|other| !Arc::ptr_eq(other, asking) && !other.seals().is_unresponsive(now))
            .collect();
        let holders: Vec<Holder> = (others.iter())
            .map(|other| Holder {
                open: other.store().map_or(0, |store| store.open_bytes()),
                same_arena: other.arena_id() == arena,
            })
            .collect();
        (choose(&holders, bytes).into_iter())
            .filter_map(|at| {
                let other = &others[at];
                let number = other.seals().ask(now, |number| other.ask_to_seal(number))?;
                Some((other.seals().clone(), number))
            })
            .collect()
    };
    if waits.is_empty() {
        return false;
    }
    log::info!(
        "extentd: a writable MAP of {bytes:#x} bytes refused for want of memory waits for {} \
         other guests asked to seal",
        waits.len()
    );
    let answered =
        futures::future::join_all(waits.iter().map(|(seals, number)| seals.wait(*number)));
    if tokio::time::timeout(SEAL_WAIT, answered).await.is_err() {
        log::warn!("extentd: guests asked to seal didn't answer within {SEAL_WAIT:?}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(open: u64, same_arena: bool) -> Holder {
        Holder { open, same_arena }
    }

    /// The asking connection's tenant first, as its pages count against its arena's budget too;
    /// the most first; as many as hold the bytes asked; none holding nothing.
    #[test]
    fn asks_the_largest_holders_of_its_arena_first() {
        let holders =
            [holder(4, false), holder(0, true), holder(2, true), holder(8, true), holder(9, false)];
        assert_eq!(choose(&holders, 1), vec![3]);
        assert_eq!(choose(&holders, 8), vec![3]);
        assert_eq!(choose(&holders, 9), vec![3, 2]);
        assert_eq!(choose(&holders, 11), vec![3, 2, 4]);
        assert_eq!(choose(&holders, 100), vec![3, 2, 4, 0]);
        assert_eq!(choose(&holders, 0), vec![3]);
        assert_eq!(choose(&[holder(0, true)], 1), Vec::<usize>::new());
    }

    /// An ask unanswered is the one asked again, told once; its answer, or a later one's, answers
    /// it; an answer to an ask not made answers no more than was asked.
    #[test]
    fn asks_unanswered_are_one() {
        let seals = Seals::default();
        let now = Instant::now();
        let told = std::cell::RefCell::new(Vec::new());
        let tell = |number| {
            told.borrow_mut().push(number);
            true
        };
        assert_eq!(seals.ask(now, tell), Some(1));
        assert_eq!(seals.ask(now, tell), Some(1));
        assert_eq!(*told.borrow(), [1]);
        assert!(seals.is_asked());
        seals.answered(7);
        assert!(!seals.is_asked());
        assert_eq!(seals.ask(now, tell), Some(2));
        assert_eq!(*told.borrow(), [1, 2]);
        futures::executor::block_on(seals.wait(1));
        seals.answered(2);
        futures::executor::block_on(seals.wait(2));
    }

    /// An ask a guest couldn't be told of isn't one: none is waited for, and the next is told.
    #[test]
    fn asks_not_told_are_not_asked() {
        let seals = Seals::default();
        let now = Instant::now();
        assert_eq!(seals.ask(now, |_| false), None);
        assert!(!seals.is_asked());
        assert_eq!(seals.ask(now, |_| true), Some(1));
    }

    /// A guest that leaves an ask unanswered past `SEAL_WAIT` isn't waited for again until it
    /// answers; nor one whose connection went, whose waiters stop waiting.
    #[test]
    fn guests_that_dont_answer_are_not_waited_for_again() {
        let seals = Seals::default();
        let now = Instant::now();
        seals.ask(now, |_| true);
        assert!(!seals.is_unresponsive(now + SEAL_WAIT / 2));
        assert!(seals.is_unresponsive(now + SEAL_WAIT));
        seals.answered(1);
        assert!(!seals.is_unresponsive(now + SEAL_WAIT));

        seals.ask(now, |_| true);
        seals.stop();
        assert!(seals.is_unresponsive(now));
        futures::executor::block_on(seals.wait(2));
    }
}

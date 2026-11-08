// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the guest request a piece of work is for, end to end (TASK-260), which logs and traces
//! name: [`current`].
//!
//! A guest's virtio extent driver numbers every request it makes of its device (doc-4's `unique`,
//! a per-device counter from 1), which costs it nothing more: that number is the request's ID. The
//! VMM serves each request with it current ([`guest_request`]); the extent daemon's client says it
//! in each call it makes for the request (`extentd::wire::REQUEST`); and the daemon serves each such
//! call with it current again, with the store its connection serves, since every VM's device
//! numbers its own requests. So the daemon's log records about a request (`extentd::logging`), the
//! object store's requests made for it among them, name it, as anything that traces it can.
//!
//! A request is current while what serves it is polled, or run, on a thread ([`InRequest`],
//! [`within_now`]), and carried by hand to another ([`within`]): the remotes' runtime's reads of
//! object storage, and the pager's.

use std::cell::Cell;
use std::future::Future;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A guest request: what [`current`] says the work on a thread is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestId {
    /// The guest's own number of it, its virtio extent device's `unique` (doc-4): unique among
    /// that device's requests, not among a host's.
    pub guest: NonZeroU64,
    /// In the extent daemon, the store whose VM process made the call, which tells one VM's device
    /// from another's: none in the VMM, which serves one device, nor before its store is open.
    pub store: Option<Arc<str>>,
}

thread_local! {
    static CURRENT: Cell<Option<RequestId>> = const { Cell::new(None) };
}

/// The guest request the work on this thread is for, if it is for one: in the VMM, as it serves
/// the request; in the extent daemon, as it serves a call for it, and as the object store is read
/// for it. What logs and traces read, as the daemon's log does for each record (`request_id`).
pub fn current() -> Option<RequestId> {
    CURRENT.with(|current| {
        let held = current.take();
        let request = held.clone();
        current.set(held);
        request
    })
}

/// The guest's number of the request current, if any: what the daemon's client says in its calls.
pub(crate) fn current_guest() -> Option<NonZeroU64> {
    CURRENT.with(|current| {
        let held = current.take();
        let guest = held.as_ref().map(|request| request.guest);
        current.set(held);
        guest
    })
}

/// `future`, polled with the guest request numbered `guest` current, unless it is 0, no number:
/// the VMM serves each of the guest's requests so.
pub fn guest_request<F: Future>(guest: u64, future: F) -> InRequest<F> {
    within(NonZeroU64::new(guest).map(|guest| RequestId { guest, store: None }), future)
}

/// `future`, polled with `request`, or none, current: what carries the request current to a
/// future polled elsewhere, as on another thread.
pub fn within<F: Future>(request: Option<RequestId>, future: F) -> InRequest<F> {
    InRequest { request, future }
}

/// Runs `f` with `request`, or none, current, and the one current before after.
pub(crate) fn within_now<R>(request: Option<RequestId>, f: impl FnOnce() -> R) -> R {
    let before = CURRENT.replace(request);
    let result = f();
    CURRENT.set(before);
    result
}

pin_project_lite::pin_project! {
    /// A future polled with a guest request, or none, current ([`guest_request`], [`within`]).
    pub struct InRequest<F> {
        request: Option<RequestId>,
        #[pin]
        future: F,
    }
}

impl<F: Future> Future for InRequest<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.project();
        let before = CURRENT.replace(this.request.take());
        let polled = this.future.poll(cx);
        // As the poll left it, for the next.
        *this.request = CURRENT.replace(before);
        polled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest(number: u64) -> Option<RequestId> {
        Some(RequestId { guest: NonZeroU64::new(number).unwrap(), store: None })
    }

    /// A future polled in a guest request has it current at each poll, and none between; guest
    /// number 0 is none.
    #[test]
    fn requests_are_current_as_their_futures_are_polled() {
        let mut polls = 0;
        let future = std::future::poll_fn(|cx| {
            assert_eq!(current(), guest(11));
            assert_eq!(current_guest(), NonZeroU64::new(11));
            polls += 1;
            if polls < 3 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        });
        let mut future = std::pin::pin!(guest_request(11, future));
        let mut cx = Context::from_waker(std::task::Waker::noop());
        while future.as_mut().poll(&mut cx).is_pending() {
            assert_eq!(current(), None, "none current between polls");
        }
        assert_eq!(polls, 3);
        let none = std::pin::pin!(guest_request(0, async { current() }));
        assert_eq!(none.poll(&mut cx), Poll::Ready(None));
    }

    /// A request run with is current within, and the one before after; one carried to another
    /// thread is current there.
    #[test]
    fn a_request_is_current_where_it_is_carried() {
        let request = Some(RequestId {
            guest: NonZeroU64::new(9).unwrap(),
            store: Some("/stores/vm1".into()),
        });
        assert_eq!(within_now(guest(1), || within_now(request.clone(), current)), request);
        assert_eq!(
            within_now(guest(1), || (within_now(None, current), current())),
            (None, guest(1))
        );
        assert_eq!(current(), None);
        let carried = within_now(request.clone(), || within(current(), async { current() }));
        let there = std::thread::spawn(move || futures::executor::block_on(carried));
        assert_eq!(there.join().unwrap(), request);
    }
}

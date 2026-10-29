// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What physical memory knows of each of its pages, its frames: an array of them indexed by frame
//! number, a page's offset into physical memory in pages. A frame is allocated, or free: on its
//! arena's free list (listed), or in a chunk no arena has, or in one going back. The free lists
//! are lists of frames ([`FrameList`]), linked through the array by frame number.
//!
//! A frame's state and links change under the PMM's lock alone, which orders them: they are
//! atomics so that the array is shared with whatever reads them, but need no ordering of their
//! own. Its staged mark, that a second stage of another process mapped it since it was allocated
//! (`pmm::quarantine`), is set by whoever holds the page, without the lock.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

/// No frame: the end of a list.
const NONE: u32 = u32::MAX;

/// A frame's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum State {
    /// Its page is allocated: its holder's.
    Allocated,
    /// Its page is free, and on no list: of a chunk no arena has, or going back.
    Free,
    /// Its page is free, on its arena's free list.
    Listed,
}

impl State {
    fn from_u8(state: u8) -> Self {
        match state {
            0 => Self::Allocated,
            1 => Self::Free,
            2 => Self::Listed,
            _ => unreachable!("a frame in state {state}"),
        }
    }
}

struct Frame {
    prev: AtomicU32,
    next: AtomicU32,
    state: AtomicU8,
    staged: AtomicBool,
}

/// The frames of physical memory.
pub(crate) struct Frames(Box<[Frame]>);

impl Frames {
    /// `count` frames, every one free, on no list.
    pub(crate) fn new(count: usize) -> Self {
        assert!(count < NONE as usize, "{count} frames, more than a frame number holds");
        let frame = || Frame {
            prev: AtomicU32::new(NONE),
            next: AtomicU32::new(NONE),
            state: AtomicU8::new(State::Free as u8),
            staged: AtomicBool::new(false),
        };
        Self((0..count).map(|_| frame()).collect())
    }

    /// How many frames there are.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    fn state(&self, frame: u32) -> State {
        State::from_u8(self.0[frame as usize].state.load(Ordering::Relaxed))
    }

    fn set_state(&self, frame: u32, state: State) {
        self.0[frame as usize].state.store(state as u8, Ordering::Relaxed);
    }

    /// Whether `frame`'s page is free, listed or not.
    pub(crate) fn is_free(&self, frame: u32) -> bool {
        self.state(frame) != State::Allocated
    }

    /// Whether `frame`'s page is free and on its arena's free list.
    pub(crate) fn is_listed(&self, frame: u32) -> bool {
        self.state(frame) == State::Listed
    }

    /// Allocates `frame`'s page, just taken off its arena's free list under the PMM's lock: from
    /// free to allocated, completing the transfer of its ownership from the PMM to its holder,
    /// under the same lock as being on the free list, or allocated, both mean the PMM owns it.
    pub(crate) fn allocate(&self, frame: u32) {
        debug_assert_eq!(self.state(frame), State::Free, "frame {frame} allocated, not free");
        self.set_state(frame, State::Allocated);
    }

    /// Frees `frame`'s page, which its holder gave up, under the PMM's lock, which puts it on its
    /// arena's free list before it is dropped: allocated again, it is no second stage's yet.
    pub(crate) fn free(&self, frame: u32) {
        debug_assert_eq!(self.state(frame), State::Allocated, "frame {frame} freed, not allocated");
        self.set_state(frame, State::Free);
        self.take_staged(frame);
    }

    /// A second stage of another process maps `frame`'s page: it may map it still as it is freed.
    pub(crate) fn stage(&self, frame: u32) {
        self.0[frame as usize].staged.store(true, Ordering::Relaxed);
    }

    /// Whether a second stage has mapped `frame`'s page since it was allocated, or since this was
    /// last asked; which it forgets.
    pub(crate) fn take_staged(&self, frame: u32) -> bool {
        self.0[frame as usize].staged.swap(false, Ordering::Relaxed)
    }

    fn prev(&self, frame: u32) -> u32 {
        self.0[frame as usize].prev.load(Ordering::Relaxed)
    }

    fn next(&self, frame: u32) -> u32 {
        self.0[frame as usize].next.load(Ordering::Relaxed)
    }

    fn set_prev(&self, frame: u32, prev: u32) {
        self.0[frame as usize].prev.store(prev, Ordering::Relaxed);
    }

    fn set_next(&self, frame: u32, next: u32) {
        self.0[frame as usize].next.store(next, Ordering::Relaxed);
    }
}

/// A list of free frames, linked through [`Frames`]: an arena's free list. A frame is on one list
/// at most, which it is listed on; each of the list's changes takes the frames it links.
pub(crate) struct FrameList {
    head: u32,
    tail: u32,
    len: usize,
}

impl FrameList {
    pub(crate) fn new() -> Self {
        Self { head: NONE, tail: NONE, len: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Lists `frame`, free and on no list, at the front.
    pub(crate) fn push_front(&mut self, frames: &Frames, frame: u32) {
        self.list(frames, frame);
        frames.set_prev(frame, NONE);
        frames.set_next(frame, self.head);
        match self.head {
            NONE => self.tail = frame,
            head => frames.set_prev(head, frame),
        }
        self.head = frame;
    }

    /// Lists `frame`, free and on no list, at the back.
    pub(crate) fn push_back(&mut self, frames: &Frames, frame: u32) {
        self.list(frames, frame);
        frames.set_next(frame, NONE);
        frames.set_prev(frame, self.tail);
        match self.tail {
            NONE => self.head = frame,
            tail => frames.set_next(tail, frame),
        }
        self.tail = frame;
    }

    /// Takes the frame at the front off the list, if any: free, on no list.
    pub(crate) fn pop_front(&mut self, frames: &Frames) -> Option<u32> {
        let frame = (self.head != NONE).then_some(self.head)?;
        self.erase(frames, frame);
        Some(frame)
    }

    /// Takes `frame`, which is on this list, off it: free, on no list.
    pub(crate) fn erase(&mut self, frames: &Frames, frame: u32) {
        assert_eq!(frames.state(frame), State::Listed, "frame {frame} erased, not listed");
        debug_assert!(self.len > 0, "frame {frame} erased from an empty list");
        let (prev, next) = (frames.prev(frame), frames.next(frame));
        match prev {
            NONE => self.head = next,
            prev => frames.set_next(prev, next),
        }
        match next {
            NONE => self.tail = prev,
            next => frames.set_prev(next, prev),
        }
        frames.set_prev(frame, NONE);
        frames.set_next(frame, NONE);
        frames.set_state(frame, State::Free);
        self.len -= 1;
    }

    /// The frames on the list, front to back.
    #[cfg(test)]
    pub(crate) fn iter<'a>(&self, frames: &'a Frames) -> impl Iterator<Item = u32> + 'a {
        let mut at = self.head;
        std::iter::from_fn(move || {
            let frame = (at != NONE).then_some(at)?;
            at = frames.next(frame);
            Some(frame)
        })
    }

    fn list(&mut self, frames: &Frames, frame: u32) {
        assert_eq!(frames.state(frame), State::Free, "frame {frame} listed, not free and unlisted");
        frames.set_state(frame, State::Listed);
        self.len += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(list: &FrameList, frames: &Frames) -> Vec<u32> {
        list.iter(frames).collect()
    }

    /// Frames pushed at either end, erased anywhere and popped from the front leave the list in
    /// order, and each frame listed exactly while it is on it.
    #[test]
    fn lists_keep_their_frames_in_order() {
        let frames = Frames::new(8);
        let mut list = FrameList::new();
        assert_eq!(list.pop_front(&frames), None);
        list.push_back(&frames, 3);
        list.push_back(&frames, 4);
        list.push_front(&frames, 2);
        list.push_back(&frames, 5);
        assert_eq!(listed(&list, &frames), [2, 3, 4, 5]);
        assert_eq!(list.len(), 4);
        assert!(frames.is_listed(4) && !frames.is_listed(1));
        list.erase(&frames, 4);
        assert!(!frames.is_listed(4) && frames.is_free(4));
        assert_eq!(listed(&list, &frames), [2, 3, 5]);
        list.erase(&frames, 2);
        list.erase(&frames, 5);
        assert_eq!(listed(&list, &frames), [3]);
        assert_eq!(list.pop_front(&frames), Some(3));
        assert!(list.is_empty());
        assert_eq!(listed(&list, &frames), [] as [u32; 0]);
        list.push_front(&frames, 7);
        assert_eq!(listed(&list, &frames), [7]);
    }

    /// A frame allocated is neither free nor listed; freed, it is free, and forgets its stage.
    #[test]
    fn frames_are_allocated_and_freed() {
        let frames = Frames::new(2);
        let mut list = FrameList::new();
        list.push_back(&frames, 1);
        assert_eq!(list.pop_front(&frames), Some(1));
        frames.allocate(1);
        assert!(!frames.is_free(1));
        frames.stage(1);
        frames.free(1);
        assert!(frames.is_free(1) && !frames.is_listed(1));
        assert!(!frames.take_staged(1), "freed, it is no stage's");
        frames.stage(0);
        assert!(frames.take_staged(0));
        assert!(!frames.take_staged(0), "taken, it is forgotten");
    }

    /// A frame is on one list at most.
    #[test]
    #[should_panic(expected = "frame 0 listed, not free and unlisted")]
    fn frames_are_listed_once() {
        let frames = Frames::new(1);
        let (mut first, mut second) = (FrameList::new(), FrameList::new());
        first.push_back(&frames, 0);
        second.push_back(&frames, 0);
    }
}

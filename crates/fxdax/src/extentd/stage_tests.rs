// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax (TASK-91.16, TASK-92.50): how a remote stage tells its VM process of the changes made to
//! its window, deferred or not, by the threads that make them.

use super::*;
use crate::kernel::{ArenaId, page_size};

/// A stage whose VM process is the test, which receives what it is told from the stage's
/// connection.
fn stage() -> (RemoteStage, Receiver) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    (RemoteStage::new(Arc::new(Sender::new(ours)), Arc::default()), Receiver::new(theirs))
}

/// The next `count` messages: their kinds, first two arguments and flags.
fn told(receiver: &mut Receiver, count: usize) -> Vec<(u32, u64, u64, u32)> {
    (0..count)
        .map(|_| {
            let header = receiver.receive().unwrap().unwrap().header;
            (header.kind, header.args[0], header.args[1], header.flags)
        })
        .collect()
}

fn phys(page: u64) -> PhysLocation {
    PhysLocation { arena: ArenaId::DEFAULT, offset: page * page_size() }
}

/// While a thread defers its waits, a page it unmaps and maps again read-only where it was, the
/// same page, has its protection changed alone; another page mapped there has the first unmapped
/// before; and a page mapped nothing at again is unmapped as the thread stops deferring, the last
/// change the deferral waits for.
#[test]
fn pages_unmapped_and_mapped_again_where_they_were_change_their_protection() {
    let (stage, mut receiver) = stage();
    let ps = page_size();
    stage.begin_deferring();
    stage.unmap_pages(0, 4 * ps, &[phys(10), phys(11), phys(12), phys(13)]);
    stage.map(0, phys(10), 2 * ps, Protection::Read);
    stage.map(2 * ps, phys(20), ps, Protection::Read);
    let deferred = stage.end_deferring();
    assert!(deferred.is_some(), "nothing deferred");
    assert_eq!(
        told(&mut receiver, 4),
        [
            (STAGE_PROTECT, 0, 2 * ps, STAGE_ACK),
            (STAGE_UNMAP, 2 * ps, ps, STAGE_ACK),
            (STAGE_MAP, 2 * ps, 20 * ps, 0),
            (STAGE_UNMAP, 3 * ps, ps, STAGE_ACK),
        ]
    );
    assert_eq!(stage.acks.state.lock().unwrap().last_taking, 4);
}

/// A page unmapped and mapped writable again where it was, the same page, is unmapped and mapped
/// anew, not reprotected: the VM process undoes a restriction of its memory to reads, which its
/// fault thread makes of a page a store found read-only, only as it maps the page anew
/// (`dax::uffd`), and its guest's stores would fault on it for good.
#[test]
fn pages_mapped_writable_again_are_mapped_anew() {
    let (stage, mut receiver) = stage();
    let ps = page_size();
    stage.begin_deferring();
    stage.unmap_pages(ps, ps, &[phys(11)]);
    stage.map(0, phys(10), 3 * ps, Protection::ReadWrite);
    assert!(stage.end_deferring().is_some(), "the unmap waited for");
    assert_eq!(
        told(&mut receiver, 2),
        [(STAGE_UNMAP, ps, ps, STAGE_ACK), (STAGE_MAP, 0, 10 * ps, 0)]
    );
}

/// A thread that defers nothing tells of pages it unmaps at once, and waits for them.
#[test]
fn pages_unmapped_without_deferring_are_unmapped_at_once() {
    let (stage, mut receiver) = stage();
    let ps = page_size();
    // Gone, the VM process is waited for no more.
    stage.acks.close();
    stage.unmap_pages(0, 2 * ps, &[phys(10), phys(11)]);
    assert_eq!(told(&mut receiver, 1), [(STAGE_UNMAP, 0, 2 * ps, STAGE_ACK)]);
}

/// A fault served on another thread that maps pages a deferring thread unmapped, before it maps
/// them again, has the VM process unmap them first, or reprotect them, the same page made
/// read-only; and the deferring thread then tells it of them no more. Told of the unmap after the
/// fault's map, the VM process would unmap the page the kernel maps, whose faults then find it
/// mapped and change nothing, for good.
#[test]
fn faults_mapping_pages_a_deferring_thread_unmapped_have_them_unmapped_first() {
    let (stage, mut receiver) = stage();
    let ps = page_size();
    stage.begin_deferring();
    stage.unmap_pages(0, 2 * ps, &[phys(10), phys(11)]);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            stage.map(0, phys(20), ps, Protection::Read);
            stage.map(ps, phys(11), ps, Protection::Read);
        });
    });
    assert!(stage.end_deferring().is_some(), "nothing deferred");
    // What comes next, past whatever the deferral told of.
    stage.window_mapped(9 * ps, ps);
    assert_eq!(
        told(&mut receiver, 4),
        [
            (STAGE_UNMAP, 0, ps, STAGE_ACK),
            (STAGE_MAP, 0, 20 * ps, 0),
            (STAGE_PROTECT, ps, ps, STAGE_ACK),
            (STAGE_MAPPED, 9 * ps, ps, 0),
        ]
    );
}

/// A deferral that deferred nothing of its own waits for what another thread deferred and the VM
/// process has yet to make: a commit that seals an open extent a `MAP` stopped mapping, its unmap
/// deferred, has another store share its pages, which the guest may still store to, only once it
/// is made.
#[test]
fn deferrals_of_nothing_wait_for_others_deferred() {
    let (stage, mut receiver) = stage();
    let ps = page_size();
    std::thread::scope(|threads| {
        threads.spawn(|| {
            stage.begin_deferring();
            stage.unmap_pages(0, ps, &[phys(10)]);
            assert!(stage.end_deferring().is_some(), "the unmap waited for");
        });
    });
    assert_eq!(told(&mut receiver, 1), [(STAGE_UNMAP, 0, ps, STAGE_ACK)]);
    stage.begin_deferring();
    let deferral = stage.end_deferring().expect("the other thread's unmap not waited for");
    let kept = Arc::new(());
    deferral.keep_until_made(Box::new(kept.clone()));
    assert_eq!(Arc::strong_count(&kept), 2, "kept nothing until the unmap is made");
    stage.acks.acked(1);
    assert_eq!(Arc::strong_count(&kept), 1, "kept once the unmap was made");
}

/// Stage operations sent by threads side by side reach the VM process in the order of their IDs,
/// as an acknowledgement says that those before it were made: one sent ahead of an earlier ID
/// would have its acknowledgement say so of a change still to be made, and what waits for that
/// change go on.
#[test]
fn stage_operations_reach_the_vm_process_in_the_order_of_their_ids() {
    const THREADS: usize = 8;
    const EACH: usize = 2000;
    let (stage, mut receiver) = stage();
    let ps = page_size();
    let ids = std::thread::spawn(move || {
        (0..THREADS * EACH)
            .map(|_| receiver.receive().unwrap().unwrap().header.id)
            .collect::<Vec<u64>>()
    });
    std::thread::scope(|threads| {
        for thread in 0..THREADS as u64 {
            let stage = &stage;
            threads.spawn(move || {
                for _ in 0..EACH {
                    stage.window_mapped(thread * ps, ps);
                }
            });
        }
    });
    let ids = ids.join().unwrap();
    let out_of_order: Vec<_> = ids.windows(2).filter(|pair| pair[1] < pair[0]).collect();
    assert_eq!(out_of_order, Vec::<&[u64]>::new(), "IDs out of order");
}

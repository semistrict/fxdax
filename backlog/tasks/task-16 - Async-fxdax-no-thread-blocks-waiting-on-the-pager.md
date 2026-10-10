---
id: TASK-16
title: 'Async fxdax: no thread blocks waiting on the pager'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 00:38'
updated_date: '2026-10-10 14:44'
labels: []
dependencies: []
type: feature
ordinal: 16000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every fxdax operation blocks its calling thread: file contents go through zx stream calls that wait on a condvar in the in-process kernel port until Fxfs's pager supplies or dirties pages, and the rest blocks on a oneshot while Fxfs's executor runs it. A VMM therefore holds one thread per request in flight, and on Linux one userfaultfd thread resolves the guest's DAX faults one at a time, each waiting on disk I/O. The "kernel" is our own library (crates/zircon), so its waits can be futures. The user asked for fxdax to be completely async. Do not commit (user instruction).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 No fxdax future blocks a thread: page requests, page allocation and stream size ordering waits in the kernel port are awaited, shown by a kernel test where a read stays pending until a test pager supplies the page, on the same thread
- [x] #2 The Linux DAX fault thread resolves faults concurrently, shown by a test
- [x] #3 The kernel port's synchronous paths are unchanged in behaviour, and the async paths share their loops rather than copying them
- [x] #4 libkrun builds against the async API and its virtiofs-fxdax and virtiofs-fxdax-nodax guest tests pass on macOS
- [x] #5 Every fxdax operation that can wait, including mount, format, close, check and DAX faults, is an async fn whose future is Send and works on any executor; operations that never wait (forget, readdir, releasedir, window creation, remove_mapping) are plain functions
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS
- [ ] #5 Deviations from Zircon documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Kernel Event: wakers alongside the condvar; wait_async with deadlines, served by one shared async timer thread.
2. A notifier for StreamSizeManager (condvar + wakers, generation-checked); Operation uses atomics so its futures are Send; async begin_*_locked waits.
3. Page requests: PageProvider::wait_on_event_async (PagerProxy keeps overtime reporting and timeout), PageSource/PageRequest/MultiPageRequest wait_async, AnonymousPageRequest::allocate_async over Pmm::wait_for_single_page_allocation_async.
4. VmObjectPaged: split read_write_internal and zero_range into a step that returns Done/Wait/Fault, driven by the existing sync loop and a new async loop; read/write_user_vector(_progress)_async. User pointers become Send.
5. StreamDispatcher read_vector_at_async / write_vector_at_async; GuestPhysicalAspace::page_fault_async; async soft faults for copies into kernel-managed memory.
6. Public async entry points for streams by handle (host iovecs).
7. fxdax: Executor::run async (await a oneshot); every Filesystem/Options/Storage/DaxWindow operation async; Drop of a mounted filesystem blocks on unmount (documented); uffd fault thread drives fault futures concurrently, woken through an eventfd.
8. Tests: kernel pending-read test with a test pager; fxdax tests async; concurrent DAX fault test; existing suites.
9. libkrun adapter and guest test call through futures::executor::block_on; macOS e2e.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Kernel: Event::wait_async with deadlines from one wakeup thread (kernel/timer.rs wake_at); kernel/wakers.rs; StreamSizeManager futures (begin_write, until_head; Operation fields atomics; abandon on drop); PageProvider::wait_on_event_async (PagerProxy keeps overtime reporting); PageRequest/MultiPageRequest wait_async; AnonymousPageRequest::allocate_async over Pmm::wait_for_single_page_allocation_async; VmObjectPaged read_write_step / zero_range_step shared by sync and async drivers; VmAspace page_fault_step + soft_fault_in_range_async; GuestPhysicalAspace::page_fault_async; StreamDispatcher read/write_vectored_at_async; syscalls stream_readv/writev_at_async over HostBuffer. Tests: zircon-core-tests async_stream (6, incl. pending-until-supplied on one thread, concurrent reads, abandon), kernel event/timer/wakers and size manager tests. fxdax: Executor::run async (OnExecutor thread-local replaces the tokio check; executor drop no longer joins), run_blocking for Drop; every waiting op async; unmount keeps storage for close, cancellation-safe; uffd thread polls fault futures woken through an eventfd. tokio is a dev-dependency only. Tests: dax::tests::faults_wait_concurrently, uffd::tests::faults_are_resolved_concurrently (Lima), operations_run_on_tokio_runtimes (replaced stream_io_on_an_executor_thread_panics, whose restriction is gone), concurrent_operations_on_one_thread, futures_are_send. Validation: macOS zircon+core+fxfs-platform 594 pass; Lima fxdax+zircon+core 498 pass.

Validation: macOS scripts/test.sh 1800 passed (4 skipped); Lima scripts/test-linux.sh 1803 passed (4 skipped); macOS and Linux (KVM in Lima) virtiofs-fxdax and virtiofs-fxdax-nodax pass. Caveats: dropping a mounted Filesystem blocks (documented; await close); closing a filesystem with a host-memory DAX window joins the fault thread, which exits at once. Not committed, at the user's instruction.

Pre-commit review fixes: StreamSizeManager's guard is now a type that wakes waiting futures after releasing the lock (dequeuing only marks them), with StreamSizeGuard::unlocked for callers that release the lock mid-operation; a misplaced doc comment in stream_dispatcher.rs fixed. Deferred: TASK-23 (timer heap entries of dropped wakeups, targeted wakeups).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax is async end to end. The in-process kernel port waits with futures where Zircon threads block (page requests, page allocations, stream size ordering, faults), each async path running the same steps as its synchronous counterpart between waits, with Zircon deviations documented. Every fxdax operation that can wait is an async fn with a Send future usable on any executor; Fxfs runs on its own executor behind a oneshot; the Linux userfaultfd thread resolves guest faults concurrently. Verified by kernel tests that hold reads pending until a test pager supplies pages on one thread, concurrent DAX fault tests on macOS and Linux, the full suites on macOS (1800) and Linux (1803), and the libkrun guest tests on both.
<!-- SECTION:FINAL_SUMMARY:END -->

// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stream reads and writes as futures (`zircon::syscalls::stream::stream_readv_at_async` and
//! `stream_writev_at_async`), which wait for the pager without blocking the thread polling them.
//!
//! These are not ported: the futures are a deviation of the port's. Each test serves the pager on
//! the same thread that polls the futures, which a blocking read or write would deadlock.

use futures::FutureExt;
use futures::task::ArcWake;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use zircon::syscalls::{stream_readv_at_async, stream_writev_at_async};
use zircon::user_copy::HostBuffer;
use zircon_core_tests::userpager::page_size;

const KEY: u64 = 7;

/// The host buffer of `buf`, which must outlive its use.
fn buffer(buf: &mut [u8]) -> HostBuffer {
    HostBuffer { addr: buf.as_mut_ptr() as usize, len: buf.len() }
}

/// The host buffer of `data`, which is only read.
fn data(data: &[u8]) -> HostBuffer {
    HostBuffer { addr: data.as_ptr() as usize, len: data.len() }
}

/// A waker that counts its wakes.
#[derive(Default)]
struct CountingWaker(AtomicUsize);

impl ArcWake for CountingWaker {
    fn wake_by_ref(this: &Arc<Self>) {
        this.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CountingWaker {
    fn wakes(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// A pager-backed VMO of `pages` pages, its pager and port, and a stream on it.
struct Paged {
    port: zx::Port,
    pager: zx::Pager,
    vmo: zx::Vmo,
    stream: zx::Stream,
}

impl Paged {
    fn new(pages: u64, options: zx::VmoOptions) -> Self {
        let port = zx::Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        let vmo = pager.create_vmo(options, &port, KEY, pages * page_size()).unwrap();
        let stream = zx::Stream::create(
            zx::StreamOptions::MODE_READ | zx::StreamOptions::MODE_WRITE,
            &vmo,
            0,
        )
        .unwrap();
        Self { port, pager, vmo, stream }
    }

    /// The page request queued on the port, which there must be.
    fn request(&self) -> zx::PagerPacket {
        let packet = self.port.wait(zx::MonotonicInstant::INFINITE_PAST).expect("no page request");
        assert_eq!(packet.key(), KEY);
        match packet.contents() {
            zx::PacketContents::Pager(request) => request,
            packet => panic!("Unexpected packet: {packet:?}"),
        }
    }

    fn no_request(&self) {
        assert_eq!(
            self.port.wait(zx::MonotonicInstant::INFINITE_PAST).map(|_| ()),
            Err(zx::Status::TIMED_OUT)
        );
    }

    /// Supplies `range` of the VMO with bytes that are each their page's index plus `base`.
    fn supply(&self, range: std::ops::Range<u64>, base: u8) {
        let aux = zx::Vmo::create(range.end - range.start).unwrap();
        for (i, offset) in range.clone().step_by(page_size() as usize).enumerate() {
            let page = vec![base + (offset / page_size()) as u8; page_size() as usize];
            aux.write(&page, i as u64 * page_size()).unwrap();
        }
        self.pager.supply_pages(&self.vmo, range, &aux, 0).unwrap();
    }
}

/// A read of a page not yet in memory stays pending, without blocking, until the pager supplies
/// it, which wakes the reader.
#[test]
fn read_waits_for_the_pager_without_blocking() {
    let paged = Paged::new(2, zx::VmoOptions::empty());
    let waker = Arc::new(CountingWaker::default());
    let task_waker = futures::task::waker(waker.clone());
    let mut cx = Context::from_waker(&task_waker);

    let mut buf = vec![0u8; page_size() as usize + 10];
    let (first, second) = buf.split_at_mut(page_size() as usize);
    let bufs = [buffer(first), buffer(second)];
    let handle = paged.stream.raw_handle();
    let mut read = Box::pin(stream_readv_at_async(handle, 0, &bufs));
    assert_eq!(waker.wakes(), 0);

    // Each buffer is read in turn, as the synchronous read does, so each asks for its page.
    for page in 0..2 {
        assert!(read.poll_unpin(&mut cx).is_pending());
        let request = paged.request();
        assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_READ);
        assert_eq!(request.range(), page * page_size()..(page + 1) * page_size());
        let wakes = waker.wakes();
        paged.supply(request.range(), 10);
        assert!(waker.wakes() > wakes, "supplying the page did not wake the reader");
    }
    assert_eq!(read.poll_unpin(&mut cx), Poll::Ready(Ok(page_size() as usize + 10)));
    drop(read);
    assert!(buf[..page_size() as usize].iter().all(|&byte| byte == 10));
    assert!(buf[page_size() as usize..].iter().all(|&byte| byte == 11));
}

/// Reads of different pages wait together, on one thread, and each completes once its page is
/// supplied, in whatever order the pager supplies them.
#[test]
fn reads_wait_concurrently() {
    const READS: u64 = 4;
    let paged = Paged::new(READS, zx::VmoOptions::empty());
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    let handle = paged.stream.raw_handle();

    let mut bufs: Vec<[u8; 1]> = vec![[0]; READS as usize];
    let slices: Vec<[HostBuffer; 1]> = bufs.iter_mut().map(|buf| [buffer(buf)]).collect();
    let mut reads: Vec<_> = slices
        .iter()
        .enumerate()
        .map(|(page, bufs)| {
            Box::pin(stream_readv_at_async(handle, page as u64 * page_size(), bufs))
        })
        .collect();
    for read in &mut reads {
        assert!(read.poll_unpin(&mut cx).is_pending());
    }
    let mut requests: Vec<_> = (0..READS).map(|_| paged.request().range()).collect();
    paged.no_request();
    requests.sort_by_key(|range| range.start);
    assert_eq!(
        requests,
        (0..READS).map(|page| page * page_size()..(page + 1) * page_size()).collect::<Vec<_>>()
    );

    for page in (0..READS).rev() {
        paged.supply(page * page_size()..(page + 1) * page_size(), 20);
        assert_eq!(reads[page as usize].poll_unpin(&mut cx), Poll::Ready(Ok(1)));
        for earlier in 0..page {
            assert!(reads[earlier as usize].poll_unpin(&mut cx).is_pending());
        }
    }
    drop(reads);
    drop(slices);
    assert_eq!(bufs, (0..READS).map(|page| [20 + page as u8]).collect::<Vec<_>>());
}

/// With dirty transitions trapped, a write to a clean page stays pending until the pager dirties
/// it, without blocking.
#[test]
fn write_waits_for_the_pager_to_dirty_without_blocking() {
    let paged = Paged::new(1, zx::VmoOptions::TRAP_DIRTY);
    paged.supply(0..page_size(), 0);
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    let handle = paged.stream.raw_handle();

    let bufs = [data(b"hello")];
    let mut write = Box::pin(stream_writev_at_async(handle, 3, &bufs));
    assert!(write.poll_unpin(&mut cx).is_pending());
    let request = paged.request();
    assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
    assert_eq!(request.range(), 0..page_size());

    paged.pager.op_range(zx::PagerOp::Dirty, &paged.vmo, request.range()).unwrap();
    assert_eq!(write.poll_unpin(&mut cx), Poll::Ready(Ok(5)));
    let mut buf = [0u8; 8];
    paged.vmo.read(&mut buf, 0).unwrap();
    assert_eq!(&buf, b"\0\0\0hello");
}

/// A write that expands the stream completes, and moves the content size, as the synchronous
/// write does.
#[test]
fn expanding_write_moves_the_content_size() {
    let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 0).unwrap();
    vmo.set_size(4 * page_size()).unwrap();
    vmo.set_stream_size(10).unwrap();
    let stream = zx::Stream::create(zx::StreamOptions::MODE_WRITE, &vmo, 0).unwrap();
    let bufs = [data(b"end")];
    let written =
        futures::executor::block_on(stream_writev_at_async(stream.raw_handle(), 100, &bufs));
    assert_eq!(written, Ok(3));
    assert_eq!(vmo.get_stream_size().unwrap(), 103);
    let mut buf = [0xffu8; 103];
    vmo.read(&mut buf, 0).unwrap();
    assert!(buf[10..100].iter().all(|&byte| byte == 0), "the gap was not zeroed");
    assert_eq!(&buf[100..], b"end");
}

/// Dropping a read before the pager supplies its pages leaves the stream as it was: the next
/// operations neither wait for it nor see it.
#[test]
fn dropped_read_is_abandoned() {
    let paged = Paged::new(1, zx::VmoOptions::empty());
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    let handle = paged.stream.raw_handle();
    let mut buf = [0u8; 1];
    {
        let bufs = [buffer(&mut buf)];
        let mut read = Box::pin(stream_readv_at_async(handle, 0, &bufs));
        assert!(read.poll_unpin(&mut cx).is_pending());
    }

    // An expanding write waits for every operation ahead of it, so it would wait for the read
    // forever if the read were still queued.
    paged.supply(0..page_size(), 30);
    paged.vmo.set_stream_size(1).unwrap();
    let bufs = [data(b"x")];
    let mut write = Box::pin(stream_writev_at_async(handle, 1, &bufs));
    assert_eq!(write.poll_unpin(&mut cx), Poll::Ready(Ok(1)));
    assert_eq!(paged.vmo.get_stream_size().unwrap(), 2);
}

/// The futures are woken from the pager's thread, so an executor on another thread runs them to
/// completion while the pager serves them.
#[test]
fn read_completes_on_an_executor_while_another_thread_serves_the_pager() {
    let paged = Arc::new(Paged::new(1, zx::VmoOptions::empty()));
    let pager = std::thread::spawn({
        let paged = paged.clone();
        move || {
            let packet = paged.port.wait(zx::MonotonicInstant::INFINITE).unwrap();
            let zx::PacketContents::Pager(request) = packet.contents() else {
                panic!("Unexpected packet");
            };
            paged.supply(request.range(), 40);
        }
    });
    let mut buf = [0u8; 4];
    let bufs = [buffer(&mut buf)];
    let read =
        futures::executor::block_on(stream_readv_at_async(paged.stream.raw_handle(), 0, &bufs));
    assert_eq!(read, Ok(4));
    assert_eq!(buf, [40; 4]);
    pager.join().unwrap();
}

// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Tests of behaviour only the Zircon kernel port had, set aside verbatim as it is deleted
// (TASK-96.1), for the user to decide on: each, where it came from, and why it no longer applies.
// Not compiled.

// From crates/fxdax/src/dax/uffd.rs: stores to clean pages of a pager-backed VMO that tracks dirty
// pages (TRAP_DIRTY) wait for the pager to dirty them. Windows map only extents now, whose open
// pages are always writable and whose sealed pages never are: nothing tracks dirty pages.

    /// Stores to pages mapped read-only, of a file whose pager tracks its dirty pages, fault as
    /// stores, which the window resolves once the pager dirties the pages, and the stores land in
    /// the file. A store following on from the last dirties the file's pages after it too.
    #[test]
    fn stores_to_clean_pages_wait_for_the_pager_to_dirty_them() {
        let ps = page_size() as usize;
        // SAFETY: Maps anonymous memory, which this test owns.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4 * ps,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        let base = memory as usize;
        let port = zx::Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, 7, 4 * ps as u64).unwrap();
        let aux = zx::Vmo::create(4 * ps as u64).unwrap();
        aux.write(&vec![5; 4 * ps], 0).unwrap();
        pager.supply_pages(&vmo, 0..4 * ps as u64, &aux, 0).unwrap();
        // SAFETY: Only the test uses the memory, which outlives the window.
        let stage = unsafe { UffdSecondStage::in_process(base, 4 * ps as u64, None) }.unwrap();
        let (dax, _window) = Dax::<()>::in_host_memory(stage).unwrap();
        dax.window.map(0, &vmo, 0, 4 * ps as u64, true).unwrap();

        // SAFETY: The memory is the window's, whose faults the fault thread serves.
        let load = move |page: usize| unsafe { ((base + page * ps) as *const u8).read_volatile() };
        let store = move |page: usize| {
            // SAFETY: As for loads.
            std::thread::spawn(move || unsafe { ((base + page * ps) as *mut u8).write_volatile(9) })
        };
        let dirtied = || {
            let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
            let zx::PacketContents::Pager(request) = packet.contents() else { panic!() };
            assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
            pager.op_range(zx::PagerOp::Dirty, &vmo, request.range()).unwrap();
            request.range()
        };
        assert_eq!((load(0), load(1)), (5, 5));
        let first = store(0);
        assert_eq!(dirtied(), 0..ps as u64);
        first.join().unwrap();
        let second = store(1);
        assert_eq!(dirtied(), ps as u64..4 * ps as u64);
        second.join().unwrap();
        let mut bytes = [0; 2];
        vmo.read(&mut bytes[..1], 0).unwrap();
        vmo.read(&mut bytes[1..], ps as u64).unwrap();
        assert_eq!(bytes, [9, 9]);

        drop(dax);
        // SAFETY: Unmaps the memory, which nothing uses any more.
        assert_eq!(unsafe { libc::munmap(memory, 4 * ps) }, 0);
    }


// From crates/fxdax/src/dax.rs: the Zircon MMU's protections (PROT_*) translated to the second
// stage's. The window gives the second stage its protections itself now; no translation is left.

    /// The kernel's protections are the second stage's.
    #[test]
    fn protections_are_the_kernels() {
        let protection = KernelSecondStage::protection;
        assert_eq!(protection(libc::PROT_NONE), Protection::None);
        assert_eq!(protection(libc::PROT_READ), Protection::Read);
        assert_eq!(protection(libc::PROT_READ | libc::PROT_WRITE), Protection::ReadWrite);
    }


// From crates/fxdax/src/dax.rs: as the uffd test above, for a window whose second stage records
// its pages: dirty tracking of pager-backed VMOs, which no source has.

    /// A store to a clean page of a file whose pager tracks its dirty pages maps it writable once
    /// the pager dirties it; writeback takes the write access away again.
    #[test]
    fn stores_dirty_pages_and_writeback_cleans_them() {
        let ps = page_size();
        let port = zx::Port::create();
        let pager = zx::Pager::create(zx::PagerOptions::empty()).unwrap();
        let vmo = pager.create_vmo(zx::VmoOptions::TRAP_DIRTY, &port, KEY, ps).unwrap();
        let aux = zx::Vmo::create(ps).unwrap();
        pager.supply_pages(&vmo, 0..ps, &aux, 0).unwrap();
        let stage = Arc::new(Recorded::default());
        let (mut dax, window) = Dax::<()>::new(ps, Some(stage.clone())).unwrap();
        dax.map(0, (), &vmo, 0, ps, true).unwrap();
        futures::executor::block_on(window.fault(0, Access::Read)).unwrap();
        assert_eq!(stage.pages(), [(0, Protection::Read)]);

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let mut store = Box::pin(window.fault(0, Access::Write));
        assert!(store.poll_unpin(&mut cx).is_pending());
        let packet = port.wait(zx::MonotonicInstant::INFINITE_PAST).unwrap();
        let zx::PacketContents::Pager(request) = packet.contents() else { panic!("{packet:?}") };
        assert_eq!(request.command(), zx::sys::zx_page_request_command_t::ZX_PAGER_VMO_DIRTY);
        pager.op_range(zx::PagerOp::Dirty, &vmo, request.range()).unwrap();
        assert_eq!(store.poll_unpin(&mut cx), Poll::Ready(Ok(())));
        assert_eq!(stage.pages(), [(0, Protection::ReadWrite)]);

        let begin = zx::PagerOp::WritebackBegin(zx::PagerWritebackBeginOptions::empty());
        pager.op_range(begin, &vmo, 0..ps).unwrap();
        assert_eq!(stage.pages(), [(0, Protection::Read)]);
    }


// From crates/fxdax/tests/kernel.rs: the kernel's memory watchdog's thresholds (warning, critical,
// out of memory), which scaled with the memory and had to decrease below it. The watchdog's
// pressure levels drove host Fxfs's writeback and Zircon's evictor, both gone: physical memory is
// configured by its size alone (crate::pages::Config). `configure_before_the_kernel_starts` also
// checked the thresholds it set; its size check is kept in tests/pages.rs.

#[test]
fn memory_scales_the_default_thresholds() {
    assert_eq!(kernel::Config::with_memory(1024 * MIB), kernel::Config::default());
    assert_eq!(
        kernel::Config::with_memory(4096 * MIB),
        kernel::Config {
            memory_bytes: 4096 * MIB,
            warning_threshold_bytes: 1200 * MIB,
            critical_threshold_bytes: 600 * MIB,
            out_of_memory_threshold_bytes: 200 * MIB,
        }
    );
    kernel::configure(kernel::Config::with_memory(4096 * MIB)).expect("configure failed");
    assert_eq!(BootOptions::get().memory_bytes, 4096 * MIB);
    // Once.
    let error = kernel::configure(kernel::Config::default()).expect_err("configured");
    assert_eq!(error.errno(), Some(Errno::EBUSY));
}

#[test]
fn too_little_memory_for_distinct_thresholds_fails() {
    // 300, 150 and 50 MiB a GiB of 4 MiB are all 0 MiB.
    let error = kernel::configure(kernel::Config::with_memory(4 * MIB)).expect_err("configured");
    assert_eq!(error.errno(), Some(Errno::EINVAL));
}

#[test]
fn thresholds_must_decrease_below_the_memory() {
    let default = kernel::Config::default();
    assert_eq!(default.memory_bytes, 1024 * MIB);
    for config in [
        kernel::Config { critical_threshold_bytes: default.warning_threshold_bytes, ..default },
        kernel::Config { out_of_memory_threshold_bytes: 200 * MIB, ..default },
        kernel::Config { memory_bytes: default.warning_threshold_bytes, ..default },
    ] {
        let error = kernel::configure(config).expect_err("configured");
        assert_eq!(error.to_string(), "memory thresholds must decrease, and be below the memory");
    }
    kernel::configure(default).expect("configure failed");
}


// From crates/fxdax/src/extents.rs: caching an extent fails with ENOMEM once the kernel's handles
// run out (TASK-104, TASK-105), each cached extent's VMO, and each arena's pager buffers, a handle
// of the kernel's. Cached extents hold no handles now: there is nothing to run out.

    /// TASK-104: with the kernel's handles run out, caching an extent fails with ENOMEM, the cache
    /// having let go of what it could and tried again, rather than panicking in the request that
    /// asked, which poisoned its store; once handles are free, it caches. TASK-105: the process's
    /// first extent, whose VMO's page requests come to a port made with the cache, not with it.
    #[test]
    fn caching_fails_when_handles_run_out() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        write(&store, id(&store, 1), 0, &pattern(1, ps as usize)).unwrap();
        store.commit(None).unwrap();
        // The cache, and its arena's buffers, made before the handles run out; no extent cached
        // that it could let go of.
        drop(extent_cache().allocator(ArenaId::DEFAULT).unwrap());
        let mut held = Vec::new();
        while let Ok(vmo) = zx::Vmo::create(0) {
            held.push(vmo);
        }
        let cached = |store: &ExtentStore| {
            super::cached(&store.store, ArenaId::DEFAULT, None, None, id(store, 1), ps).map(drop)
        };
        assert_eq!(cached(&store), Err(Errno::ENOMEM));
        drop(held);
        assert_eq!(cached(&store), Ok(()));
    }

    /// TASK-105: as TASK-104, for the first extent of an arena, which makes the arena's buffers
    /// too, and with nothing cached to let go of.
    #[test]
    fn an_arenas_first_caching_fails_when_handles_run_out() {
        let (store, _dir) = store(1 << 20);
        let ps = page_size();
        write(&store, id(&store, 1), 0, &pattern(1, ps as usize)).unwrap();
        store.commit(None).unwrap();
        // The cache, made as the process's first store is, before the handles run out.
        extent_cache();
        let mut held = Vec::new();
        while let Ok(vmo) = zx::Vmo::create(0) {
            held.push(vmo);
        }
        let cached = |store: &ExtentStore| {
            super::cached(&store.store, ArenaId::DEFAULT, None, None, id(store, 1), ps).map(drop)
        };
        assert_eq!(cached(&store), Err(Errno::ENOMEM));
        drop(held);
        assert_eq!(cached(&store), Ok(()));
    }


// From crates/fxdax/tests/filesystem.rs (TASK-79.75): a host Fxfs write fails with ENOMEM once the
// kernel's memory is full of pages nothing evicts. Host Fxfs keeps no pages in memory now.

/// TASK-79.75: a write that takes pages of the kernel's, whose memory is full of pages nothing
/// evicts (anonymous ones), fails with ENOMEM. The DIRTY request for its page, which Fxfs's
/// response failed for want of memory, was resolved for the write to ask again, which it did for
/// ever.
#[test]
fn writes_fail_with_enomem_with_memory_full_of_pages_nothing_evicts() {
    pages::configure(pages::Config::with_memory(64 << 20)).expect("configure failed");
    with_fs(async |fs| {
        let (file, _) = fs.root().create(b"file", 0o644, OWNER).await.unwrap();
        let len = zircon::vm::pmm::count_free_pages() * pages::page_size();
        let anonymous = zx::Vmo::create(len).unwrap();
        anonymous.op_range(zx::VmoOp::COMMIT, 0, len).unwrap();
        assert_eq!(zircon::vm::pmm::count_free_pages(), 0);
        assert_eq!(errno(file.write(0, b"hello, world").await), Errno::ENOMEM);
    })
}

// crates/fxdax/tests/shared_pages.rs, whole, is beside this file (shared_pages.rs): a volume and
// its snapshot share the kernel's pages, by the page queues' counts, and each reads its own
// contents. Host Fxfs shares no pages now; the contents' isolation could stay as a test of its own.

// From crates/fxdax/src/storage/image.rs: an image's I/O into transfer buffers of the kernel's
// memory fails with EFAULT on pages its page scanner made inaccessible, or not yet written, and
// the device faults them in and tries again. Transfer buffers are plain memory now, which the
// host's I/O faults in itself: the device no longer retries.

    /// The kernel's page scanner makes transfer buffers' pages inaccessible to track their
    /// accesses, which the host's I/O into them fails on; the device's I/O faults them back in.
    #[test]
    fn io_into_harvested_buffers() {
        let (device, _, _dir) = device(4 * MIB as u64);
        block_on(async {
            write(&device, extent(1), 0, &pattern(0, MIB)).await;
            let mut buffer = device.allocate_buffer(MIB).await;
            let harvest = || {
                VmAspace::harvest_all_user_accessed_bits(
                    NonTerminalAction::Retain,
                    TerminalAction::UpdateAgeAndHarvest,
                )
            };

            harvest();
            let [Part::Data { file, offset, .. }] =
                &device.store.locate(extent(1), 0, MIB as u64).unwrap()[..]
            else {
                panic!("extent 1 isn't in one piece");
            };
            // SAFETY: Reads into the buffer, which is the test's.
            let read = unsafe {
                libc::pread(
                    file.as_raw_fd(),
                    buffer.as_mut().as_mut_ptr().cast(),
                    MIB,
                    *offset as libc::off_t,
                )
            };
            assert_eq!(read, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

            harvest();
            device.read(extent(1), 0, buffer.as_mut()).await.unwrap();
            assert!(contents(buffer.as_ref()) == pattern(0, MIB));
            harvest();
            device.write(extent(2), 0, buffer.as_ref()).await.unwrap();
            let mut copy = vec![0; MIB];
            device.store.read(extent(2), 0, &mut copy).unwrap();
            assert!(copy == pattern(0, MIB));
        });
    }

    /// A fresh device's first reads land in buffers the kernel maps read-only, as the zero page,
    /// until they are written; the host's reads into them fault them in.
    #[test]
    fn first_reads_into_unwritten_buffers() {
        let (device, image, _dir) = device(4 * MIB as u64);
        // Through the store, as the device's buffers are what is being tested.
        device.store.write(extent(1), 0, &pattern(0, MIB)).unwrap();
        block_on(device.flush()).unwrap();
        drop(device);
        let device = ImageDevice::open(&image, Default::default()).unwrap();
        assert!(block_on(read(&device, extent(1), 0, MIB)) == pattern(0, MIB));
    }

// From crates/fxdax/src/error.rs: Zircon's statuses translated to errnos as fdio does, for the
// in-process kernel's and fxfs-platform's results. Nothing in fxdax gives a zx::Status now, and
// `Errno::of_status` went with it.

    /// Statuses are the errnos fdio gives them.
    #[test]
    fn statuses_are_fdios_errnos() {
        use zx::Status;
        let table = [
            (Status::NOT_FOUND, Errno::ENOENT),
            (Status::NO_MEMORY, Errno::ENOMEM),
            (Status::INVALID_ARGS, Errno::EINVAL),
            (Status::BUFFER_TOO_SMALL, Errno::EINVAL),
            (Status::TIMED_OUT, Errno::ETIMEDOUT),
            (Status::UNAVAILABLE, Errno::EBUSY),
            (Status::ALREADY_EXISTS, Errno::EEXIST),
            (Status::PEER_CLOSED, Errno::EPIPE),
            (Status::BAD_STATE, Errno::EPIPE),
            (Status::BAD_PATH, Errno::ENAMETOOLONG),
            (Status::IO, Errno::EIO),
            (Status::NOT_FILE, Errno::EISDIR),
            (Status::NOT_DIR, Errno::ENOTDIR),
            (Status::NOT_SUPPORTED, Errno::ENOTSUP),
            (Status::WRONG_TYPE, Errno::ENOTSUP),
            (Status::OUT_OF_RANGE, Errno::EINVAL),
            (Status::NO_RESOURCES, Errno::ENOMEM),
            (Status::BAD_HANDLE, Errno::EBADF),
            (Status::ACCESS_DENIED, Errno::EACCES),
            (Status::SHOULD_WAIT, Errno::EAGAIN),
            (Status::FILE_BIG, Errno::EFBIG),
            (Status::NO_SPACE, Errno::ENOSPC),
            (Status::NOT_EMPTY, Errno::ENOTEMPTY),
            (Status::IO_REFUSED, Errno::ECONNREFUSED),
            (Status::IO_INVALID, Errno::EIO),
            (Status::CANCELED, Errno::EBADF),
            (Status::PROTOCOL_NOT_SUPPORTED, Errno::EPROTONOSUPPORT),
            (Status::ADDRESS_UNREACHABLE, Errno::ENETUNREACH),
            (Status::ADDRESS_IN_USE, Errno::EADDRINUSE),
            (Status::NOT_CONNECTED, Errno::ENOTCONN),
            (Status::CONNECTION_REFUSED, Errno::ECONNREFUSED),
            (Status::CONNECTION_RESET, Errno::ECONNRESET),
            (Status::CONNECTION_ABORTED, Errno::ECONNABORTED),
            (Status::INTERNAL, Errno::EIO),
        ];
        for (status, errno) in table {
            assert_eq!(Errno::of_status(status), errno, "{status}");
        }
    }

// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An Fxfs device on an image: an extent store in a directory (crates/extent_store, backlog
//! doc-4), whose extents are packed into segments, or are files of their own if large, with an
//! index of them.
//!
//! It replaces Fuchsia's `FileBackedDevice`, which is for host tools and tests:
//!
//! - Extents' I/O bypasses the host's page cache where the host filesystem allows (`O_DIRECT` on
//!   Linux, `F_NOCACHE` on macOS), as the in-process kernel caches files' pages already.
//! - Extents' I/O runs off Fxfs's executor threads (see `io`), which only await it, as do commits,
//!   which sync the image, and cleaning, which runs after a commit once segments hold much freed
//!   space.
//! - Flushing commits the store: a barrier, durable, which seals the extents written before it.
//!
//! Transfer buffers are memory of the in-process kernel's, which the host's I/O cannot fault in:
//! it fails with `EFAULT` on a page the kernel's page scanner made inaccessible to track its
//! accesses, and, for a read, on a page not yet written, which the kernel maps read-only, as the
//! zero page. An operation that fails so touches its buffer's pages as it will access them, which
//! faults them in, and is tried again.

use super::gate::WriteGate;
use super::io::{Io, Op};
use anyhow::{Context, Error, bail};
use async_trait::async_trait;
use extent_store::{Aligned, Description, HostDir, Options, Part, Store, create_host, open_host};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId};
use storage_device::{ReadOptions, WriteFlags, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 32 * 1024 * 1024;

/// How many times an operation is tried again after faulting its buffer's pages in.
const FAULT_RETRIES: usize = 3;

/// The freed bytes in segments past which a flush cleans the image, if they are also more than a
/// quarter of the segments' bytes.
const CLEAN_PAST: u64 = 64 << 20;

/// The fraction of a segment its extents must hold for cleaning to leave it be.
const CLEAN_BELOW: f64 = 0.5;

/// Runs `f` on a thread of its own, as it blocks.
async fn unblock<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Error> {
    let (done, result) = futures::channel::oneshot::channel();
    std::thread::Builder::new().name("fxdax image".into()).spawn(move || {
        let _ = done.send(f());
    })?;
    Ok(result.await?)
}

/// An Fxfs device on an image. See the module docs.
pub(crate) struct ImageDevice {
    store: Arc<Store<HostDir>>,
    description: Description,
    io: Io,
    allocator: BufferAllocator,
    /// The writes, frees and commits, which a copy of the image pauses.
    writes: Arc<WriteGate>,
}

impl ImageDevice {
    /// Makes a new image at `dir`, which must not exist, as `description` describes, and its
    /// device, whose writes, frees and commits go through `writes`.
    pub fn create(
        dir: &Path,
        description: Description,
        writes: Arc<WriteGate>,
    ) -> Result<Self, Error> {
        std::fs::create_dir(dir).with_context(|| format!("creating {}", dir.display()))?;
        let made = || -> Result<Store<HostDir>, Error> {
            let store = create_host(dir, description, true, Options::default())?;
            // The image's directory entry is as durable as the image.
            let parent = dir.parent().filter(|parent| !parent.as_os_str().is_empty());
            std::fs::File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
            Ok(store)
        };
        match made() {
            Ok(store) => Self::new(store, writes),
            Err(error) => {
                // Leave nothing half made behind.
                let _ = std::fs::remove_dir_all(dir);
                Err(error)
            }
        }
    }

    /// The device of the image at `dir`, or of the clone whose handle `dir` is, whose writes,
    /// frees and commits go through `writes`.
    pub fn open(dir: &Path, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let opened = open_host(dir, true, Options::default());
        let store = opened.with_context(|| format!("opening {}", dir.display()))?;
        Self::new(store, writes)
    }

    /// The image's extent store.
    pub fn store(&self) -> Arc<Store<HostDir>> {
        self.store.clone()
    }

    fn new(store: Store<HostDir>, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let description = store.description();
        Ok(Self {
            store: Arc::new(store),
            description,
            io: Io::new().context("starting the image's I/O")?,
            allocator: BufferAllocator::new(
                description.block_size as usize,
                BufferSource::new(TRANSFER_HEAP_SIZE),
            ),
            writes,
        })
    }

    /// Whether I/O bypasses the host's page cache.
    pub fn is_direct(&self) -> bool {
        self.store.dir().is_direct()
    }

    /// Transfers `len` bytes at `offset` in `fd`'s file to or from memory at `addr`, as `op`
    /// makes operations of them: the first `needed` of them, and the rest as far as the file goes.
    #[allow(clippy::too_many_arguments)]
    async fn transfer(
        &self,
        fd: RawFd,
        addr: usize,
        len: usize,
        needed: usize,
        offset: u64,
        access: Access,
        op: impl Fn(usize, usize, u64) -> Op,
    ) -> Result<(), Error> {
        let mut done = 0;
        let mut faults = 0;
        while done < needed {
            let operation = op(addr + done, len - done, offset + done as u64);
            // SAFETY: The file is open as long as the caller holds it, which it does for the
            // operation, and the buffer is the caller's for the operation, which the future waits
            // for even if it is dropped.
            match unsafe { self.io.run(fd, operation) }.await {
                Ok(0) => bail!("the image's file ended at {:#x}", offset + done as u64),
                Ok(transferred) => {
                    done += transferred;
                    // The budget is for faults without progress between them.
                    faults = 0;
                }
                Err(err) if err.raw_os_error() == Some(libc::EFAULT) && faults < FAULT_RETRIES => {
                    faults += 1;
                    touch(addr + done, len - done, access);
                }
                Err(err) => {
                    return Err(
                        Error::new(err).context(format!("I/O at {:#x}", offset + done as u64))
                    );
                }
            }
        }
        Ok(())
    }

    /// Commits the store, setting its root to `root` if given, and cleans it if its segments hold
    /// much freed space, off the executor's threads.
    async fn commit(&self, root: Option<Vec<u8>>) -> Result<(), Error> {
        let _writing = self.writes.write().await;
        let store = self.store.clone();
        unblock(move || {
            store.commit(root.as_deref())?;
            let usage = store.usage()?;
            let freed = usage.segment_bytes - usage.live_bytes;
            if freed > CLEAN_PAST && freed > usage.segment_bytes / 4 {
                let cleaned = store.clean(CLEAN_BELOW)?;
                log::info!("fxdax: cleaned the image: {cleaned:?}");
            }
            Ok(())
        })
        .await?
    }
}

#[async_trait]
impl Device for ImageDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.description.block_size
    }

    fn block_count(&self) -> u64 {
        self.description.capacity / self.description.block_size as u64
    }

    /// Blocks within the extent never written, and past its end, read as zeros.
    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        let mut at = 0;
        for part in self.store.locate(extent, offset, buffer.len() as u64)? {
            match part {
                Part::Data { file, offset, len } => {
                    let mut part = buffer.reborrow().subslice_mut(at..at + len as usize);
                    let addr = part.as_mut_ptr() as usize;
                    // The host writes the buffer.
                    self.transfer(
                        file.as_raw_fd(),
                        addr,
                        len as usize,
                        len as usize,
                        offset,
                        Access::Write,
                        |addr, len, offset| Op::Read { addr, len, offset },
                    )
                    .await?;
                    at += len as usize;
                }
                // Its stored bytes, in whole blocks, to unpack.
                Part::Packed { part, skip, len } => {
                    let range = part.read_range();
                    let mut stored = Aligned::new((range.end - range.start) as usize);
                    let addr = stored.as_mut_slice().as_mut_ptr() as usize;
                    self.transfer(
                        part.file.as_raw_fd(),
                        addr,
                        stored.as_slice().len(),
                        part.stored.len as usize,
                        range.start,
                        Access::Write,
                        |addr, len, offset| Op::Read { addr, len, offset },
                    )
                    .await?;
                    let bytes = self.store.unpack(&part, stored.as_slice())?;
                    let (skip, len) = (skip as usize, len as usize);
                    let mut part = buffer.reborrow().subslice_mut(at..at + len);
                    part.copy_from_slice(&bytes[skip..skip + len]);
                    at += len;
                }
                Part::Unpacked { bytes, skip, len } => {
                    let (skip, len) = (skip as usize, len as usize);
                    let mut part = buffer.reborrow().subslice_mut(at..at + len);
                    part.copy_from_slice(&bytes[skip..skip + len]);
                    at += len;
                }
                Part::Zeros(len) => {
                    buffer.reborrow().subslice_mut(at..at + len as usize).fill(0);
                    at += len as usize;
                }
            }
        }
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        write_opts: WriteOptions,
    ) -> Result<(), Error> {
        if write_opts.flags.contains(WriteFlags::POST_BARRIER) {
            // The write, then the flush that follows it.
            self.write_with_opts(extent, offset, buffer, WriteOptions::default()).await?;
            return self.flush().await;
        }
        let _writing = self.writes.write().await;
        let write = self.store.begin_write(extent, offset, buffer.len() as u64)?;
        let (addr, len) = (buffer.as_ptr() as usize, buffer.len());
        // The host reads the buffer.
        self.transfer(
            write.file().as_raw_fd(),
            addr,
            len,
            len,
            write.offset(),
            Access::Read,
            |addr, len, offset| Op::Write { addr, len, offset },
        )
        .await?;
        write.finish();
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.description.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.store.info()?.next_sequence)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        let _writing = self.writes.write().await;
        self.store.free(extent)
    }

    async fn root(&self) -> Result<Vec<u8>, Error> {
        self.store.root()
    }

    async fn commit_root(&self, root: &[u8]) -> Result<(), Error> {
        self.commit(Some(root.to_vec())).await.context("committing the image's root")
    }

    async fn close(&self) -> Result<(), Error> {
        self.flush().await
    }

    async fn flush(&self) -> Result<(), Error> {
        self.commit(None).await.context("flushing the image")
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

/// How the host accesses a buffer.
#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

/// Touches the pages of the `len` bytes at `addr` as `access` does, faulting them in.
fn touch(addr: usize, len: usize, access: Access) {
    let page_size = crate::kernel::page_size() as usize;
    let mut page = addr - addr % page_size;
    while page < addr + len {
        let byte = page.max(addr) as *mut u8;
        // SAFETY: The memory is a transfer buffer's, which the operation that failed is the only
        // user of, and which is mapped readable and writable. Writing a byte back unchanged leaves
        // it as it was.
        unsafe {
            match access {
                Access::Read => {
                    byte.read_volatile();
                }
                Access::Write => byte.write_volatile(byte.read_volatile()),
            }
        }
        page += page_size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::path::PathBuf;
    use storage_device::NoExtent;
    use zircon::vm::arch_vm_aspace::{NonTerminalAction, TerminalAction};
    use zircon::vm::vm_aspace::VmAspace;

    const BLOCK: u32 = 4096;
    const MIB: usize = 1 << 20;
    const BRANCH: u64 = 7;

    /// A device on a new image holding `capacity` bytes, in a new directory, and the image.
    fn device(capacity: u64) -> (ImageDevice, PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        let description = Description { block_size: BLOCK, capacity, branch: BRANCH };
        (ImageDevice::create(&image, description, Default::default()).unwrap(), image, dir)
    }

    fn extent(sequence: u64) -> ExtentId {
        ExtentId::new(BRANCH, sequence)
    }

    /// Bytes each their offset modulo 251, from `offset`.
    fn pattern(offset: usize, len: usize) -> Vec<u8> {
        (offset..offset + len).map(|i| (i % 251) as u8).collect()
    }

    async fn write(device: &ImageDevice, extent: ExtentId, offset: u64, data: &[u8]) {
        let mut buffer = device.allocate_buffer(data.len()).await;
        buffer.as_mut().copy_from_slice(data);
        device.write(extent, offset, buffer.as_ref()).await.unwrap();
    }

    async fn read(device: &ImageDevice, extent: ExtentId, offset: u64, len: usize) -> Vec<u8> {
        let mut buffer = device.allocate_buffer(len).await;
        device.read(extent, offset, buffer.as_mut()).await.unwrap();
        contents(buffer.as_ref())
    }

    fn contents(buffer: BufferRef<'_>) -> Vec<u8> {
        let mut data = vec![0; buffer.len()];
        buffer.copy_to_slice(&mut data);
        data
    }

    #[test]
    fn reads_what_was_written() {
        let (device, _, _dir) = device(64 * MIB as u64);
        assert_eq!(device.block_count(), 64 * MIB as u64 / BLOCK as u64);
        assert_eq!(device.branch(), BRANCH);
        block_on(async {
            // Many at once, which all complete, and one large enough for a file of its own.
            let data: Vec<Vec<u8>> = (0..16).map(|i| pattern(i * MIB, MIB)).collect();
            futures::future::join_all(
                data.iter().enumerate().map(|(i, data)| write(&device, extent(i as u64), 0, data)),
            )
            .await;
            write(&device, extent(16), 0, &pattern(7, 5 * MIB)).await;
            device.flush().await.unwrap();
            let all =
                futures::future::join_all((0..16).map(|i| read(&device, extent(i), 0, MIB))).await;
            for (i, data) in all.iter().enumerate() {
                assert!(*data == pattern(i * MIB, MIB), "extent {i} differs");
            }
            assert!(read(&device, extent(16), 0, 5 * MIB).await == pattern(7, 5 * MIB));
            assert_eq!(device.next_sequence().await.unwrap(), 17);

            // Blocks never written read as zeros, within an extent and past its end.
            write(&device, extent(300), BLOCK as u64, &pattern(0, BLOCK as usize)).await;
            let zeros = vec![0; BLOCK as usize];
            assert_eq!(
                read(&device, extent(300), 0, 3 * BLOCK as usize).await,
                [zeros.clone(), pattern(0, BLOCK as usize), zeros].concat()
            );

            // Unaligned I/O, and I/O on extents the image doesn't hold, fail.
            let mut buffer = device.allocate_buffer(BLOCK as usize).await;
            assert!(device.read(extent(0), 1, buffer.as_mut()).await.is_err());
            let error = device.read(extent(17), 0, buffer.as_mut()).await.expect_err("read");
            assert_eq!(error.downcast_ref::<NoExtent>(), Some(&NoExtent(extent(17))));
        });
    }

    /// An image holds what was flushed when it is opened again, and what wasn't is gone.
    #[test]
    fn extents_survive_reopening() {
        let (device, image, _dir) = device(16 * MIB as u64);
        block_on(async {
            write(&device, extent(3), 0, &pattern(0, MIB)).await;
            write(&device, extent(259), 0, &pattern(1, 2 * BLOCK as usize)).await;
            device.flush().await.unwrap();
            write(&device, extent(260), 0, &pattern(2, BLOCK as usize)).await;
        });
        drop(device);
        let device = ImageDevice::open(&image, Default::default()).unwrap();
        block_on(async {
            assert_eq!(read(&device, extent(3), 0, MIB).await, pattern(0, MIB));
            assert_eq!(
                read(&device, extent(259), 0, 2 * BLOCK as usize).await,
                pattern(1, 2 * BLOCK as usize)
            );
            assert_eq!(device.next_sequence().await.unwrap(), 260);
            let mut buffer = device.allocate_buffer(BLOCK as usize).await;
            assert!(device.read(extent(260), 0, buffer.as_mut()).await.is_err());
        });
        assert_eq!(device.store.info().unwrap().used, (MIB + 2 * BLOCK as usize) as u64);
    }

    /// Freeing an extent gives back the capacity it took.
    #[test]
    fn frees_make_room() {
        let (device, _, _dir) = device(4 * MIB as u64);
        block_on(async {
            write(&device, extent(1), 0, &pattern(0, 3 * MIB)).await;
            device.flush().await.unwrap();
            let mut buffer = device.allocate_buffer(2 * MIB).await;
            assert!(device.write(extent(2), 0, buffer.as_ref()).await.is_err(), "overfilled");

            device.free(extent(1)).await.unwrap();
            device.free(extent(1)).await.expect("freeing a freed extent failed");
            device.flush().await.unwrap();
            assert!(device.read(extent(1), 0, buffer.as_mut()).await.is_err());
            device.write(extent(2), 0, buffer.as_ref()).await.expect("write failed");
            // Freeing doesn't make sequence numbers available again.
            assert_eq!(device.next_sequence().await.unwrap(), 3);
        });
    }

    /// The root is set durably, and kept across opening the image again.
    #[test]
    fn roots() {
        let (device, image, _dir) = device(MIB as u64);
        block_on(async {
            assert_eq!(device.root().await.unwrap(), Vec::<u8>::new());
            write(&device, extent(1), 0, &pattern(0, BLOCK as usize)).await;
            device.commit_root(b"first").await.unwrap();
            device.commit_root(b"second").await.unwrap();
            assert_eq!(device.root().await.unwrap(), b"second");
        });
        drop(device);
        let device = ImageDevice::open(&image, Default::default()).unwrap();
        assert_eq!(block_on(device.root()).unwrap(), b"second");
        assert_eq!(
            block_on(read(&device, extent(1), 0, BLOCK as usize)),
            pattern(0, BLOCK as usize)
        );
    }

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
}

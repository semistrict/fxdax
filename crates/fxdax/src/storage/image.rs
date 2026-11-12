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
//!   Linux, `F_NOCACHE` on macOS).
//! - Extents' I/O runs off Fxfs's executor threads (see `io`), which only await it, as do commits,
//!   which sync the image.
//! - Flushing commits the store: a barrier, durable, which seals the extents written before it.
//! - fxdax: the store's branch is kept in its bucket (see `kept`): its commits are uploaded behind
//!   them, and closing the device waits for them to be.

use super::gate::WriteGate;
use super::io::{Done, Io, IoFile, Op};
use super::kept::Kept;
use anyhow::{Context, Error, bail};
use async_trait::async_trait;
use extent_store::{Aligned, Description, HostDir, HostFile, Part, Store};
use std::path::Path;
use std::sync::Arc;
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId};
use storage_device::{ReadOptions, WriteFlags, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 32 * 1024 * 1024;

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
    image: Arc<Kept>,
    /// The image's store, the one `image` keeps.
    store: Arc<Store<HostDir>>,
    description: Description,
    io: Io,
    allocator: BufferAllocator,
    /// The writes, frees and commits, which a copy of the image pauses.
    writes: Arc<WriteGate>,
}

impl ImageDevice {
    /// Makes a new image at `dir`, which must not exist, holding at most `capacity` bytes, of
    /// `branch`, whole if `whole`, its branch kept as `keeping` says ([`Kept::create`]), and its
    /// device, whose writes, frees and commits go through `writes`.
    pub fn create(
        dir: &Path,
        made: (u64, crate::host_store::BranchOf, bool),
        keeping: extent_store::upload::Options,
        writes: Arc<WriteGate>,
    ) -> Result<Self, Error> {
        Self::new(Kept::create(dir, made, keeping)?, writes)
    }

    /// The device of the image at `dir`, an image's or a clone's, whose writes, frees and commits
    /// go through `writes`.
    pub fn open(dir: &Path, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let image = Kept::open(dir).with_context(|| format!("opening {}", dir.display()))?;
        Self::new(image, writes)
    }

    /// The image's store, kept in its bucket.
    pub fn image(&self) -> Arc<Kept> {
        self.image.clone()
    }

    fn new(image: Kept, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let store = image.store().clone();
        let description = store.description();
        Ok(Self {
            image: Arc::new(image),
            store,
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

    /// Reads `len` bytes at `offset` in `file`: the first `needed` of them, and the rest as far as
    /// the file goes. The I/O reads into a buffer of its own, which it gives back: the caller's
    /// memory is never the host's, which a future leaked would leave it (TASK-24).
    async fn read_file(
        &self,
        file: &Arc<HostFile>,
        len: usize,
        needed: usize,
        offset: u64,
    ) -> Result<Aligned, Error> {
        let mut read: Option<Aligned> = None;
        let mut done = 0;
        while done < needed {
            let at = offset + done as u64;
            let op = Op::Read { len: len - done, offset: at };
            let Done { result, buffer } = self.io.run(file.clone() as IoFile, op).await;
            let transferred = match result {
                Ok(0) => bail!("the image's file ended at {at:#x}"),
                Ok(transferred) => transferred,
                Err(err) => return Err(Error::new(err).context(format!("I/O at {at:#x}"))),
            };
            match &mut read {
                // Read whole, as it is but for a short read.
                None if transferred >= needed => return Ok(buffer),
                None => read = Some(buffer),
                Some(read) => read.as_mut_slice()[done..done + transferred]
                    .copy_from_slice(&buffer.as_slice()[..transferred]),
            }
            done += transferred;
        }
        Ok(read.unwrap_or_else(|| Aligned::new(len)))
    }

    /// Writes `data` at `offset` in `file`, from a buffer of the I/O's own, as `read_file` reads.
    async fn write_file(
        &self,
        file: &Arc<HostFile>,
        data: BufferRef<'_>,
        offset: u64,
    ) -> Result<(), Error> {
        let mut done = 0;
        while done < data.len() {
            let at = offset + done as u64;
            let mut owned = Aligned::new(data.len() - done);
            data.subslice(done..).copy_to_slice(owned.as_mut_slice());
            let op = Op::Write { data: owned, offset: at };
            match self.io.run(file.clone() as IoFile, op).await.result {
                Ok(0) => bail!("the image's file took nothing at {at:#x}"),
                Ok(transferred) => done += transferred,
                Err(err) => return Err(Error::new(err).context(format!("I/O at {at:#x}"))),
            }
        }
        Ok(())
    }

    /// Commits the store, setting its root to `root` if given, and does what follows a commit
    /// ([`Kept::committed`]): collects its bucket if that is due; off the executor's threads.
    async fn commit(&self, root: Option<Vec<u8>>) -> Result<(), Error> {
        let _writing = self.writes.write().await;
        let image = self.image.clone();
        unblock(move || {
            image.store().commit(root.as_deref())?;
            image.committed()
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
        let parts = match self.store.locate(extent, offset, buffer.len() as u64) {
            // fxdax: in the image's bucket, as a clone holds what it inherited (TASK-165.7).
            Err(ref error)
                if let Some(remote) = error.downcast_ref::<extent_store::RemoteData>() =>
            {
                let location = remote.location.clone();
                let fetcher = self.image.fetcher();
                let fetched = fetcher.read(extent, location, offset, buffer.len() as u64).await?;
                let (len, end) = (fetched.len(), buffer.len());
                buffer.reborrow().subslice_mut(..len).copy_from_slice(&fetched);
                buffer.reborrow().subslice_mut(len..end).fill(0);
                return Ok(());
            }
            parts => parts?,
        };
        let mut at = 0;
        for part in parts {
            match part {
                // A raw file's part, through the descriptor bypassing the host's page cache,
                // which the store's own reads of raw files take through the cache
                // (`File::read_cached`, TASK-91.17): such a read writes back first what it reads
                // that is dirty yet, which is slower, not other bytes. This device is an
                // in-process image's, which imports, host tools and tests use; guests' reads are
                // extentd's, through the store's.
                Part::Data { file, offset, len, .. } => {
                    let len = len as usize;
                    let read = self.read_file(&file, len, len, offset).await?;
                    buffer.reborrow().subslice_mut(at..at + len).copy_from_slice(read.as_slice());
                    at += len;
                }
                // Its stored bytes, in whole blocks, to check and unpack.
                Part::Packed { part, skip, len } => {
                    let range = part.read_range();
                    let read_len = (range.end - range.start) as usize;
                    let needed = part.stored.len as usize;
                    let stored = self.read_file(&part.file, read_len, needed, range.start).await?;
                    let bytes = self.store.unpack(&part, stored)?;
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
        write_opts: WriteOptions<'_>,
    ) -> Result<(), Error> {
        if write_opts.flags.contains(WriteFlags::POST_BARRIER) {
            // The write, then the flush, or commit of the root, that follows it.
            self.write_with_opts(extent, offset, buffer, WriteOptions::default()).await?;
            return match write_opts.root {
                Some(root) => self.commit_root(root).await,
                None => self.flush().await,
            };
        }
        let _writing = self.writes.write().await;
        // The store holds a write back while a commit waits for those in progress, which may be
        // futures of this very task, as Fxfs joins its writes: this thread must not wait, as they
        // can't end while it does, and the commit would wait for them for ever (TASK-98). A
        // thread of its own waits, and the write is tried again.
        let write = loop {
            if let Some(write) = self.store.try_begin_write(extent, offset, buffer.len() as u64)? {
                break write;
            }
            let store = self.store.clone();
            unblock(move || store.wait_until_writes_resume()).await?;
        };
        self.write_file(write.file(), buffer, write.offset()).await?;
        write.finish();
        Ok(())
    }

    fn branch(&self) -> u64 {
        self.description.branch.id
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

    /// Flushes, and waits for the image's commits to be uploaded: a closed image's branch is whole
    /// in its bucket.
    async fn close(&self) -> Result<(), Error> {
        self.flush().await?;
        let image = self.image.clone();
        unblock(move || image.wait_uploaded()).await?.context("uploading the image's commits")
    }

    async fn flush(&self) -> Result<(), Error> {
        self.commit(None).await.context("flushing the image")
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::path::PathBuf;
    use storage_device::NoExtent;

    const BLOCK: u32 = crate::host_store::BLOCK_SIZE;
    const MIB: usize = 1 << 20;
    const BRANCH: crate::Branch = extent_remote::layout::test_branch(7);

    /// A device on a new image holding `capacity` bytes, in a new directory, and the image.
    fn device(capacity: u64) -> (ImageDevice, PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        let made = (capacity, crate::host_store::BranchOf::Named(BRANCH), true);
        let keeping = crate::host_store::keeping().unwrap();
        (ImageDevice::create(&image, made, keeping, Default::default()).unwrap(), image, dir)
    }

    fn extent(sequence: u64) -> ExtentId {
        ExtentId::new(BRANCH.id, sequence)
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
        assert_eq!(device.branch(), BRANCH.id);
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

    /// fxdax: a byte the host's disk damaged in a sealed extent's file, in a part stored compressed
    /// or raw, fails the device's reads of the part, as imports and host tools read an image,
    /// rather than giving another byte; its other parts read as written (TASK-153.11).
    #[test]
    fn flipped_bytes_fail_reads() {
        let (device, image, _dir) = device(64 * MIB as u64);
        block_on(async {
            // Large enough to be stored on their own, in five parts: compressed, and raw.
            let contents = [
                (extent(1), pattern(0, 5 * MIB), extent_frame::Encoding::Zstd),
                (extent(2), crate::test_data::noise(2, 5 * MIB), extent_frame::Encoding::Raw),
            ];
            for (extent, data, _) in &contents {
                write(&device, *extent, 0, data).await;
            }
            device.flush().await.unwrap();
            for (extent, data, encoding) in &contents {
                // The middle of its fourth part's stored bytes: of its data at 3.5 MiB, if raw.
                let path =
                    image.join(format!("extents/{:016x}/{:016x}", BRANCH.id, extent.sequence));
                let mut bytes = std::fs::read(&path).unwrap();
                let stored = extent_frame::Parts::of(&bytes).unwrap().stored[3];
                assert_eq!(stored.encoding, *encoding);
                bytes[(stored.offset + stored.len / 2) as usize] ^= 0x10;
                std::fs::write(&path, bytes).unwrap();

                let mut buffer = device.allocate_buffer(BLOCK as usize).await;
                let at = (3 * MIB + MIB / 2) as u64;
                let Err(error) = device.read(*extent, at, buffer.as_mut()).await else {
                    panic!("{extent:?} read a flipped byte");
                };
                let corrupt = extent_frame::Corrupt("an object's part, by its CRC".into());
                assert_eq!(error.downcast_ref(), Some(&corrupt), "{error:#}");
                let first = read(&device, *extent, 0, BLOCK as usize).await;
                assert!(first == data[..BLOCK as usize], "{extent:?}'s first block");
            }
        });
    }

    /// A write the store holds back while a commit waits for a write of the executor's own, on an
    /// executor of one thread, goes on once that write ends: it does not hold the thread the write
    /// would end on, which the commit would wait for for ever.
    #[test]
    #[allow(clippy::disallowed_methods, reason = "a write in progress, as a commit waits for")]
    fn writes_held_back_by_a_commit_leave_the_executor_running() {
        let (device, _, _dir) = device(16 * MIB as u64);
        let store = device.store.clone();
        let device = Arc::new(device);
        let executor = crate::executor::Executor::new(1);
        // A write of the executor's in progress, which ends once told to.
        let (go, went) = futures::channel::oneshot::channel::<()>();
        let (began, beginning) = std::sync::mpsc::channel();
        let held = store.clone();
        executor.spawn(async move {
            let write = held.begin_write(extent(1), 0, BLOCK as u64).unwrap();
            began.send(()).unwrap();
            let _ = went.await;
            write.finish();
        });
        beginning.recv().unwrap();
        let committing = std::thread::spawn({
            let store = store.clone();
            move || store.commit(None).unwrap()
        });
        while !store.holding_writes() {
            std::thread::yield_now();
        }
        let (wrote, written) = std::sync::mpsc::channel();
        let writing = device.clone();
        executor.spawn(async move {
            write(&writing, extent(2), 0, &pattern(0, BLOCK as usize)).await;
            wrote.send(()).unwrap();
        });
        go.send(()).unwrap();
        let waited = written.recv_timeout(std::time::Duration::from_secs(30));
        assert_eq!(waited, Ok(()), "the held write never went on");
        committing.join().unwrap();
    }

    /// TASK-98: as above, with the write in progress a future of the held write's own task, as
    /// Fxfs joins its writes: the held write waits off the task's thread, which the other future
    /// ends on. fxdaxctl import hung so, every executor thread waiting in `begin_write` for a
    /// commit that waited for the writes those threads had begun.
    #[test]
    #[allow(clippy::disallowed_methods, reason = "a write in progress, as a commit waits for")]
    fn writes_held_back_by_a_commit_leave_their_own_task_running() {
        let (device, _, _dir) = device(16 * MIB as u64);
        let store = device.store.clone();
        let device = Arc::new(device);
        let executor = crate::executor::Executor::new(1);
        let (go, went) = futures::channel::oneshot::channel::<()>();
        let (held, holding) = futures::channel::oneshot::channel::<()>();
        let (began, beginning) = std::sync::mpsc::channel();
        let (writing, entered) = std::sync::mpsc::channel();
        let (wrote, written) = std::sync::mpsc::channel();
        let in_task = store.clone();
        executor.spawn(async move {
            // A write in progress, which ends once told to.
            let in_progress = async {
                let write = in_task.begin_write(extent(1), 0, BLOCK as u64).unwrap();
                began.send(()).unwrap();
                let _ = went.await;
                write.finish();
            };
            // And beside it, once a commit holds writes back, another.
            let held_back = async {
                let _ = holding.await;
                // In the same poll as the write begins: the other is told to end only after.
                writing.send(()).unwrap();
                write(&device, extent(2), 0, &pattern(0, BLOCK as usize)).await;
            };
            futures::join!(in_progress, held_back);
            wrote.send(()).unwrap();
        });
        beginning.recv().unwrap();
        let committing = std::thread::spawn({
            let store = store.clone();
            move || store.commit(None).unwrap()
        });
        while !store.holding_writes() {
            std::thread::yield_now();
        }
        held.send(()).unwrap();
        entered.recv().unwrap();
        go.send(()).unwrap();
        let waited = written.recv_timeout(std::time::Duration::from_secs(30));
        assert_eq!(waited, Ok(()), "the task's writes never ended");
        committing.join().unwrap();
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
}

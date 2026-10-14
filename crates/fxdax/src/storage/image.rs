//! An Fxfs device on an image file, for production use.
//!
//! It replaces Fuchsia's `FileBackedDevice`, which is for host tools and tests:
//!
//! - I/O bypasses the host's page cache where the host filesystem allows (`O_DIRECT` on Linux,
//!   `F_NOCACHE` on macOS), as the in-process kernel caches files' pages already.
//! - I/O runs off Fxfs's executor threads (see `io`), which only await it.
//! - Trimming a range punches a hole, deallocating it, so that clones of the image keep sharing
//!   its blocks and the file stays sparse.
//! - Flushing writes back to stable storage: `fdatasync`, or `F_FULLFSYNC` on macOS.
//!
//! Transfer buffers are memory of the in-process kernel's, which the host's I/O cannot fault in:
//! it fails with `EFAULT` on a page the kernel's page scanner made inaccessible to track its
//! accesses, and, for a read, on a page not yet written, which the kernel maps read-only, as the
//! zero page. An operation that fails so touches its buffer's pages as it will access them, which
//! faults them in, and is tried again.

use super::gate::WriteGate;
use super::io::{Io, Op};
use anyhow::{Context, Error, ensure};
use async_trait::async_trait;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId, ExtentRange};
use storage_device::{ReadOptions, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 32 * 1024 * 1024;

/// How many times an operation is tried again after faulting its buffer's pages in.
const FAULT_RETRIES: usize = 3;

/// An Fxfs device on an image file. See the module docs.
pub(crate) struct ImageDevice {
    file: Arc<File>,
    io: Io,
    allocator: BufferAllocator,
    block_size: u32,
    block_count: u64,
    /// Whether I/O bypasses the host's page cache.
    direct: bool,
    /// Whether the host filesystem punches holes, until it refuses to.
    punch_holes: AtomicBool,
    /// The writes, which a copy of the image pauses.
    writes: Arc<WriteGate>,
}

impl ImageDevice {
    /// A device on `file`, which must be open for reading and writing, in blocks of `block_size`
    /// bytes, whose writes and trims go through `writes`. Its size, rounded down to blocks, is
    /// the device's.
    pub fn new(file: Arc<File>, block_size: u32, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let size = file.metadata().context("reading the image's size")?.len();
        ensure!(size >= block_size as u64, "the image is smaller than a block");
        let direct = bypass_page_cache(&file);
        Ok(Self {
            io: Io::new().context("starting the image's I/O")?,
            allocator: BufferAllocator::new(
                block_size as usize,
                BufferSource::new(TRANSFER_HEAP_SIZE),
            ),
            block_size,
            block_count: size / block_size as u64,
            direct,
            punch_holes: AtomicBool::new(true),
            writes,
            file,
        })
    }

    /// Whether I/O bypasses the host's page cache.
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    /// An image holds one extent, the whole image, until the extent store replaces images
    /// (TASK-26.10).
    fn check_extent(extent: ExtentId) -> Result<(), Error> {
        ensure!(extent == ExtentId::default(), "no extent {extent:?} in an image");
        Ok(())
    }

    fn check_range(&self, offset: u64, len: usize) -> Result<(), Error> {
        let block_size = self.block_size as u64;
        ensure!(
            offset.is_multiple_of(block_size) && (len as u64).is_multiple_of(block_size),
            "unaligned I/O"
        );
        ensure!(offset + len as u64 <= self.size(), "I/O past the end of the image");
        Ok(())
    }

    /// Transfers `len` bytes at `offset` to or from memory at `addr`, as `op` makes operations of
    /// them, all of them unless the image ends.
    async fn transfer(
        &self,
        addr: usize,
        len: usize,
        offset: u64,
        access: Access,
        op: impl Fn(usize, usize, u64) -> Op,
    ) -> Result<(), Error> {
        let mut done = 0;
        let mut faults = 0;
        while done < len {
            let operation = op(addr + done, len - done, offset + done as u64);
            // SAFETY: The file is open as long as the device, and the buffer is the caller's for
            // the operation, which the future waits for even if it is dropped.
            match unsafe { self.io.run(self.file.as_raw_fd(), operation) }.await {
                Ok(0) => anyhow::bail!("the image ended at {:#x}", offset + done as u64),
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
}

#[async_trait]
impl Device for ImageDevice {
    fn allocate_buffer(&self, size: usize) -> BufferFuture<'_> {
        self.allocator.allocate_buffer(size)
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    async fn read_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        mut buffer: MutableBufferRef<'_>,
        _read_opts: ReadOptions,
    ) -> Result<(), Error> {
        Self::check_extent(extent)?;
        self.check_range(offset, buffer.len())?;
        let (addr, len) = (buffer.as_mut_ptr() as usize, buffer.len());
        // The host writes the buffer.
        self.transfer(addr, len, offset, Access::Write, |addr, len, offset| Op::Read {
            addr,
            len,
            offset,
        })
        .await
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        Self::check_extent(extent)?;
        self.check_range(offset, buffer.len())?;
        let _writing = self.writes.write().await;
        let (addr, len) = (buffer.as_ptr() as usize, buffer.len());
        // The host reads the buffer.
        self.transfer(addr, len, offset, Access::Read, |addr, len, offset| Op::Write {
            addr,
            len,
            offset,
        })
        .await
    }

    /// Deallocates the range, unless the host filesystem cannot, when trimming does nothing, as
    /// a trimmed range's contents are undefined.
    async fn trim(&self, range: ExtentRange) -> Result<(), Error> {
        Self::check_extent(range.extent)?;
        let range = range.range;
        self.check_range(range.start, (range.end - range.start) as usize)?;
        if range.is_empty() || !self.punch_holes.load(Ordering::Relaxed) {
            return Ok(());
        }
        let op = Op::PunchHole { offset: range.start, len: range.end - range.start };
        let _writing = self.writes.write().await;
        // SAFETY: The file is open as long as the device, and the operation has no buffer.
        match unsafe { self.io.run(self.file.as_raw_fd(), op) }.await {
            Ok(_) => Ok(()),
            Err(err) if super::unsupported(&err) => {
                log::warn!(
                    "fxdax: the image's filesystem cannot punch holes; trimming does nothing"
                );
                self.punch_holes.store(false, Ordering::Relaxed);
                Ok(())
            }
            Err(err) => Err(Error::new(err).context("punching a hole in the image")),
        }
    }

    async fn close(&self) -> Result<(), Error> {
        self.flush().await
    }

    async fn flush(&self) -> Result<(), Error> {
        // SAFETY: The file is open as long as the device, and the operation has no buffer.
        unsafe { self.io.run(self.file.as_raw_fd(), Op::SyncData) }
            .await
            .context("flushing the image")?;
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn supports_trim(&self) -> bool {
        true
    }
}

/// Makes I/O on `file` bypass the host's page cache, if its filesystem allows, and returns whether
/// it does.
fn bypass_page_cache(file: &File) -> bool {
    let fd = file.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        // SAFETY: Reads and sets the status flags of an open file descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_DIRECT) == 0
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: Sets an option of an open file descriptor.
        unsafe { libc::fcntl(fd, libc::F_NOCACHE, 1) == 0 }
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
    use std::os::unix::fs::MetadataExt;
    use zircon::vm::arch_vm_aspace::{NonTerminalAction, TerminalAction};
    use zircon::vm::vm_aspace::VmAspace;

    const BLOCK: u32 = 4096;
    const MIB: usize = 1 << 20;

    /// A device on a new image of `size` bytes, and the image.
    fn device(size: u64) -> (ImageDevice, Arc<File>, tempfile::TempPath) {
        let image = tempfile::NamedTempFile::new().unwrap();
        image.as_file().set_len(size).unwrap();
        let (file, path) = image.into_parts();
        let file = Arc::new(file);
        (ImageDevice::new(file.clone(), BLOCK, Default::default()).unwrap(), file, path)
    }

    /// Bytes each their offset modulo 251, from `offset`.
    fn pattern(offset: usize, len: usize) -> Vec<u8> {
        (offset..offset + len).map(|i| (i % 251) as u8).collect()
    }

    async fn write(device: &ImageDevice, offset: u64, data: &[u8]) {
        let mut buffer = device.allocate_buffer(data.len()).await;
        buffer.as_mut().copy_from_slice(data);
        device.write(ExtentId::default(), offset, buffer.as_ref()).await.unwrap();
    }

    async fn read(device: &ImageDevice, offset: u64, len: usize) -> Vec<u8> {
        let mut buffer = device.allocate_buffer(len).await;
        device.read(ExtentId::default(), offset, buffer.as_mut()).await.unwrap();
        contents(buffer.as_ref())
    }

    fn contents(buffer: BufferRef<'_>) -> Vec<u8> {
        let mut data = vec![0; buffer.len()];
        buffer.copy_to_slice(&mut data);
        data
    }

    #[test]
    fn reads_what_was_written() {
        let (device, _, _path) = device(16 * MIB as u64);
        assert_eq!(device.block_count(), 16 * MIB as u64 / BLOCK as u64);
        block_on(async {
            // Many at once, which all complete.
            let data: Vec<Vec<u8>> = (0..16).map(|i| pattern(i * MIB, MIB)).collect();
            futures::future::join_all(
                data.iter().enumerate().map(|(i, data)| write(&device, (i * MIB) as u64, data)),
            )
            .await;
            device.flush().await.unwrap();
            let read =
                futures::future::join_all((0..16).map(|i| read(&device, (i * MIB) as u64, MIB)))
                    .await;
            for (i, data) in read.iter().enumerate() {
                assert!(*data == pattern(i * MIB, MIB), "megabyte {i} differs");
            }
            // Unaligned I/O and I/O past the end fail.
            let mut buffer = device.allocate_buffer(BLOCK as usize).await;
            assert!(device.read(ExtentId::default(), 1, buffer.as_mut()).await.is_err());
            assert!(
                device.read(ExtentId::default(), 16 * MIB as u64, buffer.as_mut()).await.is_err()
            );
        });
    }

    /// Trimming deallocates the range, which then reads as zeros.
    #[test]
    fn trim_punches_holes() {
        let (device, file, _path) = device(16 * MIB as u64);
        block_on(async {
            write(&device, 0, &pattern(0, 4 * MIB)).await;
            device.flush().await.unwrap();
            let allocated = file.metadata().unwrap().blocks() * 512;
            assert!(allocated >= 4 * MIB as u64, "{allocated} bytes allocated");

            device
                .trim(ExtentRange::new(ExtentId::default(), MIB as u64..3 * MIB as u64))
                .await
                .unwrap();
            device.flush().await.unwrap();
            let trimmed = file.metadata().unwrap().blocks() * 512;
            assert!(
                trimmed <= allocated - 2 * MIB as u64,
                "{trimmed} of {allocated} bytes still allocated"
            );
            assert!(read(&device, MIB as u64, 2 * MIB).await.iter().all(|&byte| byte == 0));
            assert_eq!(read(&device, 0, MIB).await, pattern(0, MIB));
            assert_eq!(read(&device, 3 * MIB as u64, MIB).await, pattern(3 * MIB, MIB));
        });
    }

    /// The kernel's page scanner makes transfer buffers' pages inaccessible to track their
    /// accesses, which the host's I/O into them fails on; the device's I/O faults them back in.
    #[test]
    fn io_into_harvested_buffers() {
        let (device, file, _path) = device(4 * MIB as u64);
        block_on(async {
            write(&device, 0, &pattern(0, MIB)).await;
            let mut buffer = device.allocate_buffer(MIB).await;
            let harvest = || {
                VmAspace::harvest_all_user_accessed_bits(
                    NonTerminalAction::Retain,
                    TerminalAction::UpdateAgeAndHarvest,
                )
            };

            harvest();
            // SAFETY: Reads into the buffer, which is the test's.
            let read = unsafe {
                libc::pread(file.as_raw_fd(), buffer.as_mut().as_mut_ptr().cast(), MIB, 0)
            };
            assert_eq!(read, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

            harvest();
            device.read(ExtentId::default(), 0, buffer.as_mut()).await.unwrap();
            assert!(contents(buffer.as_ref()) == pattern(0, MIB));
            harvest();
            device.write(ExtentId::default(), MIB as u64, buffer.as_ref()).await.unwrap();
            assert_eq!(read_back(&file, MIB as u64, MIB), pattern(0, MIB));
        });
    }

    /// A fresh device's first reads land in buffers the kernel maps read-only, as the zero page,
    /// until they are written; the host's reads into them fault them in.
    #[test]
    fn first_reads_into_unwritten_buffers() {
        use std::os::unix::fs::FileExt;
        let (_, _, path) = device(4 * MIB as u64);
        // Through a file of its own, as the device's bypasses the page cache, needing aligned I/O.
        let file = File::options().read(true).write(true).open(&path).unwrap();
        file.write_all_at(&pattern(0, MIB), 0).unwrap();
        file.sync_all().unwrap();
        let file = Arc::new(File::options().read(true).write(true).open(&path).unwrap());
        let device = ImageDevice::new(file, BLOCK, Default::default()).unwrap();
        assert!(block_on(read(&device, 0, MIB)) == pattern(0, MIB));
    }

    /// The image's bytes, read through the host's page cache.
    fn read_back(file: &File, offset: u64, len: usize) -> Vec<u8> {
        use std::os::unix::fs::FileExt;
        let file = File::open(format!("/dev/fd/{}", file.as_raw_fd())).unwrap();
        let mut data = vec![0; len];
        file.read_exact_at(&mut data, offset).unwrap();
        data
    }
}

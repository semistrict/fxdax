//! An Fxfs device on an image: an extent store in a directory (backlog doc-4).
//!
//! The image is a directory holding:
//!
//! - `store`, the store's description: its format, block size, capacity and branch;
//! - `extents/00` to `extents/ff`, by the low byte of their sequence numbers, a file per extent,
//!   named by its ID (`<sequence>.<branch>`, in hex), as long as the extent's furthest write.
//!
//! A file per extent is the simplest store, until TASK-26.10's packs small extents into segments,
//! as doc-4's host does. It replaces Fuchsia's `FileBackedDevice`, which is for host tools and
//! tests:
//!
//! - I/O bypasses the host's page cache where the host filesystem allows (`O_DIRECT` on Linux,
//!   `F_NOCACHE` on macOS), as the in-process kernel caches files' pages already.
//! - I/O, and opening and removing files, run off Fxfs's executor threads (see `io`), which only
//!   await them.
//! - Freeing an extent removes its file.
//! - Flushing writes back to stable storage the extents written, and the directories whose files
//!   were created or removed, since the last flush: `fdatasync`, or on macOS, `fsync` and an
//!   `F_FULLFSYNC`.
//!
//! Transfer buffers are memory of the in-process kernel's, which the host's I/O cannot fault in:
//! it fails with `EFAULT` on a page the kernel's page scanner made inaccessible to track its
//! accesses, and, for a read, on a page not yet written, which the kernel maps read-only, as the
//! zero page. An operation that fails so touches its buffer's pages as it will access them, which
//! faults them in, and is tried again.

use super::gate::WriteGate;
use super::io::{Io, Op};
use anyhow::{Context, Error, anyhow, bail, ensure};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use storage_device::buffer::{BufferFuture, BufferRef, MutableBufferRef};
use storage_device::buffer_allocator::{BufferAllocator, BufferSource};
use storage_device::{Device, ExtentId};
use storage_device::{ReadOptions, WriteOptions};

/// The size of the transfer buffers' memory.
const TRANSFER_HEAP_SIZE: usize = 32 * 1024 * 1024;

/// How many times an operation is tried again after faulting its buffer's pages in.
const FAULT_RETRIES: usize = 3;

/// The first line of an image's `store` file: the image's format.
const FORMAT: &str = "fxdax extent store 1";

/// How many extents' files the device keeps open between operations.
const OPEN_FILES: usize = 256;

/// An image's description, its `store` file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Description {
    pub block_size: u32,
    /// The most bytes the image's extents may hold.
    pub capacity: u64,
    /// The branch whose extents the filesystem on the image mints.
    pub branch: u64,
}

impl Description {
    /// The description of the image at `dir`.
    pub fn read(dir: &Path) -> Result<Self, Error> {
        let path = dir.join("store");
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("opening {}", dir.display()))?;
        Self::parse(&text).with_context(|| format!("reading {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, Error> {
        let mut lines = text.lines();
        ensure!(lines.next() == Some(FORMAT), "not an image of this format");
        let mut fields = HashMap::new();
        for line in lines {
            let (key, value) = line.split_once(' ').ok_or_else(|| anyhow!("bad line {line:?}"))?;
            fields.insert(key, value.parse::<u64>().with_context(|| format!("bad {key}"))?);
        }
        let field = |key| fields.get(key).copied().ok_or_else(|| anyhow!("no {key}"));
        Ok(Self {
            block_size: field("block_size")?.try_into().context("bad block_size")?,
            capacity: field("capacity")?,
            branch: field("branch")?,
        })
    }

    /// Writes the description to the image at `dir`, replacing any it has whole.
    pub fn write(&self, dir: &Path) -> Result<(), Error> {
        let Self { block_size, capacity, branch } = self;
        let text =
            format!("{FORMAT}\nblock_size {block_size}\ncapacity {capacity}\nbranch {branch}\n");
        let temporary = dir.join("store.new");
        let mut file = File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join("store"))?;
        File::open(dir)?.sync_all()?;
        Ok(())
    }
}

/// The directory of extents whose sequence numbers' low byte is `low_byte`, in the image at `dir`.
pub(crate) fn extents_dir(dir: &Path, low_byte: u8) -> PathBuf {
    dir.join("extents").join(format!("{low_byte:02x}"))
}

/// The name of `extent`'s file.
fn file_name(extent: ExtentId) -> String {
    format!("{:016x}.{:016x}", extent.sequence, extent.branch)
}

/// The extent whose file has the name `name`.
fn parse_file_name(name: &str) -> Option<ExtentId> {
    let (sequence, branch) = name.split_once('.')?;
    if sequence.len() != 16 || branch.len() != 16 {
        return None;
    }
    Some(ExtentId::new(
        u64::from_str_radix(branch, 16).ok()?,
        u64::from_str_radix(sequence, 16).ok()?,
    ))
}

/// Creates an empty image at `dir`, which must not exist, as `description` describes.
pub(crate) fn create(dir: &Path, description: &Description) -> Result<(), Error> {
    std::fs::create_dir(dir).with_context(|| format!("creating {}", dir.display()))?;
    let made = || -> Result<(), Error> {
        std::fs::create_dir(dir.join("extents"))?;
        for low_byte in 0..=u8::MAX {
            std::fs::create_dir(extents_dir(dir, low_byte))?;
        }
        File::open(dir.join("extents"))?.sync_all()?;
        description.write(dir)?;
        // The image's directory entry is as durable as the image.
        let parent = dir.parent().filter(|parent| !parent.as_os_str().is_empty());
        File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    };
    made().inspect_err(|_| {
        // Leave nothing half made behind.
        let _ = std::fs::remove_dir_all(dir);
    })
}

/// The extents of the image at `dir`, with their files' sizes.
pub(crate) fn list_extents(dir: &Path) -> Result<Vec<(ExtentId, u64)>, Error> {
    let mut extents = Vec::new();
    for low_byte in 0..=u8::MAX {
        let extents_dir = extents_dir(dir, low_byte);
        for entry in std::fs::read_dir(&extents_dir)
            .with_context(|| format!("listing {}", extents_dir.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let extent = name
                .to_str()
                .and_then(parse_file_name)
                .filter(|extent| extent.sequence as u8 == low_byte)
                .ok_or_else(|| anyhow!("{name:?} in {} is no extent", extents_dir.display()))?;
            extents.push((extent, entry.metadata()?.len()));
        }
    }
    Ok(extents)
}

/// An Fxfs device on an image. See the module docs.
pub(crate) struct ImageDevice {
    dir: PathBuf,
    description: Description,
    io: Io,
    allocator: BufferAllocator,
    /// The image's `store` file, which flushes sync on macOS to flush the drive's cache.
    store: File,
    /// Whether I/O bypasses the host's page cache.
    direct: bool,
    state: Mutex<State>,
    /// The writes and frees, which a copy of the image pauses.
    writes: Arc<WriteGate>,
}

#[derive(Default)]
struct State {
    /// The extents, with their lengths.
    extents: HashMap<ExtentId, u64>,
    /// The bytes the extents hold.
    stored: u64,
    /// One more than the highest sequence number of the image's branch's extents.
    next_sequence: u64,
    /// Extents' open files, which operations in flight hold too.
    files: HashMap<ExtentId, Arc<OwnedFd>>,
    /// The extents written since the last flush.
    unsynced: HashSet<ExtentId>,
    /// The directories of extents whose files were created or removed since the last flush, by
    /// their extents' sequence numbers' low byte.
    unsynced_dirs: HashSet<u8>,
}

impl ImageDevice {
    /// The device of the image at `dir`, whose writes and frees go through `writes`.
    pub fn open(dir: &Path, writes: Arc<WriteGate>) -> Result<Self, Error> {
        let description = Description::read(dir)?;
        ensure!(
            description.capacity >= description.block_size as u64,
            "the image is smaller than a block"
        );
        let mut state = State::default();
        for (extent, len) in list_extents(dir)? {
            state.extents.insert(extent, len);
            state.stored += len;
            if extent.branch == description.branch {
                state.next_sequence = state.next_sequence.max(extent.sequence + 1);
            }
        }
        let store = File::open(dir.join("store")).context("opening the image's store file")?;
        let direct = bypass_page_cache(store.as_raw_fd());
        Ok(Self {
            dir: dir.to_owned(),
            io: Io::new().context("starting the image's I/O")?,
            allocator: BufferAllocator::new(
                description.block_size as usize,
                BufferSource::new(TRANSFER_HEAP_SIZE),
            ),
            description,
            store,
            direct,
            state: Mutex::new(state),
            writes,
        })
    }

    /// Whether I/O bypasses the host's page cache.
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    fn path(&self, extent: ExtentId) -> CString {
        let path = extents_dir(&self.dir, extent.sequence as u8).join(file_name(extent));
        CString::new(path.as_os_str().as_bytes()).expect("no NUL in an image's path")
    }

    fn check_alignment(&self, offset: u64, len: usize) -> Result<(), Error> {
        let block_size = self.description.block_size as u64;
        ensure!(
            offset.is_multiple_of(block_size) && (len as u64).is_multiple_of(block_size),
            "unaligned I/O"
        );
        Ok(())
    }

    /// Opens the file at `path` with `flags`, off the executor's threads.
    async fn open_path(&self, path: &CString, flags: libc::c_int) -> std::io::Result<OwnedFd> {
        // SAFETY: The path outlives the operation, which the future waits for even if dropped.
        let fd = unsafe { self.io.run(-1, Op::Open { path: path.as_ptr(), flags }) }.await?;
        // SAFETY: The operation opened the file descriptor, which nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
    }

    /// `extent`'s file, opened if it isn't, and created if `create` and it doesn't exist.
    async fn file(&self, extent: ExtentId, create: bool) -> Result<Arc<OwnedFd>, Error> {
        if let Some(file) = self.state.lock().unwrap().files.get(&extent) {
            return Ok(file.clone());
        }
        let flags = libc::O_RDWR | if create { libc::O_CREAT } else { 0 };
        let fd = self
            .open_path(&self.path(extent), flags)
            .await
            .with_context(|| format!("opening extent {extent:?}"))?;
        if self.direct {
            bypass_page_cache(fd.as_raw_fd());
        }
        let mut state = self.state.lock().unwrap();
        if state.files.len() >= OPEN_FILES {
            // Close files no operation is using.
            state.files.retain(|_, file| Arc::strong_count(file) > 1);
        }
        Ok(state.files.entry(extent).or_insert_with(|| Arc::new(fd)).clone())
    }

    /// Transfers `len` bytes at `offset` in `fd`'s file to or from memory at `addr`, as `op`
    /// makes operations of them, all of them unless the file ends.
    async fn transfer(
        &self,
        fd: RawFd,
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
            // SAFETY: The file is open as long as the caller holds it, which it does for the
            // operation, and the buffer is the caller's for the operation, which the future waits
            // for even if it is dropped.
            match unsafe { self.io.run(fd, operation) }.await {
                Ok(0) => bail!("the extent ended at {:#x}", offset + done as u64),
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

    /// Syncs the file at `path`, opened with `flags`, as `op` does.
    async fn sync_path(&self, path: PathBuf, flags: libc::c_int, op: Op) -> Result<(), Error> {
        let path = CString::new(path.as_os_str().as_bytes()).expect("no NUL in an image's path");
        let fd = self.open_path(&path, flags).await?;
        // SAFETY: The file is open until the operation, which has no buffer, completes.
        unsafe { self.io.run(fd.as_raw_fd(), op) }.await?;
        Ok(())
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
        self.check_alignment(offset, buffer.len())?;
        let Some(len) = self.state.lock().unwrap().extents.get(&extent).copied() else {
            bail!("no extent {extent:?} in the image");
        };
        let held = len.saturating_sub(offset).min(buffer.len() as u64) as usize;
        let (mut held_part, mut rest) = buffer.reborrow().split_at_mut(held);
        rest.fill(0);
        if held > 0 {
            let file = self.file(extent, false).await?;
            let addr = held_part.as_mut_ptr() as usize;
            // The host writes the buffer.
            self.transfer(
                file.as_raw_fd(),
                addr,
                held,
                offset,
                Access::Write,
                |addr, len, offset| Op::Read { addr, len, offset },
            )
            .await?;
        }
        Ok(())
    }

    async fn write_with_opts(
        &self,
        extent: ExtentId,
        offset: u64,
        buffer: BufferRef<'_>,
        _write_opts: WriteOptions,
    ) -> Result<(), Error> {
        self.check_alignment(offset, buffer.len())?;
        let _writing = self.writes.write().await;
        let end = offset + buffer.len() as u64;
        {
            let mut state = self.state.lock().unwrap();
            let len = state.extents.get(&extent).copied();
            let grows = end.saturating_sub(len.unwrap_or(0));
            ensure!(
                state.stored + grows <= self.description.capacity,
                "the image is full: its extents hold {} of {} bytes, and need {grows} more",
                state.stored,
                self.description.capacity,
            );
            state.stored += grows;
            state.extents.insert(extent, len.unwrap_or(0).max(end));
            state.unsynced.insert(extent);
            if len.is_none() {
                state.unsynced_dirs.insert(extent.sequence as u8);
            }
            if extent.branch == self.description.branch {
                state.next_sequence = state.next_sequence.max(extent.sequence + 1);
            }
        }
        let file = self.file(extent, true).await?;
        let (addr, len) = (buffer.as_ptr() as usize, buffer.len());
        // The host reads the buffer.
        self.transfer(file.as_raw_fd(), addr, len, offset, Access::Read, |addr, len, offset| {
            Op::Write { addr, len, offset }
        })
        .await
    }

    fn branch(&self) -> u64 {
        self.description.branch
    }

    async fn next_sequence(&self) -> Result<u64, Error> {
        Ok(self.state.lock().unwrap().next_sequence)
    }

    async fn free(&self, extent: ExtentId) -> Result<(), Error> {
        let _writing = self.writes.write().await;
        {
            let mut state = self.state.lock().unwrap();
            let Some(len) = state.extents.remove(&extent) else {
                return Ok(());
            };
            state.stored -= len;
            state.files.remove(&extent);
            state.unsynced.remove(&extent);
            state.unsynced_dirs.insert(extent.sequence as u8);
        }
        let path = self.path(extent);
        // SAFETY: The path outlives the operation, which the future waits for even if dropped.
        match unsafe { self.io.run(-1, Op::Unlink { path: path.as_ptr() }) }.await {
            Ok(_) => Ok(()),
            Err(err) if err.raw_os_error() == Some(libc::ENOENT) => Ok(()),
            Err(err) => Err(Error::new(err).context(format!("freeing extent {extent:?}"))),
        }
    }

    async fn close(&self) -> Result<(), Error> {
        self.flush().await
    }

    async fn flush(&self) -> Result<(), Error> {
        let (extents, dirs) = {
            let mut state = self.state.lock().unwrap();
            (std::mem::take(&mut state.unsynced), std::mem::take(&mut state.unsynced_dirs))
        };
        let sync_extent = |extent| async move {
            let file = match self.file(extent, false).await {
                Ok(file) => file,
                // Freed since.
                Err(_) if !self.state.lock().unwrap().extents.contains_key(&extent) => {
                    return Ok(());
                }
                Err(err) => return Err(err),
            };
            // SAFETY: The file is open until the operation, which has no buffer, completes.
            unsafe { self.io.run(file.as_raw_fd(), Op::Sync) }.await?;
            Ok::<_, Error>(())
        };
        futures::future::try_join_all(extents.into_iter().map(sync_extent))
            .await
            .context("syncing the image's extents")?;
        futures::future::try_join_all(dirs.into_iter().map(|low_byte| {
            self.sync_path(extents_dir(&self.dir, low_byte), libc::O_RDONLY, Op::Sync)
        }))
        .await
        .context("syncing the image's directories")?;
        if cfg!(target_os = "macos") {
            // SAFETY: The file is open as long as the device, and the operation has no buffer.
            unsafe { self.io.run(self.store.as_raw_fd(), Op::SyncData) }
                .await
                .context("flushing the image")?;
        }
        Ok(())
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

/// Makes I/O on `fd`'s file bypass the host's page cache, if its filesystem allows, and returns
/// whether it does.
fn bypass_page_cache(fd: RawFd) -> bool {
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
    use std::os::unix::fs::FileExt;
    use zircon::vm::arch_vm_aspace::{NonTerminalAction, TerminalAction};
    use zircon::vm::vm_aspace::VmAspace;

    const BLOCK: u32 = 4096;
    const MIB: usize = 1 << 20;
    const BRANCH: u64 = 7;

    /// A new image holding `capacity` bytes, in a new directory.
    fn image(capacity: u64) -> (PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        create(&image, &Description { block_size: BLOCK, capacity, branch: BRANCH }).unwrap();
        (image, dir)
    }

    /// A device on a new image holding `capacity` bytes, and the image.
    fn device(capacity: u64) -> (ImageDevice, PathBuf, tempfile::TempDir) {
        let (image, dir) = image(capacity);
        (ImageDevice::open(&image, Default::default()).unwrap(), image, dir)
    }

    fn extent(sequence: u64) -> ExtentId {
        ExtentId::new(BRANCH, sequence)
    }

    /// The path of `extent`'s file in `image`.
    fn path(image: &Path, extent: ExtentId) -> PathBuf {
        extents_dir(image, extent.sequence as u8).join(file_name(extent))
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
        let (device, _, _dir) = device(32 * MIB as u64);
        assert_eq!(device.block_count(), 32 * MIB as u64 / BLOCK as u64);
        assert_eq!(device.branch(), BRANCH);
        block_on(async {
            // Many at once, which all complete.
            let data: Vec<Vec<u8>> = (0..16).map(|i| pattern(i * MIB, MIB)).collect();
            futures::future::join_all(
                data.iter().enumerate().map(|(i, data)| write(&device, extent(i as u64), 0, data)),
            )
            .await;
            device.flush().await.unwrap();
            let all =
                futures::future::join_all((0..16).map(|i| read(&device, extent(i), 0, MIB))).await;
            for (i, data) in all.iter().enumerate() {
                assert!(*data == pattern(i * MIB, MIB), "extent {i} differs");
            }
            assert_eq!(device.next_sequence().await.unwrap(), 16);

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
            assert!(device.read(extent(16), 0, buffer.as_mut()).await.is_err());
            assert!(device.read(ExtentId::new(BRANCH + 1, 0), 0, buffer.as_mut()).await.is_err());
        });
    }

    /// An image holds what was flushed when it is opened again.
    #[test]
    fn extents_survive_reopening() {
        let (device, image, _dir) = device(16 * MIB as u64);
        block_on(async {
            write(&device, extent(3), 0, &pattern(0, MIB)).await;
            write(&device, extent(259), 0, &pattern(1, 2 * BLOCK as usize)).await;
            write(&device, ExtentId::new(BRANCH + 1, 1000), 0, &pattern(2, BLOCK as usize)).await;
            device.flush().await.unwrap();
        });
        drop(device);
        let device = ImageDevice::open(&image, Default::default()).unwrap();
        block_on(async {
            assert_eq!(read(&device, extent(3), 0, MIB).await, pattern(0, MIB));
            assert_eq!(
                read(&device, extent(259), 0, 2 * BLOCK as usize).await,
                pattern(1, 2 * BLOCK as usize)
            );
            let other = ExtentId::new(BRANCH + 1, 1000);
            assert_eq!(read(&device, other, 0, BLOCK as usize).await, pattern(2, BLOCK as usize));
            // Other branches' extents don't count.
            assert_eq!(device.next_sequence().await.unwrap(), 260);
        });
        assert_eq!(device.state.lock().unwrap().stored, (MIB + 3 * BLOCK as usize) as u64);
    }

    /// Freeing an extent removes its file, and the capacity it took.
    #[test]
    fn frees_remove_extents() {
        let (device, image, _dir) = device(4 * MIB as u64);
        block_on(async {
            write(&device, extent(1), 0, &pattern(0, 3 * MIB)).await;
            device.flush().await.unwrap();
            assert!(path(&image, extent(1)).exists());
            let mut buffer = device.allocate_buffer(2 * MIB).await;
            assert!(device.write(extent(2), 0, buffer.as_ref()).await.is_err(), "overfilled");

            device.free(extent(1)).await.unwrap();
            device.free(extent(1)).await.expect("freeing a freed extent failed");
            device.flush().await.unwrap();
            assert!(!path(&image, extent(1)).exists());
            assert!(device.read(extent(1), 0, buffer.as_mut()).await.is_err());
            device.write(extent(2), 0, buffer.as_ref()).await.expect("write failed");
            // Freeing doesn't make sequence numbers available again.
            assert_eq!(device.next_sequence().await.unwrap(), 3);
        });
    }

    #[test]
    fn descriptions() {
        let description = Description { block_size: BLOCK, capacity: 1 << 30, branch: u64::MAX };
        let (image, _dir) = image(MIB as u64);
        description.write(&image).unwrap();
        assert_eq!(Description::read(&image).unwrap(), description);
        std::fs::write(image.join("store"), "fxdax extent store 2\n").unwrap();
        assert!(Description::read(&image).is_err());
        std::fs::write(image.join("store"), format!("{FORMAT}\nblock_size 4096\n")).unwrap();
        assert!(Description::read(&image).is_err());
    }

    /// The kernel's page scanner makes transfer buffers' pages inaccessible to track their
    /// accesses, which the host's I/O into them fails on; the device's I/O faults them back in.
    #[test]
    fn io_into_harvested_buffers() {
        let (device, image, _dir) = device(4 * MIB as u64);
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
            let file = File::open(path(&image, extent(1))).unwrap();
            // SAFETY: Reads into the buffer, which is the test's.
            let read = unsafe {
                libc::pread(file.as_raw_fd(), buffer.as_mut().as_mut_ptr().cast(), MIB, 0)
            };
            assert_eq!(read, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EFAULT));

            harvest();
            device.read(extent(1), 0, buffer.as_mut()).await.unwrap();
            assert!(contents(buffer.as_ref()) == pattern(0, MIB));
            harvest();
            device.write(extent(2), 0, buffer.as_ref()).await.unwrap();
            assert_eq!(std::fs::read(path(&image, extent(2))).unwrap(), pattern(0, MIB));
        });
    }

    /// A fresh device's first reads land in buffers the kernel maps read-only, as the zero page,
    /// until they are written; the host's reads into them fault them in.
    #[test]
    fn first_reads_into_unwritten_buffers() {
        let (image, _dir) = image(4 * MIB as u64);
        // Through a file of its own, as the device's bypasses the page cache, needing aligned I/O.
        let file = File::create(path(&image, extent(1))).unwrap();
        file.write_all_at(&pattern(0, MIB), 0).unwrap();
        file.sync_all().unwrap();
        let device = ImageDevice::open(&image, Default::default()).unwrap();
        assert!(block_on(read(&device, extent(1), 0, MIB)) == pattern(0, MIB));
    }
}

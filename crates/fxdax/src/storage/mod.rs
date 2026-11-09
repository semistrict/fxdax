// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The storage a filesystem lives on: an image, or memory.

mod collect;
mod gate;
mod image;
mod io;
mod kept;

use crate::error::{Error, Result};
use crate::executor::Executor;
use crate::filesystem::DEFAULT_EXECUTOR_THREADS;
use crate::host_store;
use anyhow::Context as _;
use fxfs::filesystem::FxFilesystem;
use fxfs::fsck::errors::FsckIssue;
use fxfs::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use fxfs::object_store::volume::{list_volumes, root_volume};
use gate::WriteGate;
use image::ImageDevice;
use kept::Kept;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use storage_device::DeviceHolder;
use storage_device::fake_device::FakeDevice;

/// The storage a filesystem lives on.
///
/// A filesystem takes its storage when it mounts it, and gives it back when it is closed, ready
/// to be mounted or checked again.
pub struct Storage {
    device: DeviceHolder,
    kind: Kind,
}

#[derive(Clone)]
pub(crate) enum Kind {
    /// An image: its directory, whether the device's I/O bypasses the page cache, the device's
    /// writes and frees, which cloning the image pauses, and its store, kept in its bucket, which
    /// a clone forks.
    Image {
        dir: PathBuf,
        direct: bool,
        writes: Arc<WriteGate>,
        image: Arc<Kept>,
    },
    Memory,
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image { dir, direct, .. } => {
                f.debug_struct("Image").field("dir", dir).field("direct", direct).finish()
            }
            Self::Memory => write!(f, "Memory"),
        }
    }
}

impl Storage {
    /// The size of the blocks storage is read and written in. Storage sizes are multiples of it.
    pub const BLOCK_SIZE: u32 = host_store::BLOCK_SIZE;

    /// The image at `path`: a directory holding an extent store (see `image`), a clone's or not,
    /// its branch kept in the bucket its description names (see `kept`).
    ///
    /// Its I/O bypasses the host's page cache where the host filesystem allows, as the in-process
    /// kernel caches files' pages already, and runs on threads of its own (io_uring on Linux).
    /// It fails for an image made by older code, which kept its branch in no bucket, and which
    /// nothing converts.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = path.as_ref().to_owned();
        let writes = Arc::new(WriteGate::default());
        let device = ImageDevice::open(&dir, writes.clone()).map_err(Error::new)?;
        Ok(Self::image(dir, writes, device))
    }

    /// A new image at `path`, which must not exist, holding at most `size` bytes. It holds nothing
    /// until the filesystem writes to it, and must be formatted before it is mounted. Its branch
    /// is kept in a file bucket of its own, beside its directory (`<image>.bucket`), which its
    /// clones share and which outlives it; or, on a host of fxdax's system, in the host's bucket
    /// (`FXDAX_EXTENTD_REMOTE`).
    pub fn create(path: impl AsRef<Path>, size: u64) -> Result<Self, Error> {
        Self::make(path.as_ref(), size, true, Self::keeping()?)
    }

    /// fxdax: a new image at `path`, as [`Storage::create`] makes it, unmade until its maker says
    /// it is whole ([`Storage::made`]): what opens it before, or after its maker died part way,
    /// finds no store (TASK-196). As an import makes one, filling it first.
    pub fn create_unmade(path: impl AsRef<Path>, size: u64) -> Result<Self, Error> {
        Self::make(path.as_ref(), size, false, Self::keeping()?)
    }

    /// fxdax: a new image at `path`, as [`Storage::create_unmade`] makes it, its branch kept as
    /// `keeping` says: as the extent daemon's stores keep theirs, with the host's cache of the
    /// bucket's objects, for an import the daemon makes (TASK-237).
    pub(crate) fn create_unmade_keeping(
        path: impl AsRef<Path>,
        size: u64,
        keeping: extent_store::upload::Options,
    ) -> Result<Self, Error> {
        Self::make(path.as_ref(), size, false, keeping)
    }

    /// How an image this process makes, where no host's daemon serves it, keeps its branch
    /// ([`host_store::keeping`]): with no cache of the bucket's objects.
    fn keeping() -> Result<extent_store::upload::Options, Error> {
        host_store::keeping().map_err(Error::new)
    }

    /// fxdax: the image at `path`, made by [`Storage::create_unmade`] and closed since, is whole.
    pub fn made(path: impl AsRef<Path>) -> Result<(), Error> {
        extent_store::made_host(path.as_ref()).map_err(Error::new)
    }

    /// fxdax: whether `path` is what a making of an image, or of a store, left unmade, its maker
    /// dying part way: no store, which deleting finishes (TASK-196).
    pub fn is_unmade(path: impl AsRef<Path>) -> bool {
        host_store::unmade(path.as_ref())
    }

    /// A new image at `path`, as [`Storage::create`] makes it, whole if `whole`, its branch kept
    /// as `keeping` says.
    fn make(
        path: &Path,
        size: u64,
        whole: bool,
        keeping: extent_store::upload::Options,
    ) -> Result<Self, Error> {
        if size < Self::BLOCK_SIZE as u64 {
            return Err(Error::msg("the image is smaller than a block"));
        }
        let writes = Arc::new(WriteGate::default());
        let made = (size, host_store::new_branch(), whole);
        let device =
            ImageDevice::create(path, made, keeping, writes.clone()).map_err(Error::new)?;
        Ok(Self::image(path.to_owned(), writes, device))
    }

    /// The storage of the image at `dir` whose device is `device`, its writes going through
    /// `writes`.
    fn image(dir: PathBuf, writes: Arc<WriteGate>, device: ImageDevice) -> Self {
        let (direct, image) = (device.is_direct(), device.image());
        let kind = Kind::Image { dir, direct, writes, image };
        Self { device: DeviceHolder::new(device), kind }
    }

    /// `size` bytes of memory, which hold a filesystem until they are dropped, such as for a
    /// scratch volume or a test.
    ///
    /// # Panics
    ///
    /// If `size` is not a positive multiple of [`Storage::BLOCK_SIZE`].
    pub fn memory(size: u64) -> Self {
        let block_size = Self::BLOCK_SIZE as u64;
        assert!(size > 0 && size.is_multiple_of(block_size), "{size} is not a multiple of blocks");
        let device = FakeDevice::new(size / block_size, Self::BLOCK_SIZE).calm();
        Self { device: DeviceHolder::new(device), kind: Kind::Memory }
    }

    /// Whether the storage is an image whose I/O bypasses the host's page cache.
    pub fn bypasses_page_cache(&self) -> bool {
        matches!(self.kind, Kind::Image { direct: true, .. })
    }

    /// Clones the image to a new image at `path`, which must not exist, and returns its storage.
    /// The clone is a fork of the image's branch in its bucket, which the clone keeps its own
    /// branch in: it takes the same space whatever the image holds, inheriting the image's
    /// extents rather than copies, read from the bucket (TASK-165.7). `path` is the clone's own
    /// directory. Memory cannot be cloned.
    ///
    /// The storage must not be mounted, which it is not while it is the caller's. To clone a
    /// mounted filesystem's, see [`Filesystem::clone_storage_to`](crate::Filesystem::clone_storage_to).
    pub async fn clone_to(&self, path: impl AsRef<Path>) -> Result<Storage, Error> {
        let Kind::Image { image, writes, .. } = &self.kind else {
            return Err(Error::msg("memory cannot be cloned to an image"));
        };
        clone_image(image.clone(), writes.clone(), path.as_ref()).await
    }

    /// Deletes the image at `path`, which nothing in this process has open, a clone or not: its
    /// branch in its bucket, whose collector reclaims what no other branch inherited from it, and
    /// its directory. Its clones live on: their branches are the bucket's, which an image's own
    /// keeps beside its directory, not in it (TASK-165.7). Fails with `ENOENT` if there is none
    /// at `path`.
    pub fn delete(path: impl AsRef<Path>) -> Result<(), Error> {
        kept::delete(path.as_ref())
    }

    /// The most bytes the storage holds.
    pub fn size(&self) -> u64 {
        self.device.block_count() * self.device.block_size() as u64
    }

    /// Checks the image on the storage and every volume in it, failing on any inconsistency, or
    /// anything it warns about, and gives the storage back.
    pub async fn check(self) -> Result<Self, Error> {
        let Self { device, kind } = self;
        let executor = Executor::new(DEFAULT_EXECUTOR_THREADS);
        let device = executor.run(async move {
            let filesystem = FxFilesystem::open(device).await?;
            // fxdax: what fsck found, which its error says, as well as how many (TASK-82).
            let issues = Mutex::new(Vec::new());
            let options = FsckOptions {
                fail_on_warning: true,
                on_error: Box::new(|issue: &FsckIssue| {
                    issue.log();
                    issues.lock().unwrap().push(issue.clone());
                }),
                ..Default::default()
            };
            let checked = async {
                fsck_with_options(filesystem.clone(), &options).await?;
                let store_ids =
                    list_volumes(root_volume(filesystem.clone()).await?.volume_directory()).await?;
                for store_id in store_ids {
                    fsck_volume_with_options(filesystem.as_ref(), &options, store_id, None).await?;
                }
                anyhow::Ok(())
            };
            checked.await.with_context(|| format!("fsck found {:?}", issues.lock().unwrap()))?;
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        });
        Ok(Self::closed(device.await.map_err(Error::new)?, kind))
    }

    /// fxdax: the storage's device, for what keeps itself on a device in place of Fxfs, as a
    /// SQLite volume does (crates/sqlite_extents, TASK-296). Its user closes it: an image's
    /// close waits for its commits to be uploaded.
    pub fn into_device(self) -> DeviceHolder {
        self.device
    }

    pub(crate) fn into_parts(self) -> (DeviceHolder, Kind) {
        (self.device, self.kind)
    }

    /// The storage of `device`, which a filesystem closed.
    pub(crate) fn closed(device: DeviceHolder, kind: Kind) -> Self {
        match kind {
            // Closing an image only flushes it.
            Kind::Image { .. } => {}
            // Closing memory makes it refuse I/O until it is reopened.
            Kind::Memory => device.reopen(false),
        }
        Self { device, kind }
    }
}

impl fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage").field("kind", &self.kind).field("size", &self.size()).finish()
    }
}

/// Clones `image` to a new image at `path`, a fork of its store, and of its branch in its bucket,
/// with a branch of its own ([`Kept::fork`]), on a thread of its own, as forking commits the store
/// durably and waits for the fork's upload. The clone pauses `writes`, the device's writes and
/// frees, until the fork's commit is made, so that it is the image as it was at one time.
pub(crate) async fn clone_image(
    image: Arc<Kept>,
    writes: Arc<WriteGate>,
    path: &Path,
) -> Result<Storage, Error> {
    let path = path.to_owned();
    let (done, cloned) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("fxdax clone".into())
        .spawn(move || {
            let forked = image.fork(&path, writes.pause());
            let cloned =
                forked.map_err(|error| error.context(format!("cloning to {}", path.display())));
            let _ = done.send(cloned.map(|()| path));
        })
        .map_err(Error::new)?;
    let path = cloned.await.map_err(Error::new)??;
    Storage::open(path)
}

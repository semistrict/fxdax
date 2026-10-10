//! A filesystem: an Fxfs volume served by inode number, as FUSE serves one.
//!
//! [`Filesystem`] is a fuchsia.io client of the volume, as Starnix's remote filesystems and fdio
//! are: every inode the guest has looked up keeps a connection to its node, opened by name from its
//! parent directory with `FLAG_SEND_REPRESENTATION`, whose representation gives the node's protocol
//! and attributes. File contents are read and written through the file's stream, as futures that
//! the kernel wakes once its pager has supplied or dirtied their pages.

use crate::connection::{self, Connection, RIGHTS};
use crate::dax::{Dax, DaxWindow, SecondStage};
use crate::error::{Errno, Error, IntoErrno, Result};
use crate::executor::{Executor, assert_not_on_executor};
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::node::*;
use crate::storage::{CloneKind, Kind, Storage, clone_image};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_fs_startup::CreateOptions;
use fidl_fuchsia_io as fio;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use fxfs::filesystem::{FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
use fxfs::object_store::volume::root_volume;
use fxfs_platform::file::FlushType;
use fxfs_platform::memory_pressure::MemoryPressureMonitor;
use fxfs_platform::volume::{FxVolumeAndRoot, MemoryPressureConfig};
use fxfs_platform::volumes_directory::VolumesDirectory;
use refaults_vmo::PageRefaultCounter;
use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex};
use storage_device::DeviceHolder;
use zircon::user_copy::HostBuffer;

/// The name of the volume served.
pub(crate) const VOLUME_NAME: &str = "default";

/// The number of threads of a filesystem's executor, unless [`Options`] say otherwise.
pub(crate) const DEFAULT_EXECUTOR_THREADS: u8 = 4;

/// How a filesystem is mounted.
#[derive(Clone, Debug)]
pub struct Options {
    executor_threads: NonZeroU8,
}

impl Options {
    /// The default options.
    pub fn new() -> Self {
        Self { executor_threads: NonZeroU8::new(DEFAULT_EXECUTOR_THREADS).unwrap() }
    }

    /// The number of threads Fxfs's work runs on: its journal, pager and background writeback.
    /// They are separate from the tasks and threads calling the filesystem. The default is 4.
    pub fn executor_threads(&mut self, threads: NonZeroU8) -> &mut Self {
        self.executor_threads = threads;
        self
    }

    /// Mounts the filesystem on `storage`, which [`format`](Self::format) formatted.
    pub async fn mount(&self, storage: Storage) -> Result<Filesystem, Error> {
        Filesystem::start(self, storage, false).await
    }

    /// Formats `storage` with an empty filesystem, and mounts it.
    pub async fn format(&self, storage: Storage) -> Result<Filesystem, Error> {
        Filesystem::start(self, storage, true).await
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// A mounted Fxfs filesystem, served by inode number as FUSE serves one. See the crate docs.
///
/// Its methods may be called from any number of tasks or threads at once (see the crate docs).
pub struct Filesystem {
    executor: Executor,
    /// The filesystem and volume, until they are unmounted.
    mounted: Mutex<Option<Mounted>>,
    root_object_id: u64,
    block_size: u32,
    state: Mutex<State>,
    /// The DAX window, once it is created.
    dax: Mutex<Option<Dax>>,
    /// Completes once the filesystem is unmounted, once unmounting has begun.
    unmounted: Mutex<Option<Shared<oneshot::Receiver<()>>>>,
    /// What unmounting gave back, until it is taken.
    closed: Arc<Mutex<Option<Result<Storage, Error>>>>,
}

struct Mounted {
    filesystem: OpenFxFilesystem,
    /// The volume's volumes directory, which runs its background task and limits its dirty pages
    /// under memory pressure, as Fxfs's component does.
    volumes_directory: Arc<VolumesDirectory>,
    volume: FxVolumeAndRoot,
    storage_kind: Kind,
}

/// An inode the guest has looked up.
struct Node {
    connection: Connection,
    /// The guest's lookup count; the root's is never forgotten.
    lookups: u64,
    /// For a directory, its parent's inode and its name, which `..` and renames need.
    location: Option<(Inode, Vec<u8>)>,
}

struct State {
    /// The inodes the guest has looked up, until the filesystem is unmounted.
    nodes: Option<BTreeMap<Inode, Node>>,
    /// Open directories: snapshots of their entries, including `.` and `..`.
    dirs: BTreeMap<DirHandle, Arc<[DirEntry]>>,
    next_handle: u64,
}

impl State {
    fn nodes(&mut self) -> Result<&mut BTreeMap<Inode, Node>> {
        self.nodes.as_mut().ok_or(Errno::ENOTCONN)
    }
}

impl Filesystem {
    /// Mounts the filesystem on `storage` with the default [`Options`].
    pub async fn mount(storage: Storage) -> Result<Self, Error> {
        Options::new().mount(storage).await
    }

    /// Formats `storage` with an empty filesystem, and mounts it with the default [`Options`].
    pub async fn format(storage: Storage) -> Result<Self, Error> {
        Options::new().format(storage).await
    }

    async fn start(options: &Options, storage: Storage, format: bool) -> Result<Self, Error> {
        let (device, storage_kind) = storage.into_parts();
        let executor = Executor::new(options.executor_threads.get());
        let (mounted, root) = executor
            .run(async move {
                let filesystem = FxFilesystemBuilder::new().format(format).open(device).await?;
                let volumes_directory = VolumesDirectory::new(
                    root_volume(filesystem.clone()).await?,
                    Some(MemoryPressureMonitor::start()?),
                    Arc::new(PageRefaultCounter::new()?),
                    MemoryPressureConfig::default(),
                )
                .await?;
                let volume = if format {
                    volumes_directory
                        .create_and_mount_volume(VOLUME_NAME, None, false, CreateOptions::default())
                        .await?
                } else {
                    volumes_directory.mount_volume(VOLUME_NAME, None, false).await?
                };
                let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
                volume.root().clone().serve(RIGHTS, server_end);
                let mounted = Mounted { filesystem, volumes_directory, volume, storage_kind };
                anyhow::Ok((mounted, root))
            })
            .await
            .map_err(Error::new)?;
        let root_object_id = mounted.volume.volume().store().root_directory_object_id();
        let block_size = mounted.filesystem.block_size().get() as u32;
        let root = Node { connection: Connection::Directory(root), lookups: 1, location: None };
        Ok(Self {
            executor,
            mounted: Mutex::new(Some(mounted)),
            root_object_id,
            block_size,
            state: Mutex::new(State {
                nodes: Some(BTreeMap::from([(Inode::ROOT, root)])),
                dirs: BTreeMap::new(),
                next_handle: 1,
            }),
            dax: Mutex::new(None),
            unmounted: Mutex::new(None),
            closed: Arc::new(Mutex::new(None)),
        })
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty, and gives its
    /// storage back.
    pub async fn close(self) -> Result<Storage, Error> {
        self.unmount().await?;
        self.closed
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(Error::msg("unmounting the filesystem failed")))
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty, and keeps its
    /// storage for [`close`](Self::close) to give back. Every operation fails with `ENOTCONN`
    /// from then on.
    ///
    /// Once unmounting begins, it completes even if the future is dropped. Concurrent calls wait
    /// for the same unmount, and only the first gives its error.
    pub async fn unmount(&self) -> Result<(), Error> {
        let unmounted = match self.begin_unmount() {
            Some((closing, storage_kind, done)) => {
                let closed = self.closed.clone();
                self.executor
                    .run(async move {
                        let storage = closing
                            .await
                            .map(|device| Storage::closed(device, storage_kind))
                            .map_err(Error::new);
                        *closed.lock().unwrap() = Some(storage);
                        let _ = done.send(());
                    })
                    .await;
                let mut closed = self.closed.lock().unwrap();
                if let Some(Err(_)) = &*closed {
                    return closed.take().unwrap().map(|_| ());
                }
                return Ok(());
            }
            None => self.unmounted.lock().unwrap().clone(),
        };
        if let Some(unmounted) = unmounted {
            let _ = unmounted.await;
        }
        Ok(())
    }

    /// Clones the filesystem's image to a new file at `path`, which must not exist, while it is
    /// mounted, and returns the clone's storage and how it was cloned (see
    /// [`Storage::clone_to`]).
    ///
    /// Every file's dirty pages are written back and the journal is committed, flushing the image,
    /// before it is cloned, so the clone has everything written before this was called. Writes
    /// made while it is cloning may be in the clone or not, as after a crash, which the clone
    /// recovers from as Fxfs does. Where the host filesystem can't share the image's blocks, the
    /// image's writes wait while it is copied.
    pub async fn clone_storage_to(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(Storage, CloneKind), Error> {
        let (file, writes, volume, filesystem) = {
            let mounted = self.mounted.lock().unwrap();
            let mounted = mounted.as_ref().ok_or(Errno::ENOTCONN)?;
            let Kind::File { file, writes, .. } = &mounted.storage_kind else {
                return Err(Error::msg("memory cannot be cloned to a file"));
            };
            let volume = mounted.volume.volume().clone();
            (file.clone(), writes.clone(), volume, (*mounted.filesystem).clone())
        };
        self.executor
            .run(async move {
                volume.flush_all_files(FlushType::Sync).await;
                filesystem.sync(SyncOptions { flush_device: true, ..Default::default() }).await
            })
            .await
            .map_err(Error::new)?;
        clone_image(file, writes, path.as_ref()).await
    }

    /// Begins unmounting the filesystem, unless it is unmounted: forgets its connections and
    /// unmaps its DAX window, and returns the future that closes the volume and filesystem on
    /// Fxfs's executor, the kind of storage it gives back, and the sender to signal once it is
    /// closed, which concurrent unmounts wait for.
    #[allow(clippy::type_complexity)]
    fn begin_unmount(
        &self,
    ) -> Option<(
        impl Future<Output = anyhow::Result<DeviceHolder>> + Send + 'static,
        Kind,
        oneshot::Sender<()>,
    )> {
        let (Mounted { filesystem, volumes_directory, volume, storage_kind }, done) = {
            let mut mounted = self.mounted.lock().unwrap();
            let mounted = mounted.take()?;
            // While `mounted` is locked, so that a concurrent unmount finding it taken finds this.
            let (done, unmounted) = oneshot::channel();
            *self.unmounted.lock().unwrap() = Some(unmounted.shared());
            (mounted, done)
        };
        // Unmap the files in the DAX window, and close every connection, so that the volume's nodes
        // can be dropped. The filesystem is closed even if unmapping fails.
        if let Some(dax) = self.dax.lock().unwrap().as_mut()
            && let Err(error) = dax.unmap_all()
        {
            log::error!("fxdax: failed to unmap the DAX window: {error:#}");
        }
        let nodes = {
            let mut state = self.state.lock().unwrap();
            state.dirs.clear();
            state.nodes.take().unwrap_or_default()
        };
        let closing = async move {
            for (_, node) in nodes {
                if let Connection::Directory(proxy) = node.connection {
                    let _ = proxy.close().await;
                }
            }
            drop(volume);
            volumes_directory.terminate().await;
            drop(volumes_directory);
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        };
        Some((closing, storage_kind, done))
    }

    /// The inode number of the object `object_id`.
    fn inode(&self, object_id: u64) -> Inode {
        if object_id == self.root_object_id {
            Inode::ROOT
        } else {
            assert_ne!(object_id, Inode::ROOT.0, "object 1 would have the root's inode number");
            Inode(object_id)
        }
    }

    fn connection(&self, inode: Inode) -> Result<Connection> {
        let mut state = self.state.lock().unwrap();
        Ok(state.nodes()?.get(&inode).ok_or(Errno::ENOENT)?.connection.clone())
    }

    fn directory(&self, inode: Inode) -> Result<fio::DirectoryProxy> {
        // `Clone::clone`, as the proxy's `clone` is fuchsia.io's `Clone`.
        Ok(Clone::clone(self.connection(inode)?.directory()?))
    }

    fn file(&self, inode: Inode) -> Result<fio::FileProxy> {
        Ok(Clone::clone(self.connection(inode)?.file()?.0))
    }

    fn stream(&self, inode: Inode) -> Result<Arc<zx::Stream>> {
        Ok(self.connection(inode)?.file()?.1.clone())
    }

    /// Takes a lookup on the node `connection` connects to, remembering it if it's new, and
    /// returns its attributes.
    fn remember(
        &self,
        parent: Inode,
        name: &[u8],
        connection: Connection,
        attributes: fio::Attributes,
    ) -> Result<Attr> {
        let attr = self.attr(&connection, attributes)?;
        let location =
            matches!(connection, Connection::Directory(_)).then(|| (parent, name.to_owned()));
        let mut state = self.state.lock().unwrap();
        let node =
            state.nodes()?.entry(attr.inode).or_insert(Node { connection, lookups: 0, location });
        node.lookups += 1;
        Ok(attr)
    }

    fn attr(&self, connection: &Connection, (mutable, immutable): fio::Attributes) -> Result<Attr> {
        // Special files are Fxfs files whose mode says what they are, as Starnix makes them.
        let file_type =
            mutable.mode.and_then(FileType::from_mode).unwrap_or(connection.file_type());
        let perm = mutable.mode.map_or(file_type.default_perm(), |mode| (mode & PERM_MASK) as u16);
        let object_id = immutable.id.ok_or(Errno::EIO)?;
        Ok(Attr {
            inode: self.inode(object_id),
            size: immutable.content_size.unwrap_or(0),
            blocks: immutable.storage_size.unwrap_or(0).div_ceil(512),
            atime: time_from_nanos(mutable.access_time.unwrap_or(0)),
            mtime: time_from_nanos(mutable.modification_time.unwrap_or(0)),
            ctime: time_from_nanos(immutable.change_time.unwrap_or(0)),
            file_type,
            perm,
            nlink: immutable.link_count.unwrap_or(1) as u32,
            uid: mutable.uid.unwrap_or(0),
            gid: mutable.gid.unwrap_or(0),
            rdev: mutable.rdev.unwrap_or(0) as u32,
            blksize: self.block_size,
        })
    }

    /// Opens `name` in `parent` with `flags`, and returns a connection to the node and its
    /// attributes.
    async fn open_node(
        &self,
        parent: &fio::DirectoryProxy,
        name: &[u8],
        flags: fio::Flags,
        create_attributes: Option<fio::MutableNodeAttributes>,
    ) -> Result<(Connection, fio::Attributes)> {
        let name = connection::name(name)?;
        let parent = Clone::clone(parent);
        self.executor
            .run(async move { connection::open(&parent, &name, flags, create_attributes).await })
            .await
    }

    /// Creates `name` in `parent` as a `protocol` node with `mode`, owned by `owner`.
    async fn create_node(
        &self,
        parent: Inode,
        name: &[u8],
        protocol: fio::Flags,
        mode: u32,
        rdev: u32,
        owner: Owner,
    ) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let (connection, attributes) = self
            .open_node(
                &directory,
                name,
                protocol | fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FLAG_MUST_CREATE,
                Some(fio::MutableNodeAttributes {
                    mode: Some(mode),
                    uid: Some(owner.uid),
                    gid: Some(owner.gid),
                    rdev: Some(rdev as u64),
                    ..Default::default()
                }),
            )
            .await?;
        self.remember(parent, name, connection, attributes)
    }

    /// Looks up `name` in directory `parent`, taking a lookup on its inode (`FUSE_LOOKUP`), which
    /// stays valid until [`forget`](Self::forget) releases it.
    pub async fn lookup(&self, parent: Inode, name: &[u8]) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let (connection, attributes) =
            self.open_node(&directory, name, fio::Flags::empty(), None).await?;
        self.remember(parent, name, connection, attributes)
    }

    /// Releases `count` lookups on `inode` (`FUSE_FORGET`). With none left, its inode number is
    /// invalid until it is looked up again. The root's are never released.
    pub fn forget(&self, inode: Inode, count: u64) {
        if inode == Inode::ROOT {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let Ok(nodes) = state.nodes() else {
            return;
        };
        if let Some(node) = nodes.get_mut(&inode) {
            node.lookups = node.lookups.saturating_sub(count);
            if node.lookups == 0 {
                // Dropping the node's connection closes it.
                nodes.remove(&inode);
            }
        }
    }

    /// The attributes of `inode` (`FUSE_GETATTR`).
    pub async fn getattr(&self, inode: Inode) -> Result<Attr> {
        let connection = self.connection(inode)?;
        let query_connection = connection.clone();
        let attributes =
            self.executor.run(async move { query_connection.get_attributes().await }).await?;
        self.attr(&connection, attributes)
    }

    /// Changes the attributes of `inode` (`FUSE_SETATTR`), and returns them.
    pub async fn setattr(&self, inode: Inode, set: &SetAttr) -> Result<Attr> {
        let connection = self.connection(inode)?;
        if let Some(size) = set.size {
            let proxy = Clone::clone(connection.file()?.0);
            self.executor.run(async move { proxy.resize(size).await.errno()?.errno() }).await?;
        }
        let mut update = fio::MutableNodeAttributes {
            uid: set.uid,
            gid: set.gid,
            access_time: set.atime.map(|time| nanos_from_time(time.resolve())).transpose()?,
            modification_time: set.mtime.map(|time| nanos_from_time(time.resolve())).transpose()?,
            ..Default::default()
        };
        if set.perm.is_some() || set.kill_suid_sgid {
            let attr = self.getattr(inode).await?;
            let mut perm = attr.perm as u32;
            if set.kill_suid_sgid {
                perm &= !S_ISUID;
                if perm & S_IXGRP != 0 {
                    perm &= !S_ISGID;
                }
            }
            if let Some(new_perm) = set.perm {
                perm = new_perm as u32 & PERM_MASK;
            }
            update.mode = Some(attr.file_type.mode_bits() | perm);
        }
        if update != fio::MutableNodeAttributes::default() {
            let connection = connection.clone();
            self.executor.run(async move { connection.update_attributes(&update).await }).await?;
        }
        self.getattr(inode).await
    }

    /// The target of symlink `inode` (`FUSE_READLINK`).
    pub async fn readlink(&self, inode: Inode) -> Result<Vec<u8>> {
        let Connection::Symlink(proxy) = self.connection(inode)? else {
            return Err(Errno::EINVAL);
        };
        self.executor
            .run(async move { proxy.describe().await.errno()?.target.ok_or(Errno::EIO) })
            .await
    }

    /// Creates the regular file `name` in `parent` with `perm`, owned by `owner`, and takes a
    /// lookup on it (`FUSE_CREATE`).
    pub async fn create(
        &self,
        parent: Inode,
        name: &[u8],
        perm: u16,
        owner: Owner,
    ) -> Result<Attr> {
        let mode = FileType::File.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(parent, name, fio::Flags::PROTOCOL_FILE, mode, 0, owner).await
    }

    /// Creates the directory `name` in `parent` with `perm`, owned by `owner`, and takes a lookup
    /// on it (`FUSE_MKDIR`).
    pub async fn mkdir(&self, parent: Inode, name: &[u8], perm: u16, owner: Owner) -> Result<Attr> {
        let mode = FileType::Directory.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(parent, name, fio::Flags::PROTOCOL_DIRECTORY, mode, 0, owner).await
    }

    /// Creates the node `name` in `parent` of `file_type` with `perm`, owned by `owner`, and takes
    /// a lookup on it (`FUSE_MKNOD`). A device node stands for device `rdev`.
    ///
    /// It may be a regular file, or a special file: a FIFO, socket, or character or block device,
    /// which is an Fxfs file whose mode says what it is, as Starnix makes them. Directories and
    /// symlinks fail with `EINVAL`.
    pub async fn mknod(
        &self,
        parent: Inode,
        name: &[u8],
        file_type: FileType,
        perm: u16,
        rdev: u32,
        owner: Owner,
    ) -> Result<Attr> {
        if matches!(file_type, FileType::Directory | FileType::Symlink) {
            return Err(Errno::EINVAL);
        }
        let mode = file_type.mode_bits() | (perm as u32 & PERM_MASK);
        self.create_node(parent, name, fio::Flags::PROTOCOL_FILE, mode, rdev, owner).await
    }

    /// Creates the symlink `name` in `parent` to `target`, and takes a lookup on it
    /// (`FUSE_SYMLINK`). Its owner is the volume's default, as symlinks' connections don't update
    /// attributes.
    pub async fn symlink(&self, parent: Inode, name: &[u8], target: &[u8]) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let link_name = connection::name(name)?;
        let target = target.to_vec();
        let (proxy, attributes) = self
            .executor
            .run(async move {
                let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
                directory
                    .create_symlink(&link_name, &target, Some(server_end))
                    .await
                    .errno()?
                    .errno()?;
                let connection = Connection::Symlink(proxy);
                let attributes = connection.get_attributes().await?;
                Ok::<_, Errno>((connection, attributes))
            })
            .await?;
        self.remember(parent, name, proxy, attributes)
    }

    /// Links `inode` into `new_parent` as `new_name`, and takes another lookup on it
    /// (`FUSE_LINK`). Directories fail with `EPERM`.
    pub async fn link(&self, inode: Inode, new_parent: Inode, new_name: &[u8]) -> Result<Attr> {
        let connection = self.connection(inode)?;
        let directory = self.directory(new_parent)?;
        let new_name = connection::name(new_name)?;
        let link_connection = connection.clone();
        let attributes = self
            .executor
            .run(async move {
                let token = connection::token(&directory).await?;
                match &link_connection {
                    Connection::File(proxy, _) => proxy.link_into(token, &new_name).await,
                    Connection::Symlink(proxy) => proxy.link_into(token, &new_name).await,
                    Connection::Directory(_) => return Err(Errno::EPERM),
                }
                .errno()?
                .errno()?;
                link_connection.get_attributes().await
            })
            .await?;
        let attr = self.attr(&connection, attributes)?;
        let mut state = self.state.lock().unwrap();
        state.nodes()?.get_mut(&inode).ok_or(Errno::ENOENT)?.lookups += 1;
        Ok(attr)
    }

    /// Removes the non-directory `name` from `parent` (`FUSE_UNLINK`).
    pub async fn unlink(&self, parent: Inode, name: &[u8]) -> Result<()> {
        self.remove(parent, name, false).await
    }

    /// Removes the empty directory `name` from `parent` (`FUSE_RMDIR`).
    pub async fn rmdir(&self, parent: Inode, name: &[u8]) -> Result<()> {
        self.remove(parent, name, true).await
    }

    async fn remove(&self, parent: Inode, name: &[u8], must_be_directory: bool) -> Result<()> {
        let directory = self.directory(parent)?;
        let name = connection::name(name)?;
        let flags = must_be_directory.then_some(fio::UnlinkFlags::MUST_BE_DIRECTORY);
        self.executor
            .run(async move {
                directory.unlink(&name, &fio::UnlinkOptions { flags }).await.errno()?.errno()
            })
            .await
    }

    /// Renames `name` in `parent` to `new_name` in `new_parent`, replacing what is there unless
    /// `flags` say not to (`FUSE_RENAME2`).
    pub async fn rename(
        &self,
        parent: Inode,
        name: &[u8],
        new_parent: Inode,
        new_name: &[u8],
        flags: RenameFlags,
    ) -> Result<()> {
        if flags.contains(RenameFlags::EXCHANGE) || !RenameFlags::all().contains(flags) {
            return Err(Errno::EINVAL);
        }
        let old_directory = self.directory(parent)?;
        let new_directory = self.directory(new_parent)?;
        if flags.contains(RenameFlags::NOREPLACE) {
            match self.open_node(&new_directory, new_name, fio::Flags::empty(), None).await {
                Ok(_) => return Err(Errno::EEXIST),
                Err(Errno::ENOENT) => {}
                Err(error) => return Err(error),
            }
        }
        let old = connection::name(name)?;
        let new = connection::name(new_name)?;
        self.executor
            .run(async move {
                let token = connection::token(&new_directory).await?;
                old_directory.rename(&old, token, &new).await.errno()?.errno()
            })
            .await?;
        // A directory the guest knows moved, so its `..` did.
        let mut state = self.state.lock().unwrap();
        for node in state.nodes()?.values_mut() {
            if node.location.as_ref().is_some_and(|(p, n)| *p == parent && n == name) {
                node.location = Some((new_parent, new_name.to_vec()));
            }
        }
        Ok(())
    }

    /// Reads from file `inode` at `offset` into `buf`, and returns how much was read: less than
    /// asked only at the end of the file (`FUSE_READ`).
    pub async fn read(&self, inode: Inode, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_vectored(inode, offset, &mut [VolatileSliceMut::from(buf)]).await
    }

    /// Reads from file `inode` at `offset` into `bufs` in turn, such as a request's buffers in
    /// guest memory, and returns how much was read: less than asked only at the end of the file.
    ///
    /// The file's pages are copied straight into the buffers. Waiting for pages not yet in memory
    /// holds no thread: the future is woken once Fxfs's pager has supplied them. Dropping the
    /// future before it completes leaves the buffers with what was read so far.
    pub async fn read_vectored(
        &self,
        inode: Inode,
        offset: u64,
        bufs: &mut [VolatileSliceMut<'_>],
    ) -> Result<usize> {
        let stream = self.stream(inode)?;
        let bufs: Vec<HostBuffer> = bufs.iter().map(VolatileSliceMut::host_buffer).collect();
        zircon::syscalls::stream_readv_at_async(stream.raw_handle(), offset, &bufs).await.errno()
    }

    /// Writes `data` to file `inode` at `offset`, extending it if that is past its end, and returns
    /// how much was written (`FUSE_WRITE`).
    pub async fn write(&self, inode: Inode, offset: u64, data: &[u8]) -> Result<usize> {
        self.write_vectored(inode, offset, &[VolatileSlice::from(data)]).await
    }

    /// Writes `bufs` in turn to file `inode` at `offset`, such as a request's buffers in guest
    /// memory, and returns how much was written.
    ///
    /// Waiting for Fxfs's pager, to read pages in or mark them dirty, holds no thread. Dropping the
    /// future before it completes leaves the file with what was written so far.
    pub async fn write_vectored(
        &self,
        inode: Inode,
        offset: u64,
        bufs: &[VolatileSlice<'_>],
    ) -> Result<usize> {
        let stream = self.stream(inode)?;
        let bufs: Vec<HostBuffer> = bufs.iter().map(VolatileSlice::host_buffer).collect();
        zircon::syscalls::stream_writev_at_async(stream.raw_handle(), offset, &bufs).await.errno()
    }

    /// Allocates `len` bytes of file `inode` at `offset`, extending it if they are past its end
    /// (`FUSE_FALLOCATE` with mode 0, the only one supported).
    pub async fn allocate(&self, inode: Inode, offset: u64, len: u64) -> Result<()> {
        let proxy = self.file(inode)?;
        self.executor
            .run(async move {
                proxy.allocate(offset, len, fio::AllocateMode::empty()).await.errno()?.errno()
            })
            .await
    }

    /// Writes back `inode`'s data and metadata, and for a directory, the filesystem's
    /// (`FUSE_FSYNC`, `FUSE_FSYNCDIR`).
    pub async fn fsync(&self, inode: Inode) -> Result<()> {
        let connection = self.connection(inode)?;
        self.executor
            .run(async move {
                match &connection {
                    Connection::Directory(proxy) => proxy.sync().await.errno()?,
                    Connection::File(proxy, _) => proxy.sync().await.errno()?,
                    Connection::Symlink(_) => Ok(()),
                }
                .errno()
            })
            .await
    }

    /// The filesystem's usage (`FUSE_STATFS`).
    pub async fn statfs(&self) -> Result<StatFs> {
        let root = self.directory(Inode::ROOT)?;
        let info = self
            .executor
            .run(async move {
                let (status, info) = root.query_filesystem().await.errno()?;
                zx::Status::ok(status).errno()?;
                info.ok_or(Errno::EIO)
            })
            .await?;
        let block_size = info.block_size.max(1);
        Ok(StatFs {
            block_size,
            blocks: info.total_bytes / block_size as u64,
            blocks_free: info.total_bytes.saturating_sub(info.used_bytes) / block_size as u64,
            name_max: info.max_filename_size,
        })
    }

    /// Opens directory `inode` for reading its entries (`FUSE_OPENDIR`): takes a snapshot of them,
    /// including `.` and `..`, which its handle holds until it is released.
    pub async fn opendir(&self, inode: Inode) -> Result<DirHandle> {
        let directory = self.directory(inode)?;
        let parent = {
            let mut state = self.state.lock().unwrap();
            let node = state.nodes()?.get(&inode);
            node.and_then(|node| node.location.as_ref()).map_or(Inode::ROOT, |(parent, _)| *parent)
        };
        let raw_entries =
            self.executor.run(async move { connection::read_entries(&directory).await }).await?;
        let mut entries = vec![
            DirEntry { inode, file_type: FileType::Directory, name: b".".to_vec() },
            DirEntry { inode: parent, file_type: FileType::Directory, name: b"..".to_vec() },
        ];
        for (object_id, file_type, name) in raw_entries {
            if name != b"." {
                entries.push(DirEntry { inode: self.inode(object_id), file_type, name });
            }
        }
        let mut state = self.state.lock().unwrap();
        let handle = DirHandle(state.next_handle);
        state.next_handle += 1;
        state.dirs.insert(handle, entries.into());
        Ok(handle)
    }

    /// The entries of the open directory `handle` from `offset`, the index of the first
    /// (`FUSE_READDIR`).
    pub fn readdir(&self, handle: DirHandle, offset: u64) -> Result<DirEntries> {
        let state = self.state.lock().unwrap();
        let entries = state.dirs.get(&handle).ok_or(Errno::EBADF)?;
        Ok(DirEntries::new(entries.clone(), offset))
    }

    /// Releases the open directory `handle` (`FUSE_RELEASEDIR`).
    pub fn releasedir(&self, handle: DirHandle) {
        self.state.lock().unwrap().dirs.remove(&handle);
    }

    /// Creates the DAX window of `size` bytes that [`setup_mapping`](Self::setup_mapping) maps
    /// files into, for a hypervisor whose `second_stage` the VMM maintains (see the crate docs).
    /// The guest's faults on the window must be passed to [`DaxWindow::fault`], and the guest
    /// resumed once it completes.
    ///
    /// `size` must be a multiple of the [page size](crate::kernel::page_size). Without a second
    /// stage, only the host can access the window, at [`DaxWindow::host_base`]. A filesystem has
    /// one window.
    pub fn create_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| Ok(Dax::new(size, second_stage)?))
    }

    /// Creates the DAX window that [`setup_mapping`](Self::setup_mapping) maps files into in the
    /// `size` bytes of host memory at `base`, for a hypervisor that maps the guest's memory from
    /// host memory and follows changes to it, such as KVM (see the crate docs).
    ///
    /// The filesystem takes the memory over: it maps files' pages there as the window maps them,
    /// and serves the faults on the rest, the guest's and the hypervisor's, with userfaultfd on a
    /// thread of its own, which resolves them concurrently. That needs access to `/dev/userfaultfd`, or the
    /// `vm.unprivileged_userfaultfd` sysctl. A filesystem has one window.
    ///
    /// # Safety
    ///
    /// The memory must be a page-aligned mapping of `size` bytes that only the guest uses, until
    /// the filesystem is dropped.
    #[cfg(target_os = "linux")]
    pub unsafe fn create_dax_window_in_host_memory(
        &self,
        base: std::ptr::NonNull<u8>,
        size: u64,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| {
            // SAFETY: The caller hands the memory over.
            let stage =
                unsafe { crate::dax::uffd::UffdSecondStage::new(base.as_ptr() as usize, size) }?;
            Dax::in_host_memory(stage)
        })
    }

    /// Installs the window `create` creates, unless there is one.
    fn install_dax(
        &self,
        create: impl FnOnce() -> Result<(Dax, DaxWindow), Error>,
    ) -> Result<DaxWindow, Error> {
        let mut dax = self.dax.lock().unwrap();
        if dax.is_some() {
            return Err(Errno::EEXIST.into());
        }
        let (new, window) = create()?;
        *dax = Some(new);
        Ok(window)
    }

    /// Maps `len` bytes of file `inode` at `file_offset` into the DAX window at `window_offset`,
    /// replacing what was mapped there (`FUSE_SETUPMAPPING`). Offsets and length must be multiples
    /// of the page size. Stores through a `writable` mapping dirty the file's pages, which are
    /// written back as the file's other dirty pages are. Without a window, fails with `ENOSYS`.
    pub async fn setup_mapping(
        &self,
        inode: Inode,
        file_offset: u64,
        len: u64,
        window_offset: u64,
        writable: bool,
    ) -> Result<()> {
        self.dax.lock().unwrap().as_ref().ok_or(Errno::ENOSYS)?.check_range(window_offset, len)?;
        if !file_offset.is_multiple_of(page_size()) || file_offset.checked_add(len).is_none() {
            return Err(Errno::EINVAL);
        }
        let connection = self.connection(inode)?;
        let Connection::File(proxy, _) = &connection else {
            return Err(Errno::EINVAL);
        };
        let mut flags = fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER;
        if writable {
            flags |= fio::VmoFlags::WRITE;
        }
        let proxy = Clone::clone(proxy);
        let vmo = self
            .executor
            .run(async move { proxy.get_backing_memory(flags).await.errno()?.errno() })
            .await?;
        // The window is never removed once it is created. Unmounting unmaps it under its lock,
        // after the filesystem is no longer mounted, so a mapping made now could outlive it.
        let mut dax = self.dax.lock().unwrap();
        if self.mounted.lock().unwrap().is_none() {
            return Err(Errno::ENOTCONN);
        }
        let dax = dax.as_mut().expect("the window was created");
        dax.map(window_offset, connection, &vmo, file_offset, len, writable)
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`FUSE_REMOVEMAPPING`). Dirty
    /// pages stay the files', to be written back as their other dirty pages are. Without a window,
    /// fails with `ENOSYS`.
    pub fn remove_mapping(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        dax.unmap(window_offset, len)
    }
}

impl Drop for Filesystem {
    /// Unmounts the filesystem if it is still mounted, writing back what is dirty, blocking the
    /// dropping thread until it is done. Await `close` or `unmount` rather than dropping a mounted
    /// filesystem.
    ///
    /// # Panics
    ///
    /// If a mounted filesystem is dropped on Fxfs's executor.
    fn drop(&mut self) {
        let Some((closing, _, done)) = self.begin_unmount() else {
            return;
        };
        assert_not_on_executor();
        if let Err(error) = self.executor.run_blocking(closing) {
            log::error!("fxdax: failed to unmount: {error:#}");
        }
        let _ = done.send(());
    }
}

impl std::fmt::Debug for Filesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mounted = self.mounted.lock().unwrap().is_some();
        f.debug_struct("Filesystem").field("mounted", &mounted).finish_non_exhaustive()
    }
}

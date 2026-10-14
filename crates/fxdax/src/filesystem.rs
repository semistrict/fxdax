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
use crate::executor::assert_not_on_executor;
use crate::image::{Image, ImageShared};
use crate::kernel::page_size;
use crate::memory::{VolatileSlice, VolatileSliceMut};
use crate::node::*;
use crate::storage::{CloneKind, Storage};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_io as fio;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{BoxFuture, Shared};
use fxfs::object_store::ObjectDescriptor;
use fxfs_platform::directory::{self as fx_directory, FxDirectory};
use fxfs_platform::node::{FxNode, OpenedNode};
use fxfs_platform::volume::{FxVolume, FxVolumeAndRoot};
use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex};
use zircon::user_copy::HostBuffer;

/// The name of the volume a filesystem mounted from storage serves.
pub(crate) const VOLUME_NAME: &str = "default";

/// The number of threads of a filesystem's executor, unless [`Options`] say otherwise.
pub(crate) const DEFAULT_EXECUTOR_THREADS: u8 = 1;

/// Runs futures to completion on the threads a filesystem's Fxfs runs on, such as a VMM's loop
/// serving its virtio-fs device ([`Filesystem::spawner`]): what they await of the filesystem then
/// completes on the same thread, without waking another, which otherwise costs a request most of
/// its time. The threads are a tokio runtime's, so the futures may wait on file descriptors with
/// `tokio::io::unix::AsyncFd`; they must not block them. The executor finishes them before it
/// stops, so they must finish once the filesystem is to close.
#[derive(Clone)]
pub struct Spawner(Arc<ImageShared>);

impl Spawner {
    /// Runs `future` to completion on Fxfs's threads.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        self.0.executor.spawn(future);
    }
}

impl std::fmt::Debug for Spawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spawner").finish_non_exhaustive()
    }
}

/// How a filesystem or image is mounted.
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
    /// They are separate from the tasks and threads calling the filesystem. The default is 1:
    /// operations mostly wait for one another and the device, and on one thread they wake one
    /// another without waking other threads, which costs a request most of its time otherwise.
    pub fn executor_threads(&mut self, threads: NonZeroU8) -> &mut Self {
        self.executor_threads = threads;
        self
    }

    pub(crate) fn threads(&self) -> u8 {
        self.executor_threads.get()
    }

    /// Mounts the filesystem on `storage`, which [`format`](Self::format) formatted: the volume
    /// it created on an image of its own.
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

/// A mounted Fxfs filesystem, served by inode number as FUSE serves one: a volume of an
/// [`Image`], or of an image of its own, mounted from storage with [`mount`](Filesystem::mount).
/// See the crate docs.
///
/// Its methods may be called from any number of tasks or threads at once (see the crate docs).
pub struct Filesystem {
    /// The image whose volume the filesystem is.
    image: Arc<ImageShared>,
    /// The image the filesystem was mounted from storage with, which unmounting it closes, until
    /// then. Locked while unmounting, so that concurrent unmounts wait for the image to close.
    owned: futures::lock::Mutex<Option<Image>>,
    /// What closing an image of its own gave back, until it is taken.
    closed: Mutex<Option<Result<Storage, Error>>>,
    mount: Arc<Mount>,
    root_object_id: u64,
    block_size: u32,
}

/// A mounted volume, which its image can unmount.
pub(crate) struct Mount {
    /// The volume's name.
    name: String,
    /// The volume, until it is unmounted.
    volume: Mutex<Option<FxVolumeAndRoot>>,
    state: Mutex<State>,
    /// The DAX window, once it is created.
    dax: Mutex<Option<Dax<Connection>>>,
    /// Completes once the volume is unmounted, once unmounting has begun.
    unmounted: Mutex<Option<Shared<oneshot::Receiver<()>>>>,
}

/// An inode the guest has looked up.
struct Node {
    /// Fxfs's node, open: as an open file does, the open count keeps a file the guest unlinks
    /// until the guest forgets it, which it does only once no file it has open is the node.
    node: OpenedNode<dyn FxNode>,
    /// A fuchsia.io connection to the node, once an operation needs one. Lookups and attributes
    /// need none, and opening one costs a request most of its time.
    connection: Option<Connection>,
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

    /// Mounts or formats an image of its own on `storage`, and its volume.
    async fn start(options: &Options, storage: Storage, format: bool) -> Result<Self, Error> {
        let image = Image::start(options, storage, format).await?;
        let filesystem = if format {
            image.create_volume(VOLUME_NAME).await
        } else {
            image.open_volume(VOLUME_NAME).await
        };
        match filesystem {
            Ok(mut filesystem) => {
                *filesystem.owned.get_mut() = Some(image);
                Ok(filesystem)
            }
            Err(error) => {
                if let Err(close_error) = image.close().await {
                    log::error!("fxdax: failed to close the image: {close_error:#}");
                }
                Err(error)
            }
        }
    }

    /// The filesystem of `volume`, mounted on `image`, and its mount, which the image unmounts
    /// when it is closed.
    pub(crate) async fn new(
        image: Arc<ImageShared>,
        name: &str,
        volume: FxVolumeAndRoot,
    ) -> (Self, Arc<Mount>) {
        let root_object_id = volume.volume().store().root_directory_object_id();
        let (volume, root, root_node) = image
            .executor
            .run(async move {
                let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
                volume.root().clone().serve(RIGHTS, server_end);
                let root_node = OpenedNode::new(volume.root().clone().as_node());
                (volume, root, root_node)
            })
            .await;
        let root = Node {
            node: root_node,
            connection: Some(Connection::Directory(root)),
            lookups: 1,
            location: None,
        };
        let mount = Arc::new(Mount {
            name: name.to_owned(),
            volume: Mutex::new(Some(volume)),
            state: Mutex::new(State {
                nodes: Some(BTreeMap::from([(Inode::ROOT, root)])),
                dirs: BTreeMap::new(),
                next_handle: 1,
            }),
            dax: Mutex::new(None),
            unmounted: Mutex::new(None),
        });
        let block_size = image.block_size;
        let filesystem = Self {
            image,
            owned: futures::lock::Mutex::new(None),
            closed: Mutex::new(None),
            mount: mount.clone(),
            root_object_id,
            block_size,
        };
        (filesystem, mount)
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty, and gives the
    /// storage of the image it was mounted from back, if it was mounted from storage. A volume
    /// of an [`Image`] fails once it is unmounted, as its storage is the image's, which
    /// [`Image::close`] gives back.
    pub async fn close(self) -> Result<Storage, Error> {
        self.unmount().await?;
        self.closed.lock().unwrap().take().unwrap_or_else(|| {
            Err(Error::msg("the filesystem is a volume of an image, whose storage is the image's"))
        })
    }

    /// Unmounts the filesystem, unless it is unmounted, writing back what is dirty. A filesystem
    /// mounted from storage then closes its image, keeping the storage for
    /// [`close`](Self::close) to give back. Every operation fails with `ENOTCONN` from then on.
    ///
    /// Once unmounting begins, the volume's unmount completes even if the future is dropped.
    /// Concurrent calls wait for the same unmount, and only the first gives its error.
    pub async fn unmount(&self) -> Result<(), Error> {
        let mut owned = self.owned.lock().await;
        self.mount.unmount(&self.image).await?;
        if let Some(image) = owned.take() {
            match image.close().await {
                Ok(storage) => *self.closed.lock().unwrap() = Some(Ok(storage)),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Clones the filesystem's image to a new image at `path`, which must not exist, while it is
    /// mounted, and returns the clone's storage and how it was cloned (see
    /// [`Image::clone_storage_to`]).
    pub async fn clone_storage_to(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(Storage, CloneKind), Error> {
        if self.mount.volume.lock().unwrap().is_none() {
            return Err(Errno::ENOTCONN.into());
        }
        self.image.clone_storage_to(path.as_ref()).await
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

    /// Fxfs's node of `inode`.
    fn node(&self, inode: Inode) -> Result<Arc<dyn FxNode>> {
        let mut state = self.mount.state.lock().unwrap();
        Ok(state.nodes()?.get(&inode).ok_or(Errno::ENOENT)?.node.0.clone())
    }

    /// A connection to `inode`, which is opened the first time one is needed.
    async fn connection(&self, inode: Inode) -> Result<Connection> {
        let node = {
            let mut state = self.mount.state.lock().unwrap();
            let node = state.nodes()?.get(&inode).ok_or(Errno::ENOENT)?;
            if let Some(connection) = &node.connection {
                return Ok(connection.clone());
            }
            node.node.0.clone()
        };
        let volume = self.volume()?;
        let connection = self
            .image
            .executor
            .run(async move { connection::connect(OpenedNode::new(node), &volume).await })
            .await?;
        // Another operation may have connected meanwhile, whose connection is kept.
        let mut state = self.mount.state.lock().unwrap();
        let node = state.nodes()?.get_mut(&inode).ok_or(Errno::ENOENT)?;
        Ok(node.connection.get_or_insert(connection).clone())
    }

    async fn directory(&self, inode: Inode) -> Result<fio::DirectoryProxy> {
        // `Clone::clone`, as the proxy's `clone` is fuchsia.io's `Clone`.
        Ok(Clone::clone(self.connection(inode).await?.directory()?))
    }

    async fn file(&self, inode: Inode) -> Result<fio::FileProxy> {
        Ok(Clone::clone(self.connection(inode).await?.file()?.0))
    }

    async fn stream(&self, inode: Inode) -> Result<Arc<zx::Stream>> {
        Ok(self.connection(inode).await?.file()?.1.clone())
    }

    /// The volume, while it is mounted.
    fn volume(&self) -> Result<Arc<FxVolume>> {
        let volume = self.mount.volume.lock().unwrap();
        Ok(volume.as_ref().ok_or(Errno::ENOTCONN)?.volume().clone())
    }

    /// Takes a lookup on `node`, the child `name` of `parent`, remembering it, and `connection`
    /// to it, if it's new, and returns its attributes.
    fn remember(
        &self,
        parent: Inode,
        name: &[u8],
        node: Arc<dyn FxNode>,
        connection: Option<Connection>,
        attributes: fio::Attributes,
    ) -> Result<Attr> {
        let attr = self.attr(file_type(&*node), attributes)?;
        let location = (attr.file_type == FileType::Directory).then(|| (parent, name.to_owned()));
        let mut state = self.mount.state.lock().unwrap();
        let entry = state.nodes()?.entry(attr.inode).or_insert_with(|| Node {
            node: OpenedNode::new(node),
            connection: None,
            lookups: 0,
            location,
        });
        if entry.connection.is_none() {
            entry.connection = connection;
        }
        entry.lookups += 1;
        Ok(attr)
    }

    /// Takes a lookup on the node a new `connection` connects to, as `remember` does.
    fn remember_connected(
        &self,
        parent: Inode,
        name: &[u8],
        connection: Connection,
        attributes: fio::Attributes,
    ) -> Result<Attr> {
        let object_id = attributes.1.id.ok_or(Errno::EIO)?;
        // The connection holds the node open, so it is in the volume's cache.
        let node = self.volume()?.cache().get(object_id).ok_or(Errno::EIO)?;
        self.remember(parent, name, node, Some(connection), attributes)
    }

    fn attr(&self, node_type: FileType, (mutable, immutable): fio::Attributes) -> Result<Attr> {
        // Special files are Fxfs files whose mode says what they are, as Starnix makes them.
        let file_type = mutable.mode.and_then(FileType::from_mode).unwrap_or(node_type);
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
        self.image
            .executor
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
        let directory = self.directory(parent).await?;
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
        self.remember_connected(parent, name, connection, attributes)
    }

    /// What runs futures on the threads Fxfs runs on (see [`Spawner`]).
    pub fn spawner(&self) -> Spawner {
        Spawner(self.image.clone())
    }

    /// Looks up `name` in directory `parent`, taking a lookup on its inode (`FUSE_LOOKUP`), which
    /// stays valid until [`forget`](Self::forget) releases it.
    pub async fn lookup(&self, parent: Inode, name: &[u8]) -> Result<Attr> {
        let (node, attributes) = self.child(parent, name).await?.ok_or(Errno::ENOENT)?;
        self.remember(parent, name, node, None, attributes)
    }

    /// The child `name` of directory `parent`, and its attributes, if it has one.
    async fn child(
        &self,
        parent: Inode,
        name: &[u8],
    ) -> Result<Option<(Arc<dyn FxNode>, fio::Attributes)>> {
        let directory = self.node(parent)?.into_any().downcast::<FxDirectory>();
        let directory = directory.map_err(|_| Errno::ENOTDIR)?;
        let name = connection::name(name)?;
        self.image
            .executor
            .run(async move {
                let Some(node) = directory.lookup_child(&name).await.errno()? else {
                    return Ok(None);
                };
                let attributes = attributes(&node).await?;
                Ok(Some((node, attributes)))
            })
            .await
    }

    /// Releases `count` lookups on `inode` (`FUSE_FORGET`). With none left, its inode number is
    /// invalid until it is looked up again. The root's are never released.
    pub fn forget(&self, inode: Inode, count: u64) {
        if inode == Inode::ROOT {
            return;
        }
        let mut state = self.mount.state.lock().unwrap();
        let Ok(nodes) = state.nodes() else {
            return;
        };
        if let Some(node) = nodes.get_mut(&inode) {
            node.lookups = node.lookups.saturating_sub(count);
            if node.lookups == 0 {
                // Dropping the node closes its connection, and releases its open count.
                nodes.remove(&inode);
            }
        }
    }

    /// The attributes of `inode` (`FUSE_GETATTR`).
    pub async fn getattr(&self, inode: Inode) -> Result<Attr> {
        let node = self.node(inode)?;
        let node_type = file_type(&*node);
        let attributes = self.image.executor.run(async move { attributes(&node).await }).await?;
        self.attr(node_type, attributes)
    }

    /// Changes the attributes of `inode` (`FUSE_SETATTR`), and returns them.
    pub async fn setattr(&self, inode: Inode, set: &SetAttr) -> Result<Attr> {
        let connection = self.connection(inode).await?;
        if let Some(size) = set.size {
            let proxy = Clone::clone(connection.file()?.0);
            self.image
                .executor
                .run(async move { proxy.resize(size).await.errno()?.errno() })
                .await?;
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
            self.image
                .executor
                .run(async move { connection.update_attributes(&update).await })
                .await?;
        }
        self.getattr(inode).await
    }

    /// The target of symlink `inode` (`FUSE_READLINK`).
    pub async fn readlink(&self, inode: Inode) -> Result<Vec<u8>> {
        let Connection::Symlink(proxy) = self.connection(inode).await? else {
            return Err(Errno::EINVAL);
        };
        self.image
            .executor
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
        let directory = self.directory(parent).await?;
        let link_name = connection::name(name)?;
        let target = target.to_vec();
        let (proxy, attributes) = self
            .image
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
        self.remember_connected(parent, name, proxy, attributes)
    }

    /// Links `inode` into `new_parent` as `new_name`, and takes another lookup on it
    /// (`FUSE_LINK`). Directories fail with `EPERM`.
    pub async fn link(&self, inode: Inode, new_parent: Inode, new_name: &[u8]) -> Result<Attr> {
        let connection = self.connection(inode).await?;
        let directory = self.directory(new_parent).await?;
        let new_name = connection::name(new_name)?;
        let link_connection = connection.clone();
        let attributes = self
            .image
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
        let attr = self.attr(connection.file_type(), attributes)?;
        let mut state = self.mount.state.lock().unwrap();
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
        let directory = self.directory(parent).await?;
        let name = connection::name(name)?;
        let flags = must_be_directory.then_some(fio::UnlinkFlags::MUST_BE_DIRECTORY);
        self.image
            .executor
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
        if flags.contains(RenameFlags::NOREPLACE)
            && self.child(new_parent, new_name).await?.is_some()
        {
            return Err(Errno::EEXIST);
        }
        let old_directory = self.directory(parent).await?;
        let new_directory = self.directory(new_parent).await?;
        let old = connection::name(name)?;
        let new = connection::name(new_name)?;
        self.image
            .executor
            .run(async move {
                let token = connection::token(&new_directory).await?;
                old_directory.rename(&old, token, &new).await.errno()?.errno()
            })
            .await?;
        // A directory the guest knows moved, so its `..` did.
        let mut state = self.mount.state.lock().unwrap();
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
        let stream = self.stream(inode).await?;
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
        let stream = self.stream(inode).await?;
        let bufs: Vec<HostBuffer> = bufs.iter().map(VolatileSlice::host_buffer).collect();
        zircon::syscalls::stream_writev_at_async(stream.raw_handle(), offset, &bufs).await.errno()
    }

    /// Allocates `len` bytes of file `inode` at `offset`, extending it if they are past its end
    /// (`FUSE_FALLOCATE` with mode 0, the only one supported).
    pub async fn allocate(&self, inode: Inode, offset: u64, len: u64) -> Result<()> {
        let proxy = self.file(inode).await?;
        self.image
            .executor
            .run(async move {
                proxy.allocate(offset, len, fio::AllocateMode::empty()).await.errno()?.errno()
            })
            .await
    }

    /// Writes back `inode`'s data and metadata, and for a directory, the filesystem's
    /// (`FUSE_FSYNC`, `FUSE_FSYNCDIR`).
    pub async fn fsync(&self, inode: Inode) -> Result<()> {
        let connection = self.connection(inode).await?;
        self.image
            .executor
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
        let root = self.directory(Inode::ROOT).await?;
        let info = self
            .image
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
        let directory = self.directory(inode).await?;
        let parent = {
            let mut state = self.mount.state.lock().unwrap();
            let node = state.nodes()?.get(&inode);
            node.and_then(|node| node.location.as_ref()).map_or(Inode::ROOT, |(parent, _)| *parent)
        };
        let raw_entries = self
            .image
            .executor
            .run(async move { connection::read_entries(&directory).await })
            .await?;
        let mut entries = vec![
            DirEntry { inode, file_type: FileType::Directory, name: b".".to_vec() },
            DirEntry { inode: parent, file_type: FileType::Directory, name: b"..".to_vec() },
        ];
        for (object_id, file_type, name) in raw_entries {
            if name != b"." {
                entries.push(DirEntry { inode: self.inode(object_id), file_type, name });
            }
        }
        let mut state = self.mount.state.lock().unwrap();
        let handle = DirHandle(state.next_handle);
        state.next_handle += 1;
        state.dirs.insert(handle, entries.into());
        Ok(handle)
    }

    /// The entries of the open directory `handle` from `offset`, the index of the first
    /// (`FUSE_READDIR`).
    pub fn readdir(&self, handle: DirHandle, offset: u64) -> Result<DirEntries> {
        let state = self.mount.state.lock().unwrap();
        let entries = state.dirs.get(&handle).ok_or(Errno::EBADF)?;
        Ok(DirEntries::new(entries.clone(), offset))
    }

    /// Releases the open directory `handle` (`FUSE_RELEASEDIR`).
    pub fn releasedir(&self, handle: DirHandle) {
        self.mount.state.lock().unwrap().dirs.remove(&handle);
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
    /// `vm.unprivileged_userfaultfd` sysctl. `prefault`, if the hypervisor can map pages for the
    /// guest ahead of its accesses, is told of the pages mapped there. A filesystem has one
    /// window.
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
        prefault: Option<Arc<dyn crate::Prefault>>,
    ) -> Result<DaxWindow, Error> {
        self.install_dax(|| {
            // SAFETY: The caller hands the memory over.
            let stage = unsafe {
                crate::dax::uffd::UffdSecondStage::new(base.as_ptr() as usize, size, prefault)
            }?;
            Dax::in_host_memory(stage)
        })
    }

    /// Installs the window `create` creates, unless there is one.
    fn install_dax(
        &self,
        create: impl FnOnce() -> Result<(Dax<Connection>, DaxWindow), Error>,
    ) -> Result<DaxWindow, Error> {
        let mut dax = self.mount.dax.lock().unwrap();
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
        self.mount
            .dax
            .lock()
            .unwrap()
            .as_ref()
            .ok_or(Errno::ENOSYS)?
            .check_range(window_offset, len)?;
        if !file_offset.is_multiple_of(page_size()) || file_offset.checked_add(len).is_none() {
            return Err(Errno::EINVAL);
        }
        let connection = self.connection(inode).await?;
        let Connection::File(proxy, _) = &connection else {
            return Err(Errno::EINVAL);
        };
        let mut flags = fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER;
        if writable {
            flags |= fio::VmoFlags::WRITE;
        }
        let proxy = Clone::clone(proxy);
        let vmo = self
            .image
            .executor
            .run(async move { proxy.get_backing_memory(flags).await.errno()?.errno() })
            .await?;
        // The window is never removed once it is created. Unmounting unmaps it under its lock,
        // after the filesystem is no longer mounted, so a mapping made now could outlive it.
        let mut dax = self.mount.dax.lock().unwrap();
        if self.mount.volume.lock().unwrap().is_none() {
            return Err(Errno::ENOTCONN);
        }
        let dax = dax.as_mut().expect("the window was created");
        dax.map(window_offset, connection, &vmo, file_offset, len, writable)
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`FUSE_REMOVEMAPPING`). Dirty
    /// pages stay the files', to be written back as their other dirty pages are. Without a window,
    /// fails with `ENOSYS`.
    pub fn remove_mapping(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.mount.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Errno::ENOSYS)?;
        dax.check_range(window_offset, len)?;
        dax.unmap(window_offset, len)
    }
}

impl Mount {
    /// The volume's name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The mounted volume, until it is unmounted.
    pub(crate) fn volume(&self) -> Option<Arc<FxVolume>> {
        self.volume.lock().unwrap().as_ref().map(|volume| volume.volume().clone())
    }

    /// Unmounts the volume, unless it is unmounted, or waits for the unmount under way.
    pub(crate) async fn unmount(&self, image: &ImageShared) -> Result<(), Error> {
        image.executor.run(self.unmounting(image)).await.map_err(Error::new)
    }

    /// The future that unmounts the volume on Fxfs's executor, unless it is unmounted, or waits
    /// for the unmount under way.
    pub(crate) fn unmounting(&self, image: &ImageShared) -> BoxFuture<'static, anyhow::Result<()>> {
        match self.begin_unmount(image) {
            Some((closing, done)) => async move {
                let result = closing.await;
                let _ = done.send(());
                result
            }
            .boxed(),
            None => {
                let unmounted = self.unmounted.lock().unwrap().clone();
                async move {
                    if let Some(unmounted) = unmounted {
                        let _ = unmounted.await;
                    }
                    Ok(())
                }
                .boxed()
            }
        }
    }

    /// Begins unmounting the volume, unless it is unmounted: forgets its connections and unmaps
    /// its DAX window, and returns the future that unmounts it on Fxfs's executor, and the sender
    /// to signal once it is unmounted, which concurrent unmounts wait for.
    fn begin_unmount(
        &self,
        image: &ImageShared,
    ) -> Option<(impl Future<Output = anyhow::Result<()>> + Send + 'static, oneshot::Sender<()>)>
    {
        let (volume, done) = {
            let mut volume = self.volume.lock().unwrap();
            let volume = volume.take()?;
            // While `volume` is locked, so that a concurrent unmount finding it taken finds this.
            let (done, unmounted) = oneshot::channel();
            *self.unmounted.lock().unwrap() = Some(unmounted.shared());
            (volume, done)
        };
        // Unmap the files in the DAX window, and close every connection, so that the volume's nodes
        // can be dropped. The volume is unmounted even if unmapping fails.
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
        // Once the image is closed, its volumes are too.
        let volumes_directory = image.volumes_directory().ok();
        let closing = async move {
            for (_, node) in nodes {
                if let Some(Connection::Directory(proxy)) = node.connection {
                    let _ = proxy.close().await;
                }
            }
            let store_id = volume.volume().store().store_object_id();
            drop(volume);
            if let Some(volumes_directory) = volumes_directory {
                volumes_directory.lock().await.unmount(store_id).await?;
            }
            anyhow::Ok(())
        };
        Some((closing, done))
    }
}

/// The type of `node`, unless its mode says otherwise.
fn file_type(node: &dyn FxNode) -> FileType {
    match node.object_descriptor() {
        ObjectDescriptor::Directory => FileType::Directory,
        ObjectDescriptor::Symlink => FileType::Symlink,
        _ => FileType::File,
    }
}

/// The attributes of `node`, as fxdax makes nodes' attributes from.
async fn attributes(node: &Arc<dyn FxNode>) -> Result<fio::Attributes> {
    let attributes = fx_directory::get_attributes(node, connection::ATTRIBUTES).await.errno()?;
    Ok((attributes.mutable_attributes, attributes.immutable_attributes))
}

impl Drop for Filesystem {
    /// Unmounts the filesystem if it is still mounted, writing back what is dirty, blocking the
    /// dropping thread until it is done, and closes the image it was mounted from storage with,
    /// if it was. Await `close` or `unmount` rather than dropping a mounted filesystem.
    ///
    /// # Panics
    ///
    /// If a mounted filesystem is dropped on Fxfs's executor.
    fn drop(&mut self) {
        let Some((closing, done)) = self.mount.begin_unmount(&self.image) else {
            return;
        };
        assert_not_on_executor();
        if let Err(error) = self.image.executor.run_blocking(closing) {
            log::error!("fxdax: failed to unmount: {error:#}");
        }
        let _ = done.send(());
        // An image of its own closes as it is dropped.
    }
}

impl std::fmt::Debug for Filesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mounted = self.mount.volume.lock().unwrap().is_some();
        f.debug_struct("Filesystem").field("mounted", &mounted).finish_non_exhaustive()
    }
}

//! A FUSE-shaped view of an Fxfs volume, for virtio-fs servers.
//!
//! [`Vfs`] speaks in inode numbers and lookup counts, as FUSE does, with the shape of RedoxFS's
//! `redoxfs::vfs::Vfs`, which libkrun's RedoxFS backend serves. It is a fuchsia.io client of the
//! volume, as Starnix's remote filesystems and fdio are: every inode the kernel has looked up
//! keeps a connection to its node, opened by name from its parent directory with
//! `FLAG_SEND_REPRESENTATION`, whose representation gives the node's protocol and attributes. File
//! contents are read and written through the file's stream on the calling thread, which blocks
//! while the kernel's pager supplies pages, so it must not be one of the volume's executor threads.
//!
//! Inode numbers are Fxfs object ids, except that the root directory is inode 1, `FUSE_ROOT_ID`.

use crate::dax::{DaxWindow, SecondStage, page_size};
use crate::errno::{Error, Result, linux, raw};
use crate::executor::{Executor, assert_not_on_executor};
use fidl::endpoints::{Proxy, create_proxy};
use fidl_fuchsia_io as fio;
use futures::StreamExt;
use fxfs::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem};
use fxfs::fsck::{FsckOptions, fsck_volume_with_options, fsck_with_options};
use fxfs::object_store::volume::root_volume;
use fxfs::object_store::{NewChildStoreOptions, StoreOptions};
use fxfs_platform::directory::FxDirectory;
use fxfs_platform::volume::{FxVolumeAndRoot, MemoryPressureConfig};
use refaults_vmo::PageRefaultCounter;
use std::collections::BTreeMap;
use std::mem::MaybeUninit;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};
use storage_device::DeviceHolder;
use storage_device::file_backed_device::FileBackedDevice;

#[cfg(test)]
mod tests;

/// Inode number of the root directory.
pub const ROOT_INO: u64 = 1;

/// Fail if the target of a rename exists.
pub const RENAME_NOREPLACE: u32 = 1 << 0;

/// Exchange the source and target of a rename.
pub const RENAME_EXCHANGE: u32 = 1 << 1;

/// The name of the volume served.
const VOLUME_NAME: &str = "default";

/// The block size of image files' devices.
const IMAGE_BLOCK_SIZE: u32 = 4096;

/// The number of threads of the volume's executor.
const EXECUTOR_THREADS: u8 = 4;

// Linux's `st_mode` file types and permission bits.
const S_IFMT: u32 = 0o170000;
const S_IFSOCK: u32 = 0o140000;
const S_IFLNK: u32 = 0o120000;
const S_IFREG: u32 = 0o100000;
const S_IFBLK: u32 = 0o060000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFIFO: u32 = 0o010000;
const S_ISUID: u32 = 0o4000;
const S_ISGID: u32 = 0o2000;
const S_IXGRP: u32 = 0o0010;
const PERM_MASK: u32 = 0o7777;

/// The attributes a node's attributes are made from.
const ATTRIBUTES: fio::NodeAttributesQuery = fio::NodeAttributesQuery::MODE
    .union(fio::NodeAttributesQuery::UID)
    .union(fio::NodeAttributesQuery::GID)
    .union(fio::NodeAttributesQuery::RDEV)
    .union(fio::NodeAttributesQuery::ACCESS_TIME)
    .union(fio::NodeAttributesQuery::MODIFICATION_TIME)
    .union(fio::NodeAttributesQuery::CHANGE_TIME)
    .union(fio::NodeAttributesQuery::CONTENT_SIZE)
    .union(fio::NodeAttributesQuery::STORAGE_SIZE)
    .union(fio::NodeAttributesQuery::LINK_COUNT)
    .union(fio::NodeAttributesQuery::ID);

/// The rights of the connections to nodes: all that directories, files and symlinks have.
const RIGHTS: fio::Flags = fio::PERM_READABLE.union(fio::PERM_WRITABLE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Fifo,
    CharDevice,
    BlockDevice,
    Socket,
}

impl FileKind {
    /// The kind whose `S_IFMT` bits `mode` has, if any.
    fn of_mode(mode: u32) -> Option<Self> {
        Some(match mode & S_IFMT {
            S_IFREG => Self::File,
            S_IFDIR => Self::Directory,
            S_IFLNK => Self::Symlink,
            S_IFIFO => Self::Fifo,
            S_IFCHR => Self::CharDevice,
            S_IFBLK => Self::BlockDevice,
            S_IFSOCK => Self::Socket,
            _ => return None,
        })
    }

    /// The `S_IFMT` bits for this kind.
    pub fn mode_type(self) -> u32 {
        match self {
            Self::File => S_IFREG,
            Self::Directory => S_IFDIR,
            Self::Symlink => S_IFLNK,
            Self::Fifo => S_IFIFO,
            Self::CharDevice => S_IFCHR,
            Self::BlockDevice => S_IFBLK,
            Self::Socket => S_IFSOCK,
        }
    }

    /// The permissions of a node that has none of its own, such as the root directory of a new
    /// volume.
    fn default_perm(self) -> u16 {
        match self {
            Self::Directory => 0o755,
            Self::Symlink => 0o777,
            _ => 0o644,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timespec {
    pub secs: u64,
    pub nanos: u32,
}

impl Timespec {
    fn now() -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self { secs: now.as_secs(), nanos: now.subsec_nanos() }
    }

    fn from_nanos(nanos: u64) -> Self {
        Self { secs: nanos / 1_000_000_000, nanos: (nanos % 1_000_000_000) as u32 }
    }

    fn as_nanos(self) -> u64 {
        self.secs * 1_000_000_000 + self.nanos as u64
    }
}

impl From<(u64, u32)> for Timespec {
    fn from((secs, nanos): (u64, u32)) -> Self {
        Self { secs, nanos }
    }
}

/// Attributes of an inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    pub ino: u64,
    pub size: u64,
    /// Allocated size in 512-byte units.
    pub blocks: u64,
    pub atime: Timespec,
    pub mtime: Timespec,
    pub ctime: Timespec,
    pub kind: FileKind,
    /// Permission bits, including setuid, setgid and sticky.
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    /// The device a character or block device node stands for.
    pub rdev: u32,
    /// Preferred I/O size.
    pub blksize: u32,
}

impl Attr {
    /// `st_mode`: file type and permission bits.
    pub fn mode(&self) -> u32 {
        self.kind.mode_type() | self.perm as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetTime {
    Now,
    At(Timespec),
}

impl SetTime {
    fn resolve(self) -> Timespec {
        match self {
            Self::Now => Timespec::now(),
            Self::At(time) => time,
        }
    }
}

/// Attribute changes; `None` leaves an attribute unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct SetAttr {
    /// New permission bits (the file type cannot change).
    pub perm: Option<u16>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub atime: Option<SetTime>,
    pub mtime: Option<SetTime>,
    /// Clear setuid, and setgid if group-executable, as a write or chown by an unprivileged process
    /// does. Applied before `perm`.
    pub kill_suid_sgid: bool,
}

/// Credentials of the process creating a node, which will own it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Creds {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub kind: FileKind,
    pub name: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatFs {
    pub block_size: u32,
    pub blocks: u64,
    pub blocks_free: u64,
    pub name_max: u32,
}

/// A connection to a node.
#[derive(Clone)]
enum Connection {
    Directory(fio::DirectoryProxy),
    /// A file, and the stream its contents are read and written through.
    File(fio::FileProxy, Arc<zx::Stream>),
    Symlink(fio::SymlinkProxy),
}

impl Connection {
    async fn get_attributes(&self, query: fio::NodeAttributesQuery) -> Result<fio::Attributes> {
        match self {
            Self::Directory(proxy) => proxy.get_attributes(query).await?,
            Self::File(proxy, _) => proxy.get_attributes(query).await?,
            Self::Symlink(proxy) => proxy.get_attributes(query).await?,
        }
        .map_err(raw)
    }

    async fn update_attributes(&self, attributes: &fio::MutableNodeAttributes) -> Result<()> {
        match self {
            Self::Directory(proxy) => proxy.update_attributes(attributes).await?,
            Self::File(proxy, _) => proxy.update_attributes(attributes).await?,
            // Symlinks' connections don't update attributes.
            Self::Symlink(_) => Err(zx::Status::NOT_SUPPORTED.into_raw()),
        }
        .map_err(raw)
    }

    fn directory(&self) -> Result<&fio::DirectoryProxy> {
        match self {
            Self::Directory(proxy) => Ok(proxy),
            _ => Err(Error::new(linux::ENOTDIR)),
        }
    }

    fn file(&self) -> Result<(&fio::FileProxy, &Arc<zx::Stream>)> {
        match self {
            Self::File(proxy, stream) => Ok((proxy, stream)),
            Self::Directory(_) => Err(Error::new(linux::EISDIR)),
            Self::Symlink(_) => Err(Error::new(linux::EINVAL)),
        }
    }
}

/// An inode the kernel has looked up.
struct Node {
    connection: Connection,
    /// The kernel's lookup count; the root's is never forgotten.
    lookups: u64,
    /// For a directory, its parent's inode and its name, which `..` and renames need.
    location: Option<(u64, Vec<u8>)>,
}

/// An open directory: a snapshot of its entries, including `.` and `..`.
struct OpenDir {
    entries: Arc<[DirEntry]>,
}

struct State {
    nodes: BTreeMap<u64, Node>,
    dirs: BTreeMap<u64, OpenDir>,
    next_handle: u64,
}

/// A device's DAX window, and what is mapped into it.
struct Dax {
    window: Arc<DaxWindow>,
    /// The mapped ranges of the window, by start: their ends and the connections to the files
    /// mapped there, which keep the files open while they are mapped.
    mappings: BTreeMap<u64, (u64, Connection)>,
}

impl Dax {
    /// Forgets the mappings of the window's range `start..end`, keeping those of what is around it.
    fn forget(&mut self, start: u64, end: u64) {
        let overlapping: Vec<u64> = self
            .mappings
            .range(..end)
            .filter(|(_, (mapping_end, _))| *mapping_end > start)
            .map(|(mapping_start, _)| *mapping_start)
            .collect();
        for mapping_start in overlapping {
            let (mapping_end, connection) = self.mappings.remove(&mapping_start).unwrap();
            if mapping_start < start {
                self.mappings.insert(mapping_start, (start, connection.clone()));
            }
            if mapping_end > end {
                self.mappings.insert(end, (mapping_end, connection));
            }
        }
    }
}

/// An Fxfs volume served by inode number. See the module docs.
pub struct Vfs {
    executor: Executor,
    /// The filesystem and volume, until they are unmounted.
    mounted: Mutex<Option<Mounted>>,
    root_object_id: u64,
    block_size: u32,
    state: Mutex<State>,
    /// The DAX window, once it is created.
    dax: Mutex<Option<Dax>>,
}

struct Mounted {
    filesystem: OpenFxFilesystem,
    volume: FxVolumeAndRoot,
}

impl Vfs {
    /// Formats `device` with an empty volume and serves it.
    pub fn format(device: DeviceHolder) -> anyhow::Result<Self> {
        Self::start(device, true)
    }

    /// Serves the volume on `device`, which `format` formatted.
    pub fn mount(device: DeviceHolder) -> anyhow::Result<Self> {
        Self::start(device, false)
    }

    fn start(device: DeviceHolder, format: bool) -> anyhow::Result<Self> {
        let executor = Executor::new(EXECUTOR_THREADS);
        let (mounted, root) = executor.run(async move {
            let filesystem = FxFilesystemBuilder::new().format(format).open(device).await?;
            let root_volume = root_volume(filesystem.clone()).await?;
            let store = if format {
                root_volume
                    .new_volume(
                        VOLUME_NAME,
                        NewChildStoreOptions {
                            options: StoreOptions::default(),
                            ..NewChildStoreOptions::default()
                        },
                    )
                    .await?
            } else {
                root_volume.volume(VOLUME_NAME, StoreOptions::default()).await?
            };
            let store_object_id = store.store_object_id();
            let volume = FxVolumeAndRoot::new::<FxDirectory>(
                Weak::new(),
                store,
                store_object_id,
                VOLUME_NAME.to_owned(),
                Arc::new(PageRefaultCounter::new()?),
                MemoryPressureConfig::default(),
            )
            .await?;
            let (root, server_end) = create_proxy::<fio::DirectoryMarker>();
            volume.root().clone().serve(RIGHTS, server_end);
            anyhow::Ok((Mounted { filesystem, volume }, root))
        })?;
        let root_object_id = mounted.volume.volume().store().root_directory_object_id();
        let block_size = mounted.filesystem.block_size().get() as u32;
        let nodes = BTreeMap::from([(
            ROOT_INO,
            Node { connection: Connection::Directory(root), lookups: 1, location: None },
        )]);
        Ok(Self {
            executor,
            mounted: Mutex::new(Some(mounted)),
            root_object_id,
            block_size,
            state: Mutex::new(State { nodes, dirs: BTreeMap::new(), next_handle: 1 }),
            dax: Mutex::new(None),
        })
    }

    /// Serves the image at `path`, which `format_image` formatted.
    pub fn mount_image(path: &Path) -> anyhow::Result<Self> {
        Self::mount(image_device(path)?)
    }

    /// Formats the image at `path`, a file whose size is that of the filesystem, and serves it.
    pub fn format_image(path: &Path) -> anyhow::Result<Self> {
        Self::format(image_device(path)?)
    }

    /// Closes the volume and filesystem, writing back what is dirty, and returns the device, closed.
    pub fn close(self) -> anyhow::Result<DeviceHolder> {
        Ok(self.unmount()?.expect("the volume is unmounted"))
    }

    /// Closes the volume and filesystem, writing back what is dirty, and returns the device, closed,
    /// unless they are already unmounted. The inodes' connections are closed, and every operation
    /// fails from then on.
    pub fn unmount(&self) -> anyhow::Result<Option<DeviceHolder>> {
        let Some(Mounted { filesystem, volume }) = self.mounted.lock().unwrap().take() else {
            return Ok(None);
        };
        // Unmap the files in the DAX window, and close every connection, so that the volume's nodes
        // can be dropped.
        if let Some(dax) = self.dax.lock().unwrap().take() {
            dax.window.unmap(0, dax.window.size())?;
        }
        let nodes = std::mem::take(&mut self.state.lock().unwrap().nodes);
        self.executor.run(async move {
            for (_, node) in nodes {
                if let Connection::Directory(proxy) = node.connection {
                    let _ = proxy.close().await;
                }
            }
            volume.volume().terminate().await;
            filesystem.close().await?;
            anyhow::Ok(Some(filesystem.take_device().await))
        })
    }

    /// Checks the filesystem and volume on `device`, failing on any error or warning, and returns
    /// the device, closed.
    pub fn fsck(device: DeviceHolder) -> anyhow::Result<DeviceHolder> {
        Executor::new(EXECUTOR_THREADS).run(async move {
            let filesystem = FxFilesystem::open(device).await?;
            let options = FsckOptions { fail_on_warning: true, ..Default::default() };
            fsck_with_options(filesystem.clone(), &options).await?;
            let store_id = root_volume(filesystem.clone())
                .await?
                .volume(VOLUME_NAME, StoreOptions::default())
                .await?
                .store_object_id();
            fsck_volume_with_options(filesystem.as_ref(), &options, store_id, None).await?;
            filesystem.close().await?;
            anyhow::Ok(filesystem.take_device().await)
        })
    }

    /// The inode number of the object `object_id`.
    fn ino(&self, object_id: u64) -> u64 {
        if object_id == self.root_object_id {
            ROOT_INO
        } else {
            assert_ne!(object_id, ROOT_INO, "object 1 would have the root's inode number");
            object_id
        }
    }

    fn connection(&self, ino: u64) -> Result<Connection> {
        let state = self.state.lock().unwrap();
        Ok(state.nodes.get(&ino).ok_or(Error::new(linux::ENOENT))?.connection.clone())
    }

    fn directory(&self, ino: u64) -> Result<fio::DirectoryProxy> {
        // `Clone::clone`, as the proxy's `clone` is fuchsia.io's `Clone`.
        Ok(Clone::clone(self.connection(ino)?.directory()?))
    }

    /// Takes a lookup on the node `connection` connects to, remembering it if it's new, and
    /// returns its attributes.
    fn remember(
        &self,
        parent: u64,
        name: &[u8],
        connection: Connection,
        attributes: fio::Attributes,
    ) -> Result<Attr> {
        let attr = self.attr(&connection, attributes)?;
        let location =
            matches!(connection, Connection::Directory(_)).then(|| (parent, name.to_owned()));
        let mut state = self.state.lock().unwrap();
        let node = state.nodes.entry(attr.ino).or_insert(Node { connection, lookups: 0, location });
        node.lookups += 1;
        Ok(attr)
    }

    fn attr(&self, connection: &Connection, (mutable, immutable): fio::Attributes) -> Result<Attr> {
        let kind_of_connection = match connection {
            Connection::Directory(_) => FileKind::Directory,
            Connection::File(..) => FileKind::File,
            Connection::Symlink(_) => FileKind::Symlink,
        };
        // Special files are Fxfs files whose mode says what they are, as Starnix makes them.
        let kind = mutable.mode.and_then(FileKind::of_mode).unwrap_or(kind_of_connection);
        let perm = mutable.mode.map_or(kind.default_perm(), |mode| (mode & PERM_MASK) as u16);
        let object_id = immutable.id.ok_or(Error::new(linux::EIO))?;
        Ok(Attr {
            ino: self.ino(object_id),
            size: immutable.content_size.unwrap_or(0),
            blocks: immutable.storage_size.unwrap_or(0).div_ceil(512),
            atime: Timespec::from_nanos(mutable.access_time.unwrap_or(0)),
            mtime: Timespec::from_nanos(mutable.modification_time.unwrap_or(0)),
            ctime: Timespec::from_nanos(immutable.change_time.unwrap_or(0)),
            kind,
            perm,
            nlink: immutable.link_count.unwrap_or(1) as u32,
            uid: mutable.uid.unwrap_or(0),
            gid: mutable.gid.unwrap_or(0),
            rdev: mutable.rdev.unwrap_or(0) as u32,
            blksize: self.block_size,
        })
    }

    /// Opens `name` in `parent` with `flags`, and returns a connection to the node and its
    /// attributes, from the representation the open sends.
    fn open_node(
        &self,
        parent: &fio::DirectoryProxy,
        name: &[u8],
        flags: fio::Flags,
        create_attributes: Option<fio::MutableNodeAttributes>,
    ) -> Result<(Connection, fio::Attributes)> {
        let name = std::str::from_utf8(name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        let parent = Clone::clone(parent);
        self.executor.run(async move {
            let (node, server_end) = create_proxy::<fio::NodeMarker>();
            parent.open(
                &name,
                flags | RIGHTS | fio::Flags::FLAG_SEND_REPRESENTATION,
                &fio::Options { attributes: Some(ATTRIBUTES), create_attributes },
                server_end.into_channel(),
            )?;
            let representation = node
                .take_event_stream()
                .next()
                .await
                .ok_or(Error::new(linux::EIO))??
                .into_on_representation()
                .ok_or(Error::new(linux::EIO))?;
            let channel = node.into_channel().unwrap();
            let (connection, attributes) = match representation {
                fio::Representation::Directory(info) => (
                    Connection::Directory(fio::DirectoryProxy::from_channel(channel)),
                    info.attributes,
                ),
                fio::Representation::File(info) => {
                    let stream = info.stream.ok_or(Error::new(linux::EIO))?;
                    (
                        Connection::File(fio::FileProxy::from_channel(channel), Arc::new(stream)),
                        info.attributes,
                    )
                }
                fio::Representation::Symlink(info) => {
                    (Connection::Symlink(fio::SymlinkProxy::from_channel(channel)), info.attributes)
                }
            };
            let attributes = attributes.ok_or(Error::new(linux::EIO))?;
            Ok((connection, (attributes.mutable_attributes, attributes.immutable_attributes)))
        })
    }

    /// Creates `name` in `parent` as a `protocol` node with `mode`, owned by `creds`.
    fn create_node(
        &self,
        creds: Creds,
        parent: u64,
        name: &[u8],
        protocol: fio::Flags,
        mode: u32,
        rdev: u32,
    ) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let (connection, attributes) = self.open_node(
            &directory,
            name,
            protocol | fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::FLAG_MUST_CREATE,
            Some(fio::MutableNodeAttributes {
                mode: Some(mode),
                uid: Some(creds.uid),
                gid: Some(creds.gid),
                rdev: Some(rdev as u64),
                ..Default::default()
            }),
        )?;
        self.remember(parent, name, connection, attributes)
    }

    pub fn lookup(&self, parent: u64, name: &[u8]) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let (connection, attributes) =
            self.open_node(&directory, name, fio::Flags::empty(), None)?;
        self.remember(parent, name, connection, attributes)
    }

    pub fn forget(&self, ino: u64, count: u64) {
        if ino == ROOT_INO {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(node) = state.nodes.get_mut(&ino) {
            node.lookups = node.lookups.saturating_sub(count);
            if node.lookups == 0 {
                // Dropping the node's connection closes it.
                state.nodes.remove(&ino);
            }
        }
    }

    pub fn getattr(&self, ino: u64) -> Result<Attr> {
        let connection = self.connection(ino)?;
        let query_connection = connection.clone();
        let attributes =
            self.executor.run(async move { query_connection.get_attributes(ATTRIBUTES).await })?;
        self.attr(&connection, attributes)
    }

    pub fn setattr(&self, ino: u64, set: SetAttr) -> Result<Attr> {
        let connection = self.connection(ino)?;
        if let Some(size) = set.size {
            let (proxy, _) = connection.file()?;
            let proxy = Clone::clone(proxy);
            self.executor.run(async move { proxy.resize(size).await?.map_err(raw) })?;
        }
        let mut update = fio::MutableNodeAttributes {
            uid: set.uid,
            gid: set.gid,
            access_time: set.atime.map(|time| time.resolve().as_nanos()),
            modification_time: set.mtime.map(|time| time.resolve().as_nanos()),
            ..Default::default()
        };
        if set.perm.is_some() || set.kill_suid_sgid {
            let attr = self.getattr(ino)?;
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
            update.mode = Some(attr.kind.mode_type() | perm);
        }
        if update != fio::MutableNodeAttributes::default() {
            let connection = connection.clone();
            self.executor.run(async move { connection.update_attributes(&update).await })?;
        }
        self.getattr(ino)
    }

    pub fn readlink(&self, ino: u64) -> Result<Vec<u8>> {
        let Connection::Symlink(proxy) = self.connection(ino)? else {
            return Err(Error::new(linux::EINVAL));
        };
        self.executor
            .run(async move { proxy.describe().await?.target.ok_or(Error::new(linux::EIO)) })
    }

    pub fn create(&self, creds: Creds, parent: u64, name: &[u8], perm: u16) -> Result<Attr> {
        let mode = S_IFREG | (perm as u32 & PERM_MASK);
        self.create_node(creds, parent, name, fio::Flags::PROTOCOL_FILE, mode, 0)
    }

    /// Creates a regular file, or a special file: a FIFO, socket, or character or block device,
    /// which is an Fxfs file whose mode says what it is, as Starnix makes them.
    pub fn mknod(
        &self,
        creds: Creds,
        parent: u64,
        name: &[u8],
        mode: u32,
        rdev: u32,
    ) -> Result<Attr> {
        let kind = FileKind::of_mode(mode).unwrap_or(FileKind::File);
        if matches!(kind, FileKind::Directory | FileKind::Symlink) {
            return Err(Error::new(linux::EINVAL));
        }
        let mode = kind.mode_type() | (mode & PERM_MASK);
        self.create_node(creds, parent, name, fio::Flags::PROTOCOL_FILE, mode, rdev)
    }

    pub fn mkdir(&self, creds: Creds, parent: u64, name: &[u8], perm: u16) -> Result<Attr> {
        let mode = S_IFDIR | (perm as u32 & PERM_MASK);
        self.create_node(creds, parent, name, fio::Flags::PROTOCOL_DIRECTORY, mode, 0)
    }

    /// Creates a symlink. Its owner is the volume's default, as symlinks' connections don't update
    /// attributes.
    pub fn symlink(&self, parent: u64, name: &[u8], target: &[u8]) -> Result<Attr> {
        let directory = self.directory(parent)?;
        let name_str = std::str::from_utf8(name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        let target = target.to_vec();
        let (proxy, attributes) = self.executor.run(async move {
            let (proxy, server_end) = create_proxy::<fio::SymlinkMarker>();
            directory.create_symlink(&name_str, &target, Some(server_end)).await?.map_err(raw)?;
            let attributes = proxy.get_attributes(ATTRIBUTES).await?.map_err(raw)?;
            Ok::<_, Error>((proxy, attributes))
        })?;
        self.remember(parent, name, Connection::Symlink(proxy), attributes)
    }

    pub fn link(&self, ino: u64, new_parent: u64, new_name: &[u8]) -> Result<Attr> {
        let connection = self.connection(ino)?;
        let directory = self.directory(new_parent)?;
        let new_name =
            std::str::from_utf8(new_name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        let link_connection = connection.clone();
        let attributes = self.executor.run(async move {
            let token = token(&directory).await?;
            match &link_connection {
                Connection::File(proxy, _) => proxy.link_into(token, &new_name).await?,
                Connection::Symlink(proxy) => proxy.link_into(token, &new_name).await?,
                Connection::Directory(_) => return Err(Error::new(linux::EPERM)),
            }
            .map_err(raw)?;
            link_connection.get_attributes(ATTRIBUTES).await
        })?;
        let attr = self.attr(&connection, attributes)?;
        self.state.lock().unwrap().nodes.get_mut(&ino).ok_or(Error::new(linux::ENOENT))?.lookups +=
            1;
        Ok(attr)
    }

    pub fn unlink(&self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, false)
    }

    pub fn rmdir(&self, parent: u64, name: &[u8]) -> Result<()> {
        self.remove(parent, name, true)
    }

    fn remove(&self, parent: u64, name: &[u8], must_be_directory: bool) -> Result<()> {
        let directory = self.directory(parent)?;
        let name = std::str::from_utf8(name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        let flags = must_be_directory.then_some(fio::UnlinkFlags::MUST_BE_DIRECTORY);
        self.executor.run(async move {
            directory.unlink(&name, &fio::UnlinkOptions { flags }).await?.map_err(raw)
        })
    }

    pub fn rename(
        &self,
        old_parent: u64,
        old_name: &[u8],
        new_parent: u64,
        new_name: &[u8],
        flags: u32,
    ) -> Result<()> {
        if flags & RENAME_EXCHANGE != 0 || flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE) != 0 {
            return Err(Error::new(linux::EINVAL));
        }
        let old_directory = self.directory(old_parent)?;
        let new_directory = self.directory(new_parent)?;
        if flags & RENAME_NOREPLACE != 0 {
            match self.open_node(&new_directory, new_name, fio::Flags::empty(), None) {
                Ok(_) => return Err(Error::new(linux::EEXIST)),
                Err(error) if error.errno() == linux::ENOENT => {}
                Err(error) => return Err(error),
            }
        }
        let old = std::str::from_utf8(old_name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        let new = std::str::from_utf8(new_name).map_err(|_| Error::new(linux::EINVAL))?.to_owned();
        self.executor.run(async move {
            let token = token(&new_directory).await?;
            old_directory.rename(&old, token, &new).await?.map_err(raw)
        })?;
        // A directory the kernel knows moved, so its `..` did.
        let mut state = self.state.lock().unwrap();
        for node in state.nodes.values_mut() {
            if node.location.as_ref() == Some(&(old_parent, old_name.to_vec())) {
                node.location = Some((new_parent, new_name.to_vec()));
            }
        }
        Ok(())
    }

    /// Opens `ino`, truncating it if `truncate`.
    pub fn open(&self, ino: u64, truncate: bool) -> Result<()> {
        if truncate {
            let connection = self.connection(ino)?;
            let (proxy, _) = connection.file()?;
            let proxy = Clone::clone(proxy);
            self.executor.run(async move { proxy.resize(0).await?.map_err(raw) })?;
        }
        Ok(())
    }

    /// Reads from `ino` at `offset` into `buf`, returning how much was read.
    pub fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let stream = self.stream(ino)?;
        assert_not_on_executor();
        // SAFETY: `u8` and `MaybeUninit<u8>` have the same layout, and the stream only writes
        // initialized bytes into the buffer.
        let buf = unsafe { &mut *(buf as *mut [u8] as *mut [MaybeUninit<u8>]) };
        Ok(stream.read_at_uninit(zx::StreamReadOptions::empty(), offset, buf)?)
    }

    /// Writes `data` to `ino` at `offset`, returning how much was written.
    pub fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize> {
        let stream = self.stream(ino)?;
        assert_not_on_executor();
        Ok(stream.write_at(zx::StreamWriteOptions::empty(), offset, data)?)
    }

    fn stream(&self, ino: u64) -> Result<Arc<zx::Stream>> {
        Ok(self.connection(ino)?.file()?.1.clone())
    }

    /// Allocates `len` bytes of `ino` at `offset`, growing it if they're past its end. `mode` is
    /// `fallocate`'s; only its default, 0, is supported.
    pub fn fallocate(&self, ino: u64, mode: u32, offset: u64, len: u64) -> Result<()> {
        if mode != 0 {
            return Err(Error::new(linux::ENOTSUP));
        }
        let connection = self.connection(ino)?;
        let (proxy, _) = connection.file()?;
        let proxy = Clone::clone(proxy);
        self.executor.run(async move {
            proxy.allocate(offset, len, fio::AllocateMode::empty()).await?.map_err(raw)
        })
    }

    /// Writes back `ino`'s data and metadata, and for a directory, the filesystem's.
    pub fn fsync(&self, ino: u64) -> Result<()> {
        let connection = self.connection(ino)?;
        self.executor.run(async move {
            match &connection {
                Connection::Directory(proxy) => proxy.sync().await?,
                Connection::File(proxy, _) => proxy.sync().await?,
                Connection::Symlink(_) => Ok(()),
            }
            .map_err(raw)
        })
    }

    pub fn statfs(&self) -> Result<StatFs> {
        let root = self.directory(ROOT_INO)?;
        let info = self.executor.run(async move {
            let (status, info) = root.query_filesystem().await?;
            zx::Status::ok(status)?;
            info.ok_or(Error::new(linux::EIO))
        })?;
        let block_size = info.block_size.max(1);
        Ok(StatFs {
            block_size,
            blocks: info.total_bytes / block_size as u64,
            blocks_free: info.total_bytes.saturating_sub(info.used_bytes) / block_size as u64,
            name_max: info.max_filename_size,
        })
    }

    /// Creates the DAX window of `size` bytes that `setup_mapping` maps files into, whose mappings
    /// are also made to the hypervisor's `second_stage`. A volume serves one window.
    pub fn create_dax_window(
        &self,
        size: u64,
        second_stage: Option<Arc<dyn SecondStage>>,
    ) -> Result<Arc<DaxWindow>> {
        let mut dax = self.dax.lock().unwrap();
        if dax.is_some() {
            return Err(Error::new(linux::EEXIST));
        }
        let window = DaxWindow::new(size, second_stage)?;
        *dax = Some(Dax { window: window.clone(), mappings: BTreeMap::new() });
        Ok(window)
    }

    /// Maps `len` bytes of file `ino` at `file_offset` into the DAX window at `window_offset`,
    /// replacing what was mapped there (`FUSE_SETUPMAPPING`). Offsets and length must be page
    /// aligned. Stores through a `writable` mapping dirty the file's pages, which are written back
    /// when the file is flushed.
    pub fn setup_mapping(
        &self,
        ino: u64,
        file_offset: u64,
        len: u64,
        writable: bool,
        window_offset: u64,
    ) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Error::new(linux::ENOSYS))?;
        dax.window.check_range(window_offset, len)?;
        if file_offset % page_size() != 0 || file_offset.checked_add(len).is_none() {
            return Err(Error::new(linux::EINVAL));
        }
        let connection = self.connection(ino)?;
        let Connection::File(proxy, _) = &connection else {
            return Err(Error::new(linux::EINVAL));
        };
        let mut flags = fio::VmoFlags::READ | fio::VmoFlags::SHARED_BUFFER;
        if writable {
            flags |= fio::VmoFlags::WRITE;
        }
        let proxy = Clone::clone(proxy);
        let vmo = self
            .executor
            .run(async move { proxy.get_backing_memory(flags).await?.map_err(raw) })?;
        dax.window.map(window_offset, &vmo, file_offset, len, writable)?;
        let end = window_offset + len;
        dax.forget(window_offset, end);
        dax.mappings.insert(window_offset, (end, connection));
        Ok(())
    }

    /// Unmaps `len` bytes of the DAX window at `window_offset` (`FUSE_REMOVEMAPPING`). Dirty pages
    /// stay the files', to be written back when they are flushed.
    pub fn remove_mapping(&self, window_offset: u64, len: u64) -> Result<()> {
        let mut dax = self.dax.lock().unwrap();
        let dax = dax.as_mut().ok_or(Error::new(linux::ENOSYS))?;
        dax.window.check_range(window_offset, len)?;
        dax.window.unmap(window_offset, len)?;
        dax.forget(window_offset, window_offset + len);
        Ok(())
    }

    /// Opens directory `ino` for reading its entries, returning a handle to them.
    pub fn opendir(&self, ino: u64) -> Result<u64> {
        let directory = self.directory(ino)?;
        let parent = {
            let state = self.state.lock().unwrap();
            state.nodes.get(&ino).and_then(|node| node.location.as_ref()).map_or(ROOT_INO, |l| l.0)
        };
        let raw_entries = self.executor.run(async move {
            zx::Status::ok(directory.rewind().await?)?;
            let mut entries = Vec::new();
            loop {
                let (status, buf) = directory.read_dirents(fio::MAX_BUF).await?;
                zx::Status::ok(status)?;
                if buf.is_empty() {
                    break Ok::<_, Error>(entries);
                }
                entries.extend(dirents(&buf)?);
            }
        })?;
        let mut entries = vec![
            DirEntry { ino, kind: FileKind::Directory, name: b".".to_vec() },
            DirEntry { ino: parent, kind: FileKind::Directory, name: b"..".to_vec() },
        ];
        for (object_id, kind, name) in raw_entries {
            if name != b"." {
                entries.push(DirEntry { ino: self.ino(object_id), kind, name });
            }
        }
        let mut state = self.state.lock().unwrap();
        let handle = state.next_handle;
        state.next_handle += 1;
        state.dirs.insert(handle, OpenDir { entries: entries.into() });
        Ok(handle)
    }

    /// The entries of the open directory `handle`, from `offset`, the index of the first.
    pub fn readdir(&self, handle: u64, offset: u64) -> Result<Arc<[DirEntry]>> {
        let state = self.state.lock().unwrap();
        let entries = &state.dirs.get(&handle).ok_or(Error::new(linux::EBADF))?.entries;
        Ok(entries.get(offset as usize..).unwrap_or(&[]).into())
    }

    pub fn releasedir(&self, handle: u64) {
        self.state.lock().unwrap().dirs.remove(&handle);
    }
}

impl Drop for Vfs {
    /// Unmounts the volume if it is still mounted, writing back what is dirty.
    fn drop(&mut self) {
        if let Err(error) = self.unmount() {
            log::error!("fxdax: failed to unmount: {error:?}");
        }
    }
}

/// The device of the image at `path`.
fn image_device(path: &Path) -> anyhow::Result<DeviceHolder> {
    let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    Ok(DeviceHolder::new(FileBackedDevice::new(file, IMAGE_BLOCK_SIZE)))
}

/// A token for `directory`, for renaming and linking into it.
async fn token(directory: &fio::DirectoryProxy) -> Result<zx::Event> {
    let (status, token) = directory.get_token().await?;
    zx::Status::ok(status)?;
    Ok(zx::Event::from(token.ok_or(Error::new(linux::EIO))?))
}

/// The entries of a `ReadDirents` buffer: object ids, kinds and names. Each is an inode number, a
/// name length and a type, then the name (`fuchsia.io/Directory.ReadDirents`).
fn dirents(mut buf: &[u8]) -> Result<Vec<(u64, FileKind, Vec<u8>)>> {
    const HEADER_SIZE: usize = 10;
    let mut entries = Vec::new();
    while !buf.is_empty() {
        let (header, rest) = buf.split_at_checked(HEADER_SIZE).ok_or(Error::new(linux::EIO))?;
        let object_id = u64::from_le_bytes(header[..8].try_into().unwrap());
        let (name, rest) =
            rest.split_at_checked(header[8] as usize).ok_or(Error::new(linux::EIO))?;
        let kind = match fio::DirentType::from_primitive(header[9]) {
            Some(fio::DirentType::Directory) => FileKind::Directory,
            Some(fio::DirentType::Symlink) => FileKind::Symlink,
            _ => FileKind::File,
        };
        entries.push((object_id, kind, name.to_vec()));
        buf = rest;
    }
    Ok(entries)
}

// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Container images as filesystems (TASK-39.5): an image's layers, tar archives of what each
//! changes, applied in turn to a [`Filesystem`], such as a base store's, which VMs boot forks of.
//!
//! A layer is applied as the OCI image spec says (`layer.md`, "Applying Changesets"):
//!
//! - Each entry makes a file, directory, symlink, hard link, or device or FIFO node, with the
//!   entry's permissions, owner, times and extended attributes (PAX `SCHILY.xattr.*` records). It
//!   replaces what its path names already, except that a directory over a directory keeps its
//!   entries, taking the new one's attributes.
//! - A whiteout, `.wh.NAME`, removes `NAME`, and an opaque whiteout, `.wh..wh..opq`, everything in
//!   its directory that the layers below made.
//! - The directories an entry's path passes through that no entry made are made, root's, with
//!   permissions 0755, at the Unix epoch.
//!
//! A `docker export` tarball is an image of one layer. An image in an OCI image layout, as
//! `skopeo copy` or `docker save` makes, is [`OciImage`]: its layers, gzipped or not, which are
//! checked against their digests as they are applied.
//!
//! What an image makes depends on the image alone: the same image gives the same files, with the
//! same contents, owners, permissions, times and attributes; only their inode numbers and change
//! times differ. Directories' times are set once every layer is applied, as the entries made in a
//! directory change its times. A file's runs of zeros a block long are left as holes.

#[cfg(feature = "testing")]
pub mod testing;

use crate::error::{Errno, Error};
use crate::filesystem::{Filesystem, Node};
use crate::node::{FileType, Owner, SetAttr, SetTime, XattrMode};
use crate::storage::Storage;
use anyhow::{Context as _, bail, ensure};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The prefix of a whiteout's name.
const WHITEOUT: &[u8] = b".wh.";
/// The name of an opaque whiteout.
const OPAQUE: &[u8] = b".wh..wh..opq";
/// The PAX records' prefix of an extended attribute's name.
const PAX_XATTR: &str = "SCHILY.xattr.";
/// How much of a file is read from a layer at a time, and written to it at most at once.
const CHUNK: usize = 1 << 20;
/// The size of the holes left in a file: of its blocks this long that hold only zeros.
const HOLE: usize = 4096;
/// The root's path.
const ROOT: &[u8] = b"";

/// Makes a store at `path`, which must not exist, of `capacity` bytes, holding a filesystem of
/// the image `image`, checked: the image `reference` names in an OCI image layout (see
/// [`OciImage::open`]), or, if `image` is a file, the image of that one layer, which `reference`
/// cannot name. A store that cannot be made whole is deleted.
pub async fn store(
    path: impl AsRef<Path>,
    capacity: u64,
    image: &Path,
    reference: Option<&str>,
) -> Result<(), Error> {
    let path = path.as_ref();
    let filled = fill(Storage::create(path, capacity)?, image, reference).await;
    if filled.is_err()
        && let Err(err) = Storage::delete(path)
    {
        log::error!("fxdax: deleting the store half made at {}: {err:#}", path.display());
    }
    filled.map_err(Error::new)
}

/// Formats `storage` with a filesystem of the image `image`, as [`store`] has it, and checks it.
async fn fill(storage: Storage, image: &Path, reference: Option<&str>) -> anyhow::Result<()> {
    let fs = Filesystem::format(storage).await?;
    let imported = async {
        let mut importer = Importer::new(&fs);
        if image.is_dir() {
            OciImage::open(image, reference)?.import(&mut importer).await?;
        } else {
            ensure!(reference.is_none(), "a layer holds one image, which no reference names");
            let layer = std::fs::File::open(image).context("opening the layer")?;
            importer.apply(layer).await?;
        }
        Ok(importer.finish().await?)
    }
    .await;
    let storage = fs.close().await?;
    imported?;
    storage.check().await?;
    Ok(())
}

/// Applies the layers of an image to a filesystem, in turn, bottom first, and then
/// [`finish`](Self::finish)es, setting the directories' times. See the module docs.
pub struct Importer {
    /// The directories found or made, by path, each a handle to it: the root's at [`ROOT`].
    dirs: BTreeMap<Vec<u8>, Node>,
    /// The times of the directories the layers made, and of those they passed through, by path,
    /// which are set once every layer is applied.
    times: BTreeMap<Vec<u8>, Times>,
    /// What the layer being applied made, which its opaque whiteouts keep.
    made: BTreeSet<Vec<u8>>,
    /// What files' contents are read into from the layers, a chunk at a time: one buffer for all
    /// of them, not one each, as most are far smaller.
    chunk: Box<[u8]>,
}

/// A node's access and modification times.
#[derive(Clone, Copy)]
struct Times {
    atime: SystemTime,
    mtime: SystemTime,
}

impl Times {
    const EPOCH: Self = Self { atime: UNIX_EPOCH, mtime: UNIX_EPOCH };

    fn set_attr(self) -> SetAttr {
        SetAttr {
            atime: Some(SetTime::At(self.atime)),
            mtime: Some(SetTime::At(self.mtime)),
            ..Default::default()
        }
    }
}

/// What a layer's entry says of the node it makes, beyond its type.
struct Metadata {
    perm: u16,
    owner: Owner,
    times: Times,
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Importer {
    /// An importer of layers into `fs`, which holds what the layers below them made, or nothing.
    pub fn new(fs: &Filesystem) -> Self {
        let dirs = BTreeMap::from([(ROOT.to_vec(), fs.root().clone())]);
        let chunk = vec![0; CHUNK].into_boxed_slice();
        Self { dirs, times: BTreeMap::new(), made: BTreeSet::new(), chunk }
    }

    /// The root directory.
    fn root(&self) -> Node {
        self.dirs[ROOT].clone()
    }

    /// Applies the layer `layer`, a tar archive, gzipped or not, read as it is applied.
    pub async fn apply(&mut self, layer: impl Read) -> Result<(), Error> {
        self.apply_layer(layer).await.map_err(Error::new)
    }

    async fn apply_layer<'r>(&mut self, layer: impl Read + 'r) -> anyhow::Result<()> {
        let mut layer = BufReader::with_capacity(CHUNK, layer);
        let magic = layer.fill_buf().context("reading the layer")?;
        let layer: Box<dyn Read + 'r> = match magic {
            [0x1f, 0x8b, ..] => Box::new(flate2::bufread::MultiGzDecoder::new(layer)),
            [0x28, 0xb5, 0x2f, 0xfd, ..] => bail!("zstd-compressed layers are not supported"),
            _ => Box::new(layer),
        };
        self.made.clear();
        let mut archive = tar::Archive::new(layer);
        for entry in archive.entries().context("reading the layer")? {
            let mut entry = entry.context("reading the layer")?;
            let path = entry.path_bytes().into_owned();
            self.entry(&mut entry)
                .await
                .with_context(|| format!("applying {:?}", String::from_utf8_lossy(&path)))?;
        }
        Ok(())
    }

    /// Applies the layer's entry `entry`.
    async fn entry(&mut self, entry: &mut tar::Entry<'_, impl Read>) -> anyhow::Result<()> {
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() {
            return Ok(());
        }
        let path = normalize(&entry.path_bytes())?;
        let Some((parent, name)) = split(&path) else {
            // The root, which only takes the entry's attributes.
            ensure!(entry_type.is_dir(), "the root is not a directory");
            let metadata = metadata(entry)?;
            return self.update_directory(&self.root(), &path, metadata).await;
        };
        if name == OPAQUE {
            return self.opaque(parent).await;
        }
        if let Some(whited_out) = name.strip_prefix(WHITEOUT) {
            return self.remove(&join(parent, whited_out)).await;
        }
        let metadata = metadata(entry)?;
        let dir = self.directory(parent).await?;
        match dir.lookup(name).await {
            Ok((existing, attr))
                if entry_type.is_dir() && attr.file_type == FileType::Directory =>
            {
                let existing = self.remember_directory(&path, existing);
                self.made.insert(path.clone());
                return self.update_directory(&existing, &path, metadata).await;
            }
            Ok((existing, _)) => {
                drop(existing);
                self.remove(&path).await?;
            }
            Err(Errno::ENOENT) => {}
            Err(errno) => return Err(errno.into()),
        }
        self.made.insert(path.clone());
        let (perm, owner) = (metadata.perm, metadata.owner);
        let (node, attr) = match entry_type {
            tar::EntryType::Directory => {
                let (made, _) = dir.mkdir(name, perm, owner).await?;
                let made = self.remember_directory(&path, made);
                return self.update_directory(&made, &path, metadata).await;
            }
            tar::EntryType::Link => {
                let target = normalize(&entry.link_name_bytes().context("no link target")?)?;
                let target = self.find(&target).await?.context("the link's target is missing")?;
                // A hard link is its target's node, which keeps the target's attributes: the
                // entry's own are not set.
                target.link(&dir, name).await?;
                return Ok(());
            }
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse => {
                dir.create(name, perm, owner).await?
            }
            tar::EntryType::Symlink => {
                let target = entry.link_name_bytes().context("no symlink target")?;
                dir.symlink(name, &target, owner).await?
            }
            tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo => {
                let (file_type, rdev) = match entry_type {
                    tar::EntryType::Fifo => (FileType::Fifo, 0),
                    tar::EntryType::Char => (FileType::CharDevice, device(entry.header())?),
                    _ => (FileType::BlockDevice, device(entry.header())?),
                };
                dir.mknod(name, file_type, perm, rdev, owner).await?
            }
            other => bail!("entries of type {other:?} are not supported"),
        };
        if attr.file_type == FileType::File {
            write_contents(&node, entry, &mut self.chunk).await?;
        }
        set_metadata(&node, &metadata).await
    }

    /// Gives `dir`, the directory at `path`, the attributes `metadata` says, but its times, which
    /// are set once every layer is applied.
    async fn update_directory(
        &mut self,
        dir: &Node,
        path: &[u8],
        metadata: Metadata,
    ) -> anyhow::Result<()> {
        let set = SetAttr {
            perm: Some(metadata.perm),
            uid: Some(metadata.owner.uid),
            gid: Some(metadata.owner.gid),
            ..Default::default()
        };
        dir.setattr(&set).await?;
        set_xattrs(dir, &metadata.xattrs).await?;
        self.times.insert(path.to_vec(), metadata.times);
        Ok(())
    }

    /// The directory at `path`, made, with the directories it is in, if it isn't there.
    async fn directory(&mut self, path: &[u8]) -> anyhow::Result<Node> {
        Ok(self.walk(path, true).await?.expect("made"))
    }

    /// The directory at `path`, if there is one.
    async fn find_directory(&mut self, path: &[u8]) -> anyhow::Result<Option<Node>> {
        self.walk(path, false).await
    }

    /// The directory at `path`, walking to it from the root, remembering the directories on the
    /// way: if `make`, making those that aren't there, and failing if one isn't a directory.
    async fn walk(&mut self, path: &[u8], make: bool) -> anyhow::Result<Option<Node>> {
        let (mut dir, mut end) = (self.root(), 0);
        for name in path.split(|&byte| byte == b'/').filter(|name| !name.is_empty()) {
            end += if end == 0 { name.len() } else { name.len() + 1 };
            let prefix = &path[..end];
            if let Some(remembered) = self.dirs.get(prefix) {
                dir = remembered.clone();
                continue;
            }
            let found = match dir.lookup(name).await {
                Ok((found, attr)) if attr.file_type == FileType::Directory => found,
                Ok(_) => {
                    ensure!(!make, Errno::ENOTDIR);
                    return Ok(None);
                }
                Err(Errno::ENOENT) if make => {
                    let root = Owner { uid: 0, gid: 0 };
                    let (made, _) = dir.mkdir(name, 0o755, root).await?;
                    self.made.insert(prefix.to_vec());
                    self.times.insert(prefix.to_vec(), Times::EPOCH);
                    made
                }
                Err(Errno::ENOENT) => return Ok(None),
                Err(errno) => return Err(errno.into()),
            };
            dir = self.remember_directory(prefix, found);
        }
        Ok(Some(dir))
    }

    /// Remembers `dir`, a handle to the directory at `path`, unless one is remembered there, and
    /// returns the handle remembered.
    fn remember_directory(&mut self, path: &[u8], dir: Node) -> Node {
        self.dirs.entry(path.to_vec()).or_insert(dir).clone()
    }

    /// The node at `path`, if there is one; the directories it is in are not made.
    async fn find(&mut self, path: &[u8]) -> anyhow::Result<Option<Node>> {
        let Some((parent, name)) = split(path) else {
            return Ok(Some(self.root()));
        };
        let Some(dir) = self.find_directory(parent).await? else {
            return Ok(None);
        };
        match dir.lookup(name).await {
            Ok((found, _)) => Ok(Some(found)),
            Err(Errno::ENOENT) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    /// Removes what is at `path`, if anything, with everything in it, if it is a directory.
    async fn remove(&mut self, path: &[u8]) -> anyhow::Result<()> {
        let Some((parent, name)) = split(path) else {
            bail!("the root cannot be removed");
        };
        let Some(dir) = self.find_directory(parent).await? else {
            return Ok(());
        };
        // Let go of what is remembered of what is removed.
        let within = |key: &Vec<u8>| {
            key.strip_prefix(path).is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
        };
        self.dirs.retain(|key, _| !within(key));
        self.times.retain(|key, _| !within(key));
        self.made.retain(|key| !within(key));
        remove(&dir, name.to_vec()).await
    }

    /// Removes everything in the directory at `path` that the layers below made.
    async fn opaque(&mut self, path: &[u8]) -> anyhow::Result<()> {
        let Some(dir) = self.find_directory(path).await? else {
            return Ok(());
        };
        for name in entries(&dir).await? {
            let child = join(path, &name);
            if !self.made.contains(&child) {
                self.remove(&child).await?;
            }
        }
        Ok(())
    }

    /// Sets the times of the directories the layers made or passed through, which are final once
    /// every layer is applied.
    pub async fn finish(self) -> Result<(), Error> {
        for (path, times) in &self.times {
            self.dirs[path].setattr(&times.set_attr()).await.map_err(|errno| {
                Error::new(
                    anyhow::Error::from(errno).context(format!(
                        "setting the times of {:?}",
                        String::from_utf8_lossy(path)
                    )),
                )
            })?;
        }
        Ok(())
    }
}

/// Writes the contents of the file entry `entry` to `file`, a new file, reading them into `chunk`
/// a chunk at a time, and leaving their runs of zeros a block long as holes.
async fn write_contents(
    file: &Node,
    entry: &mut tar::Entry<'_, impl Read>,
    chunk: &mut [u8],
) -> anyhow::Result<()> {
    let size = entry.size();
    // Where the data written ends: the file's size, as it was empty.
    let mut written = 0;
    let mut offset = 0;
    while offset < size {
        let len = read_full(entry, chunk)?;
        ensure!(len > 0, "the layer ends in the file, at {offset} of {size} bytes");
        // One write a run of data: a write waits for Fxfs's pager to let it dirty its pages,
        // which it asks for together, so a run of blocks costs one round trip, not one each.
        for run in data_runs(&chunk[..len]) {
            written = offset + run.end as u64;
            write_all(file, offset + run.start as u64, &chunk[run]).await?;
        }
        offset += len as u64;
    }
    // A hole at the end, which no write extended the file over.
    if written < size {
        let set = SetAttr { size: Some(size), ..Default::default() };
        file.setattr(&set).await?;
    }
    Ok(())
}

/// The ranges of `data` to write, leaving the rest as holes: the runs of its consecutive blocks of
/// [`HOLE`] bytes, the last of them maybe shorter, that hold more than zeros.
fn data_runs(data: &[u8]) -> impl Iterator<Item = Range<usize>> + '_ {
    fn holds_data((_, block): &(usize, &[u8])) -> bool {
        block.iter().any(|&byte| byte != 0)
    }
    let mut blocks = data.chunks(HOLE).enumerate().peekable();
    std::iter::from_fn(move || {
        let (first, block) = blocks.find(holds_data)?;
        let start = first * HOLE;
        let mut end = start + block.len();
        while let Some((_, block)) = blocks.next_if(holds_data) {
            end += block.len();
        }
        Some(start..end)
    })
}

async fn write_all(file: &Node, mut offset: u64, mut data: &[u8]) -> anyhow::Result<()> {
    while !data.is_empty() {
        let written = file.write(offset, data).await?;
        ensure!(written > 0, "a write wrote nothing");
        data = &data[written..];
        offset += written as u64;
    }
    Ok(())
}

/// Gives `node`, a new node, the times and extended attributes `metadata` says.
async fn set_metadata(node: &Node, metadata: &Metadata) -> anyhow::Result<()> {
    set_xattrs(node, &metadata.xattrs).await?;
    node.setattr(&metadata.times.set_attr()).await?;
    Ok(())
}

async fn set_xattrs(node: &Node, xattrs: &[(Vec<u8>, Vec<u8>)]) -> anyhow::Result<()> {
    for (name, value) in xattrs {
        node.setxattr(name, value, XattrMode::Set).await.with_context(|| {
            format!("setting the extended attribute {:?}", String::from_utf8_lossy(name))
        })?;
    }
    Ok(())
}

/// Removes `name` from directory `dir`, with everything in it, if it is a directory.
fn remove(dir: &Node, name: Vec<u8>) -> BoxFuture<'_, anyhow::Result<()>> {
    async move {
        let (node, attr) = dir.lookup(&name).await?;
        if attr.file_type != FileType::Directory {
            drop(node);
            return Ok(dir.unlink(&name).await?);
        }
        let emptied = async {
            for child in entries(&node).await? {
                remove(&node, child).await?;
            }
            anyhow::Ok(())
        }
        .await;
        drop(node);
        emptied?;
        Ok(dir.rmdir(&name).await?)
    }
    .boxed()
}

/// The names of the entries of directory `dir`.
async fn entries(dir: &Node) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(dir.entries().await?.into_iter().map(|entry| entry.name).collect())
}

/// What `entry` says of the node it makes: its header, and its PAX records, which take
/// precedence.
fn metadata(entry: &mut tar::Entry<'_, impl Read>) -> anyhow::Result<Metadata> {
    let header = entry.header();
    let perm = (header.mode()? & 0o7777) as u16;
    let mut uid = u32::try_from(header.uid()?).context("the owner's uid is too large")?;
    let mut gid = u32::try_from(header.gid()?).context("the owner's gid is too large")?;
    let mtime = UNIX_EPOCH + Duration::from_secs(header.mtime()?);
    let mut times = Times { atime: mtime, mtime };
    let mut atime = None;
    let mut xattrs = Vec::new();
    for record in entry.pax_extensions()?.into_iter().flatten() {
        let record = record?;
        let key = record.key()?;
        if let Some(name) = key.strip_prefix(PAX_XATTR) {
            xattrs.push((name.as_bytes().to_vec(), record.value_bytes().to_vec()));
            continue;
        }
        match key {
            "uid" => uid = record.value()?.parse().context("the PAX uid")?,
            "gid" => gid = record.value()?.parse().context("the PAX gid")?,
            "mtime" => times.mtime = pax_time(record.value()?)?,
            "atime" => atime = Some(pax_time(record.value()?)?),
            _ => {}
        }
    }
    times.atime = atime.unwrap_or(times.mtime);
    // Sorted, so that they are set in the same order, however the layer lists them.
    xattrs.sort();
    Ok(Metadata { perm, owner: Owner { uid, gid }, times, xattrs })
}

/// The time a PAX record gives, as seconds since the Unix epoch, with a fraction or not. Times
/// before the epoch are the epoch.
fn pax_time(value: &str) -> anyhow::Result<SystemTime> {
    let (secs, fraction) = value.split_once('.').unwrap_or((value, ""));
    let secs: i64 = secs.parse().with_context(|| format!("the PAX time {value:?}"))?;
    let Ok(secs) = u64::try_from(secs) else {
        return Ok(UNIX_EPOCH);
    };
    ensure!(fraction.bytes().all(|byte| byte.is_ascii_digit()), "the PAX time {value:?}");
    let nanos = format!("{:0<9}", &fraction[..fraction.len().min(9)]).parse::<u32>()?;
    Ok(UNIX_EPOCH + Duration::new(secs, nanos))
}

/// The device a device node's header names, as Linux encodes it for user space (`new_encode_dev`,
/// as glibc's `makedev`), which Fxfs keeps.
fn device(header: &tar::Header) -> anyhow::Result<u32> {
    let major = header.device_major()?.context("no device major number")?;
    let minor = header.device_minor()?.context("no device minor number")?;
    ensure!(major < 1 << 12 && minor < 1 << 20, "device {major}:{minor} is too large");
    Ok((minor & 0xff) | (major << 8) | ((minor & !0xff) << 12))
}

/// `path`, an entry's, relative to the root: without leading slashes, `.` or empty components.
/// Paths through `..` are refused, as images are not to name what is outside them.
fn normalize(path: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut normal = Vec::with_capacity(path.len());
    for component in path.split(|&byte| byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => bail!("the path {:?} goes through ..", String::from_utf8_lossy(path)),
            component => {
                if !normal.is_empty() {
                    normal.push(b'/');
                }
                normal.extend_from_slice(component);
            }
        }
    }
    Ok(normal)
}

/// `path`'s parent and last component, unless it is the root.
fn split(path: &[u8]) -> Option<(&[u8], &[u8])> {
    match path.iter().rposition(|&byte| byte == b'/') {
        _ if path.is_empty() => None,
        Some(slash) => Some((&path[..slash], &path[slash + 1..])),
        None => Some((&[], path)),
    }
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    if dir.is_empty() { name.to_vec() } else { [dir, b"/", name].concat() }
}

/// Reads from `reader` into `buf` until it is full or `reader` ends, and returns how much it read.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut len = 0;
    while len < buf.len() {
        match reader.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(read) => len += read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(len)
}

/// An image in an OCI image layout (the OCI image spec's `image-layout.md`): a directory with an
/// `index.json` naming its images' manifests, and the blobs they name, under `blobs/sha256`.
pub struct OciImage {
    layout: PathBuf,
    layers: Vec<Descriptor>,
}

/// A blob's descriptor: its digest, and, in an index, its annotations and platform.
#[derive(Deserialize)]
struct Descriptor {
    digest: String,
    #[serde(default)]
    annotations: HashMap<String, String>,
    platform: Option<Platform>,
}

#[derive(Deserialize)]
struct Platform {
    architecture: String,
    os: String,
}

/// An index of manifests, or a manifest, of layers.
#[derive(Deserialize)]
struct Blob {
    manifests: Option<Vec<Descriptor>>,
    layers: Option<Vec<Descriptor>>,
}

/// The annotations that name an image in an index: OCI's, the tag, and containerd's, which
/// `docker save` writes, the image's full name.
const REF_NAMES: [&str; 2] = ["org.opencontainers.image.ref.name", "io.containerd.image.name"];

impl OciImage {
    /// The image `reference`, which an annotation of its manifest in the layout's index names
    /// (`org.opencontainers.image.ref.name`, or `io.containerd.image.name`), or the layout's only
    /// image; for this host's architecture, if it is one of several platforms'.
    pub fn open(layout: impl AsRef<Path>, reference: Option<&str>) -> Result<Self, Error> {
        let layout = layout.as_ref();
        Self::open_layout(layout, reference)
            .with_context(|| format!("opening the OCI image layout {}", layout.display()))
            .map_err(Error::new)
    }

    fn open_layout(layout: &Path, reference: Option<&str>) -> anyhow::Result<Self> {
        let index = std::fs::read(layout.join("index.json")).context("reading index.json")?;
        let index: Blob = serde_json::from_slice(&index).context("parsing index.json")?;
        let manifests = index.manifests.context("index.json lists no manifests")?;
        let names = |descriptor: &Descriptor| -> Vec<String> {
            REF_NAMES.iter().filter_map(|key| descriptor.annotations.get(*key)).cloned().collect()
        };
        let mut matching: Vec<Descriptor> = match reference {
            Some(reference) => manifests
                .into_iter()
                .filter(|descriptor| names(descriptor).iter().any(|name| name == reference))
                .collect(),
            None => manifests,
        };
        let manifest = match (matching.len(), reference) {
            (1, _) => matching.remove(0),
            (0, Some(reference)) => bail!("no image is named {reference:?}"),
            (_, _) => {
                let named: Vec<Vec<String>> = matching.iter().map(names).collect();
                bail!("the layout holds several images, so one must be named: {named:?}")
            }
        };
        let mut layers = None;
        let mut descriptor = manifest;
        // An index of the image's manifests for several platforms, or the manifest.
        for _ in 0..8 {
            let blob: Blob = serde_json::from_slice(&read_blob(layout, &descriptor.digest)?)
                .with_context(|| format!("parsing {}", descriptor.digest))?;
            if let Some(found) = blob.layers {
                layers = Some(found);
                break;
            }
            let manifests = blob.manifests.context("a blob is neither a manifest nor an index")?;
            descriptor = platform(manifests)?;
        }
        let layers = layers.context("indexes nest too deeply")?;
        Ok(Self { layout: layout.to_owned(), layers })
    }

    /// Applies the image's layers to `importer`'s filesystem, checking each against its digest
    /// first.
    pub async fn import(&self, importer: &mut Importer) -> Result<(), Error> {
        for layer in &self.layers {
            let path = verified_blob(&self.layout, &layer.digest).map_err(Error::new)?;
            let file = std::fs::File::open(&path)
                .with_context(|| format!("opening {}", path.display()))
                .map_err(Error::new)?;
            importer.apply_layer(file).await.context(layer.digest.clone()).map_err(Error::new)?;
        }
        Ok(())
    }
}

/// The manifest of `manifests`, an index's, for this host's platform.
fn platform(manifests: Vec<Descriptor>) -> anyhow::Result<Descriptor> {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    manifests
        .into_iter()
        .find(|descriptor| {
            descriptor.platform.as_ref().is_some_and(|platform| {
                platform.os == "linux" && platform.architecture == architecture
            })
        })
        .with_context(|| format!("the image has no manifest for linux/{architecture}"))
}

/// The path of the blob `digest` names in `layout`. Only SHA-256 digests are supported.
fn blob_path(layout: &Path, digest: &str) -> anyhow::Result<PathBuf> {
    let hex = digest.strip_prefix("sha256:").with_context(|| format!("the digest {digest:?}"))?;
    ensure!(
        hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the digest {digest:?} is not a SHA-256 digest"
    );
    Ok(layout.join("blobs/sha256").join(hex))
}

/// The blob `digest` names in `layout`, checked against it.
fn read_blob(layout: &Path, digest: &str) -> anyhow::Result<Vec<u8>> {
    let path = blob_path(layout, digest)?;
    let blob = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    check_digest(digest, Sha256::digest(&blob).as_slice())?;
    Ok(blob)
}

/// The path of the blob `digest` names in `layout`, whose contents are checked against it.
fn verified_blob(layout: &Path, digest: &str) -> anyhow::Result<PathBuf> {
    let path = blob_path(layout, digest)?;
    let mut file =
        std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; CHUNK];
    loop {
        let len = read_full(&mut file, &mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if len == 0 {
            break;
        }
        hasher.update(&buf[..len]);
    }
    check_digest(digest, hasher.finalize().as_slice())?;
    Ok(path)
}

fn check_digest(digest: &str, sha256: &[u8]) -> anyhow::Result<()> {
    let actual: String = sha256.iter().map(|byte| format!("{byte:02x}")).collect();
    ensure!(digest == format!("sha256:{actual}"), "the blob {digest} holds sha256:{actual}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk of a file is written a run of data at a time: the blocks that hold data, together
    /// while they are consecutive, a short last one too, but not the blocks of zeros between.
    #[test]
    #[allow(clippy::single_range_in_vec_init, reason = "a chunk of data is one run")]
    fn contents_are_written_a_run_of_data_at_a_time() {
        let runs = |data: &[u8]| data_runs(data).collect::<Vec<_>>();
        assert_eq!(runs(&vec![0x5a; CHUNK]), [0..CHUNK]);
        assert_eq!(runs(&vec![0; CHUNK]), []);
        // Data in blocks 1 and 2, 5, and the short last one.
        let mut chunk = vec![0; 8 * HOLE + 100];
        for at in [HOLE, 3 * HOLE - 1, 5 * HOLE + 7, 8 * HOLE + 99] {
            chunk[at] = 1;
        }
        assert_eq!(runs(&chunk), [HOLE..3 * HOLE, 5 * HOLE..6 * HOLE, 8 * HOLE..8 * HOLE + 100]);
        // A short last block after one of data is in its run.
        chunk[7 * HOLE] = 1;
        assert_eq!(runs(&chunk), [HOLE..3 * HOLE, 5 * HOLE..6 * HOLE, 7 * HOLE..8 * HOLE + 100]);
    }

    /// An index's manifest for this host is the one for Linux on its architecture.
    #[test]
    fn manifests_are_linuxs_on_this_architecture() {
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        let descriptor = |digest: &str, os: &str, architecture: &str| Descriptor {
            digest: digest.to_owned(),
            annotations: HashMap::new(),
            platform: Some(Platform { architecture: architecture.to_owned(), os: os.to_owned() }),
        };
        let manifests = || {
            vec![
                Descriptor {
                    digest: "none".to_owned(),
                    annotations: HashMap::new(),
                    platform: None,
                },
                descriptor("other", "linux", "riscv64"),
                descriptor("windows", "windows", architecture),
                descriptor("this", "linux", architecture),
            ]
        };
        assert_eq!(platform(manifests()).unwrap().digest, "this");
        let mut others = manifests();
        others.pop();
        let error = platform(others).err().unwrap();
        assert_eq!(
            error.to_string(),
            format!("the image has no manifest for linux/{architecture}")
        );
    }
}

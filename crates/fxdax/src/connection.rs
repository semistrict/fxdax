//! fuchsia.io connections to a volume's nodes.

use crate::error::{Errno, IntoErrno, Result};
use crate::node::FileType;
use fidl::endpoints::{Proxy, create_proxy};
use fidl_fuchsia_io as fio;
use futures::StreamExt;
use std::sync::Arc;

/// The attributes a node's attributes are made from.
pub(crate) const ATTRIBUTES: fio::NodeAttributesQuery = fio::NodeAttributesQuery::MODE
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
pub(crate) const RIGHTS: fio::Flags = fio::PERM_READABLE.union(fio::PERM_WRITABLE);

/// A connection to a node.
#[derive(Clone)]
pub(crate) enum Connection {
    Directory(fio::DirectoryProxy),
    /// A file, and the stream its contents are read and written through.
    File(fio::FileProxy, Arc<zx::Stream>),
    Symlink(fio::SymlinkProxy),
}

impl Connection {
    /// The type of the node, unless its mode says otherwise.
    pub fn file_type(&self) -> FileType {
        match self {
            Self::Directory(_) => FileType::Directory,
            Self::File(..) => FileType::File,
            Self::Symlink(_) => FileType::Symlink,
        }
    }

    pub async fn get_attributes(&self) -> Result<fio::Attributes> {
        match self {
            Self::Directory(proxy) => proxy.get_attributes(ATTRIBUTES).await,
            Self::File(proxy, _) => proxy.get_attributes(ATTRIBUTES).await,
            Self::Symlink(proxy) => proxy.get_attributes(ATTRIBUTES).await,
        }
        .errno()?
        .errno()
    }

    pub async fn update_attributes(&self, attributes: &fio::MutableNodeAttributes) -> Result<()> {
        match self {
            Self::Directory(proxy) => proxy.update_attributes(attributes).await.errno()?,
            Self::File(proxy, _) => proxy.update_attributes(attributes).await.errno()?,
            // Symlinks' connections don't update attributes.
            Self::Symlink(_) => Err(zx::Status::NOT_SUPPORTED.into_raw()),
        }
        .errno()
    }

    pub fn directory(&self) -> Result<&fio::DirectoryProxy> {
        match self {
            Self::Directory(proxy) => Ok(proxy),
            _ => Err(Errno::ENOTDIR),
        }
    }

    pub fn file(&self) -> Result<(&fio::FileProxy, &Arc<zx::Stream>)> {
        match self {
            Self::File(proxy, stream) => Ok((proxy, stream)),
            Self::Directory(_) => Err(Errno::EISDIR),
            Self::Symlink(_) => Err(Errno::EINVAL),
        }
    }
}

/// Opens `name` in `parent` with `flags`, and returns a connection to the node and its
/// attributes, from the representation the open sends.
pub(crate) async fn open(
    parent: &fio::DirectoryProxy,
    name: &str,
    flags: fio::Flags,
    create_attributes: Option<fio::MutableNodeAttributes>,
) -> Result<(Connection, fio::Attributes)> {
    let (node, server_end) = create_proxy::<fio::NodeMarker>();
    parent
        .open(
            name,
            flags | RIGHTS | fio::Flags::FLAG_SEND_REPRESENTATION,
            &fio::Options { attributes: Some(ATTRIBUTES), create_attributes },
            server_end.into_channel(),
        )
        .errno()?;
    let representation = node
        .take_event_stream()
        .next()
        .await
        .ok_or(Errno::EIO)?
        .errno()?
        .into_on_representation()
        .ok_or(Errno::EIO)?;
    let channel = node.into_channel().unwrap();
    let (connection, attributes) = match representation {
        fio::Representation::Directory(info) => {
            (Connection::Directory(fio::DirectoryProxy::from_channel(channel)), info.attributes)
        }
        fio::Representation::File(info) => {
            let stream = info.stream.ok_or(Errno::EIO)?;
            (
                Connection::File(fio::FileProxy::from_channel(channel), Arc::new(stream)),
                info.attributes,
            )
        }
        fio::Representation::Symlink(info) => {
            (Connection::Symlink(fio::SymlinkProxy::from_channel(channel)), info.attributes)
        }
    };
    let attributes = attributes.ok_or(Errno::EIO)?;
    Ok((connection, (attributes.mutable_attributes, attributes.immutable_attributes)))
}

/// A token for `directory`, for renaming and linking into it.
pub(crate) async fn token(directory: &fio::DirectoryProxy) -> Result<zx::Event> {
    let (status, token) = directory.get_token().await.errno()?;
    zx::Status::ok(status).errno()?;
    Ok(zx::Event::from(token.ok_or(Errno::EIO)?))
}

/// The entries of `directory`: object ids, types and names.
pub(crate) async fn read_entries(
    directory: &fio::DirectoryProxy,
) -> Result<Vec<(u64, FileType, Vec<u8>)>> {
    zx::Status::ok(directory.rewind().await.errno()?).errno()?;
    let mut entries = Vec::new();
    loop {
        let (status, buf) = directory.read_dirents(fio::MAX_BUF).await.errno()?;
        zx::Status::ok(status).errno()?;
        if buf.is_empty() {
            return Ok(entries);
        }
        entries.extend(dirents(&buf)?);
    }
}

/// The entries of a `ReadDirents` buffer. Each is an inode number, a name length and a type, then
/// the name (`fuchsia.io/Directory.ReadDirents`).
fn dirents(mut buf: &[u8]) -> Result<Vec<(u64, FileType, Vec<u8>)>> {
    const HEADER_SIZE: usize = 10;
    let mut entries = Vec::new();
    while !buf.is_empty() {
        let (header, rest) = buf.split_at_checked(HEADER_SIZE).ok_or(Errno::EIO)?;
        let object_id = u64::from_le_bytes(header[..8].try_into().unwrap());
        let (name, rest) = rest.split_at_checked(header[8] as usize).ok_or(Errno::EIO)?;
        let file_type = match fio::DirentType::from_primitive(header[9]) {
            Some(fio::DirentType::Directory) => FileType::Directory,
            Some(fio::DirentType::Symlink) => FileType::Symlink,
            _ => FileType::File,
        };
        entries.push((object_id, file_type, name.to_vec()));
        buf = rest;
    }
    Ok(entries)
}

/// `name` as fuchsia.io takes it.
pub(crate) fn name(name: &[u8]) -> Result<String> {
    Ok(std::str::from_utf8(name).map_err(|_| Errno::EINVAL)?.to_owned())
}

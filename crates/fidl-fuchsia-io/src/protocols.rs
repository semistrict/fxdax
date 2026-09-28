//! The `fuchsia.io/File`, `fuchsia.io/Directory` and `fuchsia.io/Symlink` protocols, served
//! in-process.
//!
//! A proxy's methods have the generated bindings' signatures: the outer `Result` is the FIDL
//! call's, the inner one the method's own error, a raw `zx.Status`. A one-way call, such as
//! `Directory.Open`, is delivered once the connection is served, after those made before it, as a
//! message would be read from a channel. The server traits have a method for each protocol method
//! fxdax's tests call.
//!
//! A synchronous proxy blocks on its server's futures on the executor that created it, and ignores
//! deadlines. `fuchsia.unknown/Cloneable` and `Queryable`, which the protocols compose, are here
//! rather than in a crate of their own.

use crate::{
    AllocateMode, Flags, ImmutableNodeAttributes, MutableNodeAttributes, NodeAttributesQuery,
    Options, Representation, SeekOrigin, SymlinkInfo, UnlinkOptions,
};
use fidl::Channel;
use fidl::endpoints::{Connection, ProtocolMarker, ServerEnd};
use futures::Stream;
use futures::future::BoxFuture;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A `fuchsia.io` method's result.
pub type FileResult<T> = Result<T, i32>;

/// The mutable and immutable attributes `GetAttributes` returns.
pub type Attributes = (MutableNodeAttributes, ImmutableNodeAttributes);

/// `fuchsia.io/Directory`'s name, which `Query` returns.
pub const DIRECTORY_PROTOCOL_NAME: &str = "fuchsia.io/Directory";
/// `fuchsia.io/File`'s name, which `Query` returns.
pub const FILE_PROTOCOL_NAME: &str = "fuchsia.io/File";
/// `fuchsia.io/Symlink`'s name, which `Query` returns.
pub const SYMLINK_PROTOCOL_NAME: &str = "fuchsia.io/Symlink";

/// An event the `fuchsia.io` protocols' servers send: `Node.OnRepresentation`, for opens with
/// `Flags.FLAG_SEND_REPRESENTATION`.
#[derive(Debug)]
pub enum NodeEvent {
    OnRepresentation { payload: Representation },
}

impl NodeEvent {
    pub fn into_on_representation(self) -> Option<Representation> {
        let NodeEvent::OnRepresentation { payload } = self;
        Some(payload)
    }
}

/// The events of `fuchsia.io/File`.
pub type FileEvent = NodeEvent;
/// The events of `fuchsia.io/Directory`.
pub type DirectoryEvent = NodeEvent;
/// The events of `fuchsia.io/Symlink`.
pub type SymlinkEvent = NodeEvent;

/// A stream of the events a proxy's server sends, as `take_event_stream` returns.
pub struct EventStream<M: ProtocolMarker> {
    connection: Connection<M>,
    closed: bool,
}

// Nothing in the stream is pinned.
impl<M: ProtocolMarker> Unpin for EventStream<M> {}

impl<M: ProtocolMarker> Stream for EventStream<M> {
    type Item = Result<NodeEvent, fidl::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.closed {
            return Poll::Ready(None);
        }
        this.connection.poll_event::<NodeEvent>(cx).map(|event| {
            this.closed = event.is_err();
            Some(event)
        })
    }
}

/// Serves `fuchsia.io/File`.
pub trait FileServer: Send + Sync {
    fn read(&self, count: u64) -> BoxFuture<'_, FileResult<Vec<u8>>>;
    fn read_at(&self, count: u64, offset: u64) -> BoxFuture<'_, FileResult<Vec<u8>>>;
    fn write<'a>(&'a self, data: &'a [u8]) -> BoxFuture<'a, FileResult<u64>>;
    fn write_at<'a>(&'a self, data: &'a [u8], offset: u64) -> BoxFuture<'a, FileResult<u64>>;
    fn seek(&self, origin: SeekOrigin, offset: i64) -> BoxFuture<'_, FileResult<u64>>;
    fn resize(&self, length: u64) -> BoxFuture<'_, FileResult<()>>;
    fn allocate(
        &self,
        offset: u64,
        length: u64,
        mode: AllocateMode,
    ) -> BoxFuture<'_, FileResult<()>>;
    fn sync(&self) -> BoxFuture<'_, FileResult<()>>;
    fn close(&self) -> BoxFuture<'_, FileResult<()>>;
    fn get_attributes(&self, query: NodeAttributesQuery) -> BoxFuture<'_, FileResult<Attributes>>;
    fn update_attributes<'a>(
        &'a self,
        attributes: &'a MutableNodeAttributes,
    ) -> BoxFuture<'a, FileResult<()>>;
    /// Describes the file, or closes the connection with an epitaph if that fails.
    fn describe(&self) -> BoxFuture<'_, Result<crate::FileInfo, zx::Status>>;

    /// Serves a new connection to the same file on `request`, as `Clone` does.
    fn clone(&self, request: Channel);
}

/// Serves `fuchsia.io/Directory`.
pub trait DirectoryServer: Send + Sync {
    /// Opens the object at `path`, serving it on `object`, or closing `object` with an epitaph if
    /// that fails.
    fn open(&self, path: &str, flags: Flags, options: &Options, object: Channel);
    fn unlink<'a>(
        &'a self,
        name: &'a str,
        options: &'a UnlinkOptions,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn get_token(&self) -> BoxFuture<'_, (i32, Option<zx::NullableHandle>)>;
    fn rename<'a>(
        &'a self,
        src: &'a str,
        dst_parent_token: zx::Event,
        dst: &'a str,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn link<'a>(
        &'a self,
        src: &'a str,
        dst_parent_token: zx::NullableHandle,
        dst: &'a str,
    ) -> BoxFuture<'a, i32>;
    fn read_dirents(&self, max_bytes: u64) -> BoxFuture<'_, (i32, Vec<u8>)>;
    fn rewind(&self) -> BoxFuture<'_, i32>;
    fn create_symlink<'a>(
        &'a self,
        name: &'a str,
        target: &'a [u8],
        connection: Option<ServerEnd<SymlinkMarker>>,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn get_attributes(&self, query: NodeAttributesQuery) -> BoxFuture<'_, FileResult<Attributes>>;
    fn update_attributes<'a>(
        &'a self,
        attributes: &'a MutableNodeAttributes,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn sync(&self) -> BoxFuture<'_, FileResult<()>>;
    fn close(&self) -> BoxFuture<'_, FileResult<()>>;

    /// Serves a new connection to the same directory on `request`, as `Clone` does.
    fn clone(&self, request: Channel);
}

/// Serves `fuchsia.io/Symlink`.
pub trait SymlinkServer: Send + Sync {
    /// Describes the symlink, or closes the connection with an epitaph if that fails.
    fn describe(&self) -> BoxFuture<'_, Result<SymlinkInfo, zx::Status>>;
    fn link_into<'a>(
        &'a self,
        dst_parent_token: zx::Event,
        dst: &'a str,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn get_attributes(&self, query: NodeAttributesQuery) -> BoxFuture<'_, FileResult<Attributes>>;
    fn close(&self) -> BoxFuture<'_, FileResult<()>>;

    /// Serves a new connection to the same symlink on `request`, as `Clone` does.
    fn clone(&self, request: Channel);
}

/// The `fuchsia.io/File` protocol.
pub struct FileMarker;

impl ProtocolMarker for FileMarker {
    type Proxy = FileProxy;
    type SynchronousProxy = FileSynchronousProxy;
    type Server = dyn FileServer;
    const DEBUG_NAME: &'static str = "fuchsia.io.File";
}

/// A client of `fuchsia.io/File`.
#[derive(Clone, Debug)]
pub struct FileProxy(Connection<FileMarker>);

impl From<Connection<FileMarker>> for FileProxy {
    fn from(connection: Connection<FileMarker>) -> Self {
        Self(connection)
    }
}

impl FileProxy {
    /// The events the server sends.
    pub fn take_event_stream(&self) -> EventStream<FileMarker> {
        EventStream { connection: self.0.clone(), closed: false }
    }

    pub async fn read(&self, count: u64) -> Result<FileResult<Vec<u8>>, fidl::Error> {
        Ok(self.0.server().await?.read(count).await)
    }

    pub async fn read_at(
        &self,
        count: u64,
        offset: u64,
    ) -> Result<FileResult<Vec<u8>>, fidl::Error> {
        Ok(self.0.server().await?.read_at(count, offset).await)
    }

    pub async fn write(&self, data: &[u8]) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server().await?.write(data).await)
    }

    pub async fn write_at(&self, data: &[u8], offset: u64) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server().await?.write_at(data, offset).await)
    }

    pub async fn seek(
        &self,
        origin: SeekOrigin,
        offset: i64,
    ) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server().await?.seek(origin, offset).await)
    }

    pub async fn resize(&self, length: u64) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.resize(length).await)
    }

    pub async fn allocate(
        &self,
        offset: u64,
        length: u64,
        mode: AllocateMode,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.allocate(offset, length, mode).await)
    }

    pub async fn sync(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.sync().await)
    }

    pub async fn close(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.close().await)
    }

    pub async fn get_attributes(
        &self,
        query: NodeAttributesQuery,
    ) -> Result<FileResult<Attributes>, fidl::Error> {
        Ok(self.0.server().await?.get_attributes(query).await)
    }

    pub async fn update_attributes(
        &self,
        attributes: &MutableNodeAttributes,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.update_attributes(attributes).await)
    }

    pub async fn describe(&self) -> Result<crate::FileInfo, fidl::Error> {
        self.0.server().await?.describe().await.map_err(|status| fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::Explicit(Err(status)),
            protocol_name: FileMarker::DEBUG_NAME,
        })
    }

    pub async fn query(&self) -> Result<Vec<u8>, fidl::Error> {
        self.0.server().await?;
        Ok(FILE_PROTOCOL_NAME.as_bytes().to_vec())
    }

    pub fn clone(&self, request: ServerEnd<CloneableMarker>) -> Result<(), fidl::Error> {
        let request = request.into_channel();
        self.0.send(move |server| FileServer::clone(&*server, request));
        Ok(())
    }
}

/// A synchronous client of `fuchsia.io/File`.
pub struct FileSynchronousProxy {
    connection: Connection<FileMarker>,
    executor: tokio::runtime::Handle,
}

impl From<Connection<FileMarker>> for FileSynchronousProxy {
    fn from(connection: Connection<FileMarker>) -> Self {
        Self { connection, executor: tokio::runtime::Handle::current() }
    }
}

impl FileSynchronousProxy {
    pub fn sync(&self, _deadline: zx::MonotonicInstant) -> Result<FileResult<()>, fidl::Error> {
        self.executor.block_on(async { Ok(self.connection.server().await?.sync().await) })
    }
}

/// The `fuchsia.unknown/Cloneable` protocol, which the `fuchsia.io` protocols compose. A request
/// to clone is served with the protocol of the connection it clones.
pub struct CloneableMarker;

impl ProtocolMarker for CloneableMarker {
    type Proxy = CloneableProxy;
    type SynchronousProxy = CloneableProxy;
    type Server = dyn std::any::Any + Send + Sync;
    const DEBUG_NAME: &'static str = "fuchsia.unknown.Cloneable";
}

/// A client of `fuchsia.unknown/Cloneable`, which makes no calls: connections are cloned through
/// the protocols composing it.
pub struct CloneableProxy;

impl From<Connection<CloneableMarker>> for CloneableProxy {
    fn from(_connection: Connection<CloneableMarker>) -> Self {
        Self
    }
}

/// The `fuchsia.io/Directory` protocol.
pub struct DirectoryMarker;

impl ProtocolMarker for DirectoryMarker {
    type Proxy = DirectoryProxy;
    type SynchronousProxy = DirectoryProxy;
    type Server = dyn DirectoryServer;
    const DEBUG_NAME: &'static str = "fuchsia.io.Directory";
}

/// A client of `fuchsia.io/Directory`.
#[derive(Clone, Debug)]
pub struct DirectoryProxy(Connection<DirectoryMarker>);

impl From<Connection<DirectoryMarker>> for DirectoryProxy {
    fn from(connection: Connection<DirectoryMarker>) -> Self {
        Self(connection)
    }
}

impl DirectoryProxy {
    /// The events the server sends.
    pub fn take_event_stream(&self) -> EventStream<DirectoryMarker> {
        EventStream { connection: self.0.clone(), closed: false }
    }

    pub fn open(
        &self,
        path: &str,
        flags: Flags,
        options: &Options,
        object: Channel,
    ) -> Result<(), fidl::Error> {
        let (path, options) = (path.to_owned(), options.clone());
        self.0.send(move |server| server.open(&path, flags, &options, object));
        Ok(())
    }

    pub async fn unlink(
        &self,
        name: &str,
        options: &UnlinkOptions,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.unlink(name, options).await)
    }

    pub async fn get_token(&self) -> Result<(i32, Option<zx::NullableHandle>), fidl::Error> {
        Ok(self.0.server().await?.get_token().await)
    }

    pub async fn rename(
        &self,
        src: &str,
        dst_parent_token: zx::Event,
        dst: &str,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.rename(src, dst_parent_token, dst).await)
    }

    pub async fn link(
        &self,
        src: &str,
        dst_parent_token: zx::NullableHandle,
        dst: &str,
    ) -> Result<i32, fidl::Error> {
        Ok(self.0.server().await?.link(src, dst_parent_token, dst).await)
    }

    pub async fn read_dirents(&self, max_bytes: u64) -> Result<(i32, Vec<u8>), fidl::Error> {
        Ok(self.0.server().await?.read_dirents(max_bytes).await)
    }

    pub async fn rewind(&self) -> Result<i32, fidl::Error> {
        Ok(self.0.server().await?.rewind().await)
    }

    pub async fn create_symlink(
        &self,
        name: &str,
        target: &[u8],
        connection: Option<ServerEnd<SymlinkMarker>>,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.create_symlink(name, target, connection).await)
    }

    pub async fn get_attributes(
        &self,
        query: NodeAttributesQuery,
    ) -> Result<FileResult<Attributes>, fidl::Error> {
        Ok(self.0.server().await?.get_attributes(query).await)
    }

    pub async fn update_attributes(
        &self,
        attributes: &MutableNodeAttributes,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.update_attributes(attributes).await)
    }

    pub async fn sync(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.sync().await)
    }

    pub async fn close(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.close().await)
    }

    pub async fn query(&self) -> Result<Vec<u8>, fidl::Error> {
        self.0.server().await?;
        Ok(DIRECTORY_PROTOCOL_NAME.as_bytes().to_vec())
    }

    pub fn clone(&self, request: ServerEnd<CloneableMarker>) -> Result<(), fidl::Error> {
        let request = request.into_channel();
        self.0.send(move |server| DirectoryServer::clone(&*server, request));
        Ok(())
    }
}

/// The `fuchsia.io/Symlink` protocol.
pub struct SymlinkMarker;

impl ProtocolMarker for SymlinkMarker {
    type Proxy = SymlinkProxy;
    type SynchronousProxy = SymlinkProxy;
    type Server = dyn SymlinkServer;
    const DEBUG_NAME: &'static str = "fuchsia.io.Symlink";
}

/// A client of `fuchsia.io/Symlink`.
#[derive(Clone, Debug)]
pub struct SymlinkProxy(Connection<SymlinkMarker>);

impl From<Connection<SymlinkMarker>> for SymlinkProxy {
    fn from(connection: Connection<SymlinkMarker>) -> Self {
        Self(connection)
    }
}

impl SymlinkProxy {
    /// The events the server sends.
    pub fn take_event_stream(&self) -> EventStream<SymlinkMarker> {
        EventStream { connection: self.0.clone(), closed: false }
    }

    pub async fn describe(&self) -> Result<SymlinkInfo, fidl::Error> {
        self.0.server().await?.describe().await.map_err(|status| fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::Explicit(Err(status)),
            protocol_name: SymlinkMarker::DEBUG_NAME,
        })
    }

    pub async fn link_into(
        &self,
        dst_parent_token: zx::Event,
        dst: &str,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.link_into(dst_parent_token, dst).await)
    }

    pub async fn get_attributes(
        &self,
        query: NodeAttributesQuery,
    ) -> Result<FileResult<Attributes>, fidl::Error> {
        Ok(self.0.server().await?.get_attributes(query).await)
    }

    pub async fn close(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server().await?.close().await)
    }

    pub async fn query(&self) -> Result<Vec<u8>, fidl::Error> {
        self.0.server().await?;
        Ok(SYMLINK_PROTOCOL_NAME.as_bytes().to_vec())
    }

    pub fn clone(&self, request: ServerEnd<CloneableMarker>) -> Result<(), fidl::Error> {
        let request = request.into_channel();
        self.0.send(move |server| SymlinkServer::clone(&*server, request));
        Ok(())
    }
}

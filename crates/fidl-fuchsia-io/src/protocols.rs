//! The `fuchsia.io/File` and `fuchsia.io/Directory` protocols, served in-process.
//!
//! A proxy's methods have the generated bindings' signatures: the outer `Result` is the FIDL
//! call's, the inner one the method's own error, a raw `zx.Status`. The server traits have a method
//! for each protocol method fxdax's tests call.
//!
//! A synchronous proxy blocks on its server's futures on the executor that created it, and ignores
//! deadlines. `fuchsia.unknown/Cloneable`, which `File` composes, is here rather than in a crate of
//! its own.

use crate::{
    AllocateMode, Flags, ImmutableNodeAttributes, MutableNodeAttributes, NodeAttributesQuery,
    Options, SeekOrigin,
};
use fidl::endpoints::{Connection, ProtocolMarker, ServerEnd};
use futures::future::BoxFuture;

/// A `fuchsia.io/File` method's result.
pub type FileResult<T> = Result<T, i32>;

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
    fn get_attributes(
        &self,
        query: NodeAttributesQuery,
    ) -> BoxFuture<'_, FileResult<(MutableNodeAttributes, ImmutableNodeAttributes)>>;
    fn update_attributes<'a>(
        &'a self,
        attributes: &'a MutableNodeAttributes,
    ) -> BoxFuture<'a, FileResult<()>>;
    fn describe(&self) -> BoxFuture<'_, crate::FileInfo>;

    /// A new connection to the same file, as `Clone` serves.
    fn clone_connection(&self) -> Result<std::sync::Arc<dyn FileServer>, zx::Status>;
}

/// Serves `fuchsia.io/Directory`.
pub trait DirectoryServer: Send + Sync {
    /// Opens the object at `path`, as `Directory.Open` does, serving it on a new connection.
    fn open<'a>(
        &'a self,
        path: &'a str,
        flags: Flags,
        options: &'a Options,
    ) -> BoxFuture<'a, Result<FileProxy, zx::Status>>;
    fn close(&self) -> BoxFuture<'_, FileResult<()>>;
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
#[derive(Clone)]
pub struct FileProxy(Connection<FileMarker>);

impl From<Connection<FileMarker>> for FileProxy {
    fn from(connection: Connection<FileMarker>) -> Self {
        Self(connection)
    }
}

impl FileProxy {
    pub async fn read(&self, count: u64) -> Result<FileResult<Vec<u8>>, fidl::Error> {
        Ok(self.0.server()?.read(count).await)
    }

    pub async fn read_at(
        &self,
        count: u64,
        offset: u64,
    ) -> Result<FileResult<Vec<u8>>, fidl::Error> {
        Ok(self.0.server()?.read_at(count, offset).await)
    }

    pub async fn write(&self, data: &[u8]) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server()?.write(data).await)
    }

    pub async fn write_at(&self, data: &[u8], offset: u64) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server()?.write_at(data, offset).await)
    }

    pub async fn seek(
        &self,
        origin: SeekOrigin,
        offset: i64,
    ) -> Result<FileResult<u64>, fidl::Error> {
        Ok(self.0.server()?.seek(origin, offset).await)
    }

    pub async fn resize(&self, length: u64) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.resize(length).await)
    }

    pub async fn allocate(
        &self,
        offset: u64,
        length: u64,
        mode: AllocateMode,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.allocate(offset, length, mode).await)
    }

    pub async fn sync(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.sync().await)
    }

    pub async fn close(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.close().await)
    }

    pub async fn get_attributes(
        &self,
        query: NodeAttributesQuery,
    ) -> Result<FileResult<(MutableNodeAttributes, ImmutableNodeAttributes)>, fidl::Error> {
        Ok(self.0.server()?.get_attributes(query).await)
    }

    pub async fn update_attributes(
        &self,
        attributes: &MutableNodeAttributes,
    ) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.update_attributes(attributes).await)
    }

    pub async fn describe(&self) -> Result<crate::FileInfo, fidl::Error> {
        Ok(self.0.server()?.describe().await)
    }

    /// Serves a new connection to the file on `request`, as `fuchsia.unknown/Cloneable.Clone`
    /// does. A connection that can't be cloned closes `request`.
    pub fn clone(&self, request: ServerEnd<CloneableMarker>) -> Result<(), fidl::Error> {
        if let Ok(connection) = self.0.server()?.clone_connection() {
            request.serve(connection);
        }
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
        let server = self.connection.server()?;
        Ok(self.executor.block_on(server.sync()))
    }
}

/// The `fuchsia.unknown/Cloneable` protocol, which `fuchsia.io/File` composes, so that its server
/// is a `FileServer`.
pub struct CloneableMarker;

impl ProtocolMarker for CloneableMarker {
    type Proxy = CloneableProxy;
    type SynchronousProxy = CloneableProxy;
    type Server = dyn FileServer;
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
#[derive(Clone)]
pub struct DirectoryProxy(Connection<DirectoryMarker>);

impl From<Connection<DirectoryMarker>> for DirectoryProxy {
    fn from(connection: Connection<DirectoryMarker>) -> Self {
        Self(connection)
    }
}

impl DirectoryProxy {
    /// Opens a file, as `Directory.Open` with a new `File` connection does, returning the error an
    /// epitaph would carry.
    pub async fn open_file(
        &self,
        path: &str,
        flags: Flags,
        options: &Options,
    ) -> Result<FileProxy, zx::Status> {
        let server = self.0.server().map_err(|_| zx::Status::PEER_CLOSED)?;
        server.open(path, flags, options).await
    }

    pub async fn close(&self) -> Result<FileResult<()>, fidl::Error> {
        Ok(self.0.server()?.close().await)
    }
}

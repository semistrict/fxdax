// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/lib/vfs/rust/src/file/connection.rs`: `StreamIoConnection`, with the
// request handling of `FileConnection` for the methods `fio::FileServer` has, serving
// `fuchsia.io/File` to in-process proxies (see the `fidl` stand-in) rather than over a channel.
// `new_connection_validate_options` is from `src/storage/lib/vfs/rust/src/file/common.rs`.
//
// A connection is a task in its scope, as upstream's request listener is, which holds the file
// open until the connection is closed, all its proxies are dropped (when, as when upstream's
// request stream closes, the file is synced first), or the scope shuts down.
//
// Deviation: a request after `Close`, or after the connection's scope has shut down, fails with
// `ZX_ERR_PEER_CLOSED`, which a closed channel would give the client.

use crate::execution_scope::ExecutionScope;
use crate::file::{File, FileOptions, SyncMode};
use crate::node::OpenNode;
use crate::protocols::ProtocolsExt;
use crate::temp_clone::{TempClonable, TempClone, unblock};
use fidl::endpoints::Connection;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use std::io::SeekFrom;
use std::sync::Arc;
use zx_status::Status;

pub trait GetVmo {
    /// True if the vmo is pager backed and the pager is serviced by the same executor as the
    /// `StreamIoConnection`.
    ///
    /// When true, stream operations that touch the contents of the vmo will be run on a
    /// separate thread pool to avoid deadlocks.
    const PAGER_ON_FIDL_EXECUTOR: bool = false;

    /// Returns the underlying VMO for the node.
    fn get_vmo(&self) -> &zx::Vmo;
}

/// What an open connection holds.
struct Open<T: 'static + File + GetVmo> {
    /// File that requests will be forwarded to.
    file: OpenNode<T>,

    /// The stream backing the connection that all read, write, and seek calls are forwarded to.
    stream: TempClonable<zx::Stream>,
}

/// Closes a connection when dropped, as the connection's task is when it finishes or its scope
/// shuts down.
struct Closer<T: 'static + File + GetVmo>(Arc<Mutex<Option<Open<T>>>>);

impl<T: 'static + File + GetVmo> Drop for Closer<T> {
    fn drop(&mut self) {
        // Close the file outside the lock.
        let open = self.0.lock().take();
        std::mem::drop(open);
    }
}

/// Wrapper around a file that forwards `File` requests to `file` and
/// `FileIo` requests to `stream`.
pub struct StreamIoConnection<T: 'static + File + GetVmo> {
    /// Execution scope this connection and any async operations and connections it creates will
    /// use.
    scope: ExecutionScope,

    /// The file and stream, until the connection is closed.
    open: Arc<Mutex<Option<Open<T>>>>,

    /// Ends the connection's task when dropped with the connection.
    _task_end: oneshot::Sender<()>,

    /// Options for this connection.
    options: FileOptions,
}

impl<T: 'static + File + GetVmo> StreamIoConnection<T> {
    /// Creates a stream-based file connection, returning a proxy for it. `file` must have been
    /// opened for the connection, which closes it. As `create_connection` does, the file is opened
    /// with `open_file`, and truncated if `protocols` ask for that.
    pub async fn create(
        scope: ExecutionScope,
        file: Arc<T>,
        protocols: impl ProtocolsExt,
    ) -> Result<fio::FileProxy, Status> {
        let file = OpenNode::new(file);
        let options = protocols.to_file_options()?;
        let stream =
            TempClonable::new(zx::Stream::create(options.to_stream_options(), file.get_vmo(), 0)?);

        new_connection_validate_options(
            &options,
            file.readable(),
            file.writable(),
            file.executable(),
        )?;

        file.open_file(&options).await?;
        if protocols.is_truncate() {
            file.truncate(0).await?;
        }

        let connection: Arc<dyn fio::FileServer> =
            Arc::new(Self::new(scope, Open { file, stream }, options));
        Ok(Connection::<fio::FileMarker>::served_by(connection).into())
    }

    /// A connection to the open file, whose task is spawned in `scope`.
    fn new(scope: ExecutionScope, open: Open<T>, options: FileOptions) -> Self {
        let open = Arc::new(Mutex::new(Some(open)));
        let (task_end, connection_dropped) = oneshot::channel::<()>();
        let closer = Closer(open.clone());
        let connection = Self { scope: scope.clone(), open, _task_end: task_end, options };
        let should_sync_before_close = connection.should_sync_before_close();
        scope.spawn(async move {
            let closer = closer;
            // The connection was dropped: sync the file unless it was closed, as `stream_closed`
            // does.
            let _ = connection_dropped.await;
            let file = closer.0.lock().as_ref().map(|open| (*open.file).clone());
            if let Some(file) = file
                && should_sync_before_close
            {
                let _ = file.sync(SyncMode::PreClose).await;
            }
        });
        connection
    }

    /// The file and a clone of the stream, if the connection is open.
    fn file_and_stream(&self) -> Result<(Arc<T>, TempClone<zx::Stream>), Status> {
        let open = self.open.lock();
        let open = open.as_ref().ok_or(Status::PEER_CLOSED)?;
        Ok(((*open.file).clone(), open.stream.temp_clone()))
    }

    fn file(&self) -> Result<Arc<T>, Status> {
        Ok(self.file_and_stream()?.0)
    }

    async fn maybe_unblock<F, R>(&self, f: F) -> Result<R, Status>
    where
        F: FnOnce(&zx::Stream) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (_file, stream) = self.file_and_stream()?;
        Ok(if T::PAGER_ON_FIDL_EXECUTOR { unblock(move || f(&*stream)).await } else { f(&*stream) })
    }

    /// Runs `request` if the connection's scope hasn't shut down.
    async fn serve<R>(
        &self,
        request: impl Future<Output = Result<R, Status>>,
    ) -> fio::FileResult<R> {
        let Some(_guard) = self.scope.try_active_guard() else {
            return Err(Status::PEER_CLOSED.into_raw());
        };
        request.await.map_err(Status::into_raw)
    }

    async fn handle_read(&self, count: u64) -> Result<Vec<u8>, Status> {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            return Err(Status::BAD_HANDLE);
        }

        if count > fio::MAX_TRANSFER_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
        self.maybe_unblock(move |stream| {
            stream.read_to_vec(zx::StreamReadOptions::empty(), count as usize)
        })
        .await?
    }

    async fn handle_read_at(&self, offset: u64, count: u64) -> Result<Vec<u8>, Status> {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        if count > fio::MAX_TRANSFER_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
        self.maybe_unblock(move |stream| {
            stream.read_at_to_vec(zx::StreamReadOptions::empty(), offset, count as usize)
        })
        .await?
    }

    async fn handle_write(&self, data: Vec<u8>) -> Result<u64, Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.maybe_unblock(move |stream| {
            let actual = stream.write(zx::StreamWriteOptions::empty(), &data)?;
            Ok(actual as u64)
        })
        .await?
    }

    async fn handle_write_at(&self, offset: u64, data: Vec<u8>) -> Result<u64, Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.maybe_unblock(move |stream| {
            let actual = stream.write_at(zx::StreamWriteOptions::empty(), offset, &data)?;
            Ok(actual as u64)
        })
        .await?
    }

    /// Move seek position to byte `offset` relative to the origin specified by `start`.
    async fn handle_seek(&self, offset: i64, origin: fio::SeekOrigin) -> Result<u64, Status> {
        let position = match origin {
            fio::SeekOrigin::Start => {
                if offset < 0 {
                    return Err(Status::INVALID_ARGS);
                }
                SeekFrom::Start(offset as u64)
            }
            fio::SeekOrigin::Current => SeekFrom::Current(offset),
            fio::SeekOrigin::End => SeekFrom::End(offset),
        };
        self.file_and_stream()?.1.seek(position)
    }

    async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.file()?.get_attributes(query).await
    }

    async fn handle_update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::UPDATE_ATTRIBUTES) {
            return Err(Status::BAD_HANDLE);
        }

        self.file()?.update_attributes(attributes).await
    }

    async fn handle_truncate(&self, length: u64) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }

        self.file()?.truncate(length).await
    }

    async fn handle_allocate(
        &self,
        offset: u64,
        length: u64,
        mode: fio::AllocateMode,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.file()?.allocate(offset, length, mode).await
    }

    async fn handle_sync(&self) -> Result<(), Status> {
        self.file()?.sync(SyncMode::Normal).await
    }

    fn should_sync_before_close(&self) -> bool {
        self.options
            .rights
            .intersects(fio::Operations::WRITE_BYTES | fio::Operations::UPDATE_ATTRIBUTES)
    }
}

impl<T: 'static + File + GetVmo> fio::FileServer for StreamIoConnection<T> {
    fn read(&self, count: u64) -> BoxFuture<'_, fio::FileResult<Vec<u8>>> {
        self.serve(self.handle_read(count)).boxed()
    }

    fn read_at(&self, count: u64, offset: u64) -> BoxFuture<'_, fio::FileResult<Vec<u8>>> {
        self.serve(self.handle_read_at(offset, count)).boxed()
    }

    fn write<'a>(&'a self, data: &'a [u8]) -> BoxFuture<'a, fio::FileResult<u64>> {
        self.serve(self.handle_write(data.to_vec())).boxed()
    }

    fn write_at<'a>(&'a self, data: &'a [u8], offset: u64) -> BoxFuture<'a, fio::FileResult<u64>> {
        self.serve(self.handle_write_at(offset, data.to_vec())).boxed()
    }

    fn seek(&self, origin: fio::SeekOrigin, offset: i64) -> BoxFuture<'_, fio::FileResult<u64>> {
        self.serve(self.handle_seek(offset, origin)).boxed()
    }

    fn resize(&self, length: u64) -> BoxFuture<'_, fio::FileResult<()>> {
        self.serve(self.handle_truncate(length)).boxed()
    }

    fn allocate(
        &self,
        offset: u64,
        length: u64,
        mode: fio::AllocateMode,
    ) -> BoxFuture<'_, fio::FileResult<()>> {
        self.serve(self.handle_allocate(offset, length, mode)).boxed()
    }

    fn sync(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.serve(self.handle_sync()).boxed()
    }

    fn close(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.serve(async move {
            let result = if self.should_sync_before_close() {
                self.file()?.sync(SyncMode::PreClose).await
            } else {
                self.file().map(|_| ())
            };
            // The file gets closed when we drop the connection, so we should do that before
            // sending the response.
            std::mem::drop(self.open.lock().take());
            result
        })
        .boxed()
    }

    fn get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> BoxFuture<'_, fio::FileResult<(fio::MutableNodeAttributes, fio::ImmutableNodeAttributes)>>
    {
        self.serve(async move {
            let attrs = self.handle_get_attributes(query).await?;
            Ok((attrs.mutable_attributes, attrs.immutable_attributes))
        })
        .boxed()
    }

    fn update_attributes<'a>(
        &'a self,
        attributes: &'a fio::MutableNodeAttributes,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        self.serve(self.handle_update_attributes(attributes.clone())).boxed()
    }

    fn describe(&self) -> BoxFuture<'_, fio::FileInfo> {
        async move {
            let stream = self
                .file_and_stream()
                .and_then(|(_file, stream)| stream.duplicate_handle(zx::Rights::SAME_RIGHTS))
                .ok();
            fio::FileInfo { stream, ..Default::default() }
        }
        .boxed()
    }

    fn clone_connection(&self) -> Result<Arc<dyn fio::FileServer>, Status> {
        let file = self.file()?;
        let stream = TempClonable::new(zx::Stream::create(
            self.options.to_stream_options(),
            file.get_vmo(),
            0,
        )?);
        file.will_clone();
        let open = Open { file: OpenNode::new(file), stream };
        Ok(Arc::new(Self::new(self.scope.clone(), open, self.options)))
    }
}

/// Validate that the requested flags for a new connection are valid.  It is a bit tricky as
/// depending on the presence of the `OPEN_FLAG_NODE_REFERENCE` flag we are effectively validating
/// two different cases: with `OPEN_FLAG_NODE_REFERENCE` the connection will be attached to the
/// file node itself, and without `OPEN_FLAG_NODE_REFERENCE` the connection will be attached to the
/// "contents" of the file, and will use the file handling, only allowing certain operations.
///
/// Changing this function can be dangerous! There are security implications.
pub fn new_connection_validate_options(
    options: &FileOptions,
    readable: bool,
    writable: bool,
    executable: bool,
) -> Result<(), Status> {
    // Nodes supporting both W+X rights are not supported.
    debug_assert!(!(writable && executable));

    // Validate permissions.
    if !readable && options.rights.contains(fio::Operations::READ_BYTES) {
        return Err(Status::ACCESS_DENIED);
    }
    if !writable && options.rights.contains(fio::Operations::WRITE_BYTES) {
        return Err(Status::ACCESS_DENIED);
    }
    if !executable && options.rights.contains(fio::Operations::EXECUTE) {
        return Err(Status::ACCESS_DENIED);
    }

    Ok(())
}

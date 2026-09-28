// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/lib/vfs/rust/src/file/connection.rs`: `StreamIoConnection`, with the
// request handling of `FileConnection` for the methods `fio::FileServer` has, serving
// `fuchsia.io/File` to in-process proxies (see the `fidl` stand-in) rather than over a channel.
// `new_connection_validate_options` and `get_backing_memory_validate_flags` are from
// `src/storage/lib/vfs/rust/src/file/common.rs`.
//
// As upstream, a connection handles its requests in order in its `RequestListener` (see
// `request_handler`), which holds the file open until the connection is closed, all its proxies
// are dropped (when, as when upstream's request stream closes, the file is synced first), or the
// scope shuts down. A request after that fails with `ZX_ERR_PEER_CLOSED`, as on a closed channel.

use crate::execution_scope::ExecutionScope;
use crate::file::{File, FileOptions, SyncMode};
use crate::name::Name;
use crate::node::OpenNode;
use crate::object_request::{ConnectionCreator, ObjectRequest, ObjectRequestRef, Representation};
use crate::protocols::{ProtocolsExt, ToFileOptions};
use crate::request_handler::{Request, RequestHandler, Requests, handle};
use crate::temp_clone::{TempClonable, unblock};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use futures::FutureExt;
use futures::future::BoxFuture;
use std::io::SeekFrom;
use std::ops::ControlFlow;
use std::pin::Pin;
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

/// Wrapper around a file that forwards `File` requests to `file` and
/// `FileIo` requests to `stream`.
pub struct StreamIoConnection<T: 'static + File + GetVmo> {
    /// Execution scope this connection and any async operations and connections it creates will
    /// use.
    scope: ExecutionScope,

    /// File that requests will be forwarded to.
    file: OpenNode<T>,

    /// The stream backing the connection that all read, write, and seek calls are forwarded to.
    stream: TempClonable<zx::Stream>,

    /// Options for this connection.
    options: FileOptions,
}

impl<T: 'static + File + GetVmo> StreamIoConnection<T> {
    /// Creates a stream-based file connection to the file. The file will be served from a new
    /// async `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        file: Arc<T>,
        options: impl ToFileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let file = OpenNode::new(file);
        let options = options.to_file_options()?;
        let stream =
            TempClonable::new(zx::Stream::create(options.to_stream_options(), file.get_vmo(), 0)?);

        new_connection_validate_options(
            &options,
            file.readable(),
            file.writable(),
            file.executable(),
        )?;

        file.open_file(&options).await?;
        if object_request.truncate {
            file.truncate(0).await?;
        }

        let connection = StreamIoConnection { scope: scope.clone(), file, stream, options };
        if let Ok(representation) = object_request.representation(&connection).await {
            let server = Arc::new(Server(Requests::spawn(&scope, Some(connection))));
            object_request.take().serve::<fio::FileMarker>(representation, server);
        }
        Ok(())
    }

    /// Similar to `create` but optimized for files whose implementation is synchronous and
    /// creating the connection is being done from a non-async context.
    pub fn create_sync(
        scope: ExecutionScope,
        file: Arc<T>,
        options: impl ToFileOptions,
        object_request: ObjectRequest,
    ) {
        scope.clone().spawn(object_request.handle_async(async |object_request| {
            Self::create(scope, file, options, object_request).await
        }));
    }

    async fn maybe_unblock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&zx::Stream) -> R + Send + 'static,
        R: Send + 'static,
    {
        if T::PAGER_ON_FIDL_EXECUTOR {
            let stream = self.stream.temp_clone();
            unblock(move || f(&*stream)).await
        } else {
            f(&*self.stream)
        }
    }

    fn handle_clone(&mut self, server_end: ServerEnd<fio::FileMarker>) {
        let stream =
            match zx::Stream::create(self.options.to_stream_options(), self.file.get_vmo(), 0) {
                Ok(stream) => TempClonable::new(stream),
                Err(status) => {
                    let _ = server_end.close_with_epitaph(status);
                    return;
                }
            };
        self.file.will_clone();
        let connection = Self {
            scope: self.scope.clone(),
            file: OpenNode::new((*self.file).clone()),
            stream,
            options: self.options,
        };
        server_end.serve(Arc::new(Server(Requests::spawn(&self.scope, Some(connection)))));
    }

    async fn handle_read(&mut self, count: u64) -> Result<Vec<u8>, Status> {
        if !self.options.rights.intersects(fio::Operations::READ_BYTES) {
            return Err(Status::BAD_HANDLE);
        }

        if count > fio::MAX_TRANSFER_SIZE {
            return Err(Status::OUT_OF_RANGE);
        }
        self.maybe_unblock(move |stream| {
            stream.read_to_vec(zx::StreamReadOptions::empty(), count as usize)
        })
        .await
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
        .await
    }

    async fn handle_write(&mut self, data: Vec<u8>) -> Result<u64, Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.maybe_unblock(move |stream| {
            let actual = stream.write(zx::StreamWriteOptions::empty(), &data)?;
            Ok(actual as u64)
        })
        .await
    }

    async fn handle_write_at(&self, offset: u64, data: Vec<u8>) -> Result<u64, Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.maybe_unblock(move |stream| {
            let actual = stream.write_at(zx::StreamWriteOptions::empty(), offset, &data)?;
            Ok(actual as u64)
        })
        .await
    }

    /// Move seek position to byte `offset` relative to the origin specified by `start`.
    async fn handle_seek(&mut self, offset: i64, origin: fio::SeekOrigin) -> Result<u64, Status> {
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
        self.stream.seek(position)
    }

    async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.file.get_attributes(query).await
    }

    async fn handle_update_attributes(
        &mut self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::UPDATE_ATTRIBUTES) {
            return Err(Status::BAD_HANDLE);
        }

        self.file.update_attributes(attributes).await
    }

    async fn handle_truncate(&mut self, length: u64) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }

        self.file.truncate(length).await
    }

    async fn handle_allocate(
        &mut self,
        offset: u64,
        length: u64,
        mode: fio::AllocateMode,
    ) -> Result<(), Status> {
        if !self.options.rights.intersects(fio::Operations::WRITE_BYTES) {
            return Err(Status::BAD_HANDLE);
        }
        self.file.allocate(offset, length, mode).await
    }

    async fn handle_get_backing_memory(&mut self, flags: fio::VmoFlags) -> Result<zx::Vmo, Status> {
        get_backing_memory_validate_flags(flags, self.options)?;
        self.file.get_backing_memory(flags).await
    }

    async fn handle_link_into(
        &mut self,
        target_parent_token: flex_client::Event,
        target_name: String,
    ) -> Result<(), Status> {
        let target_name = Name::try_from(target_name).map_err(|_| Status::INVALID_ARGS)?;

        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        if !self.options.is_linkable {
            return Err(Status::NOT_FOUND);
        }

        if !self.options.rights.contains(
            fio::Operations::READ_BYTES
                | fio::Operations::WRITE_BYTES
                | fio::Operations::GET_ATTRIBUTES
                | fio::Operations::UPDATE_ATTRIBUTES,
        ) {
            return Err(Status::ACCESS_DENIED);
        }

        let (target_parent, target_rights) = self
            .scope
            .token_registry()
            .get_owner_and_rights(target_parent_token.into())?
            .ok_or(Status::NOT_FOUND)?;

        if !target_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        (*self.file).clone().link_into(target_parent, target_name).await
    }

    fn should_sync_before_close(&self) -> bool {
        self.options
            .rights
            .intersects(fio::Operations::WRITE_BYTES | fio::Operations::UPDATE_ATTRIBUTES)
    }
}

impl<T: 'static + File + GetVmo> RequestHandler for Option<StreamIoConnection<T>> {
    type Request = Request<Self>;

    fn handle_request(
        self: Pin<&mut Self>,
        request: Self::Request,
    ) -> impl Future<Output = ControlFlow<()>> + Send {
        let scope = self.as_ref().get_ref().as_ref().unwrap().scope.clone();
        handle(&scope, self, request)
    }

    async fn stream_closed(self: Pin<&mut Self>) {
        let this = self.get_mut().as_mut().unwrap();
        if this.should_sync_before_close() {
            if let Some(_guard) = this.scope.try_active_guard() {
                let _ = this.file.sync(SyncMode::PreClose).await;
            }
        }
    }
}

impl<T: 'static + File + GetVmo> Representation for StreamIoConnection<T> {
    type Protocol = fio::FileMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::File(fio::FileInfo {
            is_append: Some(self.options.is_append),
            stream: Some(self.stream.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.file.get_attributes(requested_attributes).await?)
            },
            ..Default::default()
        }))
    }
}

impl<T: 'static + File + GetVmo> ConnectionCreator<T> for StreamIoConnection<T> {
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}

/// Sends the calls of a connection's proxies to its listener.
struct Server<T: 'static + File + GetVmo>(Requests<Option<StreamIoConnection<T>>>);

impl<T: 'static + File + GetVmo> Server<T> {
    /// Handles a request on the open connection, responding with a raw status.
    fn call<R: Send + 'static>(
        &self,
        request: impl for<'a> FnOnce(&'a mut StreamIoConnection<T>) -> BoxFuture<'a, Result<R, Status>>
        + Send
        + 'static,
    ) -> BoxFuture<'_, fio::FileResult<R>> {
        self.0
            .call(move |this| {
                async move {
                    (request(this.get_mut().as_mut().unwrap()).await, ControlFlow::Continue(()))
                }
                .boxed()
            })
            .map(|result| result.and_then(|result| result).map_err(Status::into_raw))
            .boxed()
    }
}

impl<T: 'static + File + GetVmo> fio::FileServer for Server<T> {
    fn read(&self, count: u64) -> BoxFuture<'_, fio::FileResult<Vec<u8>>> {
        self.call(move |this| this.handle_read(count).boxed())
    }

    fn read_at(&self, count: u64, offset: u64) -> BoxFuture<'_, fio::FileResult<Vec<u8>>> {
        self.call(move |this| this.handle_read_at(offset, count).boxed())
    }

    fn write<'a>(&'a self, data: &'a [u8]) -> BoxFuture<'a, fio::FileResult<u64>> {
        let data = data.to_vec();
        self.call(move |this| this.handle_write(data).boxed())
    }

    fn write_at<'a>(&'a self, data: &'a [u8], offset: u64) -> BoxFuture<'a, fio::FileResult<u64>> {
        let data = data.to_vec();
        self.call(move |this| this.handle_write_at(offset, data).boxed())
    }

    fn seek(&self, origin: fio::SeekOrigin, offset: i64) -> BoxFuture<'_, fio::FileResult<u64>> {
        self.call(move |this| this.handle_seek(offset, origin).boxed())
    }

    fn resize(&self, length: u64) -> BoxFuture<'_, fio::FileResult<()>> {
        self.call(move |this| this.handle_truncate(length).boxed())
    }

    fn allocate(
        &self,
        offset: u64,
        length: u64,
        mode: fio::AllocateMode,
    ) -> BoxFuture<'_, fio::FileResult<()>> {
        self.call(move |this| this.handle_allocate(offset, length, mode).boxed())
    }

    fn sync(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.call(move |this| this.file.sync(SyncMode::Normal).boxed())
    }

    fn close(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.0
            .call(move |mut option_this| {
                async move {
                    let this = option_this.as_mut().get_mut().as_mut().unwrap();
                    let result = if this.should_sync_before_close() {
                        this.file.sync(SyncMode::PreClose).await
                    } else {
                        Ok(())
                    };
                    // The file gets closed when we drop the connection, so we should do that
                    // before sending the response.
                    std::mem::drop(option_this.get_mut().take());
                    (result, ControlFlow::Break(()))
                }
                .boxed()
            })
            .map(|result| result.and_then(|result| result).map_err(Status::into_raw))
            .boxed()
    }

    fn get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> BoxFuture<'_, fio::FileResult<fio::Attributes>> {
        self.call(move |this| {
            async move {
                let attrs = this.handle_get_attributes(query).await?;
                Ok((attrs.mutable_attributes, attrs.immutable_attributes))
            }
            .boxed()
        })
    }

    fn update_attributes<'a>(
        &'a self,
        attributes: &'a fio::MutableNodeAttributes,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let attributes = attributes.clone();
        self.call(move |this| this.handle_update_attributes(attributes).boxed())
    }

    fn link_into<'a>(
        &'a self,
        dst_parent_token: zx::Event,
        dst: &'a str,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let dst = dst.to_owned();
        self.call(move |this| this.handle_link_into(dst_parent_token, dst).boxed())
    }

    fn get_backing_memory(&self, flags: fio::VmoFlags) -> BoxFuture<'_, fio::FileResult<zx::Vmo>> {
        self.call(move |this| this.handle_get_backing_memory(flags).boxed())
    }

    fn describe(&self) -> BoxFuture<'_, Result<fio::FileInfo, Status>> {
        self.0
            .call(move |this| {
                async move {
                    let this = this.get_mut().as_mut().unwrap();
                    let stream = this.stream.duplicate_handle(zx::Rights::SAME_RIGHTS);
                    (stream, ControlFlow::Continue(()))
                }
                .boxed()
            })
            .map(|result| {
                let stream = result.and_then(|stream| stream)?;
                Ok(fio::FileInfo { stream: Some(stream), ..Default::default() })
            })
            .boxed()
    }

    fn clone(&self, request: flex_client::Channel) {
        // A closed connection drops `request`, closing it.
        let _ = self.0.send(move |this| {
            async move {
                this.get_mut().as_mut().unwrap().handle_clone(request.into());
                ControlFlow::Continue(())
            }
            .boxed()
        });
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

pub fn get_backing_memory_validate_flags(
    vmo_flags: fio::VmoFlags,
    options: FileOptions,
) -> Result<(), Status> {
    // Disallow inconsistent flag combination.
    if vmo_flags.contains(fio::VmoFlags::PRIVATE_CLONE)
        && vmo_flags.contains(fio::VmoFlags::SHARED_BUFFER)
    {
        return Err(Status::INVALID_ARGS);
    }

    // Ensure the requested rights in vmo_flags do not exceed those of the underlying connection.
    if vmo_flags.contains(fio::VmoFlags::READ)
        && !options.rights.intersects(fio::Operations::READ_BYTES)
    {
        return Err(Status::ACCESS_DENIED);
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE)
        && !options.rights.intersects(fio::Operations::WRITE_BYTES)
    {
        return Err(Status::ACCESS_DENIED);
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE)
        && !options.rights.intersects(fio::Operations::EXECUTE)
    {
        return Err(Status::ACCESS_DENIED);
    }

    Ok(())
}

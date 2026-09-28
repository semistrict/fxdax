// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Server support for symbolic links.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/symlink.rs`, copied verbatim except that the
//! connection serves `fuchsia.io/Symlink` to in-process proxies (see the `fidl` stand-in): the
//! calls of `fio::SymlinkServer` are sent to the connection's `RequestListener` (see
//! `request_handler`), which handles them in order as upstream's `handle_request` does. There are
//! no extended attributes.

use crate::execution_scope::ExecutionScope;
use crate::name::Name;
use crate::node::{Node, OpenNode};
use crate::object_request::{ConnectionCreator, Representation};
use crate::request_handler::{Request, RequestHandler, Requests, handle};
use crate::{ObjectRequest, ObjectRequestRef, ProtocolsExt, ToObjectRequest};
use flex_fuchsia_io as fio;
use futures::FutureExt;
use futures::future::BoxFuture;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use zx_status::Status;

pub trait Symlink: Node {
    fn read_target(&self) -> impl Future<Output = Result<Vec<u8>, Status>> + Send;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SymlinkOptions {
    pub rights: fio::Operations,
}

pub struct Connection<T: Node> {
    scope: ExecutionScope,
    symlink: OpenNode<T>,
    options: SymlinkOptions,
}

impl<T: Symlink> Connection<T> {
    /// Creates a new connection to serve the symlink. The symlink will be served from a new async
    /// `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        symlink: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let options = protocols.to_symlink_options()?;
        let connection = Self { scope: scope.clone(), symlink: OpenNode::new(symlink), options };
        if let Ok(representation) = object_request.representation(&connection).await {
            let server = Arc::new(Server(Requests::spawn(&scope, connection)));
            object_request.take().serve::<fio::SymlinkMarker>(representation, server);
        }
        Ok(())
    }

    /// Similar to `create` but optimized for symlinks whose implementation is synchronous and
    /// creating the connection is being done from a non-async context.
    pub fn create_sync(
        scope: ExecutionScope,
        symlink: Arc<T>,
        options: impl ProtocolsExt,
        object_request: ObjectRequest,
    ) {
        scope.clone().spawn(object_request.handle_async(async |object_request| {
            Self::create(scope, symlink, options, object_request).await
        }));
    }

    async fn handle_get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        // Note: Symlink connections are required to have GET_ATTRIBUTES rights upon creation
        // (enforced in `to_symlink_options`). We check it here anyway for consistency and safety.
        if !self.options.rights.intersects(fio::Operations::GET_ATTRIBUTES) {
            return Err(Status::ACCESS_DENIED);
        }
        self.symlink.get_attributes(query).await
    }

    async fn handle_clone(&mut self, server_end: flex_client::Channel) {
        let mut flags = fio::Flags::PROTOCOL_SYMLINK;
        if self.options.rights.contains(fio::Operations::GET_ATTRIBUTES) {
            flags |= fio::Flags::PERM_GET_ATTRIBUTES;
        }
        if self.options.rights.contains(fio::Operations::READ_BYTES) {
            flags |= fio::Flags::PERM_READ_BYTES;
        }
        if self.options.rights.contains(fio::Operations::WRITE_BYTES) {
            flags |= fio::Flags::PERM_WRITE_BYTES;
        }
        self.symlink.will_clone();
        flags
            .to_object_request(server_end)
            .handle_async(async |object_request| {
                Self::create(self.scope.clone(), self.symlink.clone(), flags, object_request).await
            })
            .await;
    }

    async fn handle_link_into(
        &mut self,
        target_parent_token: flex_client::Event,
        target_name: String,
    ) -> Result<(), Status> {
        let target_name = Name::try_from(target_name).map_err(|_| Status::INVALID_ARGS)?;

        // Enforce the maximum supported rights for symlinks to prevent rights escalation by
        // hardlinking it into a more privileged connection. In particular, this protects
        // attributes guarded by GET_ATTRIBUTES as well as extended attributes which are governed
        // by READ_BYTES and WRITE_BYTES.
        if !self.options.rights.contains(
            fio::Operations::READ_BYTES
                | fio::Operations::WRITE_BYTES
                | fio::Operations::GET_ATTRIBUTES,
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

        self.symlink.clone().link_into(target_parent, target_name).await
    }
}

impl<T: Symlink> Representation for Connection<T> {
    type Protocol = fio::SymlinkMarker;

    async fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::Representation, Status> {
        Ok(fio::Representation::Symlink(fio::SymlinkInfo {
            attributes: if requested_attributes.is_empty() {
                None
            } else {
                Some(self.symlink.get_attributes(requested_attributes).await?)
            },
            target: Some(self.symlink.read_target().await?),
            ..Default::default()
        }))
    }
}

impl<T: Symlink> ConnectionCreator<T> for Connection<T> {
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}

impl<T: Symlink> RequestHandler for Connection<T> {
    type Request = Request<Self>;

    fn handle_request(
        self: Pin<&mut Self>,
        request: Self::Request,
    ) -> impl Future<Output = ControlFlow<()>> + Send {
        let scope = self.scope.clone();
        handle(&scope, self, request)
    }
}

/// Sends the calls of a connection's proxies to its listener.
struct Server<T: Symlink>(Requests<Connection<T>>);

impl<T: Symlink> Server<T> {
    /// Handles a request on the connection, responding with a raw status.
    fn call<R: Send + 'static>(
        &self,
        request: impl for<'a> FnOnce(&'a mut Connection<T>) -> BoxFuture<'a, Result<R, Status>>
        + Send
        + 'static,
    ) -> BoxFuture<'_, fio::FileResult<R>> {
        self.0
            .call(move |this| {
                async move { (request(this.get_mut()).await, ControlFlow::Continue(())) }.boxed()
            })
            .map(|result| result.and_then(|result| result).map_err(Status::into_raw))
            .boxed()
    }
}

impl<T: Symlink> fio::SymlinkServer for Server<T> {
    fn describe(&self) -> BoxFuture<'_, Result<fio::SymlinkInfo, Status>> {
        // Upstream closes the connection with the status if reading the target fails.
        self.0
            .call(move |this| {
                async move {
                    match this.symlink.read_target().await {
                        Ok(target) => (
                            Ok(fio::SymlinkInfo { target: Some(target), ..Default::default() }),
                            ControlFlow::Continue(()),
                        ),
                        Err(status) => (Err(status), ControlFlow::Break(())),
                    }
                }
                .boxed()
            })
            .map(|result| result.and_then(|result| result))
            .boxed()
    }

    fn link_into<'a>(
        &'a self,
        dst_parent_token: zx::Event,
        dst: &'a str,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let dst = dst.to_owned();
        self.call(move |this| this.handle_link_into(dst_parent_token, dst).boxed())
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

    fn close(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.0
            .call(move |_| async move { (Ok(()), ControlFlow::Break(())) }.boxed())
            .map(|result| result.and_then(|result| result).map_err(Status::into_raw))
            .boxed()
    }

    fn clone(&self, request: flex_client::Channel) {
        // A closed connection drops `request`, closing it.
        let _ = self.0.send(move |this| {
            async move {
                this.get_mut().handle_clone(request).await;
                ControlFlow::Continue(())
            }
            .boxed()
        });
    }
}

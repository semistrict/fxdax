// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Connection to a directory that can be modified by the client though a FIDL connection.
//!
//! An excerpt of `src/storage/lib/vfs/rust/src/directory/mutable/connection.rs`, copied verbatim
//! except that the connection serves `fuchsia.io/Directory` to in-process proxies (see the `fidl`
//! stand-in): the calls of `fio::DirectoryServer` are sent to the connection's `RequestListener`
//! (see `request_handler`), which handles them in order with the handlers upstream's
//! `handle_request` dispatches to. There are no watchers or extended attributes.

use crate::directory::connection::BaseConnection;
use crate::directory::entry_container::MutableDirectory;
use crate::execution_scope::ExecutionScope;
use crate::name::validate_name;
use crate::node::OpenNode;
use crate::object_request::ConnectionCreator;
use crate::path::Path;
use crate::request_handler::{Request, RequestHandler, Requests, handle};
use crate::token_registry::{TokenInterface, TokenRegistry, Tokenizable};
use crate::{ObjectRequestRef, ProtocolsExt};

use flex_client::NullableHandle;
use flex_fuchsia_io as fio;
use futures::FutureExt;
use futures::future::BoxFuture;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use zx_status::Status;

pub struct MutableConnection<DirectoryType: MutableDirectory> {
    base: BaseConnection<DirectoryType>,
}

impl<DirectoryType: MutableDirectory> MutableConnection<DirectoryType> {
    /// Creates a new connection to serve the mutable directory. The directory will be served from a
    /// new async `Task`, not from the current `Task`. Errors in constructing the connection are not
    /// guaranteed to be returned, they may be sent directly to the client end of the connection.
    /// This method should be called from within an `ObjectRequest` handler to ensure that errors
    /// are sent to the client end of the connection.
    pub async fn create(
        scope: ExecutionScope,
        directory: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        // Ensure we close the directory if we fail to prepare the connection.
        let directory = OpenNode::new(directory);

        let connection = MutableConnection {
            base: BaseConnection::new(scope.clone(), directory, protocols.to_directory_options()?),
        };

        if let Ok(representation) = object_request.representation(&connection.base).await {
            let server = Arc::new(Server(Requests::spawn(&scope, Tokenizable::new(connection))));
            object_request.take().serve::<fio::DirectoryMarker>(representation, server);
        }
        Ok(())
    }

    async fn handle_update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Operations::UPDATE_ATTRIBUTES) {
            return Err(Status::BAD_HANDLE);
        }
        // TODO(jfsulliv): Consider always permitting attributes to be deferrable. The risk with
        // this is that filesystems would require a background flush of dirty attributes to disk.
        self.base.directory.update_attributes(attributes).await
    }

    async fn handle_unlink(&self, name: String, options: fio::UnlinkOptions) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }

        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(Status::INVALID_ARGS);
        }

        self.base
            .directory
            .clone()
            .unlink(
                &name,
                options
                    .flags
                    .map(|f| f.contains(fio::UnlinkFlags::MUST_BE_DIRECTORY))
                    .unwrap_or(false),
            )
            .await
    }

    fn handle_get_token(this: Pin<&Tokenizable<Self>>) -> Result<NullableHandle, Status> {
        // TODO(https://fxbug.dev/503041342): The current GetToken method on directory requires
        // specific rights to get the token in the first place. The new GetToken on Node will
        // require no rights to get a token, but instead enforced on use of the token. When the old
        // one is deleted this method (and therefore this check) will be deleted too.
        if !this.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::BAD_HANDLE);
        }
        Ok(TokenRegistry::get_token(this)?)
    }

    async fn handle_rename(
        &self,
        src: String,
        dst_parent_token: NullableHandle,
        dst: String,
    ) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        let src = Path::validate_and_split(src)?;
        let dst = Path::validate_and_split(dst)?;

        if !src.is_single_component() || !dst.is_single_component() {
            return Err(Status::INVALID_ARGS);
        }

        let (dst_parent, dst_rights) = self
            .base
            .scope
            .token_registry()
            .get_owner_and_rights(dst_parent_token)?
            .ok_or(Status::NOT_FOUND)?;

        if !dst_rights.contains(fio::Rights::MODIFY_DIRECTORY) {
            return Err(Status::ACCESS_DENIED);
        }

        let is_same_dir =
            Arc::ptr_eq(&(self.base.directory.clone() as Arc<dyn MutableDirectory>), &dst_parent);

        if !is_same_dir && !self.base.options.rights.contains(fio::RW_STAR_DIR) {
            return Err(Status::ACCESS_DENIED);
        }

        dst_parent.clone().rename(self.base.directory.clone(), src, dst).await
    }

    async fn handle_create_symlink(
        &self,
        name: String,
        target: Vec<u8>,
        connection: Option<flex_client::fidl::ServerEnd<fio::SymlinkMarker>>,
    ) -> Result<(), Status> {
        if !self.base.options.rights.contains(fio::Operations::MODIFY_DIRECTORY) {
            Err(Status::ACCESS_DENIED)
        } else if validate_name(&name).is_err() {
            Err(Status::INVALID_ARGS)
        } else {
            self.base.directory.create_symlink(name, target, connection).await
        }
    }
}

impl<DirectoryType: MutableDirectory> ConnectionCreator<DirectoryType>
    for MutableConnection<DirectoryType>
{
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<DirectoryType>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a {
        Self::create(scope, node, protocols, object_request)
    }
}

impl<DirectoryType: MutableDirectory> TokenInterface for MutableConnection<DirectoryType> {
    fn get_node(&self) -> Arc<dyn MutableDirectory> {
        self.base.directory.clone()
    }

    fn get_rights(&self) -> fio::Rights {
        self.base.options.rights
    }

    fn token_registry(&self) -> &TokenRegistry {
        self.base.scope.token_registry()
    }
}

impl<DirectoryType: MutableDirectory> RequestHandler
    for Tokenizable<MutableConnection<DirectoryType>>
{
    type Request = Request<Self>;

    fn handle_request(
        self: Pin<&mut Self>,
        request: Self::Request,
    ) -> impl Future<Output = ControlFlow<()>> + Send {
        let scope = self.base.scope.clone();
        handle(&scope, self, request)
    }
}

/// Sends the calls of a connection's proxies to its listener.
struct Server<DirectoryType: MutableDirectory>(
    Requests<Tokenizable<MutableConnection<DirectoryType>>>,
);

impl<DirectoryType: MutableDirectory> Server<DirectoryType> {
    /// Handles a request on the connection, responding with a raw status.
    fn call<R: Send + 'static>(
        &self,
        request: impl for<'a> FnOnce(
            Pin<&'a mut Tokenizable<MutableConnection<DirectoryType>>>,
        ) -> BoxFuture<'a, Result<R, Status>>
        + Send
        + 'static,
    ) -> BoxFuture<'_, fio::FileResult<R>> {
        self.0
            .call(move |this| {
                async move { (request(this).await, ControlFlow::Continue(())) }.boxed()
            })
            .map(|result| result.and_then(|result| result).map_err(Status::into_raw))
            .boxed()
    }
}

impl<DirectoryType: MutableDirectory> fio::DirectoryServer for Server<DirectoryType> {
    fn open(
        &self,
        path: &str,
        flags: fio::Flags,
        options: &fio::Options,
        object: flex_client::Channel,
    ) {
        let (path, options) = (path.to_owned(), options.clone());
        // A closed connection drops `object`, closing it.
        let _ = self.0.send(move |this| {
            async move {
                this.base.handle_open_request(path, flags, options, object).await;
                ControlFlow::Continue(())
            }
            .boxed()
        });
    }

    fn unlink<'a>(
        &'a self,
        name: &'a str,
        options: &'a fio::UnlinkOptions,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let (name, options) = (name.to_owned(), options.clone());
        self.call(move |this| async move { this.handle_unlink(name, options).await }.boxed())
    }

    fn get_token(&self) -> BoxFuture<'_, (i32, Option<zx::NullableHandle>)> {
        self.call(move |this| {
            async move { MutableConnection::handle_get_token(this.into_ref()) }.boxed()
        })
        .map(|result| match result {
            Ok(token) => (0, Some(token)),
            Err(status) => (status, None),
        })
        .boxed()
    }

    fn rename<'a>(
        &'a self,
        src: &'a str,
        dst_parent_token: zx::Event,
        dst: &'a str,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let (src, dst) = (src.to_owned(), dst.to_owned());
        self.call(move |this| {
            async move { this.handle_rename(src, NullableHandle::from(dst_parent_token), dst).await }
                .boxed()
        })
    }

    fn link<'a>(
        &'a self,
        src: &'a str,
        dst_parent_token: zx::NullableHandle,
        dst: &'a str,
    ) -> BoxFuture<'a, i32> {
        let (src, dst) = (src.to_owned(), dst.to_owned());
        self.call(move |this| {
            async move { this.base.handle_link(&src, dst_parent_token, dst).await }.boxed()
        })
        .map(|result| result.err().unwrap_or(zx::sys::ZX_OK))
        .boxed()
    }

    fn read_dirents(&self, max_bytes: u64) -> BoxFuture<'_, (i32, Vec<u8>)> {
        self.call(move |this| {
            async move {
                let (status, entries) =
                    Tokenizable::as_mut(this).get_mut().base.handle_read_dirents(max_bytes).await;
                Ok((Status::result_into_raw(status), entries))
            }
            .boxed()
        })
        .map(|result| result.unwrap_or_else(|status| (status, Vec::new())))
        .boxed()
    }

    fn rewind(&self) -> BoxFuture<'_, i32> {
        self.call(move |this| {
            async move { Tokenizable::as_mut(this).get_mut().base.handle_rewind() }.boxed()
        })
        .map(|result| result.err().unwrap_or(zx::sys::ZX_OK))
        .boxed()
    }

    fn create_symlink<'a>(
        &'a self,
        name: &'a str,
        target: &'a [u8],
        connection: Option<flex_client::fidl::ServerEnd<fio::SymlinkMarker>>,
    ) -> BoxFuture<'a, fio::FileResult<()>> {
        let (name, target) = (name.to_owned(), target.to_vec());
        self.call(move |this| {
            async move { this.handle_create_symlink(name, target, connection).await }.boxed()
        })
    }

    fn get_attributes(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> BoxFuture<'_, fio::FileResult<fio::Attributes>> {
        self.call(move |this| {
            async move {
                let attrs = this.base.handle_get_attributes(query).await?;
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
        self.call(move |this| {
            async move { this.handle_update_attributes(attributes).await }.boxed()
        })
    }

    fn sync(&self) -> BoxFuture<'_, fio::FileResult<()>> {
        self.call(move |this| async move { this.base.directory.sync().await }.boxed())
    }

    fn query_filesystem(&self) -> BoxFuture<'_, (i32, Option<Box<fio::FilesystemInfo>>)> {
        self.call(move |this| async move { this.base.directory.query_filesystem() }.boxed())
            .map(|result| match result {
                Ok(info) => (zx::sys::ZX_OK, Some(Box::new(info))),
                Err(status) => (status, None),
            })
            .boxed()
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
                Tokenizable::as_mut(this).get_mut().base.handle_clone(request);
                ControlFlow::Continue(())
            }
            .boxed()
        });
    }
}

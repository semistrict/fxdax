// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// An excerpt of `src/storage/lib/vfs/rust/src/object_request.rs`: `ObjectRequest`, the channel an
// open serves a connection on, and the traits for creating connections, copied verbatim except as
// follows.
//
// - The channel is the `fidl` stand-in's, served in-process, so there are no request streams:
//   `into_request_stream` is split into `representation`, which gets the representation to send if
//   the open asked for one, and `serve`, which sends it and serves the connection's server.
// - Node connections (`PROTOCOL_NODE`) aren't served: creating one fails with
//   `ZX_ERR_NOT_SUPPORTED`.
// - There is no `create_connection_sync`, which Fxfs doesn't use.

use crate::ProtocolsExt;
use crate::execution_scope::ExecutionScope;
use crate::node::Node;
use flex_client::fidl::{ProtocolMarker, ServerEnd};
use flex_fuchsia_io as fio;
use std::future::Future;
use std::sync::Arc;
use zx_status::Status;

/// Wraps the channel provided in the open methods and provide convenience methods for sending
/// appropriate responses.  It also records actions that should be taken upon successful connection
/// such as truncating file objects.
pub struct ObjectRequest {
    // The channel, until it is taken.
    object_request: Option<flex_client::Channel>,

    // What should be sent first.
    what_to_send: ObjectRequestSend,

    // Attributes required in the open method.
    attributes: fio::NodeAttributesQuery,

    // Creation attributes.
    create_attributes: Option<Box<fio::MutableNodeAttributes>>,

    /// Truncate the object before use.
    pub truncate: bool,
}

impl ObjectRequest {
    /// Create a new [`ObjectRequest`] from a set of [`fio::Flags`] and [`fio::Options`]`.
    pub fn new(
        flags: fio::Flags,
        options: &fio::Options,
        object_request: flex_client::Channel,
    ) -> Self {
        Self {
            object_request: Some(object_request),
            what_to_send: if flags.contains(fio::Flags::FLAG_SEND_REPRESENTATION) {
                ObjectRequestSend::OnRepresentation
            } else {
                ObjectRequestSend::Nothing
            },
            attributes: options.attributes.unwrap_or(fio::NodeAttributesQuery::empty()),
            create_attributes: options.create_attributes.as_ref().map(|a| Box::new(a.clone())),
            truncate: flags.is_truncate(),
        }
    }

    pub fn attributes(&self) -> fio::NodeAttributesQuery {
        self.attributes
    }

    pub fn create_attributes(&self) -> Option<&fio::MutableNodeAttributes> {
        self.create_attributes.as_deref()
    }

    pub fn options(&self) -> fio::Options {
        fio::Options {
            attributes: (!self.attributes.is_empty()).then_some(self.attributes),
            create_attributes: self
                .create_attributes
                .as_ref()
                .map(|a| fio::MutableNodeAttributes::clone(&a)),
        }
    }

    /// The representation of `connection` to send, if the open asked for one: the first half of
    /// `into_request_stream`, which shuts the request down if getting it fails.
    pub async fn representation<T: Representation>(
        &mut self,
        connection: &T,
    ) -> Result<Option<fio::Representation>, Status> {
        match self.what_to_send {
            ObjectRequestSend::OnRepresentation => {
                match connection.get_representation(self.attributes).await {
                    Ok(representation) => Ok(Some(representation)),
                    Err(status) => {
                        self.take().shutdown(status);
                        Err(status)
                    }
                }
            }
            ObjectRequestSend::Nothing => Ok(None),
        }
    }

    /// Serves `server` on the channel after sending `representation`: the second half of
    /// `into_request_stream`, with the server that stands in for the request stream.
    pub fn serve<P: ProtocolMarker>(
        self,
        representation: Option<fio::Representation>,
        server: Arc<P::Server>,
    ) {
        let server_end = self.into_server_end::<P>();
        if let Some(payload) = representation {
            server_end.send_event(fio::NodeEvent::OnRepresentation { payload });
        }
        server_end.serve(server);
    }

    /// Converts to ServerEnd<T>.
    pub fn into_server_end<T: ProtocolMarker>(self) -> ServerEnd<T> {
        self.into_channel().into()
    }

    /// Extracts the channel (without sending on_open).
    pub fn into_channel(mut self) -> flex_client::Channel {
        self.object_request.take().expect("the object request was taken")
    }

    /// Terminates the object request with the given status.
    pub fn shutdown(mut self, status: Status) {
        if let Some(object_request) = self.object_request.take() {
            let _ = object_request.close_with_epitaph(status);
        }
    }

    /// Calls `f` and sends an error on the object request channel upon failure.
    pub fn handle<T>(
        mut self,
        f: impl FnOnce(ObjectRequestRef<'_>) -> Result<T, Status>,
    ) -> Option<T> {
        match f(&mut self) {
            Ok(o) => Some(o),
            Err(s) => {
                self.shutdown(s);
                None
            }
        }
    }

    /// Calls `f` and sends an error on the object request channel upon failure.
    pub async fn handle_async(
        mut self,
        f: impl AsyncFnOnce(&mut ObjectRequest) -> Result<(), Status>,
    ) {
        if let Err(s) = f(&mut self).await {
            self.shutdown(s);
        }
    }

    /// Take the ObjectRequest.  The caller is responsible for sending errors.
    pub fn take(&mut self) -> ObjectRequest {
        Self {
            object_request: self.object_request.take(),
            what_to_send: self.what_to_send,
            attributes: self.attributes,
            create_attributes: self.create_attributes.take(),
            truncate: self.truncate,
        }
    }

    /// Constructs a new connection to `node` and spawns an async `Task` that will handle requests
    /// on the connection. `f` is a callback that constructs the connection but it will not be
    /// called if the connection is supposed to be a node connection. This should be called from
    /// within a [`ObjectRequest::handle_async`] callback.
    pub async fn create_connection<C, N>(
        &mut self,
        scope: ExecutionScope,
        node: Arc<N>,
        protocols: impl ProtocolsExt,
    ) -> Result<(), Status>
    where
        C: ConnectionCreator<N>,
        N: Node,
    {
        if protocols.is_node() {
            Err(Status::NOT_SUPPORTED)
        } else {
            C::create(scope, node, protocols, self).await
        }
    }
}

pub type ObjectRequestRef<'a> = &'a mut ObjectRequest;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ObjectRequestSend {
    OnRepresentation,
    Nothing,
}

/// Trait to get either fio::Representation or fio::NodeInfoDeprecated.  Connection types
/// should implement this.
pub trait Representation {
    /// The protocol used for the connection.
    type Protocol: ProtocolMarker;

    /// Returns io2's Representation for the object.
    fn get_representation(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> impl Future<Output = Result<fio::Representation, Status>> + Send;
}

/// Trait for converting fio::Flags into ObjectRequest.
pub trait ToObjectRequest: ProtocolsExt {
    fn to_object_request(&self, object_request: impl Into<flex_client::Channel>) -> ObjectRequest;
}

impl ToObjectRequest for fio::Flags {
    fn to_object_request(&self, object_request: impl Into<flex_client::Channel>) -> ObjectRequest {
        ObjectRequest::new(*self, &Default::default(), object_request.into())
    }
}

/// Trait for constructing connections to nodes.
pub trait ConnectionCreator<T: Node> {
    /// Creates a new connection to `node` and spawns a new `Task` to run the connection.
    fn create<'a>(
        scope: ExecutionScope,
        node: Arc<T>,
        protocols: impl ProtocolsExt,
        object_request: ObjectRequestRef<'a>,
    ) -> impl Future<Output = Result<(), Status>> + 'a;
}

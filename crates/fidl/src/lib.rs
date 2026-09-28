// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A stand-in for Fuchsia's FIDL runtime crate (`src/lib/fidl/rust/fidl`).
//!
//! Only the items the vendored parts of other libraries use are here. Handles are the in-process
//! kernel's, as they are on Fuchsia, rather than the emulated handles upstream uses on hosts.
//!
//! There are no channels or wire format: a proxy calls the object serving its protocol in-process
//! (see `endpoints`), which is all fxdax's tests of FIDL-serving code need.

/// As `fuchsia_handles` in `src/lib/fidl/rust/fidl/src/handle.rs` aliases the zircon library.
pub use zx::{
    AsHandleRef, Handle, HandleDisposition, HandleInfo, HandleOp, HandleRef, Koid, MessageBufEtc,
    NullableHandle, ObjectType, Peered, Rights, Signals, Status,
};

/// The error type used by FIDL operations, as `src/lib/fidl/rust/fidl/src/error.rs`'s, with only
/// the errors a proxy calling a server in-process can have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The other end of the connection was closed: nothing serves the protocol.
    ClientChannelClosed {
        /// Why, as the epitaph would say.
        status: Status,
        /// The name of the protocol at the other end of the channel.
        protocol_name: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ClientChannelClosed { status, protocol_name } => {
                write!(
                    f,
                    "A FIDL client's channel to the service {protocol_name} was closed: {status}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// The server end of a connection whose protocol is not known statically, as a channel is.
pub struct Channel {
    /// The connection's `OnceLock<Arc<M::Server>>`, for the protocol `M` its server end had.
    server: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

/// From `src/lib/fidl/rust/fidl/src/endpoints.rs`, with connections in-process: a proxy and its
/// server end share a slot the server end is served with.
pub mod endpoints {
    use super::{Channel, Error, Status};
    use std::sync::{Arc, OnceLock};

    /// The fake "client" produced by `ProxyHasDomain`. Analogous to an FDomain client.
    pub struct ZirconClient;

    /// A type which allows querying a remote FIDL server over a channel; here, over a
    /// `Connection`.
    pub trait ProtocolMarker: Sized + Send + Sync + 'static {
        /// The type of the structure against which FIDL requests are made.
        type Proxy: From<Connection<Self>>;

        /// The type of the structure against which thread-blocking FIDL requests are made.
        type SynchronousProxy: From<Connection<Self>>;

        /// The object serving the protocol's requests.
        type Server: ?Sized + Send + Sync;

        /// The name of the protocol suitable for debug purposes.
        const DEBUG_NAME: &'static str;
    }

    /// The connection a proxy and a server end share, which stands in for the channel.
    pub struct Connection<M: ProtocolMarker> {
        server: Arc<OnceLock<Arc<M::Server>>>,
    }

    impl<M: ProtocolMarker> Clone for Connection<M> {
        fn clone(&self) -> Self {
            Self { server: self.server.clone() }
        }
    }

    impl<M: ProtocolMarker> Connection<M> {
        /// A connection served by `server`.
        pub fn served_by(server: Arc<M::Server>) -> Self {
            Self { server: Arc::new(OnceLock::from(server)) }
        }

        /// The object serving the connection, or the error a call on a closed channel has.
        pub fn server(&self) -> Result<&Arc<M::Server>, Error> {
            self.server.get().ok_or(Error::ClientChannelClosed {
                status: Status::PEER_CLOSED,
                protocol_name: M::DEBUG_NAME,
            })
        }
    }

    /// The server end of a FIDL connection.
    pub struct ServerEnd<M: ProtocolMarker> {
        connection: Connection<M>,
    }

    impl<M: ProtocolMarker> ServerEnd<M> {
        /// Serves the connection with `server`.
        pub fn serve(self, server: Arc<M::Server>) {
            let _ = self.connection.server.set(server);
        }

        /// Extract the inner channel.
        pub fn into_channel(self) -> Channel {
            Channel { server: self.connection.server }
        }
    }

    /// A channel carries a protocol whose server has the same type as `M`'s, such as one a
    /// protocol composes (`fuchsia.unknown/Cloneable` in `fuchsia.io/File`).
    impl<M: ProtocolMarker> From<Channel> for ServerEnd<M> {
        fn from(channel: Channel) -> Self {
            let server = channel
                .server
                .downcast::<OnceLock<Arc<M::Server>>>()
                .unwrap_or_else(|_| panic!("the channel does not carry {}", M::DEBUG_NAME));
            ServerEnd { connection: Connection { server } }
        }
    }

    /// Create a client proxy and a server endpoint connected to it.
    pub fn create_proxy<M: ProtocolMarker>() -> (M::Proxy, ServerEnd<M>) {
        let connection = Connection::<M> { server: Arc::new(OnceLock::new()) };
        (connection.clone().into(), ServerEnd { connection })
    }

    /// Create a synchronous client proxy and a server endpoint connected to it.
    pub fn create_sync_proxy<M: ProtocolMarker>() -> (M::SynchronousProxy, ServerEnd<M>) {
        let connection = Connection::<M> { server: Arc::new(OnceLock::new()) };
        (connection.clone().into(), ServerEnd { connection })
    }
}

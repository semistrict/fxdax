// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A stand-in for Fuchsia's FIDL runtime crate (`src/lib/fidl/rust/fidl`).
//!
//! Only the items the parts of other libraries copied from Fuchsia use are here. Handles are the in-process
//! kernel's, as they are on Fuchsia, rather than the emulated handles upstream uses on hosts.
//!
//! There are no channels or wire format: a proxy calls the object serving its protocol in-process
//! (see `endpoints`), which is all fxdax's tests of FIDL-serving code need.

/// As `fuchsia_handles` in `src/lib/fidl/rust/fidl/src/handle.rs` aliases the zircon library.
pub use zx::{
    AsHandleRef, Event, Handle, HandleDisposition, HandleInfo, HandleOp, HandleRef, Koid,
    MessageBufEtc, NullableHandle, ObjectType, Peered, Rights, Signals, Status,
};

/// Represents the closure condition of a FIDL channel, either via an explicit epitaph or a direct
/// peer closure.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Epitaph {
    /// The channel was closed with an explicit epitaph status (`Ok(())` for `ZX_OK` success, or
    /// `Err(Status)` for an error).
    Explicit(std::result::Result<(), zx_status::Status>),
    /// The channel was closed without an epitaph message (`PEER_CLOSED`).
    PeerClosed,
}

impl std::fmt::Display for Epitaph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Epitaph::Explicit(Ok(())) => write!(f, "ZX_OK (0)"),
            Epitaph::Explicit(Err(s)) => write!(f, "{}", s),
            Epitaph::PeerClosed => write!(f, "{}", zx_status::Status::PEER_CLOSED),
        }
    }
}

impl std::error::Error for Epitaph {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Epitaph::Explicit(Ok(())) | Epitaph::PeerClosed => None,
            Epitaph::Explicit(Err(s)) => Some(s),
        }
    }
}

impl Epitaph {
    /// Returns `true` if an explicit epitaph was received or set (`Explicit`).
    pub fn is_explicit(&self) -> bool {
        matches!(self, Epitaph::Explicit(_))
    }

    /// Returns `true` if the channel was closed without an epitaph (`PEER_CLOSED`)
    /// or with an explicit `PEER_CLOSED` epitaph.
    pub fn is_peer_closed(&self) -> bool {
        matches!(self, Epitaph::PeerClosed | Epitaph::Explicit(Err(zx_status::Status::PEER_CLOSED)))
    }

    /// Returns `true` if an explicit `ZX_OK` (`Ok(())`) epitaph was received.
    pub fn is_ok(&self) -> bool {
        matches!(self, Epitaph::Explicit(Ok(())))
    }
}

impl PartialEq<zx_status::Status> for Epitaph {
    fn eq(&self, other: &zx_status::Status) -> bool {
        match self {
            Epitaph::Explicit(Ok(())) => zx_status::Status::ok(other.into_raw()).is_ok(),
            Epitaph::Explicit(Err(s)) => s == other,
            Epitaph::PeerClosed => *other == zx_status::Status::PEER_CLOSED,
        }
    }
}

impl PartialEq<Epitaph> for zx_status::Status {
    fn eq(&self, other: &Epitaph) -> bool {
        other.eq(self)
    }
}

impl PartialEq<std::result::Result<(), zx_status::Status>> for Epitaph {
    fn eq(&self, other: &std::result::Result<(), zx_status::Status>) -> bool {
        let res: std::result::Result<(), zx_status::Status> = (*self).into();
        &res == other
    }
}

impl PartialEq<Epitaph> for std::result::Result<(), zx_status::Status> {
    fn eq(&self, other: &Epitaph) -> bool {
        other.eq(self)
    }
}

impl From<Epitaph> for std::result::Result<(), zx_status::Status> {
    fn from(epitaph: Epitaph) -> Self {
        match epitaph {
            Epitaph::Explicit(res) => res,
            Epitaph::PeerClosed => Err(zx_status::Status::PEER_CLOSED),
        }
    }
}

impl From<&Epitaph> for std::result::Result<(), zx_status::Status> {
    fn from(epitaph: &Epitaph) -> Self {
        (*epitaph).into()
    }
}

impl From<zx_status::Status> for Epitaph {
    fn from(status: zx_status::Status) -> Self {
        if zx_status::Status::ok(status.into_raw()).is_ok() {
            Epitaph::Explicit(Ok(()))
        } else {
            Epitaph::Explicit(Err(status))
        }
    }
}

impl From<std::result::Result<(), zx_status::Status>> for Epitaph {
    fn from(res: std::result::Result<(), zx_status::Status>) -> Self {
        Epitaph::Explicit(res)
    }
}

/// The error type used by FIDL operations, as `src/lib/fidl/rust/fidl/src/error.rs`'s, with only
/// the errors a proxy calling a server in-process can have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The other end of the connection was closed: nothing serves the protocol.
    ClientChannelClosed {
        /// The epitaph or `Epitaph::PeerClosed`.
        epitaph: Epitaph,
        /// The name of the protocol at the other end of the channel.
        protocol_name: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ClientChannelClosed { epitaph, protocol_name } => {
                write!(
                    f,
                    "A FIDL client's channel to the protocol {protocol_name} was closed: {epitaph}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::ClientChannelClosed { epitaph, .. } => Some(epitaph),
        }
    }
}

/// The server end of a connection whose protocol is not known statically, as a channel is. The
/// server that serves it decides the protocol, as a server of `fuchsia.io/Directory.Open` does.
///
/// Dropping it unserved closes the connection, as dropping a channel does.
pub struct Channel(endpoints::Slot);

/// From `src/lib/fidl/rust/fidl/src/endpoints.rs`, with connections in-process: a proxy and its
/// server end share a slot the server end is served with or closed on, and sends events to.
pub mod endpoints {
    use super::{Channel, Epitaph, Error, Status};
    use std::any::Any;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

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

    /// What is at the server end of a connection.
    enum State {
        /// Nothing yet: calls wait, as messages in a channel do.
        Pending,
        /// The object serving the connection, an `Arc<M::Server>` for the protocol `M` it is
        /// served with, whose name is given.
        Served(Arc<dyn Any + Send + Sync>, &'static str),
        /// Closed, with its epitaph.
        Closed(Epitaph),
    }

    /// A one-way call, which is delivered to the server, an `Arc<M::Server>`, once it is served.
    type OneWayCall = Box<dyn FnOnce(&(dyn Any + Send + Sync)) + Send>;

    /// A connection's shared state, which stands in for a channel.
    struct Inner {
        state: State,
        /// One-way calls made before the connection was served, in order, as messages wait in a
        /// channel.
        calls: Vec<OneWayCall>,
        /// Events the server sent that the client hasn't read.
        events: VecDeque<Box<dyn Any + Send>>,
        /// The tasks waiting for the connection to be served or closed, or for an event.
        wakers: Vec<Waker>,
    }

    struct Shared(Mutex<Inner>);

    impl Shared {
        fn new(state: State) -> Arc<Self> {
            Arc::new(Self(Mutex::new(Inner {
                state,
                calls: Vec::new(),
                events: VecDeque::new(),
                wakers: Vec::new(),
            })))
        }

        fn update(&self, f: impl FnOnce(&mut Inner)) {
            let wakers = {
                let mut inner = self.0.lock().unwrap();
                f(&mut inner);
                std::mem::take(&mut inner.wakers)
            };
            wakers.into_iter().for_each(Waker::wake);
        }

        /// Serves or closes the connection. A server first gets the one-way calls made while the
        /// connection was pending, in order: calls made meanwhile queue behind them. The calls on
        /// a connection closed while pending are dropped, as messages in a closed channel are.
        fn set(&self, state: State) {
            let server = match &state {
                State::Served(server, _) => Some(server.clone()),
                State::Pending | State::Closed(_) => None,
            };
            let mut state = Some(state);
            loop {
                let mut inner = self.0.lock().unwrap();
                assert!(matches!(inner.state, State::Pending), "a server end was used twice");
                let calls = std::mem::take(&mut inner.calls);
                match &server {
                    Some(server) if !calls.is_empty() => {
                        drop(inner);
                        calls.into_iter().for_each(|call| call(&**server));
                    }
                    _ => {
                        inner.state = state.take().unwrap();
                        let wakers = std::mem::take(&mut inner.wakers);
                        drop(inner);
                        drop(calls);
                        wakers.into_iter().for_each(Waker::wake);
                        return;
                    }
                }
            }
        }
    }

    /// The server end's hold on a connection, which closes it if dropped unused.
    impl Shared {
        /// Closes a served connection, as a server closes its channel.
        fn close(&self) {
            self.update(|inner| {
                assert!(matches!(inner.state, State::Served(..)), "closing an unserved connection");
                inner.state = State::Closed(Epitaph::PeerClosed);
            });
        }
    }

    pub(crate) struct Slot(Option<Arc<Shared>>);

    impl Slot {
        fn take(&mut self) -> Arc<Shared> {
            self.0.take().unwrap()
        }
    }

    impl Drop for Slot {
        fn drop(&mut self) {
            if let Some(shared) = self.0.take() {
                shared.set(State::Closed(Epitaph::PeerClosed));
            }
        }
    }

    /// A proxy's end of a connection whose protocol is not known statically, as a proxy's channel
    /// (`fuchsia_async::Channel`) is.
    pub struct AsyncChannel(Arc<Shared>);

    /// A type which allows querying a remote FIDL server over a channel.
    pub trait Proxy: Sized + Send + Sync {
        /// The protocol which this `Proxy` controls.
        type Protocol: ProtocolMarker;

        /// Create a proxy over the given channel.
        fn from_channel(inner: AsyncChannel) -> Self;

        /// Attempt to convert the proxy back into a channel.
        fn into_channel(self) -> Result<AsyncChannel, Self>;
    }

    /// The connection a proxy and a server end share, which stands in for the channel.
    pub struct Connection<M: ProtocolMarker> {
        shared: Arc<Shared>,
        marker: std::marker::PhantomData<M>,
    }

    impl<M: ProtocolMarker> Clone for Connection<M> {
        fn clone(&self) -> Self {
            Self { shared: self.shared.clone(), marker: std::marker::PhantomData }
        }
    }

    impl<M: ProtocolMarker> std::fmt::Debug for Connection<M> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("Connection").field(&M::DEBUG_NAME).finish()
        }
    }

    impl<M: ProtocolMarker> Connection<M> {
        fn new(state: State) -> Self {
            Self { shared: Shared::new(state), marker: std::marker::PhantomData }
        }

        /// The connection over `channel`, whose protocol the caller knows is `M`'s.
        pub fn from_channel(channel: AsyncChannel) -> Self {
            Self { shared: channel.0, marker: std::marker::PhantomData }
        }

        /// The channel of the connection.
        pub fn into_channel(self) -> AsyncChannel {
            AsyncChannel(self.shared)
        }

        /// A connection served by `server`.
        pub fn served_by(server: Arc<M::Server>) -> Self {
            Self::new(State::Served(Arc::new(server), M::DEBUG_NAME))
        }

        /// The object serving the connection, once it is served, or the error a call on a closed
        /// channel has.
        ///
        /// # Panics
        ///
        /// If the connection is served with another protocol, whose messages a real channel would
        /// fail to decode.
        pub async fn server(&self) -> Result<Arc<M::Server>, Error> {
            std::future::poll_fn(|cx| match self.poll_server(Some(cx.waker())) {
                Some(result) => Poll::Ready(result),
                None => Poll::Pending,
            })
            .await
        }

        /// Makes a one-way call: delivers it to the server once the connection is served, after the
        /// one-way calls made before it. A call on a closed connection is dropped, as a message
        /// written to a closed channel is.
        pub fn send(&self, call: impl FnOnce(Arc<M::Server>) + Send + 'static) {
            let call: OneWayCall =
                Box::new(move |server| {
                    call(server.downcast_ref::<Arc<M::Server>>().cloned().unwrap_or_else(|| {
                        panic!("{} is served with another protocol", M::DEBUG_NAME)
                    }))
                });
            let server = {
                let mut inner = self.shared.0.lock().unwrap();
                match &inner.state {
                    State::Pending => {
                        inner.calls.push(call);
                        return;
                    }
                    State::Served(server, _) => server.clone(),
                    State::Closed(_) => return,
                }
            };
            call(&*server);
        }

        /// As `server`, but `None` if the connection is not served yet.
        pub fn try_server(&self) -> Option<Result<Arc<M::Server>, Error>> {
            self.poll_server(None)
        }

        fn poll_server(&self, waker: Option<&Waker>) -> Option<Result<Arc<M::Server>, Error>> {
            let mut inner = self.shared.0.lock().unwrap();
            match &inner.state {
                State::Pending => {
                    inner.wakers.extend(waker.cloned());
                    None
                }
                State::Served(server, protocol_name) => {
                    Some(Ok(server.downcast_ref::<Arc<M::Server>>().cloned().unwrap_or_else(
                        || panic!("{} is served with {protocol_name}", M::DEBUG_NAME),
                    )))
                }
                State::Closed(epitaph) => Some(Err(self.closed(*epitaph))),
            }
        }

        /// The next event the server sent, or the error reading from a closed channel has once
        /// the events sent before it closed are read.
        ///
        /// # Panics
        ///
        /// If the event isn't an `E`, which a real channel would fail to decode.
        pub fn poll_event<E: 'static>(&self, cx: &mut Context<'_>) -> Poll<Result<E, Error>> {
            let mut inner = self.shared.0.lock().unwrap();
            if let Some(event) = inner.events.pop_front() {
                return Poll::Ready(Ok(*event.downcast::<E>().unwrap_or_else(|_| {
                    panic!("{} sent an event of another type", M::DEBUG_NAME)
                })));
            }
            if let State::Closed(epitaph) = inner.state {
                return Poll::Ready(Err(self.closed(epitaph)));
            }
            inner.wakers.push(cx.waker().clone());
            Poll::Pending
        }

        fn closed(&self, epitaph: Epitaph) -> Error {
            Error::ClientChannelClosed { epitaph, protocol_name: M::DEBUG_NAME }
        }
    }

    /// The server end of a FIDL connection.
    pub struct ServerEnd<M: ProtocolMarker> {
        slot: Slot,
        marker: std::marker::PhantomData<M>,
    }

    impl<M: ProtocolMarker> ServerEnd<M> {
        /// Serves the connection with `server`.
        pub fn serve(mut self, server: Arc<M::Server>) {
            self.slot.take().set(State::Served(Arc::new(server), M::DEBUG_NAME));
        }

        /// Serves the connection with `server` until the returned `Serving` is dropped, which
        /// closes it, as dropping the task serving a connection's request stream closes its
        /// channel.
        pub fn serve_until_dropped(mut self, server: Arc<M::Server>) -> Serving {
            let shared = self.slot.take();
            shared.set(State::Served(Arc::new(server), M::DEBUG_NAME));
            Serving(shared)
        }

        /// Sends an event to the client, which it reads before any epitaph.
        pub fn send_event<E: Any + Send>(&self, event: E) {
            self.slot.0.as_ref().unwrap().update(|inner| inner.events.push_back(Box::new(event)));
        }

        /// Writes an epitaph into the underlying channel before closing it.
        pub fn close_with_epitaph(
            self,
            status: impl Into<Result<(), Status>>,
        ) -> Result<(), Error> {
            self.into_channel().close_with_epitaph(status)
        }

        /// Extract the inner channel.
        pub fn into_channel(self) -> Channel {
            Channel(self.slot)
        }
    }

    /// A connection being served, which is closed when this is dropped (see
    /// `ServerEnd::serve_until_dropped`).
    #[must_use = "the connection is closed when this is dropped"]
    pub struct Serving(Arc<Shared>);

    impl Drop for Serving {
        fn drop(&mut self) {
            self.0.close();
        }
    }

    impl<M: ProtocolMarker> From<ServerEnd<M>> for Channel {
        fn from(server_end: ServerEnd<M>) -> Self {
            server_end.into_channel()
        }
    }

    impl<M: ProtocolMarker> From<Channel> for ServerEnd<M> {
        fn from(channel: Channel) -> Self {
            ServerEnd { slot: channel.0, marker: std::marker::PhantomData }
        }
    }

    impl Channel {
        /// Writes an epitaph with `status` and closes the channel.
        pub fn close_with_epitaph(
            mut self,
            status: impl Into<Result<(), Status>>,
        ) -> Result<(), Error> {
            self.0.take().set(State::Closed(Epitaph::Explicit(status.into())));
            Ok(())
        }
    }

    fn create<M: ProtocolMarker>() -> (Connection<M>, ServerEnd<M>) {
        let connection = Connection::new(State::Pending);
        let server_end = ServerEnd {
            slot: Slot(Some(connection.shared.clone())),
            marker: std::marker::PhantomData,
        };
        (connection, server_end)
    }

    /// Create a client proxy and a server endpoint connected to it.
    pub fn create_proxy<M: ProtocolMarker>() -> (M::Proxy, ServerEnd<M>) {
        let (connection, server_end) = create();
        (connection.into(), server_end)
    }

    /// Create a synchronous client proxy and a server endpoint connected to it.
    pub fn create_sync_proxy<M: ProtocolMarker>() -> (M::SynchronousProxy, ServerEnd<M>) {
        let (connection, server_end) = create();
        (connection.into(), server_end)
    }
}

#[cfg(test)]
mod tests {
    use super::endpoints::{Connection, ProtocolMarker, create_proxy};
    use super::{Epitaph, Error};
    use std::sync::Arc;

    trait Echo: Send + Sync {
        fn echo(&self, value: u32) -> u32;
    }

    struct EchoMarker;

    impl ProtocolMarker for EchoMarker {
        type Proxy = Connection<EchoMarker>;
        type SynchronousProxy = Connection<EchoMarker>;
        type Server = dyn Echo;
        const DEBUG_NAME: &'static str = "test.Echo";
    }

    struct EchoServer;

    impl Echo for EchoServer {
        fn echo(&self, value: u32) -> u32 {
            value
        }
    }

    #[test]
    fn a_connection_served_until_dropped_closes_when_dropped() {
        let (proxy, server_end) = create_proxy::<EchoMarker>();
        let serving = server_end.serve_until_dropped(Arc::new(EchoServer) as Arc<dyn Echo>);
        let server = futures::executor::block_on(proxy.server()).expect("served");
        assert_eq!(server.echo(7), 7);

        drop(serving);
        assert!(matches!(
            futures::executor::block_on(proxy.server()),
            Err(Error::ClientChannelClosed { epitaph: Epitaph::PeerClosed, .. })
        ));
    }
}

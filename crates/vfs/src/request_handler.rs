// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Requests from in-process proxies (see the `fidl` stand-in), for the vendored `RequestListener`.
//
// Upstream, a connection is a `RequestListener` task in its scope, which reads requests from the
// connection's channel and hands them to the connection, one at a time, until the connection
// breaks off, the client closes the channel (calling `RequestHandler::stream_closed`), or the scope
// shuts down. Here, the server that proxies call sends each call to the listener as a request, a
// closure run on the connection, over a queue that stands in for the channel: requests are handled
// in order, and the queue closes when every proxy, and so the server, is dropped.

#[path = "../../../vendor/fuchsia/src/storage/lib/vfs/rust/src/request_handler.rs"]
mod upstream;

pub use upstream::{RequestHandler, RequestListener};

use crate::execution_scope::ExecutionScope;
use futures::FutureExt;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use std::ops::ControlFlow;
use std::pin::Pin;
use zx_status::Status;

/// A request to a connection of type `C`: what handling it does, and whether the connection goes
/// on handling requests afterwards.
pub type Request<C> =
    Box<dyn for<'a> FnOnce(Pin<&'a mut C>) -> BoxFuture<'a, ControlFlow<()>> + Send>;

/// Sends requests to a connection's listener.
pub struct Requests<C>(mpsc::UnboundedSender<Request<C>>);

impl<C> Requests<C>
where
    C: RequestHandler<Request = Request<C>> + Send + 'static,
{
    /// Spawns a listener for `connection` in `scope`.
    pub fn spawn(scope: &ExecutionScope, connection: C) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        scope.spawn(RequestListener::new(receiver, connection));
        Self(sender)
    }

    /// Sends a request, failing as a closed channel would if the listener has finished.
    pub fn send(
        &self,
        request: impl for<'a> FnOnce(Pin<&'a mut C>) -> BoxFuture<'a, ControlFlow<()>> + Send + 'static,
    ) -> Result<(), Status> {
        self.0.unbounded_send(Box::new(request)).map_err(|_| Status::PEER_CLOSED)
    }

    /// Sends a request with a response, after which the connection goes on handling requests
    /// unless `request` returns `ControlFlow::Break`.
    pub async fn call<R: Send + 'static>(
        &self,
        request: impl for<'a> FnOnce(Pin<&'a mut C>) -> BoxFuture<'a, (R, ControlFlow<()>)>
        + Send
        + 'static,
    ) -> Result<R, Status> {
        let (responder, response) = oneshot::channel();
        self.send(move |connection| {
            async move {
                let (result, control_flow) = request(connection).await;
                let _ = responder.send(result);
                control_flow
            }
            .boxed()
        })?;
        // The request is dropped unhandled if the listener finishes first.
        response.await.map_err(|_| Status::PEER_CLOSED)
    }
}

/// Handles `request` on `connection` if `scope` is active, as upstream's request handlers do.
pub fn handle<'a, C: Send + 'a>(
    scope: &ExecutionScope,
    connection: Pin<&'a mut C>,
    request: Request<C>,
) -> BoxFuture<'a, ControlFlow<()>> {
    match scope.try_active_guard() {
        Some(guard) => async move {
            let control_flow = request(connection).await;
            std::mem::drop(guard);
            control_flow
        }
        .boxed(),
        None => async { ControlFlow::Break(()) }.boxed(),
    }
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Receiving packets from a port.
//!
//! Ported from the interface of `PacketReceiver`, `ReceiverRegistration` and `EHandle` in
//! `src/lib/fuchsia-async/src/runtime/fuchsia/executor/packets.rs` and
//! `src/lib/fuchsia-async/src/runtime/fuchsia/executor/common.rs`.
//!
//! Deviations:
//! - There is one port for the process rather than one per executor, and a thread of its own
//!   waits on it and calls the receivers, rather than the executor's threads.
//! - An `EHandle` is a handle to the tokio runtime the portable executor runs on, which tasks are
//!   spawned onto, as there is no root scope.
//! - Registrations hold their receiver in an `Arc` shared with the receiving thread, rather than
//!   deferring its deallocation to an epoch.
//! - No raw or pinned registrations.
//! - A panic in a receiver aborts the process, as panics do on Fuchsia, rather than ending the
//!   thread every receiver's packets arrive on.

use fuchsia_sync::RwLock;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::ops::Deref;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

/// A trait for handling the arrival of a packet on a `zx::Port`.
///
/// This trait should be implemented by users who wish to write their own
/// types which receive asynchronous notifications from a `zx::Port`.
/// Implementors of this trait generally contain a `futures::task::AtomicWaker` which
/// is used to wake up the task which can make progress due to the arrival of
/// the packet.
///
/// `PacketReceiver`s should be registered with a `Core` using the
/// `register_receiver` method on `Core`, `Handle`, or `Remote`.
/// Upon registration, users will receive a `ReceiverRegistration`
/// which provides `key` and `port` methods. These methods can be used to wait on
/// asynchronous signals.
///
/// Note that `PacketReceiver`s may receive false notifications intended for a
/// previous receiver, and should handle these gracefully.
pub trait PacketReceiver: Send + Sync + 'static {
    /// Receive a packet when one arrives.
    fn receive_packet(&self, packet: zx::Packet);
}

impl<T: PacketReceiver> PacketReceiver for Arc<T> {
    fn receive_packet(&self, packet: zx::Packet) {
        self.as_ref().receive_packet(packet);
    }
}

/// The process's port, and the receivers of the packets queued on it.
struct Receivers {
    port: zx::Port,
    receivers: RwLock<HashMap<u64, Arc<dyn PacketReceiver>>>,
    /// Keys are not reused, so that packets for a deregistered receiver are not received by a
    /// later one.
    next_key: AtomicU64,
}

impl Receivers {
    fn register(&self, receiver: Arc<dyn PacketReceiver>) -> u64 {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        self.receivers.write().insert(key, receiver);
        key
    }

    fn deregister(&self, key: u64) {
        self.receivers.write().remove(&key);
    }

    /// Waits for packets and hands each to its receiver, forever.
    fn receive_packets(&self) -> ! {
        loop {
            let packet = match self.port.wait(zx::MonotonicInstant::INFINITE) {
                Ok(packet) => packet,
                Err(status) => panic!("waiting on the executor's port failed: {status}"),
            };
            let receiver = self.receivers.read().get(&packet.key()).cloned();
            // Packets for a receiver that has been deregistered are dropped.
            if let Some(receiver) = receiver {
                // The panic hook has reported the panic by the time it unwinds to here.
                let received = panic::catch_unwind(AssertUnwindSafe(|| {
                    receiver.receive_packet(packet);
                }));
                if received.is_err() {
                    std::process::abort();
                }
            }
        }
    }
}

static RECEIVERS: LazyLock<&'static Receivers> = LazyLock::new(|| {
    let receivers: &'static Receivers = Box::leak(Box::new(Receivers {
        port: zx::Port::create(),
        receivers: RwLock::new(HashMap::new()),
        next_key: AtomicU64::new(0),
    }));
    std::thread::Builder::new()
        .name("port-receiver".into())
        .spawn(|| receivers.receive_packets())
        .expect("create the port receiving thread");
    receivers
});

/// A handle to an executor.
#[derive(Clone)]
pub struct EHandle {
    runtime: tokio::runtime::Handle,
}

impl fmt::Debug for EHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EHandle").finish_non_exhaustive()
    }
}

impl EHandle {
    /// Returns the thread-local executor.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn local() -> Self {
        let runtime = tokio::runtime::Handle::try_current()
            .expect("Fuchsia Executor must be created first");
        EHandle { runtime }
    }

    /// Get a reference to the Fuchsia `zx::Port` being used to listen for events.
    pub fn port(&self) -> &zx::Port {
        &RECEIVERS.port
    }

    /// Registers a `PacketReceiver` with the executor and returns a registration.
    /// The `PacketReceiver` will be deregistered when the `Registration` is dropped.
    pub fn register_receiver<T: PacketReceiver>(&self, receiver: T) -> ReceiverRegistration<T> {
        let receiver = Arc::new(receiver);
        let key = RECEIVERS.register(receiver.clone());
        ReceiverRegistration { key, receiver }
    }

    /// Spawn a new task to be run on this executor.
    ///
    /// Tasks spawned using this method must be thread-safe (implement the `Send` trait), as they
    /// may be run on either a singlethreaded or multithreaded executor.
    pub fn spawn_detached(&self, future: impl Future<Output = ()> + Send + 'static) {
        drop(self.runtime.spawn(future));
    }
}

/// A registration of a `PacketReceiver`.
///
/// When dropped, it will automatically deregister the `PacketReceiver`.
// NOTE: purposefully does not implement `Clone`.
pub struct ReceiverRegistration<T: Send + 'static> {
    key: u64,
    receiver: Arc<T>,
}

impl<T: Send + 'static> fmt::Debug for ReceiverRegistration<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiverRegistration").field("key", &self.key).finish_non_exhaustive()
    }
}

impl<T: Send + 'static> Drop for ReceiverRegistration<T> {
    fn drop(&mut self) {
        RECEIVERS.deregister(self.key);
    }
}

impl<T: Send + 'static> ReceiverRegistration<T> {
    /// The key with which `Packet`s destined for this receiver should be sent on the `zx::Port`.
    pub fn key(&self) -> u64 {
        self.key
    }

    /// The internal `PacketReceiver`.
    pub fn receiver(&self) -> &T {
        &self.receiver
    }

    /// The `zx::Port` on which packets destined for this `PacketReceiver` should be queued.
    pub fn port(&self) -> &zx::Port {
        &RECEIVERS.port
    }
}

impl<T: Send + 'static> Deref for ReceiverRegistration<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.receiver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use futures::channel::mpsc;

    struct Sender(mpsc::UnboundedSender<zx::Packet>);

    impl PacketReceiver for Sender {
        fn receive_packet(&self, packet: zx::Packet) {
            self.0.unbounded_send(packet).unwrap();
        }
    }

    fn user_packet(key: u64, value: u8) -> zx::Packet {
        let mut payload = [0u8; 32];
        payload[0] = value;
        zx::Packet::from_user_packet(key, 0, zx::UserPacket::from_u8_array(payload))
    }

    #[fuchsia::test]
    async fn packets_reach_their_receivers() {
        let (sender_a, mut receiver_a) = mpsc::unbounded();
        let (sender_b, mut receiver_b) = mpsc::unbounded();
        let ehandle = EHandle::local();
        let a = ehandle.register_receiver(Sender(sender_a));
        let b = ehandle.register_receiver(Sender(sender_b));
        assert_ne!(a.key(), b.key());

        ehandle.port().queue(&user_packet(b.key(), 2)).unwrap();
        ehandle.port().queue(&user_packet(a.key(), 1)).unwrap();

        let packet = receiver_a.next().await.unwrap();
        assert_eq!(packet.key(), a.key());
        let zx::PacketContents::User(user) = packet.contents() else { panic!("not a user packet") };
        assert_eq!(user.as_u8_array()[0], 1);
        let packet = receiver_b.next().await.unwrap();
        assert_eq!(packet.key(), b.key());
    }

    #[fuchsia::test]
    async fn deregistered_receivers_receive_nothing() {
        let (sender, mut receiver) = mpsc::unbounded();
        let ehandle = EHandle::local();
        let registration = ehandle.register_receiver(Sender(sender));
        let key = registration.key();
        drop(registration);

        ehandle.port().queue(&user_packet(key, 1)).unwrap();
        // The receiver was dropped with its registration, so its sender is gone.
        assert!(receiver.next().await.is_none());
    }

    struct Panicker;

    impl PacketReceiver for Panicker {
        fn receive_packet(&self, _packet: zx::Packet) {
            panic!("receiver panicked");
        }
    }

    /// Set in the process `panicking_receivers_abort` runs `abort_on_receiver_panic` in.
    const ABORT_CHILD: &str = "FUCHSIA_ASYNC_KERNEL_ABORT_CHILD";

    /// Queues a packet for a receiver that panics, which must abort the process. Only does so in
    /// the process `panicking_receivers_abort` starts.
    #[fuchsia::test]
    async fn abort_on_receiver_panic() {
        if std::env::var_os(ABORT_CHILD).is_none() {
            return;
        }
        let ehandle = EHandle::local();
        let registration = ehandle.register_receiver(Panicker);
        ehandle.port().queue(&user_packet(registration.key(), 1)).unwrap();
        std::future::pending::<()>().await;
    }

    #[test]
    fn panicking_receivers_abort() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "packets::tests::abort_on_receiver_panic", "--nocapture"])
            .env(ABORT_CHILD, "1")
            .output()
            .unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(output.status.signal(), Some(libc::SIGABRT));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("receiver panicked"), "stderr: {stderr}");
    }

    #[fuchsia::test]
    async fn spawned_tasks_run() {
        let (sender, mut receiver) = mpsc::unbounded();
        EHandle::local().spawn_detached(async move { sender.unbounded_send(()).unwrap() });
        assert_eq!(receiver.next().await, Some(()));
    }
}

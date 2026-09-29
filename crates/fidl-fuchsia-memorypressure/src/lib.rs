//! The `fuchsia.memorypressure` library, shaped like FIDL's generated Rust bindings
//! (`sdk/fidl/fuchsia.memorypressure`).
//!
//! The `Provider` and `Watcher` protocols are served in-process (see the `fidl` stand-in's
//! `endpoints`): their proxies call a `ProviderServer` or `WatcherServer`. Deviation: as the `fidl`
//! stand-in has no client ends, `Provider.RegisterWatcher` takes the watcher's proxy.

use fidl::endpoints::{AsyncChannel, Connection, ProtocolMarker, Proxy};
use futures::future::BoxFuture;

/// `fuchsia.memorypressure/Level` (strict enum : uint32): the memory pressure level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// The memory pressure level is healthy.
    Normal,
    /// The memory pressure level is somewhat constrained, and might cross over to the critical
    /// pressure range if left unchecked.
    Warning,
    /// The memory pressure level is very constrained.
    Critical,
}

impl Level {
    pub fn from_primitive(prim: u32) -> Option<Self> {
        match prim {
            1 => Some(Self::Normal),
            2 => Some(Self::Warning),
            3 => Some(Self::Critical),
            _ => None,
        }
    }

    pub const fn into_primitive(self) -> u32 {
        match self {
            Self::Normal => 1,
            Self::Warning => 2,
            Self::Critical => 3,
        }
    }
}

/// Serves `fuchsia.memorypressure/Provider`.
pub trait ProviderServer: Send + Sync {
    /// Registers `watcher` for memory pressure level changes, sending it the current level.
    fn register_watcher(&self, watcher: WatcherProxy);
}

/// The `fuchsia.memorypressure/Provider` protocol.
pub struct ProviderMarker;

impl ProtocolMarker for ProviderMarker {
    type Proxy = ProviderProxy;
    type SynchronousProxy = ProviderProxy;
    type Server = dyn ProviderServer;
    const DEBUG_NAME: &'static str = "fuchsia.memorypressure.Provider";
}

/// A client of `fuchsia.memorypressure/Provider`.
#[derive(Clone, Debug)]
pub struct ProviderProxy(Connection<ProviderMarker>);

impl From<Connection<ProviderMarker>> for ProviderProxy {
    fn from(connection: Connection<ProviderMarker>) -> Self {
        Self(connection)
    }
}

impl Proxy for ProviderProxy {
    type Protocol = ProviderMarker;

    fn from_channel(inner: AsyncChannel) -> Self {
        Self(Connection::from_channel(inner))
    }

    fn into_channel(self) -> Result<AsyncChannel, Self> {
        Ok(self.0.into_channel())
    }
}

impl ProviderProxy {
    pub fn register_watcher(&self, watcher: WatcherProxy) -> Result<(), fidl::Error> {
        self.0.send(move |server| server.register_watcher(watcher));
        Ok(())
    }
}

/// Serves `fuchsia.memorypressure/Watcher`.
pub trait WatcherServer: Send + Sync {
    /// Handles a level change, replying once the watcher has acted on it.
    fn on_level_changed(&self, level: Level) -> BoxFuture<'_, ()>;
}

/// The `fuchsia.memorypressure/Watcher` protocol.
pub struct WatcherMarker;

impl ProtocolMarker for WatcherMarker {
    type Proxy = WatcherProxy;
    type SynchronousProxy = WatcherProxy;
    type Server = dyn WatcherServer;
    const DEBUG_NAME: &'static str = "fuchsia.memorypressure.Watcher";
}

/// A client of `fuchsia.memorypressure/Watcher`.
#[derive(Clone, Debug)]
pub struct WatcherProxy(Connection<WatcherMarker>);

impl From<Connection<WatcherMarker>> for WatcherProxy {
    fn from(connection: Connection<WatcherMarker>) -> Self {
        Self(connection)
    }
}

impl Proxy for WatcherProxy {
    type Protocol = WatcherMarker;

    fn from_channel(inner: AsyncChannel) -> Self {
        Self(Connection::from_channel(inner))
    }

    fn into_channel(self) -> Result<AsyncChannel, Self> {
        Ok(self.0.into_channel())
    }
}

impl WatcherProxy {
    pub async fn on_level_changed(&self, level: Level) -> Result<(), fidl::Error> {
        self.0.server().await?.on_level_changed(level).await;
        Ok(())
    }
}

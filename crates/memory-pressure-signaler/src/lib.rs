// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia's memory pressure signaler (`src/developer/memory/pressure_signaler`): the system
//! component that watches the kernel's memory pressure events and serves
//! `fuchsia.memorypressure/Provider`, through which Fxfs's volumes follow memory pressure.
//!
//! Here it runs in-process, as the kernel does: [`provider`] connects to the process's signaler,
//! which starts on first use and lasts as long as the process, as the component does the system.

mod notifier;
mod observer;

pub use observer::Level;

use fidl::endpoints::create_proxy;
use fidl_fuchsia_memorypressure::{ProviderMarker, ProviderProxy};
use std::sync::OnceLock;

/// Connects to the process's memory pressure signaler, starting it if need be.
pub fn provider() -> ProviderProxy {
    static PROVIDER: OnceLock<ProviderProxy> = OnceLock::new();
    PROVIDER
        .get_or_init(|| {
            let (proxy, server_end) = create_proxy::<ProviderMarker>();
            notifier::start(server_end);
            proxy
        })
        .clone()
}

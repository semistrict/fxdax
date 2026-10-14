// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/lib/fuchsia-async/src/lib.rs`, for the host: without the exports of Fuchsia's
// handles and instrumentation.

//! A futures-rs executor designed specifically for Fuchsia.
//!
//! # Example:
//! A simple, singlethreaded print server:
//!
//! ```no_run
//! #[fuchsia_async::run_singlethreaded]
//! async fn main() {
//!     let op = say_world();
//!
//!     // This println! will happen first
//!     println!("Hello...");
//!
//!     // Calling `.await` on `op` starts executing `say_world`.
//!     op.await;
//! }
//!
//!
//! async fn say_world() {
//!     println!("...world");
//! }
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]
#![deny(clippy::await_holding_lock)]
#![deny(clippy::await_holding_refcell_ref)]

mod runtime;
pub use self::runtime::*;

mod handle;
pub use self::handle::channel::{Channel, RecvMsg};
pub use self::handle::on_signals::OnSignalsRef;
pub use self::handle::socket::Socket;

/// Asynchronous networking abstractions.
#[cfg(not(target_os = "none"))]
pub mod net;

pub use fuchsia_async_macro::{run, run_singlethreaded, run_until_stalled};

pub mod condition;
/// Testing support for repeated runs
// fxdax: its threads aren't the guest kernel's (TASK-26.6).
#[cfg(not(target_os = "none"))]
pub mod test_support;

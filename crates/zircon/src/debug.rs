// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Stand-ins for the kernel's `debug` crate.
//!
//! As in Zircon, tracing is compiled in per file: the macros print when the
//! calling module's `LOCAL_TRACE` is at least the requested level.

macro_rules! ltracef {
    ($($arg:tt)*) => {
        $crate::debug::ltracef_level!(1, $($arg)*)
    };
}
pub(crate) use ltracef;

macro_rules! ltracef_level {
    ($level:expr, $($arg:tt)*) => {
        if LOCAL_TRACE >= $level {
            eprint!("{}: {}", module_path!(), format_args!($($arg)*));
        }
    };
}
pub(crate) use ltracef_level;

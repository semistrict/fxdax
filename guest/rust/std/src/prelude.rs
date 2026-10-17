// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! std's preludes: `core`'s, and what `alloc` adds.

macro_rules! prelude {
    ($edition:ident) => {
        pub mod $edition {
            pub use alloc_crate::borrow::ToOwned;
            pub use alloc_crate::boxed::Box;
            pub use alloc_crate::string::{String, ToString};
            pub use alloc_crate::vec::Vec;
            pub use core::prelude::$edition::*;
            // std's macros, which crates use without importing them.
            pub use crate::{dbg, eprint, eprintln, print, println, thread_local};
            pub use alloc_crate::{format, vec};
        }
    };
}

prelude!(v1);
prelude!(rust_2015);
prelude!(rust_2018);
prelude!(rust_2021);
prelude!(rust_2024);

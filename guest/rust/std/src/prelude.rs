//! std's preludes: `core`'s, and what `alloc` adds.

macro_rules! prelude {
    ($edition:ident) => {
        pub mod $edition {
            pub use alloc_crate::borrow::ToOwned;
            pub use alloc_crate::boxed::Box;
            pub use alloc_crate::string::{String, ToString};
            pub use alloc_crate::vec::Vec;
            pub use core::prelude::$edition::*;
        }
    };
}

prelude!(v1);
prelude!(rust_2015);
prelude!(rust_2018);
prelude!(rust_2021);
prelude!(rust_2024);

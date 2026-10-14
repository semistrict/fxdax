//! What is particular to the kernel the guest's std runs in, as `std::os` has for each platform.

pub mod kernel {
    //! The kernel's own: futexes, and the locks std's are, for crates that make locks of their
    //! own, such as with `lock_api`.

    pub mod futex {
        //! Futexes over the kernel's wait queues for addresses.
        pub use crate::sys::sync::futex::{Futex, futex_wait, futex_wake, futex_wake_all};
    }

    pub mod sync {
        //! std's raw locks, over futexes: `lock` and `unlock` without guards.
        pub use crate::sys::sync::{Mutex, RwLock};
    }
}

//! Threads, which the kernel's Rust does not start: futures run on the kernel's workers instead.
//! Parking spins, as a spurious wakeup may: milestone 1's sleeps.

use crate::io;
use crate::time::{Duration, Instant};
use alloc_crate::boxed::Box;
use alloc_crate::string::String;

pub use crate::thread_local;
pub use local::{AccessError, LocalKey};

/// Never: panics abort.
pub fn panicking() -> bool {
    false
}

/// Spins for `dur`.
pub fn sleep(dur: Duration) {
    let start = Instant::now();
    while start.elapsed() < dur {
        core::hint::spin_loop();
    }
}

/// Spins once.
pub fn yield_now() {
    core::hint::spin_loop();
}

/// Returns, as a spurious wakeup may.
pub fn park() {
    core::hint::spin_loop();
}

/// Returns, as a spurious wakeup may.
pub fn park_timeout(_dur: Duration) {
    core::hint::spin_loop();
}

/// The current thread, which can be unparked, to no effect.
#[derive(Clone, Debug)]
pub struct Thread(());

impl Thread {
    pub fn unpark(&self) {}

    pub fn name(&self) -> Option<&str> {
        None
    }
}

pub fn current() -> Thread {
    Thread(())
}

/// A thread's handle, which is never had.
pub struct JoinHandle<T>(core::marker::PhantomData<T>);

impl<T> JoinHandle<T> {
    pub fn join(self) -> Result<T, Box<dyn core::any::Any + Send + 'static>> {
        unreachable!("no thread was spawned")
    }

    pub fn thread(&self) -> &Thread {
        unreachable!("no thread was spawned")
    }
}

/// Panics: the kernel's Rust starts no threads.
pub fn spawn<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(_f: F) -> JoinHandle<T> {
    panic!("the kernel's Rust starts no threads")
}

/// Builds threads, which fails.
pub struct Builder(());

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self(())
    }

    pub fn name(self, _name: String) -> Self {
        self
    }

    pub fn stack_size(self, _size: usize) -> Self {
        self
    }

    pub fn spawn<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
        self,
        _f: F,
    ) -> io::Result<JoinHandle<T>> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

mod local {
    //! Thread-local values. The kernel has no thread-local storage that Rust can use, and its
    //! threads move between CPUs, so a value is shared by every thread: `thread_local!` asks for
    //! types that are `Sync`, and code that needs one per thread does not build.

    use core::fmt;

    /// A value `thread_local!` declares.
    pub struct LocalKey<T: 'static> {
        value: &'static T,
    }

    /// An error accessing a value, which never happens.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct AccessError;

    impl fmt::Display for AccessError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("already destroyed")
        }
    }

    impl<T: 'static> LocalKey<T> {
        #[doc(hidden)]
        pub const fn new(value: &'static T) -> Self {
            Self { value }
        }

        pub fn with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
            f(self.value)
        }

        pub fn try_with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> Result<R, AccessError> {
            Ok(f(self.value))
        }
    }
}

/// Declares values that, in the kernel, every thread shares (see `LocalKey`).
#[macro_export]
macro_rules! thread_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const { $init:expr }; $($rest:tt)*) => {
        $crate::thread_local!($(#[$attr])* $vis static $name: $t = $init; $($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block; $($rest:tt)*) => {
        $crate::thread_local!($(#[$attr])* $vis static $name: $t = $init; $($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => {
        $(#[$attr])* $vis static $name: $crate::thread::LocalKey<$t> = {
            static VALUE: $crate::sync::LazyLock<$t> = $crate::sync::LazyLock::new(|| $init);
            $crate::thread::LocalKey::new(&VALUE)
        };
        $crate::thread_local!($($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => {
        $crate::thread_local!($(#[$attr])* $vis static $name: $t = $init;);
    };
}

//! Threads, which the kernel's Rust does not start: futures run on the kernel's workers instead.
//! Parking spins, as a spurious wakeup may: milestone 1's sleeps.

use crate::io;
use crate::time::{Duration, Instant};
use alloc_crate::boxed::Box;
use alloc_crate::string::String;

pub use crate::thread_local;
pub use local::{AccessError, LocalKey};

/// A thread's result: what it returned, or what it panicked with.
pub type Result<T> = core::result::Result<T, Box<dyn core::any::Any + Send + 'static>>;

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
    pub fn join(self) -> Result<T> {
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
    //! Thread-local values: a thread's are in a block its kernel task points to (`fxfs_tls`, a
    //! field fxdax's guest kernel adds to `task_struct`), made as the thread first uses one, and
    //! dropped when the task exits. Interrupts have no thread of their own, so a value used there
    //! panics.

    use alloc_crate::boxed::Box;
    use alloc_crate::vec::Vec;
    use core::any::Any;
    use core::ffi::c_void;
    use core::fmt;

    /// A thread's values, by their keys' addresses. Each value is boxed, so that its address stays
    /// while others are added.
    type Block = Vec<(usize, Box<dyn Any>)>;

    /// A value `thread_local!` declares.
    pub struct LocalKey<T: 'static> {
        init: fn() -> T,
    }

    /// An error accessing a value, which never happens: values outlive their threads' code.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct AccessError;

    impl fmt::Display for AccessError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("already destroyed")
        }
    }

    /// The calling thread's block.
    fn block() -> *mut Block {
        // SAFETY: Returns the current task's slot, which only it uses, or null in an interrupt.
        let slot = unsafe { crate::sys::fxfs_rt_tls() };
        assert!(!slot.is_null(), "thread-local values are not for interrupts");
        // SAFETY: The slot holds null, or a block this module made.
        unsafe {
            if (*slot).is_null() {
                *slot = Box::into_raw(Box::new(Block::new())).cast::<c_void>();
            }
            (*slot).cast::<Block>()
        }
    }

    /// Drops a task's block, as the task exits.
    #[unsafe(no_mangle)]
    pub extern "C" fn fxfs_rust_tls_free(block: *mut c_void) {
        // SAFETY: The kernel passes the block `block` made, once, as its task exits.
        drop(unsafe { Box::from_raw(block.cast::<Block>()) });
    }

    impl<T: 'static> LocalKey<T> {
        #[doc(hidden)]
        pub const fn new(init: fn() -> T) -> Self {
            Self { init }
        }

        pub fn with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
            let key = self as *const Self as usize;
            let block = block();
            let find = |block: *mut Block| -> Option<*const T> {
                // SAFETY: Only this thread uses its block, and no reference into it is held while
                // others are added: values are found by pointer.
                unsafe { &*block }
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, value)| value.downcast_ref::<T>().unwrap() as *const T)
            };
            let value = match find(block) {
                Some(value) => value,
                None => {
                    // Initializing may use other values, which adds them to the block.
                    let value = Box::new((self.init)());
                    // SAFETY: As above.
                    let block = unsafe { &mut *block };
                    block.push((key, value));
                    block.last().unwrap().1.downcast_ref::<T>().unwrap() as *const T
                }
            };
            // SAFETY: The value is boxed, and lives until the thread exits.
            f(unsafe { &*value })
        }

        pub fn try_with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> Result<R, AccessError> {
            Ok(self.with(f))
        }
    }

    impl<T: 'static> LocalKey<core::cell::Cell<T>> {
        pub fn set(&'static self, value: T) {
            self.with(|cell| cell.set(value))
        }

        pub fn get(&'static self) -> T
        where
            T: Copy,
        {
            self.with(core::cell::Cell::get)
        }

        pub fn take(&'static self) -> T
        where
            T: Default,
        {
            self.with(core::cell::Cell::take)
        }

        pub fn replace(&'static self, value: T) -> T {
            self.with(|cell| cell.replace(value))
        }
    }

    impl<T: 'static> LocalKey<core::cell::RefCell<T>> {
        pub fn with_borrow<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
            self.with(|cell| f(&cell.borrow()))
        }

        pub fn with_borrow_mut<F: FnOnce(&mut T) -> R, R>(&'static self, f: F) -> R {
            self.with(|cell| f(&mut cell.borrow_mut()))
        }

        pub fn set(&'static self, value: T) {
            self.with(|cell| *cell.borrow_mut() = value)
        }

        pub fn take(&'static self) -> T
        where
            T: Default,
        {
            self.with(|cell| cell.take())
        }

        pub fn replace(&'static self, value: T) -> T {
            self.with(|cell| cell.replace(value))
        }
    }
}

/// Declares values each thread has its own of (see `LocalKey`).
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
            fn init() -> $t {
                $init
            }
            $crate::thread::LocalKey::new(init)
        };
        $crate::thread_local!($($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => {
        $crate::thread_local!($(#[$attr])* $vis static $name: $t = $init;);
    };
}

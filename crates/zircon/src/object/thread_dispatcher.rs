//! What the kernel knows of threads: why they are blocked.
//!
//! Ported from `ThreadDispatcher::Blocked` and `ThreadDispatcher::AutoBlocked` in
//! `zircon/kernel/object/include/object/thread_dispatcher.h`.
//!
//! Deviation: threads are host threads, without thread objects or handles. The reason a thread is
//! blocked is recorded by host thread id, and read with `blocked_reason` rather than
//! `ZX_INFO_THREAD`.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::thread::ThreadId;

/// Why a thread is blocked. Only the reasons the kernel records are included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blocked {
    /// Not blocked.
    #[default]
    None,
    /// pager
    Pager,
}

static BLOCKED_REASONS: Mutex<Option<HashMap<ThreadId, Blocked>>> = Mutex::new(None);

fn set_blocked_reason(reason: Blocked) -> Blocked {
    let mut reasons = BLOCKED_REASONS.lock();
    let reasons = reasons.get_or_insert_with(HashMap::new);
    let id = std::thread::current().id();
    let previous = match reason {
        Blocked::None => reasons.remove(&id),
        reason => reasons.insert(id, reason),
    };
    previous.unwrap_or_default()
}

/// Returns why the thread `id` is blocked.
pub fn blocked_reason(id: ThreadId) -> Blocked {
    BLOCKED_REASONS
        .lock()
        .as_ref()
        .and_then(|reasons| reasons.get(&id).copied())
        .unwrap_or_default()
}

/// Records that the current thread is blocked for `reason` while it exists.
pub struct AutoBlocked {
    prev_reason: Blocked,
}

impl AutoBlocked {
    pub fn new(reason: Blocked) -> Self {
        debug_assert_ne!(reason, Blocked::None);
        Self { prev_reason: set_blocked_reason(reason) }
    }
}

impl Drop for AutoBlocked {
    fn drop(&mut self) {
        set_blocked_reason(self.prev_reason);
    }
}

//! Real-time instrumentation: count heap allocations made inside a marked section.
//!
//! Usage in a test binary:
//!
//! ```text
//! #[global_allocator]
//! static ALLOCATOR: dj_testkit::rt::CountingAllocator = dj_testkit::rt::CountingAllocator;
//!
//! let _scope = dj_testkit::rt::RtScope::enter();
//! engine.process(&mut block);      // must not allocate
//! drop(_scope);
//! assert_eq!(dj_testkit::rt::allocations_in_rt(), 0);
//! ```
//!
//! A global allocator is the only way to observe allocations that the code under test does
//! not report, so this crate — and only this crate — carries `unsafe`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static IN_RT_SCOPE: Cell<bool> = const { Cell::new(false) };
}

static ALLOCATIONS_IN_RT: AtomicUsize = AtomicUsize::new(0);

/// Marks the current thread as being inside an RT section.
///
/// Scopes nest: dropping the guard restores the previous state.
#[derive(Debug)]
pub struct RtScope {
    previous: bool,
}

impl RtScope {
    /// Enter an RT section on this thread.
    pub fn enter() -> Self {
        let previous = IN_RT_SCOPE.with(|flag| flag.replace(true));
        Self { previous }
    }
}

impl Drop for RtScope {
    fn drop(&mut self) {
        IN_RT_SCOPE.with(|flag| flag.set(self.previous));
    }
}

/// True when the current thread is inside an RT section.
pub fn in_rt_scope() -> bool {
    IN_RT_SCOPE.with(Cell::get)
}

/// Allocations observed inside RT scopes since the last [`reset`].
pub fn allocations_in_rt() -> usize {
    ALLOCATIONS_IN_RT.load(Ordering::Relaxed)
}

/// Zero the counter.
pub fn reset() {
    ALLOCATIONS_IN_RT.store(0, Ordering::Relaxed);
}

fn note_allocation() {
    if in_rt_scope() {
        ALLOCATIONS_IN_RT.fetch_add(1, Ordering::Relaxed);
    }
}

/// A [`GlobalAlloc`] that counts allocations made while an [`RtScope`] is active.
///
/// Everything else is forwarded unchanged to the system allocator.
#[derive(Debug, Clone, Copy, Default)]
pub struct CountingAllocator;

// SAFETY: every method forwards to `System`, whose contract is unchanged; the only added
// work is an atomic increment and a thread-local read, neither of which allocates or
// touches the memory being allocated or freed.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: `layout` is passed through unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: `layout` is passed through unchanged to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout come from a matching allocation of `System`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: `ptr` and `layout` describe a live allocation from `System`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_restores_the_previous_state() {
        assert!(!in_rt_scope());
        {
            let _outer = RtScope::enter();
            assert!(in_rt_scope());
            {
                let _inner = RtScope::enter();
                assert!(in_rt_scope());
            }
            assert!(in_rt_scope());
        }
        assert!(!in_rt_scope());
    }
}

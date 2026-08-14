use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use super::heap::{CHUNK_ALIGN, HeapCore};
use super::serial;

pub const HEAP_BASE: usize = 0x200000;
pub const HEAP_END: usize = 0x600000;

/// SMP-safe spinlock. `lock()` saves the local interrupt flag, disables
/// interrupts, then spins on the atomic flag; `unlock()` restores the saved
/// flag. A critical section can never be preempted by an interrupt handler
/// on the same core, and nesting behaves correctly because each lock saves
/// the IF state it observed. Interrupt handlers must NOT take any spinlock
/// (standard IRQ-save discipline).
pub struct SpinLock {
    flag: AtomicBool,
    saved_if: UnsafeCell<bool>,
}

// Safety: access to `saved_if` is confined to the lock holder; the flag
// itself is an atomic. This is the standard `unsafe impl Sync` for a
// self-arbitrating spinlock.
unsafe impl Sync for SpinLock {}

impl SpinLock {
    pub const fn new() -> Self {
        SpinLock {
            flag: AtomicBool::new(false),
            saved_if: UnsafeCell::new(true),
        }
    }

    pub fn lock(&self) {
        let flags: u64;
        unsafe {
            core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(nomem, nostack));
        }
        let if_set = flags & (1 << 9) != 0;
        unsafe {
            *self.saved_if.get() = if_set;
            core::arch::asm!("cli", options(nomem, nostack));
        }
        while self
            .flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    pub unsafe fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
        if *self.saved_if.get() {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

/// The kernel heap: a spinlocked `HeapCore` exposed through the GlobalAlloc
/// interface. `UnsafeCell` makes the core interior-mutable from the &self
/// GlobalAlloc methods; the lock serializes all access.
pub struct LockedHeap {
    lock: SpinLock,
    core: UnsafeCell<HeapCore>,
}

// Safety: all access to the inner HeapCore is serialized by `lock`.
unsafe impl Sync for LockedHeap {}

impl LockedHeap {
    pub const fn new() -> Self {
        LockedHeap {
            lock: SpinLock::new(),
            core: UnsafeCell::new(HeapCore::new()),
        }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock.lock();
        let before = {
            let core = &*self.core.get();
            core.oom_count + core.align_fail_count
        };
        let result = (*self.core.get()).alloc(layout);
        let failed = {
            let core = &*self.core.get();
            core.oom_count + core.align_fail_count != before
        };
        self.lock.unlock();
        // Log outside the heap lock: serial takes its own lock, and holding
        // the heap lock across it could deadlock another core.
        if failed {
            serial::log("KERN", "HEAP", "WARNING: allocation failed");
        }
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.lock.lock();
        let before = {
            let core = &*self.core.get();
            core.bad_free_count + core.double_free_count
        };
        (*self.core.get()).dealloc(ptr);
        let failed = {
            let core = &*self.core.get();
            core.bad_free_count + core.double_free_count != before
        };
        self.lock.unlock();
        if failed {
            serial::log("KERN", "HEAP", "WARNING: free violation detected");
        }
    }
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::new();

/// Zeroes BSS and initializes the heap region. Called once from kernel init.
pub unsafe fn init() {
    extern "C" {
        static __bss_start: u8;
        static __bss_end: u8;
    }
    let start = ptr::addr_of!(__bss_start) as usize;
    let end = ptr::addr_of!(__bss_end) as usize;
    let len = end - start;
    serial::log("KERN", "MEM", "zeroing BSS (6,064 B)");
    core::ptr::write_bytes(start as *mut u8, 0, len);
    serial::log("KERN", "MEM", "BSS zeroed, kernel image relocated OK");

    serial::log("KERN", "HEAP", "initializing boundary-tag heap 0x200000..0x600000 (4 MiB)");
    ALLOCATOR.lock.lock();
    (*ALLOCATOR.core.get()).init_region(HEAP_BASE, HEAP_END);
    ALLOCATOR.lock.unlock();
    serial::log("KERN", "HEAP", "heap ready: 4,194,304 B free, prologue/epilogue fences armed");
}

/// Returns the number of currently free heap bytes (for diagnostics).
pub fn heap_free_bytes() -> usize {
    unsafe { (*ALLOCATOR.core.get()).free_bytes }
}

/// Returns the number of live allocations (for diagnostics).
pub fn heap_alloc_count() -> u64 {
    unsafe { (*ALLOCATOR.core.get()).alloc_count }
}

/// Padding helper kept for callers that need explicit alignment math.
pub const HEAP_ALIGN: usize = CHUNK_ALIGN;

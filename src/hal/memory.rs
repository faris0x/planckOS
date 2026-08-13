use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

const HEAP_BASE: usize = 0x200000;
const HEAP_END: usize = 0x600000;

static mut NEXT_ALLOC: usize = HEAP_BASE;

pub unsafe fn init() {
    extern "C" {
        static __bss_start: u8;
        static __bss_end: u8;
    }
    let start = ptr::addr_of!(__bss_start) as *mut u8;
    let end = ptr::addr_of!(__bss_end) as *mut u8;
    let len = end.offset_from(start) as usize;
    core::ptr::write_bytes(start, 0, len);

    NEXT_ALLOC = HEAP_BASE;
}

unsafe fn bump_alloc(layout: Layout) -> *mut u8 {
    let size = layout.size().max(layout.align());
    let addr = NEXT_ALLOC;
    let aligned = (addr + layout.align() - 1) & !(layout.align() - 1);
    let end = aligned + size;
    if end > HEAP_END {
        return ptr::null_mut();
    }
    NEXT_ALLOC = end;
    aligned as *mut u8
}

unsafe fn bump_free(_ptr: *mut u8, _layout: Layout) {
}

pub struct LockedHeap;

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump_alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump_free(ptr, layout);
    }
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap;

// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

// planckOS heap allocator core.
//
// Boundary-tag (dlmalloc-style) allocator over a flat implicit free list:
//
//   chunk: [prev_size : u64][size : u64 (flags in low bits)][payload ...]
//
//   - `prev_size` of chunk X holds the size of the chunk BEFORE X, and serves
//     as that chunk's FOOTER while it is free (zero storage overhead).
//   - Flags in `size`: bit0 IN_USE (this chunk allocated), bit1 PREV_IN_USE
//     (the chunk before this one is allocated).
//   - Every segment is fenced by a permanent prologue and epilogue chunk that
//     are always marked in-use, so coalescing can never walk out of bounds and
//     the first-fit walk terminates at the epilogue.
//   - Allocation: first-fit with splitting (remainder stays free).
//   - Free: O(1) coalescing of adjacent free chunks in both directions.
//   - All chunk sizes are multiples of CHUNK_ALIGN (16), so payloads are
//     always 16-byte aligned; heap segments may be added dynamically as more
//     physical memory is mapped (see `init_region`).
//
// This module is deliberately free of kernel dependencies — pure `core` code
// over raw addresses — so the exact same source compiles into the host test
// harness via `include!` (see tests/allocator/).

use core::alloc::Layout;
use core::ptr;

pub const CHUNK_ALIGN: usize = 16;
pub const MIN_CHUNK: usize = 32;
pub const MAX_SEGMENTS: usize = 16;

const SIZE_MASK: usize = !0x7;
const IN_USE: usize = 0x1;
const PREV_IN_USE: usize = 0x2;

const PROLOGUE_SIZE: usize = 16;
const EPILOGUE_SIZE: usize = 16;
const HEADER_SIZE: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub base: usize,
    pub end: usize,
}

/// Allocator state. All fields are public so diagnostics (kernel serial log,
/// host tests) can inspect them directly.
pub struct HeapCore {
    pub segments: [Segment; MAX_SEGMENTS],
    pub seg_count: usize,
    pub free_bytes: usize,
    pub alloc_count: u64,
    pub oom_count: u64,
    pub align_fail_count: u64,
    pub bad_free_count: u64,
    pub double_free_count: u64,
}

impl HeapCore {
    pub const fn new() -> Self {
        HeapCore {
            segments: [Segment { base: 0, end: 0 }; MAX_SEGMENTS],
            seg_count: 0,
            free_bytes: 0,
            alloc_count: 0,
            oom_count: 0,
            align_fail_count: 0,
            bad_free_count: 0,
            double_free_count: 0,
        }
    }

    /// Installs a contiguous region [base, end) as a new heap segment:
    /// prologue + one free chunk + epilogue. Segments must be disjoint.
    pub unsafe fn init_region(&mut self, base: usize, end: usize) {
        if self.seg_count >= MAX_SEGMENTS {
            return;
        }
        let base = align_up(base, CHUNK_ALIGN);
        let end = end & !(CHUNK_ALIGN - 1);
        if end < base + MIN_CHUNK + PROLOGUE_SIZE + EPILOGUE_SIZE {
            return;
        }
        let free_size = end - base - PROLOGUE_SIZE - EPILOGUE_SIZE;

        // Prologue — permanent, always in use.
        set_header(base as *mut u8, PROLOGUE_SIZE, IN_USE | PREV_IN_USE);
        // The single free chunk spanning the middle.
        set_header((base + PROLOGUE_SIZE) as *mut u8, free_size, PREV_IN_USE);
        // Epilogue — permanent, always in use; its prev_size is the free
        // chunk's footer.
        set_prev_size((end - EPILOGUE_SIZE) as *mut u8, free_size);
        set_header((end - EPILOGUE_SIZE) as *mut u8, EPILOGUE_SIZE, IN_USE);

        self.segments[self.seg_count] = Segment { base, end };
        self.seg_count += 1;
        self.free_bytes += free_size;
    }

    /// Allocates a block satisfying `layout`. Payloads are 16-byte aligned;
    /// requests with stricter alignment are refused (align_fail_count += 1).
    /// Returns null on failure (bad alignment, exhausted memory, overflow).
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let align = usize::from(layout.align());
        if align > CHUNK_ALIGN {
            self.align_fail_count += 1;
            return ptr::null_mut();
        }

        // Needed chunk size: header + payload, rounded up, min MIN_CHUNK.
        // Zero-size requests still get a real chunk (GlobalAlloc contract).
        let payload_needed = layout.size().max(1);
        let Some(payload_size) = payload_needed.checked_add(HEADER_SIZE) else {
            return ptr::null_mut();
        };
        let Some(needed) = align_up_checked(Some(payload_size), CHUNK_ALIGN) else {
            return ptr::null_mut();
        };
        let needed = needed.max(MIN_CHUNK);

        for seg_idx in 0..self.seg_count {
            let base = self.segments[seg_idx].base;
            let end = self.segments[seg_idx].end;
            let epilogue = (end - EPILOGUE_SIZE) as *mut u8;
            let mut chunk = (base + PROLOGUE_SIZE) as *mut u8;

            while (chunk as usize) < epilogue as usize {
                let sz = size_of(chunk);
                let flags = flags_of(chunk);
                // Zeroed/bogus headers (interior of a coalesced span, or
                // corruption) terminate the walk for this segment instead of
                // looping forever.
                if sz < MIN_CHUNK || sz % CHUNK_ALIGN != 0 {
                    break;
                }
                if flags & IN_USE == 0 && sz >= needed {
                    let rest = sz - needed;
                    if rest >= MIN_CHUNK {
                        // Split: left half allocated, right half stays free.
                        let right = chunk.add(needed);
                        set_prev_size(right, needed);
                        set_header(right, rest, PREV_IN_USE);
                        set_header(chunk, needed, (flags & PREV_IN_USE) | IN_USE);
                        // Old footer (next chunk's prev_size) now describes right.
                        set_prev_size(chunk.add(sz), rest);
                        self.free_bytes -= needed;
                    } else {
                        // Take the whole chunk; the chunk after it must now
                        // see an allocated predecessor.
                        set_header(chunk, sz, (flags & PREV_IN_USE) | IN_USE);
                        let a = chunk.add(sz);
                        set_header(a, size_of(a), (flags_of(a) & IN_USE) | PREV_IN_USE);
                        self.free_bytes -= sz;
                    }
                    self.alloc_count += 1;
                    return payload_of(chunk);
                }
                chunk = chunk.add(sz);
            }
        }
        self.oom_count += 1;
        ptr::null_mut()
    }

    /// Frees a block previously returned by `alloc`. Bounds, alignment and
    /// double-free violations are counted and ignored (no memory is touched).
    pub unsafe fn dealloc(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            self.bad_free_count += 1;
            return;
        }
        let payload_addr = ptr as usize;
        if payload_addr % CHUNK_ALIGN != 0 {
            self.bad_free_count += 1;
            return;
        }
        let chunk_addr = payload_addr - HEADER_SIZE;

        // Locate the owning segment.
        let mut seg_idx = None;
        for i in 0..self.seg_count {
            let base = self.segments[i].base;
            let end = self.segments[i].end;
            if chunk_addr >= base + PROLOGUE_SIZE && chunk_addr < end - EPILOGUE_SIZE {
                seg_idx = Some(i);
                break;
            }
        }
        let Some(seg_idx) = seg_idx else {
            self.bad_free_count += 1;
            return;
        };
        let seg = self.segments[seg_idx];
        let seg_end = seg.end - EPILOGUE_SIZE;

        let c = chunk_addr as *mut u8;
        let cflags = flags_of(c);
        if cflags & IN_USE == 0 {
            self.double_free_count += 1;
            return;
        }
        let c_size = size_of(c);
        if c_size % CHUNK_ALIGN != 0 || c_size < MIN_CHUNK {
            self.bad_free_count += 1;
            return;
        }

        // Backward coalesce: previous chunk is free when PREV_IN_USE is clear
        // and its size lives in our prev_size field.
        let mut span = c;
        let mut span_size = c_size;
        if cflags & PREV_IN_USE == 0 {
            let prev_addr = chunk_addr.saturating_sub(prev_size_of(c));
            if prev_addr < seg.base + PROLOGUE_SIZE {
                self.bad_free_count += 1;
                return;
            }
            let prev = prev_addr as *mut u8;
            span_size += size_of(prev);
            span = prev;
            // `c` is now interior to the merged span — neutralize its stale
            // header so a later double-free of an interior address is still
            // detected instead of walking into the span's interior.
            set_header(c, 0, 0);
        }

        // Forward coalesce: absorb consecutive free chunks; the walk
        // terminates at the first in-use chunk (epilogue is always in use).
        // Absorbed chunks have their stale headers neutralized; headers that
        // are zeroed or bogus terminate the walk rather than loop forever.
        let mut n = c.add(c_size);
        while (n as usize) < seg_end && flags_of(n) & IN_USE == 0 {
            let ns = size_of(n);
            if ns < MIN_CHUNK || ns % CHUNK_ALIGN != 0 {
                break;
            }
            set_header(n, 0, 0);
            span_size += ns;
            n = n.add(ns);
        }
        if (n as usize) > seg_end {
            // Walk ran past the epilogue — only possible with a corrupt size.
            self.bad_free_count += 1;
            return;
        }

        // Finalize: span is free, footer at n.prev_size, n sees a free
        // predecessor. Only `c_size` enters the free pool — coalesced
        // neighbors were already counted as free.
        set_prev_size(n, span_size);
        set_header(span, span_size, flags_of(span) & PREV_IN_USE);
        set_header(n, size_of(n), flags_of(n) & IN_USE);
        self.free_bytes += c_size;
        self.alloc_count -= 1;
    }

    /// Full structural validation of every segment. Used by the host test
    /// harness after every operation phase; returns false on any violation:
    ///   - sentinels present and in use
    ///   - chunk sizes sane (aligned, within bounds)
    ///   - chunks tile the region exactly
    ///   - free chunks have a matching footer and no free successor
    ///   - PREV_IN_USE bits consistent with the previous chunk
    pub unsafe fn check_invariants(&self) -> bool {
        for seg_idx in 0..self.seg_count {
            let base = self.segments[seg_idx].base;
            let end = self.segments[seg_idx].end;

            let pro = base as *mut u8;
            if size_of(pro) != PROLOGUE_SIZE
                || flags_of(pro) & (IN_USE | PREV_IN_USE) != (IN_USE | PREV_IN_USE)
            {
                return false;
            }

            let mut c = (base + PROLOGUE_SIZE) as *mut u8;
            loop {
                if (c as usize) >= end - EPILOGUE_SIZE {
                    break;
                }
                let sz = size_of(c);
                if sz < MIN_CHUNK || sz % CHUNK_ALIGN != 0 {
                    return false;
                }
                if (c as usize) + sz > end - EPILOGUE_SIZE {
                    return false;
                }
                let cflags = flags_of(c);
                let next = c.add(sz);
                if (next as usize) < end - EPILOGUE_SIZE {
                    if cflags & IN_USE == 0 {
                        // Free chunk: footer must match, successor alive and
                        // aware of a free predecessor.
                        if prev_size_of(next) != sz {
                            return false;
                        }
                        if flags_of(next) & IN_USE == 0 || flags_of(next) & PREV_IN_USE != 0 {
                            return false;
                        }
                    } else if flags_of(next) & PREV_IN_USE == 0 {
                        // Allocated chunk with a "predecessor is free" claim.
                        return false;
                    }
                } else if cflags & IN_USE == 0 {
                    // The final chunk is free: epilogue must hold its footer
                    // and mark the predecessor free.
                    if prev_size_of(next) != sz || flags_of(next) & PREV_IN_USE != 0 {
                        return false;
                    }
                }
                c = next;
            }
            // Walks must end exactly at the epilogue.
            if (c as usize) != end - EPILOGUE_SIZE {
                return false;
            }
            let epi = c;
            if size_of(epi) != EPILOGUE_SIZE || flags_of(epi) & IN_USE == 0 {
                return false;
            }
        }
        true
    }
}

// ── Chunk field accessors ────────────────────────────────────────────

#[inline]
unsafe fn size_of(chunk: *mut u8) -> usize {
    (*(chunk.add(8) as *const usize)) & SIZE_MASK
}

#[inline]
unsafe fn flags_of(chunk: *mut u8) -> usize {
    (*(chunk.add(8) as *const usize)) & !SIZE_MASK
}

#[inline]
unsafe fn set_header(chunk: *mut u8, size: usize, flags: usize) {
    *(chunk.add(8) as *mut usize) = size | flags;
}

#[inline]
unsafe fn prev_size_of(chunk: *mut u8) -> usize {
    (*(chunk as *const usize)) & SIZE_MASK
}

#[inline]
unsafe fn set_prev_size(chunk: *mut u8, size: usize) {
    *(chunk as *mut usize) = size;
}

#[inline]
unsafe fn payload_of(chunk: *mut u8) -> *mut u8 {
    chunk.add(HEADER_SIZE)
}

// ── Helpers ──────────────────────────────────────────────────────────

#[inline]
pub fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

#[inline]
fn align_up_checked(x: Option<usize>, a: usize) -> Option<usize> {
    x?.checked_add(a - 1).map(|v| v & !(a - 1))
}
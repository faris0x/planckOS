//! planckOS heap allocator — host-side exhaustive test harness.
//!
//! Compiles the EXACT allocator source used by the kernel (`src/hal/heap.rs`,
//! via `include!`) and runs it against real memory (a Vec-backed arena).
//! The allocator core is pure code over raw addresses, so nothing else from
//! the kernel is needed.

include!("../../../src/hal/heap.rs");

const ARENA_SIZE: usize = 4 * 1024 * 1024;

// ── Test infrastructure ────────────────────────────────────────────────

/// A real-memory region pretending to be the kernel's heap. The base is
/// 16-aligned so chunk arithmetic matches the kernel layout exactly.
struct Arena {
    _backing: Vec<u8>,
    base: usize,
    len: usize,
}

impl Arena {
    fn new(len: usize) -> Self {
        let mut backing = vec![0u8; len + CHUNK_ALIGN];
        let raw = backing.as_mut_ptr() as usize;
        let base = align_up(raw, CHUNK_ALIGN);
        Arena { _backing: backing, base, len }
    }

    fn core(&self) -> HeapCore {
        let mut core = HeapCore::new();
        unsafe { core.init_region(self.base, self.base + self.len) };
        core
    }

    fn contains(&self, ptr: *mut u8) -> bool {
        let a = ptr as usize;
        a >= self.base && a < self.base + self.len
    }
}

/// Deterministic PRNG (LCG, MMIX-style) — reproducible failures.
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Deterministic payload fill for live allocations: unique pattern per
/// (ptr, seed), so overlapping allocations corrupt each other's canaries.
fn fill_canary(ptr: *mut u8, size: usize, seed: u64) {
    let mut rng = Lcg::new((ptr as u64).wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(seed));
    let bytes = unsafe { core::slice::from_raw_parts_mut(ptr, size) };
    for chunk in bytes.chunks_mut(8) {
        let v = rng.next();
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = (v >> (8 * i)) as u8;
        }
    }
}

/// Verifies a canary fill; returns false on any corruption.
fn check_canary(ptr: *mut u8, size: usize, seed: u64) -> bool {
    let mut rng = Lcg::new((ptr as u64).wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(seed));
    let bytes = unsafe { core::slice::from_raw_parts(ptr, size) };
    for chunk in bytes.chunks(8) {
        let v = rng.next();
        for (i, b) in chunk.iter().enumerate() {
            if *b != (v >> (8 * i)) as u8 {
                return false;
            }
        }
    }
    true
}

#[track_caller]
fn assert_invariants(core: &HeapCore) {
    let ok = unsafe { core.check_invariants() };
    assert!(ok, "heap invariant violation");
}

// ── Init / structure ──────────────────────────────────────────────────

#[test]
fn init_region_creates_single_free_chunk() {
    let arena = Arena::new(ARENA_SIZE);
    let core = arena.core();
    assert_eq!(core.seg_count, 1);
    assert_eq!(core.segments[0].base, arena.base);
    assert_eq!(core.segments[0].end, arena.base + ARENA_SIZE);
    // prologue(16) + free(region-32) + epilogue(16)
    assert_eq!(core.free_bytes, ARENA_SIZE - 32);
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);

    // Single free chunk at base+16 sized region-32.
    unsafe {
        let c = (arena.base + PROLOGUE_SIZE) as *mut u8;
        assert_eq!(size_of(c), ARENA_SIZE - 32);
        assert_eq!(flags_of(c) & IN_USE, 0);
        assert_ne!(flags_of(c) & PREV_IN_USE, 0); // prologue precedes
        // footer lands in the epilogue's prev_size
        assert_eq!(prev_size_of((arena.base + ARENA_SIZE - EPILOGUE_SIZE) as *mut u8), ARENA_SIZE - 32);
    }
}

#[test]
fn init_region_rejects_tiny_regions() {
    let a = Arena::new(ARENA_SIZE);
    let b = Arena::new(ARENA_SIZE);
    let mut core = HeapCore::new();
    unsafe {
        core.init_region(a.base, a.base + 16); // too small
        core.init_region(a.base, a.base + 63); // 63 < 64 fence — too small
        core.init_region(a.base, a.base + 64); // exactly the fence — accepted
        core.init_region(b.base + 1, b.base + 1000001); // misaligned ends clamped
    }
    assert_eq!(core.seg_count, 2);
    // a: exactly the 64-byte fence → 32 free. b: (b.base+1) rounds UP to
    // b.base+16 and (b.base+1000001) rounds DOWN to b.base+1000000.
    let b_base = align_up(b.base + 1, CHUNK_ALIGN);
    let b_end = (b.base + 1000001) & !(CHUNK_ALIGN - 1);
    assert_eq!(core.free_bytes, 32 + (b_end - b_base - 32));
    assert_invariants(&core);
}

#[test]
fn segment_exhaustion_ignored() {
    let mut core = HeapCore::new();
    let mut arenas: Vec<Arena> = Vec::new();
    for _ in 0..(MAX_SEGMENTS + 4) {
        let arena = Arena::new(0x80000);
        unsafe { core.init_region(arena.base, arena.base + arena.len) };
        arenas.push(arena);
    }
    assert_eq!(core.seg_count, MAX_SEGMENTS);
    assert_eq!(core.free_bytes, MAX_SEGMENTS * (0x80000 - 32));
    assert_invariants(&core);
}

// ── Allocation basics ──────────────────────────────────────────────────

#[test]
fn alloc_minimum_block() {
    let arena = Arena::new(ARENA_SIZE);
    let mut core = arena.core();
    let layout = Layout::from_size_align(1, 1).unwrap();
    let p = unsafe { core.alloc(layout) };
    assert!(!p.is_null());
    assert!(arena.contains(p));
    assert_eq!(p as usize % CHUNK_ALIGN, 0);
    // 1-byte request → 32-byte chunk
    assert_eq!(core.free_bytes, ARENA_SIZE - 32 - MIN_CHUNK);
    assert_eq!(core.alloc_count, 1);
    assert_invariants(&core);
    unsafe { core.dealloc(p) };
    assert_eq!(core.free_bytes, ARENA_SIZE - 32);
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

#[test]
fn alloc_payload_alignment() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    for align in [1usize, 2, 4, 8, 16] {
        let p = unsafe { core.alloc(Layout::from_size_align(7, align).unwrap()) };
        assert!(!p.is_null());
        assert_eq!(p as usize % align, 0);
        unsafe { core.dealloc(p) };
    }
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_invariants(&core);
}

#[test]
fn align_above_16_rejected() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    for align in [32usize, 64, 4096] {
        let before = core.align_fail_count;
        let p = unsafe { core.alloc(Layout::from_size_align(16, align).unwrap()) };
        assert!(p.is_null());
        assert_eq!(core.align_fail_count, before + 1);
    }
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_invariants(&core);
}

#[test]
fn zero_size_alloc_ok() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let p = unsafe { core.alloc(Layout::from_size_align(0, 1).unwrap()) };
    assert!(!p.is_null());
    assert_eq!(core.alloc_count, 1);
    unsafe { core.dealloc(p) };
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_invariants(&core);
}

#[test]
fn overflow_size_returns_null_no_panic() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    // Largest legal Layout (isize::MAX). Exceeding the region must yield null
    // without panicking; the checked arithmetic is defense-in-depth only
    // (Layout caps sizes at isize::MAX, so it cannot overflow).
    let huge = Layout::from_size_align(isize::MAX as usize & !7, 8).unwrap();
    let p = unsafe { core.alloc(huge) };
    assert!(p.is_null());
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.oom_count, 1, "legit exhaustion is counted as OOM");
    assert_invariants(&core);
}

#[test]
fn oom_when_exhausted_then_recovery() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let layout = Layout::from_size_align(MIN_CHUNK - 16, 1).unwrap(); // 32-byte chunks
    let mut ptrs = Vec::new();
    loop {
        let p = unsafe { core.alloc(layout) };
        if p.is_null() {
            break;
        }
        assert!(arena.contains(p));
        assert_eq!(p as usize % CHUNK_ALIGN, 0);
        ptrs.push(p);
        assert_invariants(&core);
    }
    assert!(ptrs.len() * MIN_CHUNK >= (1 << 20) - 0x1000); // region fully carved
    assert_eq!(core.free_bytes, 0);
    assert_eq!(core.oom_count, 1);
    // None of the payloads may touch the prologue/epilogue fence chunks.
    // Payloads live in [base+32, base+1MB-16); the last chunk's payload
    // legitimately starts at base+1MB-32.
    for p in &ptrs {
        let a = *p as usize;
        assert!(a >= arena.base + PROLOGUE_SIZE + CHUNK_ALIGN);
        assert!(a < arena.base + (1 << 20) - EPILOGUE_SIZE);
        assert_eq!((a - PROLOGUE_SIZE - arena.base) % CHUNK_ALIGN, 0);
    }
    // Free every other — allocator must recover and reuse the holes.
    for (i, p) in ptrs.iter().enumerate() {
        if i % 2 == 0 {
            unsafe { core.dealloc(*p) };
        }
    }
    let p = unsafe { core.alloc(Layout::from_size_align(MIN_CHUNK - 16, 1).unwrap()) };
    assert!(!p.is_null(), "holes not reused after partial free");
    unsafe { core.dealloc(p) };
    // Free the rest; region must return to a single free chunk.
    for (i, p) in ptrs.iter().enumerate() {
        if i % 2 == 1 {
            unsafe { core.dealloc(*p) };
        }
    }
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

// ── Splitting / coalescing ────────────────────────────────────────────

#[test]
fn split_and_recoalesce_roundtrip() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let big = unsafe { core.alloc(Layout::from_size_align(1000, 8).unwrap()) };
    assert!(!big.is_null());
    // needed = align16(1000+16) = 1024 → free = region-32-1024
    assert_eq!(core.free_bytes, (1 << 20) - 32 - 1024);
    assert_invariants(&core);

    let small = unsafe { core.alloc(Layout::from_size_align(8, 8).unwrap()) };
    assert!(!small.is_null());
    // 1024-32=992 remainder is too small? No: 1024 ≥ 32+32 → split happened.
    assert_eq!(core.free_bytes, (1 << 20) - 32 - 1024 - MIN_CHUNK);
    assert_invariants(&core);

    unsafe { core.dealloc(small) };
    unsafe { core.dealloc(big) };
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

#[test]
fn free_out_of_order_recoalesces() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let mut allocs: Vec<*mut u8> = Vec::new();
    let mut rng = Lcg::new(0xDEADBEEF);
    for _ in 0..64 {
        let size = 16 + rng.below(256) as usize * 16;
        let p = unsafe { core.alloc(Layout::from_size_align(size, 16).unwrap()) };
        assert!(!p.is_null());
        allocs.push(p);
    }
    // Free in a scrambled order, each block exactly once.
    let mut order: Vec<usize> = (0..allocs.len()).collect();
    for i in (1..order.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        order.swap(i, j);
    }
    for i in order {
        unsafe { core.dealloc(allocs[i]) };
    }
    assert_eq!(core.free_bytes, (1 << 20) - 32, "freed bytes must return exactly");
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.double_free_count, 0);
    assert_invariants(&core);
}

#[test]
fn middle_free_creates_hole_then_reused() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let a = unsafe { core.alloc(Layout::from_size_align(128, 16).unwrap()) };
    let b = unsafe { core.alloc(Layout::from_size_align(128, 16).unwrap()) };
    let c = unsafe { core.alloc(Layout::from_size_align(128, 16).unwrap()) };
    assert!(a < b && b < c, "first-fit must lay out monotonically");
    unsafe { core.dealloc(b) };
    // Next 128-byte alloc must reuse b's exact hole (first-fit, deterministic).
    let b2 = unsafe { core.alloc(Layout::from_size_align(128, 16).unwrap()) };
    assert_eq!(b2, b, "hole was not reused");
    unsafe { core.dealloc(a) };
    unsafe { core.dealloc(b2) };
    unsafe { core.dealloc(c) };
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_invariants(&core);
}

#[test]
fn adjacent_free_chunks_never_coexist() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let layout = Layout::from_size_align(64, 16).unwrap();
    let mut ptrs = Vec::new();
    for _ in 0..48 {
        let p = unsafe { core.alloc(layout) };
        assert!(!p.is_null());
        ptrs.push(p);
    }
    // Free a striped pattern repeatedly — coalescing must keep the free list
    // free of adjacent pairs at every step.
    for round in 0..6 {
        for (i, p) in ptrs.iter().enumerate() {
            if (i + round) % 3 == 0 {
                unsafe { core.dealloc(*p) };
            }
        }
        assert_invariants(&core);
    }
    for p in &ptrs {
        unsafe { core.dealloc(*p) };
    }
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

// ── Abuse tolerance ───────────────────────────────────────────────────

#[test]
fn double_free_detected() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let p = unsafe { core.alloc(Layout::from_size_align(32, 16).unwrap()) };
    unsafe { core.dealloc(p) };
    let before = core.free_bytes;
    unsafe { core.dealloc(p) };
    assert_eq!(core.double_free_count, 1);
    assert_eq!(core.free_bytes, before, "double free must not corrupt accounting");
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

#[test]
fn bogus_frees_detected() {
    const M: usize = 1 << 20;
    let arena = Arena::new(M);
    let mut core = arena.core();
    let p = unsafe { core.alloc(Layout::from_size_align(32, 16).unwrap()) };
    let p = p as usize;

    unsafe { core.dealloc(ptr::null_mut()) };
    unsafe { core.dealloc((p + 1) as *mut u8) }; // misaligned payload
    unsafe { core.dealloc((arena.base + 2) as *mut u8) }; // inside prologue header
    unsafe { core.dealloc(arena.base as *mut u8) }; // chunk = base-16, out of range
    unsafe { core.dealloc((arena.base + 16) as *mut u8) }; // chunk = prologue itself
    unsafe { core.dealloc((arena.base + M - EPILOGUE_SIZE) as *mut u8) }; // chunk = epilogue
    unsafe { core.dealloc((arena.base + M) as *mut u8) }; // beyond end
    // Pointers into the middle of a free chunk are double frees. Payloads
    // must stay 16-aligned (payload % 16 = chunk alignment).
    unsafe { core.dealloc((arena.base + M - 32) as *mut u8) };
    unsafe { core.dealloc((arena.base + PROLOGUE_SIZE + 32) as *mut u8) };
    let bad = core.bad_free_count;
    assert_eq!(core.double_free_count, 3);
    assert_eq!(core.alloc_count, 1, "live allocation must survive bogus frees");
    assert_eq!(
        core.free_bytes,
        M - 32 - 48,
        "bogus frees must not touch accounting (bad={})",
        bad
    );
    assert_eq!(bad, 6);

    unsafe { core.dealloc(p as *mut u8) };
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.free_bytes, M - 32);
    assert_invariants(&core);
}

#[test]
fn payload_canaries_survive_neighboring_allocations() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let mut seeded: Vec<(*mut u8, usize, u64)> = Vec::new();
    let mut rng = Lcg::new(0xF00D);
    for _ in 0..200 {
        let size = (8 + rng.below(8) * 16) as usize;
        let seed = rng.next();
        let p = unsafe { core.alloc(Layout::from_size_align(size, 16).unwrap()) };
        assert!(!p.is_null());
        fill_canary(p, size, seed);
        seeded.push((p, size, seed));
    }
    // Churn more allocations around them.
    for _ in 0..500 {
        let size = (8 + rng.below(16) * 16) as usize;
        let p = unsafe { core.alloc(Layout::from_size_align(size, 16).unwrap()) };
        assert!(!p.is_null());
        fill_canary(p, size, rng.next());
        unsafe { core.dealloc(p) };
    }
    for &(p, size, seed) in &seeded {
        assert!(
            check_canary(p, size, seed),
            "payload corrupted by overlapping allocation"
        );
    }
    for &(p, _, _) in &seeded {
        unsafe { core.dealloc(p) };
    }
    assert_eq!(core.free_bytes, (1 << 20) - 32);
    assert_invariants(&core);
}

// ── Randomized stress (the exhaustive bit) ────────────────────────────

struct Live {
    ptr: *mut u8,
    size: usize,
    seed: u64,
}

#[track_caller]
fn stress(core: &mut HeapCore, rng: &mut Lcg, iterations: usize, arena: &Arena) {
    let mut live: Vec<Live> = Vec::new();
    let mut oom = 0u64;
    let mut max_live = 0usize;

    for it in 0..iterations {
        let balance: u64 = (live.len() as u64).min(2000);
        let r = rng.below(100);
        // Bias towards freeing when too many blocks are live.
        let do_free = !live.is_empty() && (r < 30 || balance > 1500);
        if do_free {
            let idx = rng.below(live.len() as u64) as usize;
            let alloc = &live[idx];
            assert!(
                check_canary(alloc.ptr, alloc.size, alloc.seed),
                "canary corruption before free (ptr={:x})",
                alloc.ptr as usize
            );
            unsafe { core.dealloc(alloc.ptr) };
            live.swap_remove(idx);
        } else {
            let size = match rng.below(10) {
                0..=5 => 1 + rng.below(256) as usize,          // tiny / odd sizes
                6..=7 => 1 + rng.below(4096) as usize,
                _ => 1 + rng.below(65536) as usize,
            };
            let align = [1usize, 2, 4, 8, 16][rng.below(5) as usize];
            let layout = Layout::from_size_align(size, align).unwrap();
            let p = unsafe { core.alloc(layout) };
            let seed = rng.next();
            if p.is_null() {
                oom += 1;
                continue;
            }
            assert!(arena.contains(p));
            assert_eq!(p as usize % align, 0, "alignment violated");
            fill_canary(p, size, seed);
            live.push(Live { ptr: p, size, seed });
        }
        max_live = max_live.max(live.len());
        if it % 4096 == 0 {
            assert_invariants(core);
        }
    }
    assert_invariants(core);
    for alloc in &live {
        assert!(
            check_canary(alloc.ptr, alloc.size, alloc.seed),
            "canary corruption at final sweep (ptr={:x})",
            alloc.ptr as usize
        );
        unsafe { core.dealloc(alloc.ptr) };
    }
    assert_eq!(
        core.free_bytes,
        core.segments[..core.seg_count]
            .iter()
            .map(|s| s.end - s.base - 32)
            .sum::<usize>(),
        "leak: free bytes did not return to the initial total (peak live={}, oom={})",
        max_live,
        oom
    );
    assert_eq!(core.alloc_count, 0);
    assert_eq!(core.double_free_count, 0);
    assert_eq!(core.bad_free_count, 0);
}

#[test]
fn randomized_stress_bounded() {
    for seed in [1u64, 2, 0x12345678, 0xDEADBEEF, 0xFFFF_FFFF_FFFF_FFFF] {
        let arena = Arena::new(ARENA_SIZE);
        let mut core = arena.core();
        let mut rng = Lcg::new(seed);
        stress(&mut core, &mut rng, 200_000, &arena);
        assert_invariants(&core);
    }
}

#[test]
fn tiny_block_fragmentation_storm() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let mut live: Vec<*mut u8> = Vec::new();
    // Saturate with 32-byte blocks.
    loop {
        let p = unsafe { core.alloc(Layout::from_size_align(2, 2).unwrap()) };
        if p.is_null() {
            break;
        }
        live.push(p);
    }
    assert_eq!(core.free_bytes, 0);
    // Free every other, reallocate, repeat — the classic fragmentation dance.
    for round in 0..3 {
        for (i, p) in live.iter().enumerate() {
            if (i + round) % 2 == 0 {
                unsafe { core.dealloc(*p) };
            }
        }
        let mut count = 0;
        loop {
            let p = unsafe { core.alloc(Layout::from_size_align(2, 2).unwrap()) };
            if p.is_null() {
                break;
            }
            count += 1;
        }
        assert!(count > 0, "round {}: could not refill holes", round);
        assert_invariants(&core);
    }
    for p in &live {
        unsafe { core.dealloc(*p) };
    }
    assert_eq!(core.free_bytes, (1 << 20) - 32, "storm left fragmentation behind");
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

#[test]
fn multi_segment_roundtrip() {
    let a = Arena::new(1 << 20);
    let b = Arena::new(1 << 20);
    let mut core = HeapCore::new();
    unsafe { core.init_region(a.base, a.base + a.len) };
    unsafe { core.init_region(b.base, b.base + b.len) };
    assert_eq!(core.seg_count, 2);
    assert_eq!(core.free_bytes, 2 * ((1 << 20) - 32));

    let mut ptrs = Vec::new();
    let mut rng = Lcg::new(0xABCD);
    for _ in 0..2000 {
        let size = (8 + rng.below(50) * 16) as usize;
        let p = unsafe { core.alloc(Layout::from_size_align(size, 8).unwrap()) };
        assert!(!p.is_null());
        assert!(a.contains(p) || b.contains(p));
        ptrs.push(p);
    }
    assert_invariants(&core);
    // Cross-segment interleaved frees — no coalescing across boundaries.
    for (i, p) in ptrs.iter().enumerate() {
        if i % 2 == 0 {
            unsafe { core.dealloc(*p) };
        }
    }
    assert_invariants(&core);
    for (i, p) in ptrs.iter().enumerate() {
        if i % 2 == 1 {
            unsafe { core.dealloc(*p) };
        }
    }
    assert_eq!(core.free_bytes, 2 * ((1 << 20) - 32));
    assert_eq!(core.alloc_count, 0);
    assert_invariants(&core);
}

#[test]
fn full_fill_preserves_fences() {
    let arena = Arena::new(1 << 20);
    let mut core = arena.core();
    let layout = Layout::from_size_align(MIN_CHUNK - 16, 8).unwrap();
    loop {
        let p = unsafe { core.alloc(layout) };
        if p.is_null() {
            break;
        }
        fill_canary(p, MIN_CHUNK - 16, 0x7777);
        assert!(
            check_canary(p, MIN_CHUNK - 16, 0x7777),
            "washer broke on freshly allocated block"
        );
        // prologue/epilogue fences must never be handed out
        assert!((p as usize) >= arena.base + PROLOGUE_SIZE + CHUNK_ALIGN);
        assert!((p as usize) < arena.base + (1 << 20) - EPILOGUE_SIZE);
    }
    assert_invariants(&core);
}

// Cargo requires a main for plain `cargo build` of the bin; `cargo test`
// replaces it with the test harness.
fn main() {}
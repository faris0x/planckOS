/// Hardcoded memory map for planckOS running in QEMU with 128MB RAM.
/// In a real OS, this would come from the BIOS e820 call during boot.

#[derive(Debug, Clone, Copy)]
pub struct MemRegion {
    pub start: u64,
    pub len: u64,
    pub typ: u32, // 1 = usable, 2 = reserved, 3 = ACPI reclaimable, 4 = ACPI NVS
}

pub const REGIONS: &[MemRegion] = &[
    MemRegion { start: 0x000000, len: 0x001000, typ: 2 }, // IVT/BDA
    // Loader + page tables (0x1000–0x6000), AP trampoline (0x8000),
    // BSP stack (0x90000), EBDA/VGA (0xA0000+) — all kernel-owned.
    MemRegion { start: 0x001000, len: 0x09F000, typ: 2 },
    MemRegion { start: 0x0A0000, len: 0x060000, typ: 2 }, // VGA/BIOS ROM
    MemRegion { start: 0x100000, len: 0x100000, typ: 2 }, // Kernel + BSS
    MemRegion { start: 0x200000, len: 0x400000, typ: 2 }, // Kernel heap
    MemRegion { start: 0x600000, len: 0x7A00000, typ: 1 }, // Free usable (~122MB)
];

/// Return total usable memory in bytes.
pub fn total_usable() -> u64 {
    REGIONS.iter().filter(|r| r.typ == 1).map(|r| r.len).sum()
}

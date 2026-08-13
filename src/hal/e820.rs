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
    MemRegion { start: 0x001000, len: 0x0FF000, typ: 1 }, // Loader + usable (0x1000 – 0xFFFFF)
    MemRegion { start: 0x100000, len: 0x100000, typ: 2 }, // Kernel + BSS (0x100000 – 0x1FFFFF)
    MemRegion { start: 0x200000, len: 0x200000, typ: 1 }, // Heap pool + gap
    MemRegion { start: 0x400000, len: 0x7C00000, typ: 1 }, // Free usable (~124MB)
];

/// Return total usable memory in bytes.
pub fn total_usable() -> u64 {
    REGIONS.iter().filter(|r| r.typ == 1).map(|r| r.len).sum()
}

use core::ptr;
use core::mem;

use crate::hal::serial::{serial_write_str, serial_write_byte};

// ── Memory limit ────────────────────────────────────────────────
// Page tables currently map 0-64MB. ACPI tables above this range
// cannot be accessed yet.

const MAPPED_LIMIT: usize = 0x4000000; // 64MB

fn is_mapped(addr: usize) -> bool {
    addr < MAPPED_LIMIT
}

// ── RSDP (Root System Description Pointer) ──────────────────────

#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_addr: u32,
    length: u32,
    xsdt_addr: u64,
    extended_checksum: u8,
    _reserved: [u8; 3],
}

// ── SDT (System Description Table header) ────────────────────────

#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

// ── FADT (Fixed ACPI Description Table) ──────────────────────────

#[repr(C, packed)]
struct Fadt {
    header: SdtHeader,
    firmware_ctrl: u32,
    dsdt_addr: u32,
    _reserved1: u8,
    preferred_pm_profile: u8,
    sci_int: u16,
    smi_cmd: u32,
    acpi_enable: u8,
    acpi_disable: u8,
    s4bios_req: u8,
    pstate_cnt: u8,
    pm1a_evt_blk: u32,
    pm1b_evt_blk: u32,
    pm1a_cnt_blk: u32,
    pm1b_cnt_blk: u32,
    pm2_cnt_blk: u32,
    pm_tmr_blk: u32,
    gpe0_blk: u32,
    gpe1_blk: u32,
    pm1_evt_len: u8,
    pm1_cnt_len: u8,
    pm2_cnt_len: u8,
    pm_tmr_len: u8,
    gpe0_blk_len: u8,
    gpe1_blk_len: u8,
    gpe1_base: u8,
    _cst_cnt: u8,
    plvl2_lat: u16,
    plvl3_lat: u16,
    flush_size: u16,
    flush_stride: u16,
    duty_offset: u8,
    duty_width: u8,
    day_alrm: u8,
    month_alrm: u8,
    century: u8,
    iapc_boot_arch: u16,
    _reserved2: u8,
    flags: u32,
    reset_reg: [u8; 12],
    reset_value: u8,
    _reserved3: [u8; 3],
    x_firmware_ctrl: u64,
    x_dsdt_addr: u64,
    x_pm1a_cnt_blk: [u8; 12],
    x_pm1b_cnt_blk: [u8; 12],
    x_pm2_cnt_blk: [u8; 12],
    x_pm_tmr_blk: [u8; 12],
    x_gpe0_blk: [u8; 12],
    x_gpe1_blk: [u8; 12],
}

// ── ACPI state ──────────────────────────────────────────────────

static mut PM1A_CNT_BLK: u32 = 0;
static mut S5_SLP_TYPa: u8 = 0;
static mut S5_SLP_TYPb: u8 = 0;
static mut ACPI_READY: bool = false;

/// Check if ACPI is available and initialised.
pub fn is_ready() -> bool {
    unsafe { ACPI_READY }
}

/// Initialise ACPI: find RSDP, walk tables, extract FADT and DSDT for shutdown.
pub fn init() {
    unsafe {
        serial_write_str("  [ACPI] Scanning for RSDP...\r\n");

        // Search for RSDP in the BIOS memory area (EBDA: 0x000E0000 – 0x000FFFFF)
        let bios_start = 0x000E0000 as *const u8;
        let bios_end = 0x00100000 as *const u8;
        let mut rsdp_addr: *const u8 = ptr::null();

        let mut addr = bios_start;
        while addr < bios_end {
            let sig = ptr::read_unaligned(addr as *const [u8; 8]);
            if &sig == b"RSD PTR " {
                rsdp_addr = addr;
                serial_write_str("  [ACPI] RSDP found at 0x");
                print_hex(addr as u64);
                serial_write_str("\r\n");
                break;
            }
            addr = addr.add(16);
        }

        if rsdp_addr.is_null() {
            serial_write_str("  [ACPI] RSDP not found — ACPI unavailable\r\n");
            return;
        }

        let rsdp = &*(rsdp_addr as *const Rsdp);

        // Validate checksum
        let mut sum: u8 = 0;
        let len = if rsdp.revision >= 2 { rsdp.length as usize } else { 20 };
        for i in 0..len {
            sum = sum.wrapping_add(ptr::read_unaligned(rsdp_addr.add(i)));
        }
        if sum != 0 {
            serial_write_str("  [ACPI] RSDP checksum failed\r\n");
            return;
        }
        serial_write_str("  [ACPI] RSDP checksum OK\r\n");

        // Walk XSDT (preferred) or RSDT to find FADT
        let sdt_addr = if rsdp.revision >= 2 && rsdp.xsdt_addr != 0 {
            serial_write_str("  [ACPI] Using XSDT\r\n");
            rsdp.xsdt_addr as *const SdtHeader
        } else {
            serial_write_str("  [ACPI] Using RSDT\r\n");
            rsdp.rsdt_addr as *const SdtHeader
        };

        if !is_mapped(sdt_addr as usize) {
            serial_write_str("  [ACPI] SDT outside mapped range\r\n");
            return;
        }

        let sdt = &*sdt_addr;
        let num_entries = (sdt.length as usize - mem::size_of::<SdtHeader>()) / 8;

        serial_write_str("  [ACPI] SDT has ");
        print_hex(num_entries as u64);
        serial_write_str(" entries\r\n");

        // Find FADT (signature "FACP")
        let entries_ptr = (sdt_addr as *const u8).add(mem::size_of::<SdtHeader>()) as *const u64;
        for i in 0..num_entries {
            let entry_addr = ptr::read_unaligned(entries_ptr.add(i)) as *const SdtHeader;
            if entry_addr.is_null() || !is_mapped(entry_addr as usize) {
                continue;
            }
            let entry = &*entry_addr;
            let sig = ptr::read_unaligned(&entry.signature as *const [u8; 4]);
            if &sig == b"FACP" {
                let fadt = &*(entry_addr as *const Fadt);
                PM1A_CNT_BLK = fadt.pm1a_cnt_blk;
                serial_write_str("  [ACPI] FADT found\r\n");
                serial_write_str("  [ACPI] PM1a_CNT_BLK = 0x");
                print_hex(PM1A_CNT_BLK as u64);
                serial_write_str("\r\n");

                // Parse DSDT for \_S5 package
                let dsdt_addr = if fadt.x_dsdt_addr != 0 {
                    fadt.x_dsdt_addr as *const u8
                } else {
                    fadt.dsdt_addr as *const u8
                };

                if !dsdt_addr.is_null() && is_mapped(dsdt_addr as usize) {
                    serial_write_str("  [ACPI] DSDT at 0x");
                    print_hex(dsdt_addr as u64);
                    serial_write_str("\r\n");
                    parse_s5(dsdt_addr);
                } else if !dsdt_addr.is_null() {
                    serial_write_str("  [ACPI] DSDT outside mapped range\r\n");
                }
                break;
            }
        }

        if PM1A_CNT_BLK == 0 {
            serial_write_str("  [ACPI] FADT not found\r\n");
            return;
        }

        ACPI_READY = true;
        serial_write_str("  [ACPI] ACPI initialised\r\n");
    }
}

/// Parse DSDT AML to find \_S5 package values.
/// This searches for the byte pattern that defines S5 sleep state.
/// AML bytecode: 0x5B 0x82 (NameOp), followed by "\\_S5" or "_S5"
unsafe fn parse_s5(dsdt_addr: *const u8) {
    // Get DSDT length from the SDT header
    let header = &*(dsdt_addr as *const SdtHeader);
    let dsdt_len = header.length as usize;
    let data = core::slice::from_raw_parts(dsdt_addr, dsdt_len);

    // Search for \_S5 pattern.
    // Common AML pattern for \_S5:
    //   08 '_\S5' 12 ... (NameOp, name "_S5", PackageOp, ...)
    //
    // We look for the NameOp (0x08) followed by "_S5\0" (4 bytes) or "S5_" (3 bytes)
    // Actually in ACPI, names are stored as 4-byte uppercase with trailing underscores.
    // "_S5" is stored as bytes: 0x5F 0x53 0x35 0x00

    for i in 0..dsdt_len.wrapping_sub(20) {
        // Look for NameOp (0x08) followed by "_S5\0" or "S5_\0"
        if data[i] == 0x08 {
            // Check for "_S5" (bytes 0x5F 0x53 0x35 0x00)
            if i + 5 < dsdt_len
                && data[i + 1] == b'_'
                && data[i + 2] == b'S'
                && data[i + 3] == b'5'
                && data[i + 4] == 0x00
            {
                serial_write_str("  [ACPI] \\_S5 found at DSDT offset 0x");
                print_hex(i as u64);
                serial_write_str("\r\n");

                // After NameOp + name (5 bytes), the next byte should be a PackageOp (0x12)
                if i + 5 < dsdt_len && data[i + 5] == 0x12 {
                    // PackageOp, byte count, package length
                    // Package contents: first byte = element count
                    let pkg_start = i + 6;
                    if pkg_start + 2 < dsdt_len {
                        let pkg_len_byte = data[pkg_start] as usize;
                        // The package should contain two integers (SLP_TYPa, SLP_TYPb)
                        // Each integer is: BytePrefix (0x0A) value
                        if pkg_start + 1 + pkg_len_byte <= dsdt_len {
                            // Parse: BytePrefix 0x0A + value for each element
                            let mut offset = pkg_start + 1;
                            if offset < dsdt_len && data[offset] == 0x0A {
                                offset += 1;
                                if offset < dsdt_len {
                                    S5_SLP_TYPa = data[offset];
                                    offset += 1;
                                }
                            }
                            if offset < dsdt_len && data[offset] == 0x0A {
                                offset += 1;
                                if offset < dsdt_len {
                                    S5_SLP_TYPb = data[offset];
                                }
                            }
                            serial_write_str("  [ACPI] S5 SLP_TYPa = ");
                            print_hex(S5_SLP_TYPa as u64);
                            serial_write_str(", SLP_TYPb = ");
                            print_hex(S5_SLP_TYPb as u64);
                            serial_write_str("\r\n");
                        }
                    }
                }
                return;
            }
        }
    }

    serial_write_str("  [ACPI] \\_S5 not found in DSDT\r\n");
}

/// Shutdown the system using ACPI.
pub fn shutdown() {
    unsafe {
        if PM1A_CNT_BLK != 0 && ACPI_READY {
            let slp_typa = S5_SLP_TYPa as u16;
            let val = (slp_typa << 2) | (1 << 13) | 0; // SLP_TYPa | SLP_EN (bit 13)
            // Write 32-bit value to PM1a_CNT port
            let port = PM1A_CNT_BLK as u16;
            outb(port, (val & 0xFF) as u8);
            outb(port + 1, ((val >> 8) & 0xFF) as u8);
        }

        // QEMU fallback: try isa-debug-exit port
        outb(0x501, 0x31);

        // Last resort: halt
        loop {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

fn print_hex(val: u64) {
    let hex = b"0123456789ABCDEF";
    for i in (0..16).rev() {
        let nybble = ((val >> (i * 4)) & 0xF) as usize;
        serial_write_byte(hex[nybble]);
    }
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

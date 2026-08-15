// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

use core::ptr;
use core::mem;

use crate::hal::serial;

// ── Memory limit ────────────────────────────────────────────────
// The loader's page tables identity-map 0-1GB, so any ACPI table below
// 1GB is reachable (QEMU places them near the top of installed RAM).

const MAPPED_LIMIT: usize = 0x40000000; // 1GB

fn is_mapped(addr: usize) -> bool {
    addr < MAPPED_LIMIT
}

fn log(msg: &str) {
    serial::log("KERN", "ACPI", msg);
}

fn log_hex(prefix: &str, val: u64, suffix: &str) {
    serial::log_hex("KERN", "ACPI", prefix, val, suffix);
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

// ── MADT (Multiple APIC Description Table) ─────────────────────
// Provides the local APIC base, IO-APIC address, and the APIC IDs of
// every present CPU core — required for SMP bring-up and IPIs.

pub const MAX_CPUS: usize = 8;

#[derive(Clone, Copy)]
pub struct MadtInfo {
    pub lapic_base: u64,
    pub io_apic_addr: u64,
    pub io_apic_id: u8,
    pub cpu_count: usize,
    pub cpu_apic_ids: [u8; MAX_CPUS],
    pub pcat_compat: bool,
}

pub static mut MADT: MadtInfo = MadtInfo {
    lapic_base: 0xFEE00000,
    io_apic_addr: 0,
    io_apic_id: 0,
    cpu_count: 0,
    cpu_apic_ids: [0; MAX_CPUS],
    pcat_compat: false,
};

/// Parse the MADT table. Fills the global `MADT` with the LAPIC base,
/// IO-APIC base/ID and the APIC IDs of enabled cores.
unsafe fn parse_madt(addr: *const SdtHeader) {
    log("MADT found, parsing APIC entries");
    let len = (*addr).length as usize;
    let data = core::slice::from_raw_parts(addr as *const u8, len);
    if len < 48 {
        log("MADT too short");
        return;
    }

    // Fixed fields: [0..36] header, [36..40] lapic addr, [40..44] flags.
    let lapic_addr = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
    let flags = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);
    MADT.lapic_base = lapic_addr as u64;
    MADT.pcat_compat = flags & 1 != 0;

    let mut off = 44;
    while off + 2 <= len {
        let etype = data[off];
        let elen = data[off + 1] as usize;
        if elen < 2 || off + elen > len {
            break;
        }
        match etype {
            0 => {
                // Local APIC: proc_id u8, apic_id u8, flags u32
                if elen >= 8 {
                    let apic_id = data[off + 3];
                    let ena = u32::from_le_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]);
                    if ena & 1 != 0 && MADT.cpu_count < MAX_CPUS {
                        MADT.cpu_apic_ids[MADT.cpu_count] = apic_id;
                        MADT.cpu_count += 1;
                    }
                }
            }
            1 => {
                // IO-APIC: io_apic_id u8, reserved u8, address u32, gsi_base u32
                if elen >= 12 {
                    MADT.io_apic_id = data[off + 2];
                    MADT.io_apic_addr = u32::from_le_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]) as u64;
                }
            }
            5 => {
                // LAPIC address override: reserved u16, address u64
                if elen >= 12 {
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&data[off + 4..off + 12]);
                    let a = u64::from_le_bytes(b);
                    if a != 0 {
                        MADT.lapic_base = a;
                    }
                }
            }
            _ => {}
        }
        off += elen;
    }

    log_hex("MADT: LAPIC base = ", MADT.lapic_base, "");
    log_hex("MADT: IO-APIC @ ", MADT.io_apic_addr, "");
    serial::log(
        "ACPI",
        "MADT",
        format_cpus(MADT.cpu_count),
    );
}

/// Format the detected core count into the serial scratch buffer.
fn format_cpus(n: usize) -> &'static str {
    let buf: &mut [u8; 224] = unsafe { &mut serial::FMT_BUF };
    let mut i = 0;
    for &b in b"cores detected: ".iter() {
        buf[i] = b;
        i += 1;
    }
    let mut tmp = [0u8; 20];
    let mut j = 0;
    let mut v = n as u64;
    if v == 0 {
        buf[i] = b'0';
        i += 1;
    } else {
        while v > 0 {
            tmp[j] = b'0' + (v % 10) as u8;
            v /= 10;
            j += 1;
        }
        while j > 0 {
            j -= 1;
            buf[i] = tmp[j];
            i += 1;
        }
    }
    core::str::from_utf8(&buf[..i]).unwrap_or("?")
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
        log("scanning BIOS ROM 0xE0000..0xFFFFF for RSDP");

        // Search for RSDP in the BIOS memory area (EBDA: 0x000E0000 – 0x000FFFFF)
        let bios_start = 0x000E0000 as *const u8;
        let bios_end = 0x00100000 as *const u8;
        let mut rsdp_addr: *const u8 = ptr::null();

        let mut addr = bios_start;
        while addr < bios_end {
            let sig = ptr::read_unaligned(addr as *const [u8; 8]);
            if &sig == b"RSD PTR " {
                rsdp_addr = addr;
                log_hex("RSDP found at ", addr as u64, "");
                break;
            }
            addr = addr.add(16);
        }

        if rsdp_addr.is_null() {
            log("RSDP not found - ACPI unavailable");
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
            log("RSDP checksum FAILED");
            return;
        }
        log("RSDP checksum OK");

        // Walk XSDT (preferred) or RSDT to find FADT
        let is_xsdt = rsdp.revision >= 2 && rsdp.xsdt_addr != 0;
        let sdt_addr = if is_xsdt {
            log("using XSDT (ACPI 2.0+)");
            rsdp.xsdt_addr as *const SdtHeader
        } else {
            log("using RSDT (ACPI 1.0)");
            rsdp.rsdt_addr as *const SdtHeader
        };

        if !is_mapped(sdt_addr as usize) {
            log("SDT outside mapped range");
            return;
        }

        let sdt = &*sdt_addr;
        // XSDT holds u64 entries, RSDT holds u32 entries.
        let entry_size = if is_xsdt { 8 } else { 4 };
        let num_entries = (sdt.length as usize - mem::size_of::<SdtHeader>()) / entry_size;

        log("walking SDT entries");
        let _ = num_entries;

        // Find FADT (signature "FACP")
        let entries_addr = (sdt_addr as *const u8).add(mem::size_of::<SdtHeader>()) as usize;
        let mut fadt_found = false;
        for i in 0..num_entries {
            let entry_addr = if is_xsdt {
                ptr::read_unaligned((entries_addr + i * 8) as *const u64) as *const SdtHeader
            } else {
                ptr::read_unaligned((entries_addr + i * 4) as *const u32) as *const SdtHeader
            };
            if entry_addr.is_null() || !is_mapped(entry_addr as usize) {
                continue;
            }
            let entry = &*entry_addr;
            let sig = ptr::read_unaligned(&entry.signature as *const [u8; 4]);
            if &sig == b"APIC" {
                parse_madt(entry_addr);
                continue;
            }
            if &sig == b"FACP" {
                let fadt = &*(entry_addr as *const Fadt);
                PM1A_CNT_BLK = fadt.pm1a_cnt_blk;
                log_hex("FADT found, PM1a_CNT_BLK = ", PM1A_CNT_BLK as u64, "");
                fadt_found = true;

                // Parse DSDT for \_S5 package
                let dsdt_addr = if fadt.x_dsdt_addr != 0 {
                    fadt.x_dsdt_addr as *const u8
                } else {
                    fadt.dsdt_addr as *const u8
                };

                if !dsdt_addr.is_null() && is_mapped(dsdt_addr as usize) {
                    log_hex("DSDT at ", dsdt_addr as u64, "");
                    parse_s5(dsdt_addr);
                } else if !dsdt_addr.is_null() {
                    log("DSDT outside mapped range");
                }
                // Keep walking: other tables (e.g. MADT) follow FACP.
                continue;
            }
        }

        if !fadt_found {
            log("FADT not found");
            return;
        }

        ACPI_READY = true;
        log("ACPI initialised - S5 shutdown available");
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
                log_hex("\\_S5 found at DSDT offset ", i as u64, "");

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
                            log_hex("S5 SLP_TYPa = ", S5_SLP_TYPa as u64, "");
                            log_hex("S5 SLP_TYPb = ", S5_SLP_TYPb as u64, "");
                        }
                    }
                }
                return;
            }
        }
    }

    log("\\_S5 not found in DSDT, shutdown via ACPI unavailable");
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
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

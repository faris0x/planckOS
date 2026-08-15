// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

// ── Advanced Programmable Interrupt Controller (xAPIC) ──────────
//
// Local APIC (MMIO at LAPIC_BASE) + IO-APIC + local APIC timer + IPIs.
// The local APIC registers are accessed via the identity-mapped 3-4 GB
// MMIO region set up by the loader (cache-disabled PDEs), so no page
// table work is needed here.

use core::sync::atomic::{AtomicBool, Ordering};

use super::acpi;
use super::serial;

pub const IOAPIC_DEFAULT_BASE: u64 = 0xFEC0_0000;

// Local APIC register offsets (from LAPIC base).
const REG_LAPIC_ID: u32 = 0x20;
const REG_SVR: u32 = 0xF0;
const REG_EOI: u32 = 0xB0;
const REG_ICR_LO: u32 = 0x300;
const REG_ICR_HI: u32 = 0x310;
const REG_LVT_TIMER: u32 = 0x320;
const REG_INIT_COUNT: u32 = 0x380;
const REG_DIVIDE: u32 = 0x3E0;

// IO-APIC register window.
const IOAPIC_SEL: u64 = 0x00;
const IOAPIC_WIN: u64 = 0x10;

pub const TIMER_VECTOR: u8 = 0x20;

static APIC_READY: AtomicBool = AtomicBool::new(false);
static mut BSP_APIC_ID: u8 = 0;
static mut IOAPIC_BASE: u64 = IOAPIC_DEFAULT_BASE;
static mut TICKS_PER_MS: u32 = 0;

// ── MMIO helpers ────────────────────────────────────────────────

unsafe fn lapic_read(reg: u32) -> u32 {
    let base = acpi::MADT.lapic_base;
    core::ptr::read_volatile((base as usize + reg as usize) as *const u32)
}

unsafe fn lapic_write(reg: u32, val: u32) {
    let base = acpi::MADT.lapic_base;
    core::ptr::write_volatile((base as usize + reg as usize) as *mut u32, val);
}

unsafe fn ioapic_write(reg: u32, val: u32) {
    let base = IOAPIC_BASE;
    core::ptr::write_volatile((base as usize + IOAPIC_SEL as usize) as *mut u32, reg);
    core::ptr::write_volatile((base as usize + IOAPIC_WIN as usize) as *mut u32, val);
}

unsafe fn ioapic_read(reg: u32) -> u32 {
    let base = IOAPIC_BASE;
    core::ptr::write_volatile((base as usize + IOAPIC_SEL as usize) as *mut u32, reg);
    core::ptr::read_volatile((base as usize + IOAPIC_WIN as usize) as *const u32)
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let hi: u32;
    let lo: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | lo as u64
}

unsafe fn wrmsr(msr: u32, val: u64) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") val as u32,
        in("edx") (val >> 32) as u32,
        options(nomem, nostack)
    );
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

// ── Public API ──────────────────────────────────────────────────

pub fn is_ready() -> bool {
    APIC_READY.load(Ordering::Acquire)
}

/// This core's local APIC ID (CPUID-correct once the APIC is enabled).
pub fn lapic_id() -> u8 {
    unsafe { ((lapic_read(REG_LAPIC_ID) >> 24) & 0xFF) as u8 }
}

/// Enable the local APIC on the running core (SVR bit 8). INIT resets the
/// APIC, so each AP must re-enable it before interrupts/IPIs can be
/// delivered to it.
pub fn enable_local() {
    unsafe {
        let svr = lapic_read(REG_SVR) | 0x100;
        lapic_write(REG_SVR, svr);
    }
}

/// Index of the running core within the MADT cpu list, or 0 if the APIC
/// is not yet ready. Used to tag logs and to find per-CPU state.
pub fn current_cpu_index() -> usize {
    if !is_ready() {
        return 0;
    }
    let id = lapic_id();
    for i in 0..unsafe { acpi::MADT.cpu_count } {
        if unsafe { acpi::MADT.cpu_apic_ids[i] } == id {
            return i;
        }
    }
    0
}

/// Enable the local APIC, mask the PICs, program the IO-APIC, and
/// calibrate the local APIC timer against the TSC. Call once, on the BSP.
pub fn init() {
    unsafe {
        // Enable the local APIC via APIC_BASE MSR (bit 11 = APIC enable).
        let msr = rdmsr(0x1B);
        if msr & (1 << 11) == 0 {
            wrmsr(0x1B, msr | (1 << 11));
        }

        BSP_APIC_ID = lapic_id();

        // SVR: software enable + spurious vector 0xFF.
        let svr = lapic_read(REG_SVR) | 0x100 | 0xFF;
        lapic_write(REG_SVR, svr);

        // IO-APIC: pick address from MADT, fall back to the default.
        if acpi::MADT.io_apic_addr != 0 {
            IOAPIC_BASE = acpi::MADT.io_apic_addr;
        }
        ioapic_mask_all();
        // Keyboard (IRQ1) delivered to the BSP.
        ioapic_route(1, 0x21, BSP_APIC_ID);

        // Fully mask the 8259 PICs.
        outb(0x21, 0xFF);
        outb(0xA1, 0xFF);

        // Arm the APIC timer (periodic, unmasked per QEMU convention) and
        // set the divider before calibrating. Use a huge count so it cannot
        // flood while the boot continues (INIT_COUNT defaults to 0 = fire
        // constantly). `calibrate_hz()` sets the real reload count later.
        lapic_write(REG_LVT_TIMER, 0x20000 | (TIMER_VECTOR as u32));
        lapic_write(REG_DIVIDE, 0x0B);
        lapic_write(REG_INIT_COUNT, 0xFFFF_FFFF);

        serial::log_hex("KERN", "APIC", "local APIC ID ", BSP_APIC_ID as u64, " enabled");

        APIC_READY.store(true, Ordering::Release);
    }
}

/// Empirically converge the APIC timer reload count so it fires at ~1000 Hz.
/// QEMU's TCG APIC-timer countdown is not a reliable frequency source, so
/// this calibrates against actual interrupts delivered through the IDT.
/// Call after the IDT is loaded, the timer armed, and `sti` executed.
pub fn calibrate_hz() {
    unsafe {
        let mut init = 0x0100_0000u32;
        for _ in 0..6 {
            lapic_write(REG_INIT_COUNT, init);
            let t0 = super::idt::TICK_COUNT.load(Ordering::Relaxed);
            let start = serial::rdtsc();
            let target = start.wrapping_add(serial::TSC_PER_MS.saturating_mul(100));
            while serial::rdtsc() < target {
                core::hint::spin_loop();
            }
            let ticks = super::idt::TICK_COUNT.load(Ordering::Relaxed) - t0;
            let hz = ticks * 10;
            serial::log_hex("KERN", "APIC", "timer rate (Hz): ", hz as u64, "");
            if hz == 0 {
                break;
            }
            let next = (((init as u64) * (hz as u64)) / 1000).max(1) as u32;
            if next == init {
                break;
            }
            init = next;
        }
        lapic_write(REG_INIT_COUNT, init);
        TICKS_PER_MS = init;
        serial::log_hex("KERN", "APIC", "timer reload count: ", init as u64, "");
    }
}

/// Log the current tick rate measured over a 100 ms window.
pub fn verify_hz(tag: &str) {
    let t0 = super::idt::TICK_COUNT.load(Ordering::Relaxed);
    let start = serial::rdtsc();
    let target = start.wrapping_add(unsafe { serial::TSC_PER_MS }.saturating_mul(100));
    while serial::rdtsc() < target {
        core::hint::spin_loop();
    }
    let hz = (super::idt::TICK_COUNT.load(Ordering::Relaxed) - t0) * 10;
    serial::log("KERN", "APIC", tag);
    serial::log_hex("KERN", "APIC", "verify rate (Hz): ", hz as u64, "");
}





/// Acknowledge the local APIC interrupt.
pub fn eoi() {
    unsafe {
        lapic_write(REG_EOI, 0);
    }
}

/// Blocking delay using the TSC (only safe once the TSC is calibrated).
pub fn delay_ms(ms: u64) {
    let start = serial::rdtsc();
    let per_ms = unsafe { serial::TSC_PER_MS };
    let target = start.wrapping_add(per_ms.saturating_mul(ms));
    loop {
        if serial::rdtsc() >= target {
            break;
        }
        core::hint::spin_loop();
    }
}

// ── IPI ─────────────────────────────────────────────────────────

unsafe fn wait_icr_idle() {
    while lapic_read(REG_ICR_LO) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
}

/// Send a fixed-delivery IPI to a specific APIC ID.
pub fn send_ipi(apic_id: u8, vector: u8) {
    unsafe {
        wait_icr_idle();
        lapic_write(REG_ICR_HI, (apic_id as u32) << 24);
        lapic_write(REG_ICR_LO, vector as u32);
        wait_icr_idle();
    }
}

/// Send the INIT deassert/assert IPI to a target core.
pub fn send_init(apic_id: u8) {
    unsafe {
        wait_icr_idle();
        lapic_write(REG_ICR_HI, (apic_id as u32) << 24);
        lapic_write(REG_ICR_LO, 0x500);
        wait_icr_idle();
    }
}

/// Send a STARTUP (SIPI) IPI targeting the 4K page `vector * 0x1000`.
pub fn send_sipi(apic_id: u8, vector: u8) {
    unsafe {
        wait_icr_idle();
        lapic_write(REG_ICR_HI, (apic_id as u32) << 24);
        lapic_write(REG_ICR_LO, 0x600 | (vector as u32));
        wait_icr_idle();
    }
}

// ── IO-APIC ─────────────────────────────────────────────────────

/// Mask every IO-APIC redirection entry.
unsafe fn ioapic_mask_all() {
    let ver = ioapic_read(0x01);
    let max_redir = ((ver >> 16) & 0xFF) as u32;
    for i in 0..=max_redir {
        let reg = 0x10 + i * 2;
        ioapic_write(reg + 1, 0); // destination
        ioapic_write(reg, 1 << 16); // masked
    }
}

/// Route an IO-APIC pin to a target core's vector (physical delivery).
pub fn ioapic_route(irq: u32, vector: u8, dest_apic_id: u8) {
    unsafe {
        let reg = 0x10 + irq * 2;
        ioapic_write(reg + 1, (dest_apic_id as u32) << 24);
        ioapic_write(reg, vector as u32); // fixed, edge, unmasked
    }
}

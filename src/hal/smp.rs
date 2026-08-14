// ── SMP: secondary-core bring-up (INIT-SIPI + per-core context) ──

use alloc::alloc::{alloc, Layout};

use super::acpi::{self, MAX_CPUS};
use super::{apic, idt, serial};

pub const STACK_SIZE: usize = 0x8000; // 32 KiB per AP stack
pub const IPI_VECTOR: u8 = 0x40;

const TRAMPOLINE_BASE: usize = 0x8000;
const STATE_RUNNING: u8 = 2;

/// Per-core state. Plain fields (not atomics) so the const-initialised array
/// stays `Copy`; cross-core handshakes use volatile access through raw
/// pointers (x86 TSO makes these single-writer/single-reader protocols safe).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PerCpu {
    pub idt: [idt::IdtEntry; 256],
    pub gdt: [u64; 7],
    pub tss: [u8; 104],
    pub stack_top: usize,
    pub apic_id: u8,
    pub index: usize,
    pub state: u8,
    pub ipi_count: usize,
    pub ipi_flag: bool,
}

const PER_CPU_INIT: PerCpu = PerCpu {
    idt: [idt::IdtEntry::zero(); 256],
    gdt: [0; 7],
    tss: [0; 104],
    stack_top: 0,
    apic_id: 0,
    index: 0,
    state: 0,
    ipi_count: 0,
    ipi_flag: false,
};

static mut CPU_AREA: [PerCpu; MAX_CPUS] = [PER_CPU_INIT; MAX_CPUS];

// Trampoline binary assembled by build.rs (nasm -> OUT_DIR/trampoline.bin).
static TRAMPOLINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/trampoline.bin"));

// Marker signatures in trampoline.asm for locating patch slots.
const M_CR3: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
const M_GDT: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const M_IDT: [u8; 4] = [0x01, 0x23, 0x45, 0x67];
const M_STACK: [u8; 4] = [0x89, 0xAB, 0xCD, 0xEF];
const M_CPU: [u8; 4] = [0xFE, 0xDC, 0xBA, 0x98];
const M_ENTRY: [u8; 4] = [0x76, 0x54, 0x32, 0x10];

pub fn cpu_idt_mut(cpu: usize) -> &'static mut [idt::IdtEntry; 256] {
    unsafe { &mut *core::ptr::addr_of_mut!(CPU_AREA[cpu].idt) }
}

fn cpu_ptr(cpu: usize) -> *mut PerCpu {
    unsafe { core::ptr::addr_of_mut!(CPU_AREA[cpu]) }
}

// Volatile per-core field access (single-writer/single-reader handshakes).
unsafe fn get_state(pc: *mut PerCpu) -> u8 {
    core::ptr::read_volatile(&(*pc).state)
}
unsafe fn set_state(pc: *mut PerCpu, v: u8) {
    core::ptr::write_volatile(&mut (*pc).state, v);
}
unsafe fn set_ipi_flag(pc: *mut PerCpu) {
    core::ptr::write_volatile(&mut (*pc).ipi_flag, true);
}
unsafe fn take_ipi_flag(pc: *mut PerCpu) -> bool {
    let v = core::ptr::read_volatile(&(*pc).ipi_flag);
    core::ptr::write_volatile(&mut (*pc).ipi_flag, false);
    v
}

// ── BSP entry ────────────────────────────────────────────────────

/// Prepare all per-CPU areas, then boot every secondary core. Called on the
/// BSP after the BSP IDT is loaded and the APIC is initialised.
pub fn init() {
    let count = unsafe { acpi::MADT.cpu_count };
    serial::log_hex("SMP", "SMP ", "cores detected: ", count as u64, "");
    if count == 0 {
        serial::log("SMP", "SMP ", "no MADT entries - SMP disabled");
        return;
    }
    for i in 0..count {
        setup_cpu(i);
    }
    unsafe { set_state(cpu_ptr(0), STATE_RUNNING) }

    if count < 2 {
        serial::log("SMP", "SMP ", "single-core system");
        return;
    }
    for i in 1..count {
        boot_ap(i);
    }

    // Smoke test: BSP -> AP1 IPI ping.
    if unsafe { get_state(cpu_ptr(1)) } == STATE_RUNNING {
        apic::delay_ms(5);
        apic::send_ipi(unsafe { acpi::MADT.cpu_apic_ids[1] }, IPI_VECTOR);
        serial::log("SMP", "IPI ", "ping sent to AP core 1");
    }
}

/// Build the per-CPU GDT (incl. TSS descriptor), IDT and bookkeeping.
fn setup_cpu(cpu: usize) {
    let pc = cpu_ptr(cpu);
    unsafe {
        (*pc).apic_id = acpi::MADT.cpu_apic_ids[cpu];
        (*pc).index = cpu;
        // GDT: null + flat code32/data32/code64/data64 + TSS descriptor.
        (*pc).gdt[0] = 0x0000000000000000;
        (*pc).gdt[1] = 0x00CF9A000000FFFF;
        (*pc).gdt[2] = 0x00CF92000000FFFF;
        (*pc).gdt[3] = 0x00209A0000000000;
        (*pc).gdt[4] = 0x0000920000000000;
        let tss_base = core::ptr::addr_of_mut!((*pc).tss) as u64;
        (*pc).gdt[5] = tss_desc_lo(tss_base, 103);
        (*pc).gdt[6] = tss_base >> 32;
        idt::build_idt_into(&mut (*pc).idt);
    }
}

/// INIT-SIPI a secondary core and wait (bounded) for it to come online.
fn boot_ap(cpu: usize) {
    let id = unsafe { acpi::MADT.cpu_apic_ids[cpu] };
    serial::log_hex("SMP", "BOOT", "starting AP (apic id ", id as u64, ")");

    let layout = Layout::from_size_align(STACK_SIZE, 16).unwrap();
    let stk = unsafe { alloc(layout) };
    if stk.is_null() {
        serial::log("SMP", "BOOT", "stack allocation FAILED");
        return;
    }
    let stack_top = (stk as usize) + STACK_SIZE;
    unsafe {
        (*cpu_ptr(cpu)).stack_top = stack_top;
        let tss =
            core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!((*cpu_ptr(cpu)).tss) as *mut u8, 104);
        tss[4..12].copy_from_slice(&(stack_top as u64).to_le_bytes());
    }

    // Patch a private copy of the trampoline, then place it at 0x8000.
    let mut blob = [0u8; 4096];
    let n = TRAMPOLINE.len().min(blob.len());
    blob[..n].copy_from_slice(&TRAMPOLINE[..n]);
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack)); }
    let gdt_base = unsafe { core::ptr::addr_of_mut!((*cpu_ptr(cpu)).gdt) } as u64;
    let gdt_limit = (core::mem::size_of::<[u64; 7]>() - 1) as u16;
    let idt_base = unsafe { core::ptr::addr_of_mut!((*cpu_ptr(cpu)).idt) } as u64;
    let idt_limit = (core::mem::size_of::<[idt::IdtEntry; 256]>() - 1) as u16;

    // Resolve all patch offsets first (avoids mutable/immutable borrow clash).
    let o_cr3 = find_marker(&blob, M_CR3);
    let o_gdt = find_marker(&blob, M_GDT);
    let o_idt = find_marker(&blob, M_IDT);
    let o_stack = find_marker(&blob, M_STACK);
    let o_cpu = find_marker(&blob, M_CPU);
    let o_entry = find_marker(&blob, M_ENTRY);

    write_u64(&mut blob, o_cr3, cr3);
    write_u16(&mut blob, o_gdt, gdt_limit);
    write_u64(&mut blob, o_gdt + 2, gdt_base);
    write_u16(&mut blob, o_idt, idt_limit);
    write_u64(&mut blob, o_idt + 2, idt_base);
    write_u64(&mut blob, o_stack, stack_top as u64);
    write_u64(&mut blob, o_cpu, cpu as u64);
    write_u64(
        &mut blob,
        o_entry,
        (ap_main64 as extern "C" fn(u64) -> !) as usize as u64,
    );

    unsafe {
        core::ptr::copy_nonoverlapping(blob.as_ptr(), TRAMPOLINE_BASE as *mut u8, n);
    }

    // Wake the core: INIT, 10 ms, then two SIPIs.
    apic::send_init(id);
    apic::delay_ms(10);
    apic::send_sipi(id, 0x08);
    apic::delay_ms(1);
    apic::send_sipi(id, 0x08);

    // Wait (bounded) for the AP to report RUNNING.
    let deadline = serial::rdtsc().wrapping_add(unsafe { serial::TSC_PER_MS }.saturating_mul(500));
    loop {
        if unsafe { get_state(cpu_ptr(cpu)) } == STATE_RUNNING {
            serial::log_hex("SMP", "BOOT", "AP core ", cpu as u64, " online");
            return;
        }
        if serial::rdtsc() >= deadline {
            serial::log_hex("SMP", "BOOT", "AP core ", cpu as u64, " STARTUP TIMEOUT");
            return;
        }
        core::hint::spin_loop();
    }
}

// ── AP entry (executed on the new core, long mode, own IDT/GDT) ──

#[no_mangle]
pub extern "C" fn ap_main64(cpu: u64) -> ! {
    let idx = cpu as usize;
    let pc = cpu_ptr(idx);
    unsafe {
        // INIT reset this core's local APIC; re-enable it before sti so
        // IPIs can be delivered.
        apic::enable_local();
        set_state(pc, STATE_RUNNING);
        core::arch::asm!("sti", options(nomem, nostack));
    }
    serial::log("SMP", "AP  ", "core online - idle loop");

    loop {
        if unsafe { take_ipi_flag(pc) } {
            serial::log("SMP", "IPI ", "IPI received on AP");
        } else {
            unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
        }
    }
}

// ── IPI handler (runs on the receiving core) ─────────────────────

pub fn handle_ipi() {
    let idx = apic::current_cpu_index();
    let pc = cpu_ptr(idx);
    unsafe {
        let n = core::ptr::read_volatile(&(*pc).ipi_count);
        core::ptr::write_volatile(&mut (*pc).ipi_count, n + 1);
        set_ipi_flag(pc);
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn find_marker(blob: &[u8], marker: [u8; 4]) -> usize {
    blob.windows(4)
        .position(|w| w == marker)
        .expect("trampoline marker not found")
        + 4
}

fn write_u16(blob: &mut [u8], off: usize, val: u16) {
    blob[off..off + 2].copy_from_slice(&val.to_le_bytes());
}

fn write_u64(blob: &mut [u8], off: usize, val: u64) {
    blob[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

/// Build the low 64 bits of a 64-bit TSS descriptor in the per-CPU GDT.
fn tss_desc_lo(base: u64, limit: u64) -> u64 {
    let mut lo = 0u64;
    lo |= limit & 0xFFFF; // limit[15:0]
    lo |= (base & 0xFFFF) << 16; // base[15:0]
    lo |= ((base >> 16) & 0xFF) << 32; // base[23:16]
    lo |= 0x89u64 << 40; // present, available 64-bit TSS
    lo |= ((limit >> 16) & 0xF) << 48; // limit[19:16]
    lo |= ((base >> 24) & 0xFF) << 56; // base[31:24]
    lo
}

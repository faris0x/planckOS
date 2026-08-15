// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

use super::display::VgaDisplay;
use super::input;
use super::apic;
use super::serial;
use super::smp;
use super::Display;

use core::sync::atomic::{AtomicUsize, Ordering};

/// Number of timer ticks since boot (~1000 Hz local APIC timer).
pub static TICK_COUNT: AtomicUsize = AtomicUsize::new(0);

// ── ISR stubs ────────────────────────────────────────────────────

core::arch::global_asm!(
    ".globl isr_0",
    "isr_0: push 0; push 0; jmp idt_common",
    ".globl isr_1",
    "isr_1: push 0; push 1; jmp idt_common",
    ".globl isr_2",
    "isr_2: push 0; push 2; jmp idt_common",
    ".globl isr_3",
    "isr_3: push 0; push 3; jmp idt_common",
    ".globl isr_4",
    "isr_4: push 0; push 4; jmp idt_common",
    ".globl isr_5",
    "isr_5: push 0; push 5; jmp idt_common",
    ".globl isr_6",
    "isr_6: push 0; push 6; jmp idt_common",
    ".globl isr_7",
    "isr_7: push 0; push 7; jmp idt_common",
    ".globl isr_8",
    "isr_8: push 8; jmp idt_common",
    ".globl isr_9",
    "isr_9: push 0; push 9; jmp idt_common",
    ".globl isr_10",
    "isr_10: push 10; jmp idt_common",
    ".globl isr_11",
    "isr_11: push 11; jmp idt_common",
    ".globl isr_12",
    "isr_12: push 12; jmp idt_common",
    ".globl isr_13",
    "isr_13: push 13; jmp idt_common",
    ".globl isr_14",
    "isr_14: push 14; jmp idt_common",
    ".globl isr_15",
    "isr_15: push 0; push 15; jmp idt_common",
    ".globl isr_16",
    "isr_16: push 0; push 16; jmp idt_common",
    ".globl isr_17",
    "isr_17: push 17; jmp idt_common",
    ".globl isr_18",
    "isr_18: push 0; push 18; jmp idt_common",
    ".globl isr_19",
    "isr_19: push 0; push 19; jmp idt_common",
    ".globl isr_20",
    "isr_20: push 0; push 20; jmp idt_common",
    ".globl isr_32",
    "isr_32: push 0; push 32; jmp idt_common",
    ".globl isr_33",
    "isr_33: push 0; push 33; jmp idt_common",
    ".globl isr_64",
    "isr_64: push 0; push 64; jmp idt_common",
    "idt_common:",
    "  push rax",
    "  push rcx",
    "  push rdx",
    "  push rbx",
    "  push rbp",
    "  push rsi",
    "  push rdi",
    "  push r8",
    "  push r9",
    "  push r10",
    "  push r11",
    "  push r12",
    "  push r13",
    "  push r14",
    "  push r15",
    "  mov rdi, [rsp + 15*8]",
    "  mov rsi, [rsp + 16*8]",
    "  call idt_handler",
    "  pop r15",
    "  pop r14",
    "  pop r13",
    "  pop r12",
    "  pop r11",
    "  pop r10",
    "  pop r9",
    "  pop r8",
    "  pop rdi",
    "  pop rsi",
    "  pop rbp",
    "  pop rbx",
    "  pop rdx",
    "  pop rcx",
    "  pop rax",
    "  add rsp, 16",
    "  iretq"
);

extern "C" {
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_32();
    fn isr_33();
    fn isr_64();
}

// ── IDT structures ───────────────────────────────────────────────

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
pub(crate) struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    pub(crate) const fn zero() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }
}

#[repr(C, packed(2))]
struct Idtr {
    limit: u16,
    base: u64,
}

const GDT_CODE64: u16 = 0x18;
const IDT_INTR_GATE: u8 = 0x8E;

// Vector map: 0x20 APIC timer, 0x21 PS/2 keyboard, 0x40 IPI doorbell.
const TIMER_VECTOR: u8 = 0x20;
const KBD_VECTOR: u8 = 0x21;
const IPI_VECTOR: u8 = 0x40;

/// Fill `idt` with the standard exception + planckOS interrupt vectors.
pub fn build_idt_into(idt: &mut [IdtEntry; 256]) {
    unsafe {
        macro_rules! set_isr {
            ($n:expr, $f:ident) => { set_entry(idt, $n, $f as *const () as u64) }
        }
        set_isr!(0, isr_0);
        set_isr!(1, isr_1);
        set_isr!(2, isr_2);
        set_isr!(3, isr_3);
        set_isr!(4, isr_4);
        set_isr!(5, isr_5);
        set_isr!(6, isr_6);
        set_isr!(7, isr_7);
        set_isr!(8, isr_8);
        set_isr!(9, isr_9);
        set_isr!(10, isr_10);
        set_isr!(11, isr_11);
        set_isr!(12, isr_12);
        set_isr!(13, isr_13);
        set_isr!(14, isr_14);
        set_isr!(15, isr_15);
        set_isr!(16, isr_16);
        set_isr!(17, isr_17);
        set_isr!(18, isr_18);
        set_isr!(19, isr_19);
        set_isr!(20, isr_20);
        set_isr!(TIMER_VECTOR, isr_32);
        set_isr!(KBD_VECTOR, isr_33);
        set_isr!(IPI_VECTOR, isr_64);
    }
}

/// Load the given IDT into the running core.
pub fn load_idt_into(idt: &[IdtEntry; 256]) {
    unsafe {
        let idtr = Idtr {
            limit: (core::mem::size_of::<IdtEntry>() * 256 - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &idtr, options(readonly, nostack));
    }
}

unsafe fn set_entry(idt: &mut [IdtEntry; 256], num: u8, handler: u64) {
    let idx = num as usize;
    idt[idx] = IdtEntry {
        offset_low: handler as u16,
        selector: GDT_CODE64,
        ist: 0,
        flags: IDT_INTR_GATE,
        offset_mid: (handler >> 16) as u16,
        offset_high: (handler >> 32) as u32,
        reserved: 0,
    };
}

// ── Interrupt controller (BSP entry point) ──────────────────────

pub struct InterruptController {
    display: VgaDisplay,
}

impl InterruptController {
    pub const fn new(display: VgaDisplay) -> Self {
        InterruptController { display }
    }

    /// Build and load the BSP IDT. The APIC timer is armed separately
    /// via `apic::start_timer()` once `apic::init()` has completed.
    pub fn init(&mut self) {
        let idt = smp::cpu_idt_mut(0);
        build_idt_into(idt);
        load_idt_into(idt);
    }
}

// ── Handler ──────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn idt_handler(int_no: u64, error_code: u64) {
    match int_no {
        32 => {
            TICK_COUNT.fetch_add(1, Ordering::Relaxed);
            apic::eoi();
        }
        33 => {
            input::irq1_handler();
            apic::eoi();
        }
        64 => {
            smp::handle_ipi();
            apic::eoi();
        }
        _ => {
            fault(int_no, error_code);
        }
    }
}

/// Fatal exception: log on serial; show on VGA from the BSP; halt the
/// offending core (other cores keep running — isolation for the shim).
fn fault(int_no: u64, error_code: u64) {
    let cpu = apic::current_cpu_index();
    serial::log_hex("KERN", "EXC", "exception #", int_no, "");
    serial::log_hex("KERN", "EXC", "error code ", error_code, "");
    if int_no == 14 {
        let cr2: u64;
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        }
        serial::log_hex("KERN", "EXC", "CR2 = ", cr2, "");
        serial::log_hex("KERN", "EXC", "CR3 = ", cr3, "");
    }
    if cpu == 0 {
        let mut display = VgaDisplay::new();
        display.write("EXC #");
        print_hex(&mut display, int_no);
        display.write(" ERR=");
        print_hex(&mut display, error_code);
        display.writeln(" -- HALT");
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
    }
}

fn print_hex(display: &mut VgaDisplay, val: u64) {
    let hex = b"0123456789ABCDEF";
    for i in (0..16).rev() {
        let nybble = ((val >> (i * 4)) & 0xF) as usize;
        display.putchar(hex[nybble]);
    }
}

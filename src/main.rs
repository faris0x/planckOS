#![no_std]
#![no_main]

extern crate alloc;

mod hal;
mod shell;
mod applets;

use hal::display::VgaDisplay;
use hal::framebuffer::FramebufferDisplay;
use hal::input::Ps2Keyboard;
use hal::idt::InterruptController;
use hal::memory;
use hal::acpi;
use hal::serial;
use hal::ata::AtaChannel;
use hal::fat32;
use hal::{Display, DisplayBackend, Input};
use shell::Shell;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
}

/// Boot info structure stored at physical address 0x7000 by the loader.
#[repr(C, packed)]
struct BootInfo {
    vbe_available: u8,
    _pad: [u8; 7],
    fb_addr: u64,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u8,
    _pad2: [u8; 3],
    tsc_per_ms: u64,
    boot_start_tsc: u64,
}

unsafe fn read_boot_info() -> BootInfo {
    let ptr = 0x7000 as *const BootInfo;
    core::ptr::read(ptr)
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    let boot_info = unsafe { read_boot_info() };
    serial::bootlog_init(boot_info.tsc_per_ms);

    serial::log("KERN", "ENTRY", "planckOS v0.1 x86-64 kernel @ 0x100000");
    serial::log_hex("KERN", "BOOTI", "VBE available: ", boot_info.vbe_available as u64, "");
    serial::log_hex("KERN", "BOOTI", "framebuffer @ ", boot_info.fb_addr, "");
    serial::log(
        "KERN",
        "BOOTI",
        format_res("TSC ", boot_info.tsc_per_ms, " ticks/ms (calibrated by boot sector)"),
    );

    unsafe { memory::init(); }

    let mut display = if boot_info.vbe_available == 1 {
        let fb_addr = boot_info.fb_addr;
        let width = boot_info.width as usize;
        let height = boot_info.height as usize;
        let pitch = boot_info.pitch as usize;
        let bpp = boot_info.bpp;
        serial::log_hex("KERN", "VIDEO", "framebuffer @ ", fb_addr, "");
        serial::log("KERN", "VIDEO", format_xy(width, height, bpp, pitch));
        DisplayBackend::Framebuffer(FramebufferDisplay::new(fb_addr, width, height, pitch, bpp))
    } else {
        serial::log("KERN", "VIDEO", "VBE unavailable, using VGA text mode");
        let mut vga = VgaDisplay::new();
        vga.init();
        DisplayBackend::Vga(vga)
    };

    display.writeln("planckOS v0.1 - x86-64 Rust");
    display.writeln("Copyright (C) 2026 Faris Alfarhan");
    display.writeln("Licensed GPLv3");

    // Interrupt controller — uses a separate VGA for exception display
    let mut idt = InterruptController::new(VgaDisplay::new());
    idt.init();
    serial::log("KERN", "IDT", "256-vector IDT loaded, PIC remapped to 0x20-0x2F, PIT IRQ0 armed");

    // PS/2 keyboard
    let mut input = Ps2Keyboard::new();
    input.init();
    serial::log("KERN", "INPUT", "PS/2 keyboard controller ready");

    // ACPI tables
    acpi::init();

    // Probe secondary master for FAT32 filesystem
    unsafe {
        static mut ATA: AtaChannel = AtaChannel::new(0x170, true);
        let ata: &'static mut AtaChannel = &mut *core::ptr::addr_of_mut!(ATA);

        if ata.probe() {
            serial::log("KERN", "ATA", "drive detected on secondary master (0x170)");
            match fat32::f_mount(ata) {
                Ok(()) => {
                    serial::log("KERN", "FS", "FAT32 mounted OK");
                    fat32::fat32_test();
                }
                Err(_e) => {
                    serial::log("KERN", "FS", "mount FAILED");
                }
            }
        } else {
            serial::log("KERN", "ATA", "no drive on secondary master (0x170)");
        }
    }

    // Enable interrupts
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) }

    // Build applet registry
    let registry = crate::applets::build_registry();

    // Run shell
    serial::log("KERN", "SHELL", "starting interactive shell");
    let shell = Shell::new(display, input, registry);
    shell.run();
}

/// Formats `prefix + decimal + suffix` into the serial scratch buffer.
fn format_res(prefix: &str, val: u64, suffix: &str) -> &'static str {
    let buf: &mut [u8; 224] = unsafe { &mut serial::FMT_BUF };
    let mut i = 0;
    for &b in prefix.as_bytes().iter() {
        buf[i] = b;
        i += 1;
    }
    let mut tmp = [0u8; 20];
    let mut j = 0;
    if val == 0 {
        buf[i] = b'0';
        i += 1;
    } else {
        let mut v = val;
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
    for &b in suffix.as_bytes().iter() {
        buf[i] = b;
        i += 1;
    }
    core::str::from_utf8(&buf[..i]).unwrap_or("?")
}

/// Formats "1920x1080 @ 32bpp, pitch 7680" into the serial scratch buffer.
fn format_xy(width: usize, height: usize, bpp: u8, pitch: usize) -> &'static str {
    let buf: &mut [u8; 224] = unsafe { &mut serial::FMT_BUF };
    let mut i = 0;
    i = format_dec(buf, i, width as u64);
    i = putc(buf, i, b'x');
    i = format_dec(buf, i, height as u64);
    i = putc(buf, i, b' ');
    i = putc(buf, i, b'@');
    i = putc(buf, i, b' ');
    i = format_dec(buf, i, bpp as u64);
    i = put_str(buf, i, "bpp, pitch ");
    i = format_dec(buf, i, pitch as u64);
    core::str::from_utf8(&buf[..i]).unwrap_or("?")
}

fn format_dec(buf: &mut [u8; 224], mut i: usize, mut v: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut j = 0;
    if v == 0 {
        buf[i] = b'0';
        return i + 1;
    }
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
    i
}

fn putc(buf: &mut [u8; 224], i: usize, b: u8) -> usize {
    buf[i] = b;
    i + 1
}

fn put_str(buf: &mut [u8; 224], mut i: usize, s: &str) -> usize {
    for &b in s.as_bytes() {
        buf[i] = b;
        i += 1;
    }
    i
}

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
}

unsafe fn read_boot_info() -> BootInfo {
    let ptr = 0x7000 as *const BootInfo;
    core::ptr::read(ptr)
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    unsafe { memory::init(); }

    serial::serial_debug(b"  [+] Kernel started in 64-bit mode\r\n\0");

    let boot_info = unsafe { read_boot_info() };

    let mut display = if boot_info.vbe_available == 1 {
        serial::serial_debug(b"  [+] VBE framebuffer available\r\n\0");
        let fb_addr = boot_info.fb_addr;
        let width = boot_info.width as usize;
        let height = boot_info.height as usize;
        let pitch = boot_info.pitch as usize;
        let bpp = boot_info.bpp;
        serial::serial_debug(b"  [+] Creating FramebufferDisplay\r\n\0");
        DisplayBackend::Framebuffer(FramebufferDisplay::new(fb_addr, width, height, pitch, bpp))
    } else {
        serial::serial_debug(b"  [+] VBE unavailable, using VGA text mode\r\n\0");
        let mut vga = VgaDisplay::new();
        vga.init();
        DisplayBackend::Vga(vga)
    };

    display.writeln("planckOS v0.1 - x86-64 Rust");

    // Interrupt controller — uses a separate VGA for exception display
    let mut idt = InterruptController::new(VgaDisplay::new());
    idt.init();

    // PS/2 keyboard
    let mut input = Ps2Keyboard::new();
    input.init();

    // ACPI tables
    acpi::init();

    // Probe secondary master for FAT32 filesystem
    unsafe {
        static mut ATA: AtaChannel = AtaChannel::new(0x170, true);
        let ata: &'static mut AtaChannel = &mut *core::ptr::addr_of_mut!(ATA);

        if ata.probe() {
            serial::serial_debug(b"  [ATA] Drive detected\r\n\0");

            match fat32::f_mount(ata) {
                Ok(()) => {
                    serial::serial_debug(b"  [FAT32] Mounted OK\r\n\0");
                    fat32::fat32_test();
                }
                Err(_e) => {
                    serial::serial_debug(b"  [FAT32] Mount failed\r\n\0");
                }
            }
        } else {
            serial::serial_debug(b"  [ATA] No drive\r\n\0");
        }
    }

    // Enable interrupts
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) }

    // Build applet registry
    let registry = crate::applets::build_registry();

    // Run shell
    serial::serial_debug(b"  [MAIN] Starting shell...\r\n\0");
    let shell = Shell::new(display, input, registry);
    shell.run();
}

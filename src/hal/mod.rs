pub mod display;
pub mod input;
pub mod idt;
pub mod memory;
pub mod serial;
pub mod acpi;
pub mod cpuid;
pub mod e820;
pub mod rtc;
pub mod block;
pub mod ata;
pub mod fat32;
pub mod framebuffer;

use display::VgaDisplay;
use framebuffer::FramebufferDisplay;

// ── HAL Traits ────────────────────────────────────────────────────
// Applets interact with hardware exclusively through these traits.
// No direct port I/O or memory-mapped access from applet code.

pub trait Display {
    fn clear(&mut self);
    fn write(&mut self, text: &str);
    fn writeln(&mut self, text: &str);
    fn putchar(&mut self, c: u8);
    fn set_cursor(&mut self, row: usize, col: usize);
    fn scroll(&mut self);
    fn reset_cursor(&mut self);
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;
}

pub trait Input {
    fn getchar(&mut self) -> u8;
    fn init(&mut self);
}

pub enum DisplayBackend {
    Vga(VgaDisplay),
    Framebuffer(FramebufferDisplay),
}

impl Display for DisplayBackend {
    fn clear(&mut self) {
        match self {
            DisplayBackend::Vga(d) => d.clear(),
            DisplayBackend::Framebuffer(d) => d.clear(),
        }
    }

    fn write(&mut self, text: &str) {
        match self {
            DisplayBackend::Vga(d) => d.write(text),
            DisplayBackend::Framebuffer(d) => d.write(text),
        }
    }

    fn writeln(&mut self, text: &str) {
        match self {
            DisplayBackend::Vga(d) => d.writeln(text),
            DisplayBackend::Framebuffer(d) => d.writeln(text),
        }
    }

    fn putchar(&mut self, c: u8) {
        match self {
            DisplayBackend::Vga(d) => d.putchar(c),
            DisplayBackend::Framebuffer(d) => d.putchar(c),
        }
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        match self {
            DisplayBackend::Vga(d) => d.set_cursor(row, col),
            DisplayBackend::Framebuffer(d) => d.set_cursor(row, col),
        }
    }

    fn scroll(&mut self) {
        match self {
            DisplayBackend::Vga(d) => d.scroll(),
            DisplayBackend::Framebuffer(d) => d.scroll(),
        }
    }

    fn reset_cursor(&mut self) {
        match self {
            DisplayBackend::Vga(d) => d.reset_cursor(),
            DisplayBackend::Framebuffer(d) => d.reset_cursor(),
        }
    }

    fn cols(&self) -> usize {
        match self {
            DisplayBackend::Vga(d) => d.cols(),
            DisplayBackend::Framebuffer(d) => d.cols(),
        }
    }

    fn rows(&self) -> usize {
        match self {
            DisplayBackend::Vga(d) => d.rows(),
            DisplayBackend::Framebuffer(d) => d.rows(),
        }
    }
}

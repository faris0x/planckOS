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
}

pub trait Input {
    fn getchar(&mut self) -> u8;
    fn init(&mut self);
}

use core::ptr::{read_volatile, write_volatile};

use super::Display;

const BUF: *mut u16 = 0xB8000 as *mut u16;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

const COLOR: u8 = 0x5F; // white on deep purple

static mut COL: usize = 0;
static mut ROW: usize = 0;

fn entry(c: u8, color: u8) -> u16 {
    (color as u16) << 8 | c as u16
}

pub struct VgaDisplay;

impl VgaDisplay {
    pub const fn new() -> Self {
        VgaDisplay
    }

    pub fn init(&self) {
        set_palette();
        self.clear_impl();
        self.set_cursor_impl(0, 0);
    }
}

impl Display for VgaDisplay {
    fn clear(&mut self) {
        self.clear_impl();
    }

    fn write(&mut self, text: &str) {
        self.write_impl(text);
    }

    fn writeln(&mut self, text: &str) {
        self.writeln_impl(text);
    }

    fn putchar(&mut self, c: u8) {
        self.putchar_impl(c);
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        self.set_cursor_impl(row, col);
    }

    fn scroll(&mut self) {
        self.scroll_impl();
    }

    fn reset_cursor(&mut self) {
        self.reset_cursor_impl();
    }

    fn cols(&self) -> usize {
        WIDTH
    }

    fn rows(&self) -> usize {
        HEIGHT
    }
}

// ── Implementation methods (trait-unaware, for internal use) ─────

impl VgaDisplay {
    fn clear_impl(&self) {
        let blank = entry(b' ', COLOR);
        for i in 0..(WIDTH * HEIGHT) {
            unsafe { write_volatile(BUF.add(i), blank) }
        }
        self.reset_cursor_impl();
    }

    fn reset_cursor_impl(&self) {
        unsafe {
            COL = 0;
            ROW = 0;
            self.set_cursor_impl(0, 0);
        }
    }

    fn set_cursor_impl(&self, row: usize, col: usize) {
        let pos = row * WIDTH + col;
        unsafe {
            outb(0x3D4, 0x0E);
            outb(0x3D5, (pos >> 8) as u8);
            outb(0x3D4, 0x0F);
            outb(0x3D5, (pos & 0xFF) as u8);
        }
    }

    fn scroll_impl(&self) {
        let blank = entry(b' ', COLOR);
        for row in 0..(HEIGHT - 1) {
            for col in 0..WIDTH {
                let src = (row + 1) * WIDTH + col;
                let dst = row * WIDTH + col;
                unsafe {
                    let c = read_volatile(BUF.add(src));
                    write_volatile(BUF.add(dst), c);
                }
            }
        }
        let last_row = (HEIGHT - 1) * WIDTH;
        for col in 0..WIDTH {
            unsafe { write_volatile(BUF.add(last_row + col), blank) }
        }
    }

    fn putchar_impl(&self, c: u8) {
        if c == 0x08 {
            unsafe {
                if COL > 0 {
                    COL -= 1;
                    self.set_cursor_impl(ROW, COL);
                }
            }
            return;
        }
        if c == 0x0D {
            unsafe {
                COL = 0;
                self.set_cursor_impl(ROW, COL);
            }
            return;
        }
        if c == b'\n' {
            unsafe {
                COL = 0;
                ROW += 1;
                if ROW >= HEIGHT {
                    self.scroll_impl();
                    ROW = HEIGHT - 1;
                }
                self.set_cursor_impl(ROW, COL);
            }
            return;
        }
        unsafe {
            if COL >= WIDTH {
                COL = 0;
                ROW += 1;
                if ROW >= HEIGHT {
                    self.scroll_impl();
                    ROW = HEIGHT - 1;
                }
            }
            write_volatile(BUF.add(ROW * WIDTH + COL), entry(c, COLOR));
            COL += 1;
            self.set_cursor_impl(ROW, COL);
        }
    }

    fn write_impl(&self, s: &str) {
        for &b in s.as_bytes() {
            self.putchar_impl(b);
        }
    }

    fn writeln_impl(&self, s: &str) {
        self.write_impl(s);
        self.putchar_impl(b'\n');
    }
}

fn set_palette() {
    unsafe {
        outb(0x3C8, 5);
        outb(0x3C9, 4);
        outb(0x3C9, 0);
        outb(0x3C9, 4);
    }
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

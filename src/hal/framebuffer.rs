use crate::hal::Display;
use core::ptr;

const FONT_DATA: &[u8] = include_bytes!("fonts/spleen-32x64.psfu");

// PSF2 header structure
#[repr(C, packed)]
struct Psf2Header {
    magic: [u8; 4],
    version: u32,
    header_size: u32,
    flags: u32,
    num_glyphs: u32,
    glyph_size: u32,
    height: u32,
    width: u32,
}

pub struct FramebufferDisplay {
    fb: &'static mut [u32],
    width: usize,
    height: usize,
    pitch: usize,
    bpp: u8,
    cursor_x: usize,
    cursor_y: usize,
    fg: u32,
    bg: u32,
    scale: usize,
    glyph_w: usize,
    glyph_h: usize,
    glyph_data: &'static [u8],
    glyph_count: u32,
    unicode_start: *const u16,
    unicode_len: u32,
}

impl FramebufferDisplay {
    pub fn new(fb_addr: u64, width: usize, height: usize, pitch: usize, bpp: u8) -> Self {
        // Flush TLB by reloading CR3
        unsafe {
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nomem, nostack));
        }

        let fb_size = height * pitch;
        let fb_ptr = fb_addr as *mut u32;
        let fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr, fb_size / 4) };

        // Parse PSF2 header
        let header = unsafe { &*(FONT_DATA.as_ptr() as *const Psf2Header) };
        let g_w = if header.width != 0 { header.width as usize } else { 8 };
        let g_h = if header.height != 0 { header.height as usize } else { 16 };
        let g_size = header.glyph_size as usize;
        let n_glyphs = header.num_glyphs;
        let hdr_size = header.header_size as usize;

        // Glyph bitmap data starts after header
        let glyph_data = &FONT_DATA[hdr_size..hdr_size + g_size * n_glyphs as usize];

        // Unicode table starts after glyph data
        let unicode_start = if header.flags & 0x01 != 0 {
            let table_off = hdr_size + g_size * n_glyphs as usize;
            FONT_DATA[table_off..].as_ptr() as *const u16
        } else {
            ptr::null()
        };

        let unicode_len = if !unicode_start.is_null() {
            ((FONT_DATA.len() - (hdr_size + g_size * n_glyphs as usize)) / 2) as u32
        } else {
            0
        };

        let mut display = FramebufferDisplay {
            fb,
            width,
            height,
            pitch,
            bpp,
            cursor_x: 0,
            cursor_y: 0,
            fg: 0x00FFFFFF,
            bg: 0x00000000,
            scale: 1,
            glyph_w: g_w,
            glyph_h: g_h,
            glyph_data,
            glyph_count: n_glyphs,
            unicode_start,
            unicode_len,
        };

        display.clear();
        display
    }

    fn glyph_index(&self, c: u8) -> usize {
        let cp = c as u32;

        // Try Unicode table lookup for the codepoint
        if !self.unicode_start.is_null() {
            let table = unsafe { core::slice::from_raw_parts(self.unicode_start, self.unicode_len as usize) };
            let mut i = 0;
            while i < self.unicode_len as usize - 1 {
                if table[i] == cp as u16 {
                    return table[i + 1] as usize;
                }
                if table[i] == 0xFFFF {
                    i += 1;
                }
                i += 1;
            }
        }

        // Default: codepoint = index if within range
        if (cp as usize) < self.glyph_count as usize {
            cp as usize
        } else {
            cp as usize % self.glyph_count as usize
        }
    }

    fn draw_glyph(&mut self, glyph_idx: usize, x: usize, y: usize) {
        let g_size = self.glyph_w * self.glyph_h / 8;
        let base = glyph_idx * g_size;
        if base + g_size > self.glyph_data.len() {
            return;
        }

        let glyph = &self.glyph_data[base..base + g_size];
        let scale = self.scale;

        for row in 0..self.glyph_h {
            for col in 0..self.glyph_w {
                let byte_idx = row * (self.glyph_w / 8) + col / 8;
                let bit_idx = 7 - (col % 8);
                let on = if byte_idx < g_size {
                    (glyph[byte_idx] >> bit_idx) & 1
                } else {
                    0
                };

                let color = if on != 0 { self.fg } else { self.bg };

                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = x + col * scale + sx;
                        let py = y + row * scale + sy;
                        if px < self.width && py < self.height {
                            self.fb[py * (self.pitch / 4) + px] = color;
                        }
                    }
                }
            }
        }
    }
}

impl Display for FramebufferDisplay {
    fn putchar(&mut self, c: u8) {
        match c {
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\n' => {
                self.cursor_x = 0;
                self.cursor_y += self.glyph_h * self.scale;
                if self.cursor_y + self.glyph_h * self.scale > self.height {
                    self.scroll();
                }
            }
            0x08 => {
                // Backspace
                if self.cursor_x >= self.glyph_w * self.scale {
                    self.cursor_x -= self.glyph_w * self.scale;
                }
                self.draw_glyph(0, self.cursor_x, self.cursor_y); // space glyph
            }
            _ => {
                let idx = self.glyph_index(c);
                self.draw_glyph(idx, self.cursor_x, self.cursor_y);
                self.cursor_x += self.glyph_w * self.scale;
                if self.cursor_x + self.glyph_w * self.scale > self.width {
                    self.cursor_x = 0;
                    self.cursor_y += self.glyph_h * self.scale;
                    if self.cursor_y + self.glyph_h * self.scale > self.height {
                        self.scroll();
                    }
                }
            }
        }
    }

    fn write(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.putchar(b);
        }
    }

    fn writeln(&mut self, s: &str) {
        self.write(s);
        self.putchar(b'\n');
    }

    fn clear(&mut self) {
        for pixel in self.fb.iter_mut() {
            *pixel = self.bg;
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_x = col * self.glyph_w * self.scale;
        self.cursor_y = row * self.glyph_h * self.scale;
    }

    fn scroll(&mut self) {
        let row_bytes = self.glyph_h * self.scale * (self.pitch / 4);
        let total_rows = self.height / (self.glyph_h * self.scale);
        let fb_width = self.pitch / 4;

        unsafe {
            ptr::copy(
                self.fb.as_ptr().add(row_bytes),
                self.fb.as_mut_ptr(),
                (total_rows - 1) * row_bytes,
            );
        }

        for y in (total_rows - 1) * (self.glyph_h * self.scale)..self.height {
            for x in 0..self.width {
                self.fb[y * fb_width + x] = self.bg;
            }
        }

        if self.cursor_y > 0 {
            self.cursor_y = self.cursor_y.saturating_sub(self.glyph_h * self.scale);
        }
    }

    fn reset_cursor(&mut self) {
        self.cursor_x = 0;
        self.cursor_y = 0;
    }
}

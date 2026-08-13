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
    prev_cursor_x: usize,
    prev_cursor_y: usize,
    cursor_visible: bool,
    fg: u32,
    bg: u32,
    scale: usize,
    downscale: usize,
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
            prev_cursor_x: 0,
            prev_cursor_y: 0,
            cursor_visible: false,
            fg: 0x00FFFFFF,
            bg: 0x0017002E,
            scale: 1,
            downscale: 2,
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
        let downscale = self.downscale;
        let rendered_w = self.glyph_w / downscale;
        let rendered_h = self.glyph_h / downscale;

        for row in 0..rendered_h {
            for col in 0..rendered_w {
                let src_row = row * downscale;
                let src_col = col * downscale;
                let byte_idx = src_row * (self.glyph_w / 8) + src_col / 8;
                let bit_idx = 7 - (src_col % 8);
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

    fn draw_cursor(&mut self) {
        let char_w = (self.glyph_w / self.downscale) * self.scale;
        let char_h = (self.glyph_h / self.downscale) * self.scale;
        let cursor_h = 2;
        let fb_width = self.pitch / 4;

        // Erase cursor from previous position
        if self.cursor_visible {
            for y in (self.prev_cursor_y + char_h - cursor_h)..self.prev_cursor_y + char_h {
                for x in self.prev_cursor_x..self.prev_cursor_x + char_w {
                    if x < self.width && y < self.height {
                        self.fb[y * fb_width + x] = self.bg;
                    }
                }
            }
        }

        // Draw cursor at new position
        for y in (self.cursor_y + char_h - cursor_h)..self.cursor_y + char_h {
            for x in self.cursor_x..self.cursor_x + char_w {
                if x < self.width && y < self.height {
                    self.fb[y * fb_width + x] = self.fg;
                }
            }
        }

        // Update previous position
        self.prev_cursor_x = self.cursor_x;
        self.prev_cursor_y = self.cursor_y;
        self.cursor_visible = true;
    }
}

impl Display for FramebufferDisplay {
    fn putchar(&mut self, c: u8) {
        let char_w = (self.glyph_w / self.downscale) * self.scale;
        let char_h = (self.glyph_h / self.downscale) * self.scale;
        
        // Erase cursor from current position before drawing
        if self.cursor_visible {
            let cursor_h = 2;
            let fb_width = self.pitch / 4;
            for y in (self.cursor_y + char_h - cursor_h)..self.cursor_y + char_h {
                for x in self.cursor_x..self.cursor_x + char_w {
                    if x < self.width && y < self.height {
                        self.fb[y * fb_width + x] = self.bg;
                    }
                }
            }
            self.cursor_visible = false;
        }
        
        match c {
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\n' => {
                self.cursor_x = 0;
                self.cursor_y += char_h;
                if self.cursor_y + char_h > self.height {
                    self.scroll();
                }
            }
            0x08 => {
                if self.cursor_x >= char_w {
                    self.cursor_x -= char_w;
                }
                self.draw_glyph(self.glyph_index(b' '), self.cursor_x, self.cursor_y);
            }
            _ => {
                let idx = self.glyph_index(c);
                self.draw_glyph(idx, self.cursor_x, self.cursor_y);
                self.cursor_x += char_w;
                if self.cursor_x + char_w > self.width {
                    self.cursor_x = 0;
                    self.cursor_y += char_h;
                    if self.cursor_y + char_h > self.height {
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
        let char_w = (self.glyph_w / self.downscale) * self.scale;
        let char_h = (self.glyph_h / self.downscale) * self.scale;
        self.cursor_x = col * char_w;
        self.cursor_y = row * char_h;
    }

    fn scroll(&mut self) {
        let char_h = (self.glyph_h / self.downscale) * self.scale;
        let row_bytes = char_h * (self.pitch / 4);
        let total_rows = self.height / char_h;
        let fb_width = self.pitch / 4;

        unsafe {
            ptr::copy(
                self.fb.as_ptr().add(row_bytes),
                self.fb.as_mut_ptr(),
                (total_rows - 1) * row_bytes,
            );
        }

        for y in (total_rows - 1) * char_h..self.height {
            for x in 0..self.width {
                self.fb[y * fb_width + x] = self.bg;
            }
        }

        if self.cursor_y > 0 {
            self.cursor_y = self.cursor_y.saturating_sub(char_h);
        }
    }

    fn reset_cursor(&mut self) {
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn cols(&self) -> usize {
        let char_w = (self.glyph_w / self.downscale) * self.scale;
        self.width / char_w
    }

    fn rows(&self) -> usize {
        let char_h = (self.glyph_h / self.downscale) * self.scale;
        self.height / char_h
    }

    fn show_cursor(&mut self) {
        self.draw_cursor();
    }
}

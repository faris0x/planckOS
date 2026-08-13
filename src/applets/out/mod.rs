use crate::hal::display::VgaDisplay;

use crate::hal::input::Ps2Keyboard;
use crate::hal::fat32;
use crate::hal::fat32::FA_READ;
use crate::hal::Display;

const BUF_SIZE: usize = 512;
const HEX_COLS: usize = 16;

pub fn run(display: &mut VgaDisplay, _input: &mut Ps2Keyboard, args: &[&str]) {
    let (path, limit) = match parse_args(args) {
        Ok(r) => r,
        Err(e) => {
            display.write("out: ");
            display.writeln(e);
            return;
        }
    };

    if path.is_empty() {
        display.writeln("out: missing file operand");
        return;
    }

    let info = match fat32::f_stat(path) {
        Ok(i) => i,
        Err(_) => {
            display.write("out: cannot access '");
            display.write(path);
            display.writeln("': No such file or directory");
            return;
        }
    };

    if info.attributes & 0x10 != 0 {
        display.write("out: '");
        display.write(path);
        display.writeln("': Is a directory");
        return;
    }

    let mut file = match fat32::f_open(path, FA_READ) {
        Ok(f) => f,
        Err(_) => {
            display.write("out: cannot open '");
            display.write(path);
            display.writeln("'");
            return;
        }
    };

    let size = fat32::f_size(&file);

    // Auto-detect: check first BUF_SIZE bytes for null bytes
    let mut probe = [0u8; BUF_SIZE];
    let probe_len = fat32::f_read(&mut file, &mut probe).unwrap_or(0);
    let is_binary = probe[..probe_len].contains(&0);

    // Seek back to start
    let _ = fat32::f_lseek(&mut file, 0);

    if is_binary {
        // Hex dump mode
        let remain = if limit > 0 { limit.min(size as usize) } else { size as usize };
        let mut offset = 0u32;
        let mut hex_buf = [0u8; BUF_SIZE];

        while offset < remain as u32 {
            let to_read = (BUF_SIZE).min(remain - offset as usize);
            let read = fat32::f_read(&mut file, &mut hex_buf[..to_read]).unwrap_or(0);
            if read == 0 { break; }

            for row_start in (0..read).step_by(HEX_COLS) {
                // Offset
                let o = offset + row_start as u32;
                let off_str = [
                    b"0123456789ABCDEF"[((o >> 28) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 24) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 20) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 16) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 12) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 8) & 0xF) as usize],
                    b"0123456789ABCDEF"[((o >> 4) & 0xF) as usize],
                    b"0123456789ABCDEF"[(o & 0xF) as usize],
                    0u8,
                ];
                display.write(core::str::from_utf8(&off_str[..8]).unwrap_or("????????"));
                display.write("  ");

                // Hex bytes
                let end = (row_start + HEX_COLS).min(read);
                for j in row_start..end {
                    let b = hex_buf[j];
                    let hex_chars = [
                        b"0123456789ABCDEF"[(b >> 4) as usize],
                        b"0123456789ABCDEF"[(b & 0xF) as usize],
                        0u8,
                    ];
                    display.write(core::str::from_utf8(&hex_chars[..2]).unwrap_or("??"));
                    display.putchar(b' ');
                }
                // Padding for incomplete last row
                for _ in end..row_start + HEX_COLS {
                    display.write("   ");
                }

                display.write(" |");

                // ASCII
                for j in row_start..end {
                    let c = hex_buf[j];
                    if c >= 0x20 && c < 0x7F {
                        display.putchar(c);
                    } else {
                        display.putchar(b'.');
                    }
                }
                display.writeln("|");
            }

            offset += read as u32;
        }
    } else {
        // Text mode
        let remain = if limit > 0 { limit.min(size as usize) } else { size as usize };
        let mut remaining = remain;
        let mut text_buf = [0u8; BUF_SIZE];

        while remaining > 0 {
            let to_read = BUF_SIZE.min(remaining);
            let read = fat32::f_read(&mut file, &mut text_buf[..to_read]).unwrap_or(0);
            if read == 0 { break; }

            let s = core::str::from_utf8(&text_buf[..read]).unwrap_or("?");
            display.write(s);

            remaining -= read;
        }

        // Ensure trailing newline
        display.putchar(b'\n');
    }

    let _ = fat32::f_close(file);
}

fn parse_args<'a>(args: &[&'a str]) -> Result<(&'a str, usize), &'static str> {
    let mut path = "";
    let mut limit = 0usize;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i];
        if arg.starts_with('-') && arg.len() > 1 {
            let mut chars = arg[1..].chars();
            match chars.next() {
                Some('n') => {
                    i += 1;
                    if i < args.len() {
                        limit = args[i].parse().unwrap_or(0);
                    }
                }
                Some(_) => return Err("invalid option"),
                None => {}
            }
        } else {
            path = arg;
        }
        i += 1;
    }

    Ok((path, limit))
}
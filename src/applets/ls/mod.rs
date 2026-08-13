
use alloc::vec::Vec;


use crate::hal::input::Ps2Keyboard;
use crate::hal::fat32;
use crate::hal::fat32::FileInfo;
use crate::hal::fat32::Dir;
use crate::hal::Display;

struct LsFlags {
    long: bool,
    all: bool,
    sort_size: bool,
    reverse: bool,
}

impl Default for LsFlags {
    fn default() -> Self {
        LsFlags { long: false, all: false, sort_size: false, reverse: false }
    }
}

struct LsEntry {
    name: [u8; 13],
    size: u32,
    is_dir: bool,
}

const MAX_ENTRIES: usize = 256;
const WIDTH: usize = 80;

fn parse_ls_args<'a>(args: &[&'a str]) -> Result<(Vec<&'a str>, LsFlags), &'static str> {
    let mut flags = LsFlags::default();
    let mut paths = Vec::new();

    for arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            for (_, c) in arg[1..].char_indices() {
                match c {
                    'l' => flags.long = true,
                    'a' => flags.all = true,
                    'S' => flags.sort_size = true,
                    'r' => flags.reverse = true,
                    _ => return Err("invalid option"),
                }
            }
        } else {
            paths.push(*arg);
        }
    }

    if paths.is_empty() {
        paths.push("/");
    }

    Ok((paths, flags))
}

pub fn run(display: &mut dyn Display, _input: &mut Ps2Keyboard, args: &[&str]) {
    let (paths, flags) = match parse_ls_args(args) {
        Ok(r) => r,
        Err(e) => {
            display.write("ls: ");
            display.writeln(e);
            return;
        }
    };

    for path in &paths {
        if paths.len() > 1 {
            display.write(&path);
            display.writeln(":");
        }

        match fat32::f_opendir(path) {
            Ok(dir) => list_dir(display, dir, &flags),
            Err(_) => {
                display.write("ls: cannot access '");
                display.write(path);
                display.writeln("'");
            }
        }

        if paths.len() > 1 {
            display.putchar(b'\n');
        }
    }
}

fn list_dir(display: &mut dyn Display, mut dir: fat32::Dir, flags: &LsFlags) {
    let mut entries: [LsEntry; MAX_ENTRIES] = unsafe { core::mem::zeroed() };
    let mut count = 0usize;
    let mut name = [0u8; 13];

    loop {
        match fat32::f_readdir(&mut dir, &mut name) {
            Ok(true) => {
                // Skip . and .. unless -a
                if !flags.all && name[0] == b'.' && (name[1] == 0 || (name[1] == b'.' && name[2] == 0)) {
                    continue;
                }
                if count >= MAX_ENTRIES {
                    display.writeln("ls: too many entries");
                    break;
                }
                let name_str = core::str::from_utf8(&name).unwrap_or("?");
                let end = name_str.find('\0').unwrap_or(name_str.len());
                if end == 0 { continue; }

                let mut entry_name = [0u8; 13];
                entry_name[..end].copy_from_slice(&name[..end]);

                // Get file info via f_stat on the full path
                let mut full_path = [0u8; 256];
                full_path[0] = b'/';
                let path_end = 1;
                for j in 0..end {
                    if path_end + j < 256 {
                        full_path[path_end + j] = entry_name[j];
                    }
                }
                let path_str = core::str::from_utf8(&full_path[..path_end + end]).unwrap_or("?");
                let info = fat32::f_stat(path_str).ok();
                let (size, is_dir) = match info {
                    Some(i) => (i.size, i.attributes & 0x10 != 0),
                    None => (0, false),
                };

                entries[count] = LsEntry { name: entry_name, size, is_dir };
                count += 1;
            }
            Ok(false) => break,
            Err(_) => {
                display.writeln("ls: error reading directory");
                break;
            }
        }
    }

    let _ = fat32::f_closedir(dir);

    if count == 0 { return; }

    // Build index list for sorting
    let mut indices: [usize; MAX_ENTRIES] = unsafe { core::mem::zeroed() };
    for i in 0..count { indices[i] = i; }

    // Sort
    for i in 0..count {
        for j in i + 1..count {
            let a = entries[indices[i]].name;
            let b = entries[indices[j]].name;
            let a_str = core::str::from_utf8(&a).unwrap_or("?").trim_end_matches('\0');
            let b_str = core::str::from_utf8(&b).unwrap_or("?").trim_end_matches('\0');

            let a_size = entries[indices[i]].size;
            let b_size = entries[indices[j]].size;

            let swap = if flags.sort_size {
                if flags.reverse { a_size < b_size } else { a_size > b_size }
            } else {
                let cmp = a_str.as_bytes().iter().zip(b_str.as_bytes())
                    .find(|&(x, y)| x != y)
                    .map(|(x, y)| x.cmp(y))
                    .unwrap_or(a_str.len().cmp(&b_str.len()));
                if flags.reverse { cmp.is_gt() } else { cmp.is_lt() }
            };

            if swap {
                indices.swap(i, j);
            }
        }
    }

    if flags.long {
        // Long format: one entry per line
        for i in 0..count {
            let idx = indices[i];
            let e = &entries[idx];
            let name_str = core::str::from_utf8(&e.name).unwrap_or("?");
            let end = name_str.find('\0').unwrap_or(name_str.len());

            // Type character
            if e.is_dir { display.putchar(b'd'); } else { display.putchar(b' '); }
            display.putchar(b' ');

            // Size right-aligned to 8 chars
            let mut size_buf = [0u8; 9];
            let mut size_pos = 8;
            let mut v = e.size;
            loop {
                size_pos -= 1;
                size_buf[size_pos] = (v % 10) as u8 + b'0';
                v /= 10;
                if v == 0 || size_pos == 0 { break; }
            }
            while size_pos > 0 {
                size_pos -= 1;
                size_buf[size_pos] = b' ';
            }
            let _ = core::str::from_utf8(&size_buf).map(|s| display.write(s));
            display.putchar(b' ');

            if end > 0 { display.writeln(&name_str[..end]); }
        }
    } else {
        // Multi-column layout
        let mut max_w = 0usize;
        for i in 0..count {
            let idx = indices[i];
            let name_str = core::str::from_utf8(&entries[idx].name).unwrap_or("?");
            let end = name_str.find('\0').unwrap_or(name_str.len());
            let extra = if entries[idx].is_dir { 1 } else { 0 };
            if end + extra > max_w { max_w = end + extra; }
        }

        let col_w = (max_w + 2).max(4); // minimum 4 per column (e.g., "..  ")
        let cols = (WIDTH / col_w).max(1);
        let rows = (count + cols - 1) / cols;

        for row in 0..rows {
            for col in 0..cols {
                let idx = col * rows + row;
                if idx >= count { break; }
                let i = indices[idx];
                let name_str = core::str::from_utf8(&entries[i].name).unwrap_or("?");
                let end = name_str.find('\0').unwrap_or(name_str.len());

                if end > 0 { display.write(&name_str[..end]); }
                if entries[i].is_dir { display.putchar(b'/'); }

                // Padding
                if col < cols - 1 {
                    let written = if entries[i].is_dir { end + 1 } else { end };
                    let pad = col_w.saturating_sub(written);
                    for _ in 0..pad { display.putchar(b' '); }
                }
            }
            display.putchar(b'\n');
        }
    }
}
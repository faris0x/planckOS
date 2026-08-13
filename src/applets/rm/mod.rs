use crate::hal::display::VgaDisplay;
use alloc::vec::Vec;


use crate::hal::input::Ps2Keyboard;
use crate::hal::fat32;
use crate::hal::Display;

const MAX_PATH: usize = 256;

pub fn run(display: &mut VgaDisplay, _input: &mut Ps2Keyboard, args: &[&str]) {
    let (paths, recursive, force) = match parse_args(args) {
        Ok(r) => r,
        Err(e) => {
            display.write("rm: ");
            display.writeln(e);
            return;
        }
    };

    if paths.is_empty() {
        display.writeln("rm: missing operand");
        return;
    }

    let mut path_buf = [0u8; MAX_PATH];

    for path in paths {
        remove_path(display, path, recursive, force, &mut path_buf);
    }
}

fn remove_path(display: &mut VgaDisplay, path: &str, recursive: bool, force: bool, path_buf: &mut [u8; MAX_PATH]) {
    let info = match fat32::f_stat(path) {
        Ok(i) => i,
        Err(_) => {
            if !force {
                display.write("rm: cannot remove '");
                display.write(path);
                display.writeln("': No such file or directory");
            }
            return;
        }
    };

    let is_dir = info.attributes & 0x10 != 0;

    if is_dir {
        if !recursive {
            display.write("rm: cannot remove '");
            display.write(path);
            display.writeln("': Is a directory");
            return;
        }

        // Recursive delete: list directory contents and delete each entry
        match fat32::f_opendir(path) {
            Ok(mut dir) => {
                let mut name = [0u8; 13];
                loop {
                    match fat32::f_readdir(&mut dir, &mut name) {
                        Ok(true) => {
                            // Skip . and ..
                            if name[0] == b'.' && (name[1] == 0 || (name[1] == b'.' && name[2] == 0)) {
                                continue;
                            }
                            let name_str = core::str::from_utf8(&name).unwrap_or("?");
                            let end = name_str.find('\0').unwrap_or(name_str.len());
                            if end == 0 { continue; }

                            // Build child path in path_buf then copy out
                            let plen = path.len();
                            let nlen = end;
                            if plen + 1 + nlen >= MAX_PATH { continue; }

                            path_buf[..plen].copy_from_slice(path.as_bytes());
                            path_buf[plen] = b'/';
                            path_buf[plen + 1..plen + 1 + nlen].copy_from_slice(&name[..nlen]);

                            let mut child_copy = [0u8; MAX_PATH];
                            let child_len = plen + 1 + nlen;
                            child_copy[..child_len].copy_from_slice(&path_buf[..child_len]);
                            let child = core::str::from_utf8(&child_copy[..child_len]).unwrap_or("?");

                            remove_path(display, child, recursive, force, path_buf);
                        }
                        Ok(false) => break,
                        Err(_) => {
                            if !force {
                                display.writeln("rm: error reading directory");
                            }
                            break;
                        }
                    }
                }
                let _ = fat32::f_closedir(dir);
            }
            Err(_) => {
                if !force {
                    display.write("rm: cannot open directory '");
                    display.write(path);
                    display.writeln("'");
                }
                return;
            }
        }
    }

    // Delete the file or (now-empty) directory
    match fat32::f_unlink(path) {
        Ok(()) => {}
        Err(fat32::FError::NoFile) if force => {}
        Err(fat32::FError::NoFile) => {
            display.write("rm: cannot remove '");
            display.write(path);
            display.writeln("': No such file or directory");
        }
        Err(_) => {
            display.write("rm: cannot remove '");
            display.write(path);
            display.writeln("'");
        }
    }
}

fn parse_args<'a>(args: &[&'a str]) -> Result<(Vec<&'a str>, bool, bool), &'static str> {
    let mut paths = Vec::new();
    let mut recursive = false;
    let mut force = false;

    for arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            for c in arg[1..].chars() {
                match c {
                    'r' => recursive = true,
                    'f' => force = true,
                    _ => return Err("invalid option"),
                }
            }
        } else {
            paths.push(*arg);
        }
    }

    Ok((paths, recursive, force))
}
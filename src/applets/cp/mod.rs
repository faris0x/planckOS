// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only


use alloc::vec::Vec;


use crate::hal::input::Ps2Keyboard;
use crate::hal::fat32;
use crate::hal::fat32::{FA_READ, FA_WRITE, FA_CREATE_NEW};
use crate::hal::Display;

const BUF_SIZE: usize = 512;
const MAX_PATH: usize = 256;

pub fn run(display: &mut dyn Display, _input: &mut Ps2Keyboard, args: &[&str]) {
    let (sources, dest, recursive) = match parse_args(args) {
        Ok(r) => r,
        Err(e) => {
            display.write("cp: ");
            display.writeln(e);
            return;
        }
    };

    if sources.is_empty() || dest.is_empty() {
        display.writeln("cp: missing file operand");
        return;
    }

    let mut path_buf = [0u8; MAX_PATH];
    let mut data_buf = [0u8; BUF_SIZE];

    for src in &sources {
        copy_path(display, src, dest, recursive, &mut path_buf, &mut data_buf);
    }
}

fn copy_path(
    display: &mut dyn Display,
    src: &str,
    dst: &str,
    recursive: bool,
    path_buf: &mut [u8; MAX_PATH],
    data_buf: &mut [u8; BUF_SIZE],
) {
    let info = match fat32::f_stat(src) {
        Ok(i) => i,
        Err(_) => {
            display.write("cp: cannot stat '");
            display.write(src);
            display.writeln("': No such file or directory");
            return;
        }
    };

    let is_dir = info.attributes & 0x10 != 0;

    if is_dir {
        if !recursive {
            display.write("cp: omitting directory '");
            display.write(src);
            display.writeln("'");
            return;
        }

        // Create destination directory
        if let Err(_) = fat32::f_mkdir(dst) {
            // May already exist — that's fine
        }

        // Iterate source directory
        match fat32::f_opendir(src) {
            Ok(mut dir) => {
                let mut name = [0u8; 13];
                loop {
                    match fat32::f_readdir(&mut dir, &mut name) {
                        Ok(true) => {
                            if name[0] == b'.' && (name[1] == 0 || (name[1] == b'.' && name[2] == 0)) {
                                continue;
                            }
                            let n = core::str::from_utf8(&name).unwrap_or("?");
                            let end = n.find('\0').unwrap_or(n.len());
                            if end == 0 { continue; }

                            let nl = end;

                            // Build child src path into path_buf
                            let sl = src.len();
                            let s_total = sl + 1 + nl;
                            if s_total >= MAX_PATH { continue; }
                            path_buf[..sl].copy_from_slice(src.as_bytes());
                            path_buf[sl] = b'/';
                            path_buf[sl + 1..sl + 1 + nl].copy_from_slice(&name[..nl]);
                            let mut s_copy = [0u8; MAX_PATH];
                            s_copy[..s_total].copy_from_slice(&path_buf[..s_total]);

                            // Build child dst path into path_buf
                            let dl = dst.len();
                            let d_total = dl + 1 + nl;
                            if d_total >= MAX_PATH { continue; }
                            path_buf[..dl].copy_from_slice(dst.as_bytes());
                            path_buf[dl] = b'/';
                            path_buf[dl + 1..dl + 1 + nl].copy_from_slice(&name[..nl]);
                            let mut d_copy = [0u8; MAX_PATH];
                            d_copy[..d_total].copy_from_slice(&path_buf[..d_total]);

                            let child_src = core::str::from_utf8(&s_copy[..s_total]).unwrap_or("?");
                            let child_dst = core::str::from_utf8(&d_copy[..d_total]).unwrap_or("?");

                            copy_path(display, child_src, child_dst, recursive, path_buf, data_buf);
                        }
                        Ok(false) => break,
                        Err(_) => {
                            display.writeln("cp: error reading directory");
                            break;
                        }
                    }
                }
                let _ = fat32::f_closedir(dir);
            }
            Err(_) => {
                display.write("cp: cannot open directory '");
                display.write(src);
                display.writeln("'");
            }
        }
    } else {
        // Copy a single file
        let mut src_file = match fat32::f_open(src, FA_READ) {
            Ok(f) => f,
            Err(_) => {
                display.write("cp: cannot open '");
                display.write(src);
                display.writeln("'");
                return;
            }
        };

        let mut dst_file = match fat32::f_open(dst, FA_WRITE | FA_CREATE_NEW) {
            Ok(f) => f,
            Err(fat32::FError::Exists) => {
                display.write("cp: cannot create '");
                display.write(dst);
                display.writeln("': File exists");
                let _ = fat32::f_close(src_file);
                return;
            }
            Err(_) => {
                display.write("cp: cannot create '");
                display.write(dst);
                display.writeln("'");
                let _ = fat32::f_close(src_file);
                return;
            }
        };

        loop {
            let read = fat32::f_read(&mut src_file, &mut data_buf[..]).unwrap_or(0);
            if read == 0 { break; }
            let written = fat32::f_write(&mut dst_file, &data_buf[..read]).unwrap_or(0);
            if written < read {
                display.writeln("cp: write error");
                break;
            }
        }

        let _ = fat32::f_sync(&mut dst_file);
        let _ = fat32::f_close(dst_file);
        let _ = fat32::f_close(src_file);
    }
}

fn parse_args<'a>(args: &[&'a str]) -> Result<(Vec<&'a str>, &'a str, bool), &'static str> {
    let mut all = Vec::new();
    let mut recursive = false;

    for arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            for c in arg[1..].chars() {
                match c {
                    'r' => recursive = true,
                    _ => return Err("invalid option"),
                }
            }
        } else {
            all.push(*arg);
        }
    }

    if all.len() < 2 {
        return Err("missing file operand");
    }

    let dest = all[all.len() - 1];
    let sources: Vec<&str> = all[..all.len() - 1].to_vec();
    Ok((sources, dest, recursive))
}
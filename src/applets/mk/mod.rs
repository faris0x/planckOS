// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only


use alloc::vec::Vec;


use crate::hal::input::Ps2Keyboard;
use crate::hal::fat32;
use crate::hal::fat32::{FA_WRITE, FA_CREATE_NEW};
use crate::hal::Display;

pub fn run(display: &mut dyn Display, _input: &mut Ps2Keyboard, args: &[&str]) {
    let (paths, parents) = match parse_args(args) {
        Ok(r) => r,
        Err(e) => {
            display.write("mk: ");
            display.writeln(e);
            return;
        }
    };

    if paths.is_empty() {
        display.writeln("mk: missing operand");
        return;
    }

    for path in paths {
        create_path(display, path, parents);
    }
}

fn show_err(display: &mut dyn Display, path: &str, label: &str, e: fat32::FError) {
    display.write("mk: cannot create ");
    display.write(label);
    display.write(" '");
    display.write(path);
    display.write("': ");
    display.writeln(match e {
        fat32::FError::DiskErr => "Disk error",
        fat32::FError::Exists => "File exists",
        fat32::FError::NoPath => "No such path",
        fat32::FError::Invalid => "Invalid name",
        fat32::FError::MkfsAborted => "Out of clusters",
        _ => "Unknown error",
    });
}

fn create_path(display: &mut dyn Display, path: &str, parents: bool) {
    let is_dir = path.ends_with('/');
    let path = path.trim_end_matches('/');
    if path.is_empty() { return; }

    if is_dir {
        if let Ok(info) = fat32::f_stat(path) {
            if info.attributes & 0x10 != 0 { return; }
            show_err(display, path, "directory", fat32::FError::Exists);
            return;
        }
        match fat32::f_mkdir(path) {
            Ok(()) => {}
            Err(fat32::FError::NoPath) if parents => {
                if let Some(parent) = parent_path(path) {
                    create_path(display, parent, true);
                    let _ = fat32::f_mkdir(path);
                }
            }
            Err(e) => show_err(display, path, "directory", e),
        }
    } else {
        match fat32::f_open(path, FA_WRITE | FA_CREATE_NEW) {
            Ok(f) => { let _ = fat32::f_close(f); }
            Err(fat32::FError::Exists) => {}
            Err(fat32::FError::NoPath) if parents => {
                if let Some(parent) = parent_path(path) {
                    create_path(display, parent, true);
                    let _ = fat32::f_open(path, FA_WRITE | FA_CREATE_NEW).map(|f| fat32::f_close(f));
                }
            }
            Err(e) => show_err(display, path, "file", e),
        }
    }
}

fn parent_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let pos = trimmed.rfind('/')?;
    if pos == 0 { Some("/") } else { Some(&trimmed[..pos]) }
}

fn parse_args<'a>(args: &[&'a str]) -> Result<(Vec<&'a str>, bool), &'static str> {
    let mut paths = Vec::new();
    let mut parents = false;

    for arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            for c in arg[1..].chars() {
                match c {
                    'p' => parents = true,
                    _ => return Err("invalid option"),
                }
            }
        } else {
            paths.push(*arg);
        }
    }

    Ok((paths, parents))
}
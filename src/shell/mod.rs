// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

mod builtins;

use crate::hal::input::Ps2Keyboard;
use crate::hal::{Display, DisplayBackend, Input};
use crate::applets::AppletRegistry;
use crate::applets::ls;
use crate::applets::out;
use crate::applets::mk;
use crate::applets::rm;
use crate::applets::cp;

const PROMPT: &str = "$ ";
const MAX_CMD: usize = 256;
const HISTORY_SIZE: usize = 64;

pub struct Shell {
    display: DisplayBackend,
    input: Ps2Keyboard,
    registry: AppletRegistry,
    cursor_pos: usize,
    history_entries: [[u8; MAX_CMD]; HISTORY_SIZE],
    history_lens: [usize; HISTORY_SIZE],
    history_count: usize,
    history_cursor: isize,
}

impl Shell {
    pub fn new(
        display: DisplayBackend,
        input: Ps2Keyboard,
        registry: AppletRegistry,
    ) -> Self {
        Shell {
            display,
            input,
            registry,
            cursor_pos: 0,
            history_entries: [[0; MAX_CMD]; HISTORY_SIZE],
            history_lens: [0; HISTORY_SIZE],
            history_count: 0,
            history_cursor: -1,
        }
    }

    fn history_push(&mut self, buf: &[u8]) {
        let len = buf.len().min(MAX_CMD);
        if len == 0 {
            return;
        }
        // Shift existing entries right by one to make room at index 0
        let max_shift = self.history_count.min(HISTORY_SIZE - 1);
        for i in (0..max_shift).rev() {
            self.history_entries[i + 1] = self.history_entries[i];
            self.history_lens[i + 1] = self.history_lens[i];
        }
        // Copy new entry to front
        self.history_entries[0][..len].copy_from_slice(&buf[..len]);
        self.history_lens[0] = len;
        if self.history_count < HISTORY_SIZE {
            self.history_count += 1;
        }
    }

    fn history_get(&self, idx: usize) -> &[u8] {
        if idx < self.history_count {
            &self.history_entries[idx][..self.history_lens[idx]]
        } else {
            &[]
        }
    }

    fn redraw_input(&mut self, buf: &[u8], len: usize) {
        let prompt_len = 2;
        let line_clear = self.display.cols().saturating_sub(prompt_len).saturating_sub(1);
        self.display.putchar(0x0D);
        self.display.write(PROMPT);
        for _ in 0..line_clear {
            self.display.putchar(b' ');
        }
        self.display.putchar(0x0D);
        self.display.write(PROMPT);
        for &b in &buf[..len] {
            self.display.putchar(b);
        }
        let moves = len.saturating_sub(self.cursor_pos);
        for _ in 0..moves {
            self.display.putchar(0x08);
        }
    }

    pub fn run(mut self) -> ! {
        loop {
            self.display.write(PROMPT);
            self.display.show_cursor();
            let mut buf = [0u8; MAX_CMD];
            let mut len = 0usize;
            self.cursor_pos = 0;
            self.history_cursor = -1;

            loop {
                let c = self.input.getchar();
                match c {
                    // Enter
                    b'\n' | b'\r' => {
                        self.display.putchar(b'\n');
                        if len > 0 {
                            self.history_push(&buf[..len]);
                        }
                        break;
                    }
                    // Backspace
                    0x08 | 0x7F => {
                        if self.cursor_pos > 0 && len > 0 {
                            // Shift characters left
                            for i in self.cursor_pos..len {
                                buf[i - 1] = buf[i];
                            }
                            self.cursor_pos -= 1;
                            len -= 1;
                            self.redraw_input(&buf, len);
                            self.display.show_cursor();
                        }
                    }
                    // Left arrow
                    0x03 => {
                        if self.cursor_pos > 0 {
                            self.cursor_pos -= 1;
                            self.display.putchar(0x08);
                            self.display.show_cursor();
                        }
                    }
                    // Right arrow
                    0x04 => {
                        if self.cursor_pos < len {
                            let c = buf[self.cursor_pos];
                            self.display.putchar(c);
                            self.cursor_pos += 1;
                            self.display.show_cursor();
                        }
                    }
                    // Up arrow — history back
                    0x01 => {
                        if self.history_cursor < self.history_count as isize - 1 {
                            self.history_cursor += 1;
                            let entry = self.history_get(self.history_cursor as usize);
                            len = entry.len().min(MAX_CMD);
                            buf[..len].copy_from_slice(&entry[..len]);
                            self.cursor_pos = len;
                            self.redraw_input(&buf, len);
                            self.display.show_cursor();
                        }
                    }
                    // Down arrow — history forward
                    0x02 => {
                        if self.history_cursor >= 0 {
                            self.history_cursor -= 1;
                            if self.history_cursor == -1 {
                                len = 0;
                                self.cursor_pos = 0;
                                self.redraw_input(&buf, len);
                            } else {
                                let entry = self.history_get(self.history_cursor as usize);
                                len = entry.len().min(MAX_CMD);
                                buf[..len].copy_from_slice(&entry[..len]);
                                self.cursor_pos = len;
                                self.redraw_input(&buf, len);
                            }
                            self.display.show_cursor();
                        }
                    }
                    // Printable characters
            c if c >= 0x20 && c < 0x7F => {
                let max_len = self.display.cols().saturating_sub(2);
                if len < max_len {
                    // Shift characters right to make room
                    for i in (self.cursor_pos..len).rev() {
                        buf[i + 1] = buf[i];
                    }
                    buf[self.cursor_pos] = c;
                    self.cursor_pos += 1;
                    len += 1;
                    self.redraw_input(&buf, len);
                    self.display.show_cursor();
                }
            }
                    _ => {}
                }
            }

            if len == 0 {
                continue;
            }

            let cmd = &buf[..len];

            let (command, args) = if let Some(space) = find_space(cmd) {
                (&cmd[..space], &cmd[space + 1..])
            } else {
                (cmd, b"" as &[u8])
            };

            // First check built-in commands
            if self.handle_builtin(command, args) {
                continue;
            }

            // Then check registered applets
            if let Some(applet_name) = core::str::from_utf8(command).ok() {
                if let Some(applet) = self.registry.find(applet_name) {
                    let mut args_arr: [&str; 16] = [""; 16];
                    let mut args_count = 0;
                    let mut remaining = args;
                    while !remaining.is_empty() {
                        if let Some(space) = remaining.iter().position(|&b| b == b' ') {
                            if let Ok(s) = core::str::from_utf8(&remaining[..space]) {
                                if !s.is_empty() && args_count < 16 {
                                    args_arr[args_count] = s;
                                    args_count += 1;
                                }
                            }
                            remaining = &remaining[space + 1..];
                        } else {
                            if let Ok(s) = core::str::from_utf8(remaining) {
                                if !s.is_empty() && args_count < 16 {
                                    args_arr[args_count] = s;
                                    args_count += 1;
                                }
                            }
                            break;
                        }
                    }
                    (applet.run)(&mut self.display, &mut self.input, &args_arr[..args_count]);
                    continue;
                }
            }

            // Unknown command
            self.display.write("Unknown command: ");
            if let Ok(s) = core::str::from_utf8(command) {
                self.display.writeln(s);
            } else {
                self.display.putchar(b'?');
                self.display.putchar(b'\n');
            }
        }
    }

    fn handle_builtin(&mut self, command: &[u8], args: &[u8]) -> bool {
        let mut args_arr: [&str; 16] = [""; 16];
        let mut args_count = 0;
        let mut remaining = args;
        while !remaining.is_empty() {
            if let Some(space) = remaining.iter().position(|&b| b == b' ') {
                if let Ok(s) = core::str::from_utf8(&remaining[..space]) {
                    if !s.is_empty() && args_count < 16 {
                        args_arr[args_count] = s;
                        args_count += 1;
                    }
                }
                remaining = &remaining[space + 1..];
            } else {
                if let Ok(s) = core::str::from_utf8(remaining) {
                    if !s.is_empty() && args_count < 16 {
                        args_arr[args_count] = s;
                        args_count += 1;
                    }
                }
                break;
            }
        }
        let applet_args = &args_arr[..args_count];

        match command {
            b"echo" => {
                if let Ok(s) = core::str::from_utf8(args) {
                    self.display.writeln(s);
                }
                true
            }
            b"cls" => {
                self.display.clear();
                true
            }
            b"banner" => {
                self.display.clear();
                self.display.writeln("planckOS v0.1 - x86-64 Rust");
                self.display.writeln("Copyright (C) 2026 Faris Alfarhan");
                self.display.writeln("Licensed GPLv3");
                true
            }
            b"help" => {
                builtins::print_help(&mut self.display, &self.registry);
                true
            }
            b"history" => {
                for i in 0..self.history_count {
                    let idx = self.history_count - 1 - i;
                    // Print line number (1-based)
                    let num = idx + 1;
                    let tens = (num / 10) as u8 + b'0';
                    let ones = (num % 10) as u8 + b'0';
                    self.display.putchar(b' ');
                    self.display.putchar(tens);
                    self.display.putchar(ones);
                    self.display.write("  ");
                    // Copy entry data to avoid borrow conflict
                    let mut entry_buf = [0u8; 256];
                    let entry_len = if idx < self.history_count {
                        let len = self.history_lens[idx].min(256);
                        entry_buf[..len].copy_from_slice(&self.history_entries[idx][..len]);
                        len
                    } else { 0 };
                    if let Ok(s) = core::str::from_utf8(&entry_buf[..entry_len]) {
                        self.display.writeln(s);
                    } else {
                        self.display.writeln("?");
                    }
                }
                true
            }
            b"shutdown" => {
                self.display.writeln("Shutting down...");
                crate::hal::acpi::shutdown();
                true
            }
            b"ls" => { ls::run(&mut self.display, &mut self.input, applet_args); true }
            b"out" => { out::run(&mut self.display, &mut self.input, applet_args); true }
            b"mk" => { mk::run(&mut self.display, &mut self.input, applet_args); true }
            b"rm" => { rm::run(&mut self.display, &mut self.input, applet_args); true }
            b"cp" => { cp::run(&mut self.display, &mut self.input, applet_args); true }
            _ => false,
        }
    }
}

fn find_space(s: &[u8]) -> Option<usize> {
    for (i, &c) in s.iter().enumerate() {
        if c == b' ' {
            return Some(i);
        }
    }
    None
}

// Copyright (c) 2026 Faris Alfarhan
// SPDX-License-Identifier: GPL-3.0-only

/// Serial port driver (COM1, 0x3F8) for debug output.
///
/// Used during boot to confirm stage progression before the VGA
/// driver is initialised.

const COM1: u16 = 0x3F8;

/// Serialises cross-core access to the COM1 port.
static SERIAL_LOCK: super::memory::SpinLock = super::memory::SpinLock::new();

/// Initialise the serial port (115200 baud, 8N1).
pub fn serial_init() {
    unsafe {
        // Disable interrupts
        outb(COM1 + 1, 0x00);
        // Enable DLAB (set baud rate divisor)
        outb(COM1 + 3, 0x80);
        // Set divisor to 1 (115200 baud)
        outb(COM1 + 0, 0x01);
        outb(COM1 + 1, 0x00);
        // 8 bits, no parity, one stop bit
        outb(COM1 + 3, 0x03);
        // Enable FIFO, clear them, 14-byte threshold
        outb(COM1 + 2, 0xC7);
        // IRQs enabled, RTS/DSR set
        outb(COM1 + 4, 0x0B);
    }
}

/// Write a single byte to the serial port, blocking until ready.
pub fn serial_write_byte(byte: u8) {
    SERIAL_LOCK.lock();
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {}
        outb(COM1, byte);
    }
    unsafe { SERIAL_LOCK.unlock() }
}

/// Write a string to the serial port.
pub fn serial_write_str(s: &str) {
    for &b in s.as_bytes() {
        serial_write_byte(b);
    }
}

/// Wait until the UART transmit shift register is empty.
pub fn flush() {
    SERIAL_LOCK.lock();
    unsafe {
        while (inb(COM1 + 5) & 0x40) == 0 {}
    }
    unsafe { SERIAL_LOCK.unlock() }
}

/// Busy-wait for a number of milliseconds using the calibrated TSC.
pub fn wait_ms(ms: u64) {
    let per_ms = unsafe { TSC_PER_MS };
    if per_ms == 0 {
        return;
    }
    let start = rdtsc();
    let ticks = per_ms.saturating_mul(ms);
    while rdtsc().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

pub fn serial_debug(msg: &[u8]) {
    SERIAL_LOCK.lock();
    for &c in msg {
        if c == 0 { break; }
        unsafe {
            while (inb(COM1 + 5) & 0x20) == 0 {}
            outb(COM1, c);
        }
    }
    unsafe { SERIAL_LOCK.unlock() }
}

// ── Boot logging (unified [PART] [SUB] format with per-phase timing) ──

/// TSC ticks per millisecond, calibrated by the boot sector and passed to
/// the kernel through the loader's boot info block.
pub static mut TSC_PER_MS: u64 = 1_000_000;
/// TSC snapshot at the previous log line (per-line phase timing).
pub static mut LAST_TSC: u64 = 0;
/// Scratch buffer for building log lines.
pub static mut LOG_BUF: [u8; 224] = [0; 224];
/// Scratch buffer for callers building message text (kept separate from
/// LOG_BUF so log_ms can copy from it without overlap).
pub static mut FMT_BUF: [u8; 224] = [0; 224];

const LOG_RING_LINES: usize = 64;
const LOG_LINE_LEN: usize = 192;
static mut LOG_RING: [[u8; LOG_LINE_LEN]; LOG_RING_LINES] = [[0; LOG_LINE_LEN]; LOG_RING_LINES];
static mut LOG_LENS: [usize; LOG_RING_LINES] = [0; LOG_RING_LINES];
static mut LOG_NEXT: usize = 0;
static mut LOG_COUNT: usize = 0;
static LOG_LOCK: super::memory::SpinLock = super::memory::SpinLock::new();

/// Read the TSC (cycle counter).
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | lo as u64
}

/// Seed the logger with the calibrated TSC frequency. Call once, first.
pub fn bootlog_init(tsc_per_ms: u64) {
    let t = rdtsc();
    unsafe {
        TSC_PER_MS = if tsc_per_ms == 0 { 1 } else { tsc_per_ms };
        LAST_TSC = t;
    }
}

/// Milliseconds (x10, i.e. deciseconds) elapsed since `start` TSC.
pub fn ms10_since(start: u64) -> u64 {
    let now = rdtsc();
    let per_ms = unsafe { TSC_PER_MS };
    (now.wrapping_sub(start) as u128 * 10 / per_ms as u128) as u64
}

fn record_log(line: &[u8]) {
    let mut len = line.len();
    while len > 0 && matches!(line[len - 1], b'\r' | b'\n') {
        len -= 1;
    }
    if len == 0 {
        return;
    }
    let copy_len = len.min(LOG_LINE_LEN);
    LOG_LOCK.lock();
    unsafe {
        let index = LOG_NEXT;
        LOG_RING[index][..copy_len].copy_from_slice(&line[..copy_len]);
        LOG_LENS[index] = copy_len;
        LOG_NEXT = (index + 1) % LOG_RING_LINES;
        if LOG_COUNT < LOG_RING_LINES {
            LOG_COUNT += 1;
        }
    }
    unsafe { LOG_LOCK.unlock() }
}

/// Print the recorded kernel log lines to a display.
pub fn dump_log(display: &mut dyn super::Display) {
    LOG_LOCK.lock();
    let count = unsafe { LOG_COUNT };
    if count == 0 {
        display.writeln("dmesg: no log entries");
        unsafe { LOG_LOCK.unlock() }
        return;
    }
    let next = unsafe { LOG_NEXT };
    let start = (next + LOG_RING_LINES - count) % LOG_RING_LINES;
    for offset in 0..count {
        let index = (start + offset) % LOG_RING_LINES;
        let len = unsafe { LOG_LENS[index] };
        let line = unsafe { &LOG_RING[index][..len] };
        if let Ok(text) = core::str::from_utf8(line) {
            display.writeln(text);
        }
    }
    unsafe { LOG_LOCK.unlock() }
}

/// Log a line with an explicitly measured elapsed time (deciseconds).
/// The message is copied to a local buffer first so callers may pass
/// messages that alias LOG_BUF (e.g. built by format helpers).
pub fn log_ms(part: &str, sub: &str, msg: &str, ms10: u64) {
    let mut msg_copy = [0u8; 112];
    let ml = msg.len().min(112);
    msg_copy[..ml].copy_from_slice(&msg.as_bytes()[..ml]);
    let msg = core::str::from_utf8(&msg_copy[..ml]).unwrap_or("?");
    let buf: &mut [u8; 224] = unsafe { &mut LOG_BUF };
    let mut i = 0;
    let cpu = super::apic::current_cpu_index() as u64;
    i = put_str(buf, i, "[CPU");
    i = put_dec(buf, i, cpu);
    // Pad the "CPU<n>" tag to 6 wide, matching the PART/SUB fields.
    let mut n = cpu;
    let mut ndigits = 1;
    while n >= 10 {
        n /= 10;
        ndigits += 1;
    }
    for _ in (3 + ndigits)..6 {
        i = put(buf, i, b' ');
    }
    i = put_str(buf, i, "] ");
    i = put(buf, i, b'[');
    i = put_pad(buf, i, part, 6);
    i = put(buf, i, b']');
    i = put(buf, i, b' ');
    i = put(buf, i, b'[');
    i = put_pad(buf, i, sub, 6);
    i = put(buf, i, b']');
    i = put(buf, i, b' ');
    i = put_str(buf, i, msg);
    i = put(buf, i, b' ');
    i = put(buf, i, b'(');
    i = put_dec(buf, i, ms10 / 10);
    i = put(buf, i, b'.');
    i = put_dec(buf, i, ms10 % 10);
    i = put_str(buf, i, " ms)\r\n");
    buf[i] = 0;
    record_log(&buf[..i]);
    serial_debug(&buf[..i]);
}

/// Log a line; the elapsed time is measured since the previous log line.
pub fn log(part: &str, sub: &str, msg: &str) {
    let start = unsafe { LAST_TSC };
    let ms10 = ms10_since(start);
    unsafe { LAST_TSC = rdtsc(); }
    log_ms(part, sub, msg, ms10);
}

/// Log a line whose message is `prefix` + "0x" + minimal hex + `suffix`.
/// Builds the message in FMT_BUF so it can be passed to `log` safely.
pub fn log_hex(part: &str, sub: &str, prefix: &str, val: u64, suffix: &str) {
    let buf: &mut [u8; 224] = unsafe { &mut FMT_BUF };
    let mut i = 0;
    i = put_str(buf, i, prefix);
    i = put(buf, i, b'0');
    i = put(buf, i, b'x');
    i = put_hex(buf, i, val);
    i = put_str(buf, i, suffix);
    buf[i] = 0;
    let msg = core::str::from_utf8(&buf[..i]).unwrap_or("?");
    log(part, sub, msg);
}

fn put(buf: &mut [u8; 224], mut i: usize, b: u8) -> usize {
    if i < buf.len() {
        buf[i] = b;
        i += 1;
    }
    i
}

fn put_str(buf: &mut [u8; 224], mut i: usize, s: &str) -> usize {
    for &b in s.as_bytes() {
        i = put(buf, i, b);
    }
    i
}

fn put_pad(buf: &mut [u8; 224], mut i: usize, s: &str, width: usize) -> usize {
    for &b in s.as_bytes().iter().take(width) {
        i = put(buf, i, b);
    }
    let used = s.len().min(width);
    for _ in used..width {
        i = put(buf, i, b' ');
    }
    i
}

fn put_dec(buf: &mut [u8; 224], mut i: usize, mut v: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut j = 0;
    if v == 0 {
        return put(buf, i, b'0');
    }
    while v > 0 {
        tmp[j] = b'0' + (v % 10) as u8;
        v /= 10;
        j += 1;
    }
    while j > 0 {
        j -= 1;
        i = put(buf, i, tmp[j]);
    }
    i
}

fn put_hex(buf: &mut [u8; 224], mut i: usize, mut v: u64) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut started = false;
    for shift in (0..16).rev() {
        let nib = ((v >> (shift * 4)) & 0xF) as usize;
        if nib != 0 || started || shift == 0 {
            started = true;
            i = put(buf, i, HEX[nib]);
        }
    }
    i
}

pub fn serial_debug_hex(val: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as usize;
        unsafe {
            while (inb(COM1 + 5) & 0x20) == 0 {}
            outb(COM1, HEX[nibble]);
        }
    }
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}

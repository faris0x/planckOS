use crate::hal::display::VgaDisplay;
use crate::applets::Applet;

pub static APPLET: Applet = Applet {
    name: "sysinfo",
    description: "Display system information",
    run: run,
};


use crate::hal::input::Ps2Keyboard;
use crate::hal::cpuid;
use crate::hal::e820;
use crate::hal::idt::TICK_COUNT;
use crate::hal::Display;
use core::sync::atomic::Ordering;

pub fn run(display: &mut VgaDisplay, _input: &mut Ps2Keyboard, _args: &[&str]) {
    let cpu = cpuid::query();

    display.writeln("planckOS v0.1 - System Information");
    display.writeln("");

    display.write("Kernel:       planckOS v0.1 (x86-64, Rust)\r\n");

    display.write("CPU Vendor:   ");
    display.writeln(cpuid::vendor_str(&cpu));
    display.write("CPU Model:    ");
    display.writeln(cpuid::brand_str(&cpu));

    let mut buf1 = [0u8; 16];
    let mut buf2 = [0u8; 16];
    let mut buf3 = [0u8; 16];
    display.write("CPU Revision: Family ");
    display.write(fmt_num(cpu.family as u32, &mut buf1));
    display.write(", Model ");
    display.write(fmt_num(cpu.model as u32, &mut buf2));
    display.write(", Stepping ");
    display.writeln(fmt_num(cpu.stepping as u32, &mut buf3));

    display.write("Features:     ");
    let mut features = [""; 16];
    let mut count = 0;
    if cpuid::has_feature_edx(&cpu, 0) { features[count] = "FPU"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 4) { features[count] = "TSC"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 23) { features[count] = "MMX"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 25) { features[count] = "SSE"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 26) { features[count] = "SSE2"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 0) { features[count] = "SSE3"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 9) { features[count] = "SSSE3"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 12) { features[count] = "FMA"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 19) { features[count] = "SSE4.1"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 20) { features[count] = "SSE4.2"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 28) { features[count] = "AVX"; count += 1; }
    if cpuid::has_feature_ecx(&cpu, 30) { features[count] = "RDRAND"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 24) { features[count] = "FXSR"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 16) { features[count] = "PAT"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 5) { features[count] = "MSR"; count += 1; }
    if cpuid::has_feature_edx(&cpu, 13) { features[count] = "PSE"; count += 1; }

    for i in 0..count {
        if i > 0 {
            display.write(" ");
        }
        display.write(features[i]);
    }
    display.putchar(b'\n');

    // Memory
    let total_mb = (e820::total_usable() / (1024 * 1024)) as u32;
    let mut mb_buf = [0u8; 16];
    display.write("Memory:       ");
    display.write(fmt_num(total_mb, &mut mb_buf));
    display.writeln(" MB usable");

    // Uptime
    let ticks = TICK_COUNT.load(Ordering::Relaxed);
    let seconds = ticks / 18;
    let mins = seconds / 60;
    let secs = seconds % 60;
    let mut mins_buf = [0u8; 16];
    let mut secs_buf = [0u8; 16];
    display.write("Uptime:       ");
    display.write(fmt_num(mins as u32, &mut mins_buf));
    display.write("m ");
    display.write(fmt_num(secs as u32, &mut secs_buf));
    display.writeln("s");
}

/// Format a u32 as a string into the given buffer. Returns a &str view.
fn fmt_num<'a>(n: u32, buf: &'a mut [u8; 16]) -> &'a str {
    let mut i = 15;
    let mut v = n;
    buf[15] = 0;
    loop {
        i -= 1;
        buf[i] = (v % 10) as u8 + b'0';
        v /= 10;
        if v == 0 || i == 0 { break; }
    }
    let len = 15 - i;
    core::str::from_utf8(&buf[i..i + len]).unwrap_or("?")
}

fn hex_str<'a>(n: u8, buf: &'a mut [u8; 16]) -> &'a str {
    let hex = b"0123456789ABCDEF";
    buf[0] = hex[((n >> 4) & 0xF) as usize];
    buf[1] = hex[(n & 0xF) as usize];
    core::str::from_utf8(&buf[..2]).unwrap_or("??")
}
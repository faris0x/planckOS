use crate::applets::Applet;
use crate::hal::input::Ps2Keyboard;
use crate::hal::memory;
use crate::hal::Display;

pub static APPLET: Applet = Applet {
    name: "heap",
    description: "Show heap allocator statistics",
    run: run,
};

// Static scratch buffers for formatting (single-threaded, called from the
// shell loop, so a plain static is fine).
static mut NUM_BUF: [u8; 32] = [0; 32];

pub fn run(display: &mut dyn Display, _input: &mut Ps2Keyboard, _args: &[&str]) {
    let total = memory::HEAP_END - memory::HEAP_BASE;
    let free = memory::heap_free_bytes();
    let used = total - free;
    let pct = if total > 0 { used * 100 / total } else { 0 };

    display.writeln("planckOS v0.1 - Heap Allocator");
    write_row(display, "Total heap:     ", &hex_str(total));
    write_row(display, "Free:           ", &hex_str(free));
    write_row(display, "Used:           ", &hex_str(used));
    write_row(display, "Live allocs:    ", &dec_str(memory::heap_alloc_count() as usize));
    write_row(display, "Utilization:    ", &dec_str(pct));
    display.write("%\r\n");
}

fn write_row(display: &mut dyn Display, label: &str, value: &str) {
    display.write(label);
    display.writeln(value);
}

fn dec_str(mut v: usize) -> &'static str {
    let buf: &mut [u8; 32] = unsafe { &mut NUM_BUF };
    let mut i = 0;
    if v == 0 {
        buf[0] = b'0';
        i = 1;
    } else {
        let mut tmp = [0u8; 20];
        let mut j = 0;
        while v > 0 {
            tmp[j] = b'0' + (v % 10) as u8;
            v /= 10;
            j += 1;
        }
        while j > 0 {
            j -= 1;
            buf[i] = tmp[j];
            i += 1;
        }
    }
    core::str::from_utf8(&buf[..i]).unwrap_or("?")
}

fn hex_str(v: usize) -> &'static str {
    let buf: &mut [u8; 32] = unsafe { &mut NUM_BUF };
    buf[0] = b'0';
    buf[1] = b'x';
    for k in 0..16 {
        let nib = (v >> ((15 - k) * 4)) & 0xF;
        buf[2 + k] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib as u8 - 10) };
    }
    core::str::from_utf8(&buf[..18]).unwrap_or("?")
}
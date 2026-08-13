use core::sync::atomic::{AtomicUsize, Ordering};

use super::Input;

const BUF_SZ: usize = 256;

struct RingBuf {
    buf: [u8; BUF_SZ],
    head: AtomicUsize,
    tail: AtomicUsize,
}

static mut KB_BUF: RingBuf = RingBuf {
    buf: [0; BUF_SZ],
    head: AtomicUsize::new(0),
    tail: AtomicUsize::new(0),
};

static SHIFT: AtomicUsize = AtomicUsize::new(0);
static CTRL: AtomicUsize = AtomicUsize::new(0);
static mut EXTENDED: bool = false;
static mut RELEASE: bool = false;

const SHIFT_MASK: usize = 1;
const CTRL_MASK: usize = 2;

const SCANCODE_SET2: [[u8; 2]; 128] = _scancode_set2();
const fn _scancode_set2() -> [[u8; 2]; 128] {
    let mut t = [[0u8; 2]; 128];
    t[0x0D] = [b'\t', b'\t'];
    t[0x0E] = [b'`', b'~'];
    t[0x15] = [b'q', b'Q'];
    t[0x16] = [b'1', b'!'];
    t[0x1A] = [b'z', b'Z'];
    t[0x1B] = [b's', b'S'];
    t[0x1C] = [b'a', b'A'];
    t[0x1D] = [b'w', b'W'];
    t[0x1E] = [b'2', b'@'];
    t[0x21] = [b'c', b'C'];
    t[0x22] = [b'x', b'X'];
    t[0x23] = [b'd', b'D'];
    t[0x24] = [b'e', b'E'];
    t[0x25] = [b'4', b'$'];
    t[0x26] = [b'3', b'#'];
    t[0x29] = [b' ', b' '];
    t[0x2A] = [b'v', b'V'];
    t[0x2B] = [b'f', b'F'];
    t[0x2C] = [b't', b'T'];
    t[0x2D] = [b'r', b'R'];
    t[0x2E] = [b'5', b'%'];
    t[0x31] = [b'n', b'N'];
    t[0x32] = [b'b', b'B'];
    t[0x33] = [b'h', b'H'];
    t[0x34] = [b'g', b'G'];
    t[0x35] = [b'y', b'Y'];
    t[0x36] = [b'6', b'^'];
    t[0x3A] = [b'm', b'M'];
    t[0x3B] = [b'j', b'J'];
    t[0x3C] = [b'u', b'U'];
    t[0x3D] = [b'7', b'&'];
    t[0x3E] = [b'8', b'*'];
    t[0x41] = [b',', b'<'];
    t[0x42] = [b'k', b'K'];
    t[0x43] = [b'i', b'I'];
    t[0x44] = [b'o', b'O'];
    t[0x45] = [b'0', b')'];
    t[0x46] = [b'9', b'('];
    t[0x49] = [b'.', b'>'];
    t[0x4A] = [b'/', b'?'];
    t[0x4B] = [b'l', b'L'];
    t[0x4C] = [b';', b':'];
    t[0x4D] = [b'p', b'P'];
    t[0x4E] = [b'-', b'_'];
    t[0x52] = [b'\'', b'"'];
    t[0x54] = [b'[', b'{'];
    t[0x55] = [b'=', b'+'];
    t[0x5B] = [b']', b'}'];
    t[0x5A] = [b'\n', b'\n'];
    t[0x61] = [b'\\', b'|'];
    t[0x66] = [0x08, 0x08];
    t[0x76] = [b'\x1b', b'\x1b'];
    t
}

pub struct Ps2Keyboard;

impl Ps2Keyboard {
    pub const fn new() -> Self {
        Ps2Keyboard
    }
}

impl Input for Ps2Keyboard {
    fn getchar(&mut self) -> u8 {
        getchar_impl()
    }

    fn init(&mut self) {
        init_impl();
    }
}

// ── IRQ handler (called from hal::idt) ───────────────────────────

pub fn irq1_handler() {
    irq1_handler_impl();
}

// ── Implementation methods ───────────────────────────────────────

fn scancode_to_ascii(scancode: u8) -> Option<u8> {
    let idx = scancode as usize;
    if idx >= 128 {
        return None;
    }
    let shift = (SHIFT.load(Ordering::Relaxed) & SHIFT_MASK) != 0;
    let c = SCANCODE_SET2[idx][shift as usize];
    if c == 0 {
        return None;
    }
    Some(c)
}

fn push_char(c: u8) {
    unsafe {
        let kb = core::ptr::addr_of_mut!(KB_BUF);
        let head = (*kb).head.load(Ordering::Relaxed);
        let next = (head + 1) % BUF_SZ;
        if next != (*kb).tail.load(Ordering::Relaxed) {
            (*kb).buf[head] = c;
            (*kb).head.store(next, Ordering::Relaxed);
        }
    }
}

fn getchar_impl() -> u8 {
    loop {
        unsafe {
            let kb = core::ptr::addr_of_mut!(KB_BUF);
            let tail = (*kb).tail.load(Ordering::Relaxed);
            let head = (*kb).head.load(Ordering::Relaxed);
            if tail != head {
                let c = (*kb).buf[tail];
                (*kb).tail.store((tail + 1) % BUF_SZ, Ordering::Relaxed);
                return c;
            }
        }
        core::hint::spin_loop();
    }
}

fn init_impl() {
    unsafe {
        let _ = inb(0x60);
        wait_accept();
        outb(0x64, 0xAD);
        wait_accept();
        outb(0x64, 0xA7);
        let _ = inb(0x60);

        wait_accept();
        outb(0x64, 0x20);
        wait_data();
        let mut config = inb(0x60);

        config |= 1;
        config &= !0x40;
        config &= !0x10;

        wait_accept();
        outb(0x64, 0x60);
        wait_accept();
        outb(0x60, config);

        wait_accept();
        outb(0x64, 0xAE);

        while inb(0x64) & 1 != 0 {
            let _ = inb(0x60);
        }
    }
}

fn irq1_handler_impl() {
    unsafe {
        let scancode = inb(0x60);

        if scancode == 0xE0 {
            EXTENDED = true;
            return;
        }
        if scancode == 0xF0 {
            RELEASE = true;
            return;
        }

        if RELEASE {
            let make = scancode;
            match make {
                0x12 | 0x59 => { SHIFT.fetch_and(!SHIFT_MASK, Ordering::Relaxed); }
                0x14 => { CTRL.fetch_and(!CTRL_MASK, Ordering::Relaxed); }
                _ => {}
            }
            RELEASE = false;
            EXTENDED = false;
            return;
        }

        if EXTENDED {
            EXTENDED = false;
            let token = match scancode {
                0x75 => 0x01, // Up
                0x72 => 0x02, // Down
                0x6B => 0x03, // Left
                0x74 => 0x04, // Right
                _ => return,
            };
            push_char(token);
            return;
        }

        match scancode {
            0x12 | 0x59 => { SHIFT.fetch_or(SHIFT_MASK, Ordering::Relaxed); }
            0x14 => { CTRL.fetch_or(CTRL_MASK, Ordering::Relaxed); }
            _ => {
                if let Some(c) = scancode_to_ascii(scancode) {
                    push_char(c);
                }
            }
        }
        EXTENDED = false;
    }
}

unsafe fn wait_data() {
    loop {
        let status = inb(0x64);
        if status & 1 != 0 {
            return;
        }
    }
}

unsafe fn wait_accept() {
    loop {
        let status = inb(0x64);
        if status & 2 == 0 {
            return;
        }
    }
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

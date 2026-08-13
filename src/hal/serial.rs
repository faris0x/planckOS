/// Serial port driver (COM1, 0x3F8) for debug output.
///
/// Used during boot to confirm stage progression before the VGA
/// driver is initialised.

const COM1: u16 = 0x3F8;

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
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {}
        outb(COM1, byte);
    }
}

/// Write a string to the serial port.
pub fn serial_write_str(s: &str) {
    for &b in s.as_bytes() {
        serial_write_byte(b);
    }
}

pub fn serial_debug(msg: &[u8]) {
    for &c in msg {
        if c == 0 { break; }
        unsafe {
            while (inb(COM1 + 5) & 0x20) == 0 {}
            outb(COM1, c);
        }
    }
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

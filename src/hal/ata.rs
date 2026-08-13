/// ATA PIO driver — reads and writes sectors on IDE channels via port I/O.
///
/// Primary channel:   I/O base 0x1F0, control 0x3F6
/// Secondary channel: I/O base 0x170, control 0x376

use super::block::BlockDevice;
use core::arch::asm;

const PRIMARY: u16   = 0x1F0;
const SECONDARY: u16 = 0x170;

const DATA: u16      = 0;  // offset from base
const ERROR: u16     = 1;
const SECTOR_COUNT: u16 = 2;
const LBA_LO: u16    = 3;
const LBA_MID: u16   = 4;
const LBA_HI: u16    = 5;
const DRIVE: u16     = 6;
const CMD_STATUS: u16 = 7;

const STAT_BSY: u8   = 0x80;
const STAT_DRDY: u8  = 0x40;
const STAT_DRQ: u8   = 0x08;
const STAT_ERR: u8   = 0x01;
const STAT_DF: u8    = 0x20;

const CMD_IDENTIFY: u8  = 0xEC;
const CMD_READ: u8      = 0x20;
const CMD_WRITE: u8     = 0x30;

const LBA_BIT: u8       = 0x40; // bit 6 in Drive register for LBA mode

/// A single IDE channel (primary or secondary).
pub struct AtaChannel {
    base: u16,
    drive_select: u8,
    sector_count: u64,
}

impl AtaChannel {
    pub const fn new(base: u16, master: bool) -> Self {
        AtaChannel {
            base,
            drive_select: if master { 0xA0 } else { 0xB0 },
            sector_count: 0,
        }
    }

    /// Probe the channel. If a drive responds, populate sector_count.
    pub fn probe(&mut self) -> bool {
        unsafe {
            // Select drive with LBA mode
            outb(self.base + DRIVE, self.drive_select | LBA_BIT);
            self.delay();

            // Read status — if it's 0xFF, no drive present
            let st = inb(self.base + CMD_STATUS);
            if st == 0xFF {
                return false;
            }

            // Send IDENTIFY command
            // First zero the sector count + LBA registers
            outb(self.base + SECTOR_COUNT, 0);
            outb(self.base + LBA_LO, 0);
            outb(self.base + LBA_MID, 0);
            outb(self.base + LBA_HI, 0);
            outb(self.base + CMD_STATUS, CMD_IDENTIFY);
            self.delay();

            // Wait for BSY clear
            let mut st = self.wait_bsy();
            if st & STAT_ERR != 0 {
                return false;
            }
            if st & STAT_DRQ == 0 {
                return false;
            }

            // Read 256 words (512 bytes) — the identification data
            let mut ident = [0u16; 256];
            for word in &mut ident {
                *word = inw(self.base + DATA);
            }

            // Read status to acknowledge IDENTIFY completion
            let _st = inb(self.base + CMD_STATUS);

            let low = ident[60] as u64;
            let high = ident[61] as u64;
            self.sector_count = low | (high << 16);

            true
        }
    }

    pub fn is_present(&self) -> bool {
        self.sector_count > 0
    }

    pub fn read_sector(&self, lba: u32, buf: &mut [u8; 512]) -> bool {
        unsafe {
            let mut st = self.wait_bsy();
            if st & STAT_DRDY == 0 {
                let control = self.base + 0x206;
                outb(control, 0x04);
                self.delay();
                outb(control, 0x00);
                self.delay();
                self.wait_bsy();
            }

            self.lba_write(lba, 1);
            outb(self.base + CMD_STATUS, CMD_READ);

            st = self.wait_for_drq();
            if st & STAT_ERR != 0 {
                let _err = inb(self.base + 1);
                return false;
            }

            // Read 256 words
            for chunk in buf.chunks_mut(2) {
                let w = inw(self.base + DATA);
                chunk[0] = (w & 0xFF) as u8;
                chunk[1] = ((w >> 8) & 0xFF) as u8;
            }

            inb(self.base + CMD_STATUS);
            true
        }
    }

    unsafe fn wait_for_drq(&self) -> u8 {
        loop {
            let st = inb(self.base + CMD_STATUS);
            if st & STAT_BSY == 0 && (st & STAT_DRQ != 0 || st & STAT_ERR != 0) {
                return st;
            }
        }
    }

    pub fn write_sector(&self, lba: u32, buf: &[u8; 512]) -> bool {
        unsafe {
            let st = self.wait_bsy();
            if st & STAT_DRDY == 0 {
                let control = self.base + 0x206;
                outb(control, 0x04);
                self.delay();
                outb(control, 0x00);
                self.delay();
                self.wait_bsy();
            }

            self.lba_write(lba, 1);
            outb(self.base + CMD_STATUS, CMD_WRITE);
            self.delay();

            let st = self.wait_bsy();
            if st & STAT_ERR != 0 {
                return false;
            }
            let st = self.wait_drq();
            if st & STAT_ERR != 0 {
                return false;
            }

            // Write 256 words
            for chunk in buf.chunks(2) {
                let w = (chunk[0] as u16) | ((chunk[1] as u16) << 8);
                outw(self.base + DATA, w);
            }

            // Wait for write to complete, then clear any error
            let st = self.wait_bsy();
            if st & STAT_ERR != 0 {
                inb(self.base + 1); // clear error register
            }
            true
        }
    }

    // ── helpers ──────────────────────────────────────────────

    unsafe fn lba_write(&self, lba: u32, count: u8) {
        outb(self.base + SECTOR_COUNT, count);
        outb(self.base + LBA_LO, (lba & 0xFF) as u8);
        outb(self.base + LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(self.base + LBA_HI, ((lba >> 16) & 0xFF) as u8);
        outb(self.base + DRIVE, self.drive_select | LBA_BIT | (((lba >> 24) & 0x0F) as u8));
    }

    unsafe fn wait_bsy(&self) -> u8 {
        loop {
            let st = inb(self.base + CMD_STATUS);
            if st & STAT_BSY == 0 {
                return st;
            }
        }
    }

    unsafe fn wait_drq(&self) -> u8 {
        loop {
            let st = inb(self.base + CMD_STATUS);
            if st & STAT_DRQ != 0 || st & STAT_ERR != 0 {
                return st;
            }
        }
    }

    fn delay(&self) {
        // 400ns delay per ATA spec: read alternate status port 4 times
        unsafe {
            inb(self.base + 0x206);
            inb(self.base + 0x206);
            inb(self.base + 0x206);
            inb(self.base + 0x206);
        }
    }
}

impl BlockDevice for AtaChannel {
    fn read_sector(&self, lba: u32, buf: &mut [u8; 512]) -> bool {
        self.read_sector(lba, buf)
    }

    fn write_sector(&self, lba: u32, buf: &[u8; 512]) -> bool {
        self.write_sector(lba, buf)
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

// ── Port I/O ─────────────────────────────────────────────────

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}

unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack));
    val
}

unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

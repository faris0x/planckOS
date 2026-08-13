/// Block device abstraction — the FAT32 driver operates on any
/// implementation of this trait, not on ATA ports directly.

pub trait BlockDevice {
    /// Read one 512-byte sector. Returns `true` on success.
    fn read_sector(&self, lba: u32, buf: &mut [u8; 512]) -> bool;

    /// Write one 512-byte sector. Returns `true` on success.
    fn write_sector(&self, lba: u32, buf: &[u8; 512]) -> bool;

    /// Total number of 512-byte sectors on the device.
    fn sector_count(&self) -> u64;
}

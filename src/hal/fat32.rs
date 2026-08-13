use crate::hal::block::BlockDevice;

// ── Return codes ────────────────────────────────────────────────

pub type FResult<T> = Result<T, FError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FError {
    Ok,
    DiskErr,
    NoFile,
    NoPath,
    NotFound,
    Denied,
    Exists,
    Invalid,
    WriteProtected,
    MkfsAborted,
    Timeout,
    Locked,
    NotEnabled,
    TooManyOpenFiles,
    InvalidParameter,
}

// ── File attributes ─────────────────────────────────────────────

pub const FA_READ: u8       = 0x01;
pub const FA_WRITE: u8      = 0x02;
pub const FA_OPEN_EXISTING: u8 = 0x00;
pub const FA_CREATE_NEW: u8 = 0x04;
pub const FA_CREATE_ALWAYS: u8 = 0x08;
pub const FA_OPEN_ALWAYS: u8 = 0x10;
pub const FA_OPEN_APPEND: u8 = 0x30;

// ── FAT32 BPB constants ────────────────────────────────────────

const ATTR_READ_ONLY: u8  = 0x01;
const ATTR_HIDDEN: u8     = 0x02;
const ATTR_SYSTEM: u8     = 0x04;
const ATTR_VOLUME: u8     = 0x08;
const ATTR_DIRECTORY: u8  = 0x10;
const ATTR_ARCHIVE: u8    = 0x20;
const ATTR_LFN: u8        = 0x0F;

const DIR_SIZE: usize     = 32;
const SECTOR_SIZE: usize  = 512;

// ── File / Directory objects ──────────────────────────────────

#[derive(Clone)]
pub struct File {
    pub size: u32,
    pub position: u32,
    start_cluster: u32,
    current_cluster: u32,
    sector_in_cluster: u16,
    byte_in_sector: u16,
    flags: u8,
    dir_sector: u32,
    dir_offset: u16,
    modified: bool,
    pub error: FError,
}

pub struct Dir {
    start_cluster: u32,
    current_cluster: u32,
    sector_in_cluster: u16,
    entry_index: u16,
    ended: bool,
    pattern: [u8; 11],
    pat_len: u8,
}

pub struct FileInfo {
    pub size: u32,
    pub attributes: u8,
    pub start_cluster: u32,
    pub write_time: u16,
    pub write_date: u16,
}

// ── FAT32 volume state ────────────────────────────────────────

static mut VOL: Option<FatVolume> = None;

struct FatVolume {
    dev: *const dyn BlockDevice,
    sectors_per_cluster: u16,
    reserved_sectors: u32,
    num_fats: u8,
    sectors_per_fat: u32,
    root_cluster: u32,
    data_region_lba: u32,
    fat_region_lba: u32,
    sector_buf: [u8; SECTOR_SIZE],
}

// ── Mount / Unmount ───────────────────────────────────────────

pub fn f_mount(dev: &'static dyn BlockDevice) -> FResult<()> {
    if dev.sector_count() == 0 {
        crate::hal::serial::serial_debug(b"  [FAT32] Sector count is 0\r\n\0");
        return Err(FError::DiskErr);
    }

    let mut buf = [0u8; SECTOR_SIZE];
    if !dev.read_sector(0, &mut buf) {
        crate::hal::serial::serial_debug(b"  [FAT32] Read sector 0 failed\r\n\0");
        return Err(FError::DiskErr);
    }

    // Validate BPB signature
    if buf[0x1FE] != 0x55 || buf[0x1FF] != 0xAA {
        crate::hal::serial::serial_debug(b"  [FAT32] Bad boot signature\r\n\0");
        return Err(FError::MkfsAborted);
    }

    let bytes_per_sector = u16::from_le_bytes([buf[0x0B], buf[0x0C]]);
    if bytes_per_sector != SECTOR_SIZE as u16 {
        crate::hal::serial::serial_debug(b"  [FAT32] Bad bytes/sector\r\n\0");
        return Err(FError::Invalid);
    }

    let spc = buf[0x0D];
    if spc == 0 || !spc.is_power_of_two() {
        crate::hal::serial::serial_debug(b"  [FAT32] Bad SPC\r\n\0");
        return Err(FError::Invalid);
    }

    let reserved = u16::from_le_bytes([buf[0x0E], buf[0x0F]]) as u32;
    let num_fats = buf[0x10];
    let root_ent_cnt = u16::from_le_bytes([buf[0x11], buf[0x12]]);

    let fatsz = u32::from_le_bytes([
        buf[0x24], buf[0x25], buf[0x26], buf[0x27],
    ]);
    let root_cluster = u32::from_le_bytes([
        buf[0x2C], buf[0x2D], buf[0x2E], buf[0x2F],
    ]);

    // Validate FAT32 (root_ent_cnt must be 0 for FAT32, fatsz > 0)
    if root_ent_cnt != 0 || fatsz == 0 {
        crate::hal::serial::serial_debug(b"  [FAT32] Not FAT32\r\n\0");
        return Err(FError::Invalid);
    }

    let fat_region = reserved;
    let data_region = fat_region + num_fats as u32 * fatsz;

    unsafe {
        VOL = Some(FatVolume {
            dev: dev as *const dyn BlockDevice,
            sectors_per_cluster: spc as u16,
            reserved_sectors: reserved,
            num_fats,
            sectors_per_fat: fatsz,
            root_cluster,
            data_region_lba: data_region,
            fat_region_lba: fat_region,
            sector_buf: [0u8; SECTOR_SIZE],
        });
    }

    cwd_init();

    Ok(())
}

pub fn f_unmount() {
    unsafe { VOL = None; }
}

// ── Internal helpers ──────────────────────────────────────────

fn with_vol<F, T>(f: F) -> FResult<T>
where
    F: FnOnce(&mut FatVolume) -> FResult<T>,
{
    unsafe {
        match VOL.as_mut() {
            Some(v) => f(v),
            None => Err(FError::NotEnabled),
        }
    }
}

impl FatVolume {
    fn dev(&self) -> &dyn BlockDevice {
        unsafe { &*self.dev }
    }

    fn cluster_to_lba(&self, cluster: u32) -> u32 {
        self.data_region_lba + (cluster - 2) as u32 * self.sectors_per_cluster as u32
    }

    fn read_fat_entry(&mut self, cluster: u32) -> FResult<u32> {
        let fat_offset = cluster as u32 * 4;
        let sector = self.fat_region_lba + fat_offset / SECTOR_SIZE as u32;
        let offset = (fat_offset as usize) % SECTOR_SIZE;

        let mut buf = [0u8; SECTOR_SIZE];
        if !self.dev().read_sector(sector, &mut buf) {
            return Err(FError::DiskErr);
        }

        let val = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        Ok(val & 0x0FFFFFFF)
    }

    fn write_fat_entry(&mut self, cluster: u32, val: u32) -> FResult<()> {
        let fat_offset = cluster as u32 * 4;
        let sector = self.fat_region_lba + fat_offset / SECTOR_SIZE as u32;
        let offset = (fat_offset as usize) % SECTOR_SIZE;

        let mut buf = [0u8; SECTOR_SIZE];
        if !self.dev().read_sector(sector, &mut buf) {
            return Err(FError::DiskErr);
        }

        let masked = val & 0x0FFFFFFF;
        let old = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        let merged = (old & 0xF0000000) | masked;
        buf[offset..offset + 4].copy_from_slice(&merged.to_le_bytes());

        // Write to all FAT copies
        for fat_idx in 0..self.num_fats {
            let fat_start = self.fat_region_lba + fat_idx as u32 * self.sectors_per_fat;
            if !self.dev().write_sector(fat_start + sector - self.fat_region_lba, &buf) {
                return Err(FError::DiskErr);
            }
        }

        Ok(())
    }

    fn read_cluster(&mut self, cluster: u32, buf: &mut [u8]) -> FResult<()> {
        let lba = self.cluster_to_lba(cluster);
        let num_sectors = self.sectors_per_cluster as usize;
        let base = buf.as_mut_ptr();
        for i in 0..num_sectors {
            let offset = i * SECTOR_SIZE;
            let sector_buf = unsafe { &mut *core::ptr::slice_from_raw_parts_mut(base.add(offset), SECTOR_SIZE).cast::<[u8; SECTOR_SIZE]>() };
            if !self.dev().read_sector(lba + i as u32, sector_buf) {
                return Err(FError::DiskErr);
            }
        }
        Ok(())
    }

    fn write_cluster(&mut self, cluster: u32, buf: &[u8]) -> FResult<()> {
        let lba = self.cluster_to_lba(cluster);
        let num_sectors = self.sectors_per_cluster as usize;
        let base = buf.as_ptr();
        for i in 0..num_sectors {
            let offset = i * SECTOR_SIZE;
            let sector_buf = unsafe { &*core::ptr::slice_from_raw_parts(base.add(offset), SECTOR_SIZE).cast::<[u8; SECTOR_SIZE]>() };
            if !self.dev().write_sector(lba + i as u32, sector_buf) {
                return Err(FError::DiskErr);
            }
        }
        Ok(())
    }

    fn find_free_cluster(&mut self) -> FResult<u32> {
        // Start from cluster 2 and scan the FAT
        let max_cluster = self.sectors_per_fat * SECTOR_SIZE as u32 / 4;
        for c in 2..max_cluster {
            if self.read_fat_entry(c)? == 0 {
                return Ok(c);
            }
        }
        Err(FError::MkfsAborted) // out of clusters
    }

    fn count_free_clusters(&mut self) -> FResult<u32> {
        let max_cluster = self.sectors_per_fat * SECTOR_SIZE as u32 / 4;
        let mut count = 0;
        for c in 2..max_cluster {
            if self.read_fat_entry(c)? == 0 {
                count += 1;
            }
        }
        Ok(count)
    }

    fn read_dir_entry_name(&self, entry: &[u8; 32]) -> Option<[u8; 13]> {
        let mut name = [0u8; 13];
        let mut i = 0;

        // 8.3 name
        let mut j = 0;
        while j < 8 && entry[j] != b' ' && entry[j] != 0 {
            name[i] = entry[j];
            i += 1;
            j += 1;
        }
        // Extension
        let ext_byte = entry[8];
        if ext_byte != b' ' && ext_byte != 0 {
            name[i] = b'.';
            i += 1;
            let mut j = 8;
            while j < 11 && entry[j] != b' ' && entry[j] != 0 {
                name[i] = entry[j];
                i += 1;
                j += 1;
            }
        }

        if i == 0 { None } else { Some(name) }
    }

    fn find_entry_in_dir(
        &mut self,
        dir_cluster: u32,
        target_name: &[u8],
    ) -> FResult<Option<(u32, u16)>> {
        let mut cluster = dir_cluster;
        let cluster_size = self.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];

        while cluster >= 2 && cluster < 0xFFFFFF8 {
            self.read_cluster(cluster, &mut buf)?;

            for offset in (0..cluster_size).step_by(32) {
                let entry = &buf[offset..offset + 32];
                let attr = entry[0x0B];

                // Skip LFN entries and volume label
                if attr == ATTR_LFN || attr & ATTR_VOLUME != 0 {
                    continue;
                }

                // Check for end marker
                if entry[0] == 0 {
                    return Ok(None);
                }
                // Skip deleted entries
                if entry[0] == 0xE5 {
                    continue;
                }

                    if let Some(name_bytes) = self.read_dir_entry_name(entry.try_into().unwrap()) {
                        let mut name_len = 0;
                        while name_len < target_name.len()
                            && name_bytes[name_len].eq_ignore_ascii_case(&target_name[name_len])
                        {
                            name_len += 1;
                        }
                        if name_len == target_name.len() {
                            return Ok(Some((cluster, (offset / 32) as u16)));
                        }
                }
            }

            cluster = self.read_fat_entry(cluster)?;
        }

        Ok(None)
    }

    fn read_entry_data(
        &mut self,
        dir_cluster: u32,
        entry_idx: u16,
    ) -> FResult<[u8; 32]> {
        let cluster_size = self.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        self.read_cluster(dir_cluster, &mut buf)?;
        let offset = entry_idx as usize * 32;
        let mut entry = [0u8; 32];
        entry.copy_from_slice(&buf[offset..offset + 32]);
        Ok(entry)
    }
}

// ── File Operations ───────────────────────────────────────────

pub fn f_open(name: &str, flags: u8) -> FResult<File> {
    with_vol(|vol| {
        let (dir_cluster, name_part) = resolve_path(vol, name)?;
        let name_bytes = name_part.as_bytes();

        if let Some((parent_cluster, entry_idx)) =
            vol.find_entry_in_dir(dir_cluster, name_bytes)?
        {
            // File exists
            if flags & FA_CREATE_NEW != 0 {
                return Err(FError::Exists);
            }

            let entry = vol.read_entry_data(parent_cluster, entry_idx)?;
            let start_cluster = u32::from_le_bytes([
                entry[0x1A], entry[0x1B], entry[0x14], entry[0x15],
            ]);
            let mut size = u32::from_le_bytes([
                entry[0x1C], entry[0x1D], entry[0x1E], entry[0x1F],
            ]);
            let mut current = start_cluster;

            // FA_CREATE_ALWAYS: truncate to zero
            if flags & FA_CREATE_ALWAYS != 0 {
                // Free the cluster chain
                let mut clu = start_cluster;
                while clu >= 2 && clu < 0xFFFFFF8 {
                    let next = vol.read_fat_entry(clu)?;
                    vol.write_fat_entry(clu, 0)?;
                    clu = next;
                }
                size = 0;
                current = 0;
            }

            Ok(File {
                size,
                position: 0,
                start_cluster,
                current_cluster: current,
                sector_in_cluster: 0,
                byte_in_sector: 0,
                flags,
                dir_sector: parent_cluster,
                dir_offset: entry_idx,
                modified: false,
                error: FError::Ok,
            })
        } else {
            // File doesn't exist
            if flags & (FA_CREATE_NEW | FA_CREATE_ALWAYS | FA_OPEN_ALWAYS) == 0 {
                return Err(FError::NoFile);
            }

            // Allocate a cluster FIRST — if this fails, no ghost entry is created
            let new_cluster = vol.find_free_cluster()?;
            vol.write_fat_entry(new_cluster, 0x0FFFFFFF)?;

            // Find a free slot in the parent directory
            let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
            let mut slot_cluster = dir_cluster;
            let mut slot_offset = None;
            loop {
                let mut buf = alloc::vec![0u8; cluster_size];
                vol.read_cluster(slot_cluster, &mut buf)?;
                for off in (0..cluster_size).step_by(32) {
                    if buf[off] == 0 || buf[off] == 0xE5 {
                        slot_offset = Some((slot_cluster, off, buf));
                        break;
                    }
                }
                if slot_offset.is_some() { break; }
                let next = vol.read_fat_entry(slot_cluster)?;
                if next >= 0xFFFFFF8 {
                    return Err(FError::DiskErr);
                }
                slot_cluster = next;
            }

            let (dest_cluster, dest_off, mut dest_buf) = slot_offset.unwrap();

            // Write the 8.3 name
            dest_buf[dest_off..dest_off + 11].fill(b' ');
            let dot = name_bytes.iter().position(|&b| b == b'.');
            let name_end = dot.unwrap_or(name_bytes.len()).min(8);
            for j in 0..name_end {
                dest_buf[dest_off + j] = name_bytes[j];
            }
            if let Some(d) = dot {
                let ext_end = (d + 1 + 3).min(name_bytes.len());
                for j in 0..ext_end - d - 1 {
                    dest_buf[dest_off + 8 + j] = name_bytes[d + 1 + j];
                }
            }
            dest_buf[dest_off + 0x0B] = ATTR_ARCHIVE;
            let clus_low = (new_cluster & 0xFFFF) as u16;
            let clus_high = (new_cluster >> 16) as u16;
            dest_buf[dest_off + 0x1A..dest_off + 0x1C].copy_from_slice(&clus_low.to_le_bytes());
            dest_buf[dest_off + 0x14..dest_off + 0x16].copy_from_slice(&clus_high.to_le_bytes());

            vol.write_cluster(dest_cluster, &mut dest_buf)?;

            Ok(File {
                size: 0,
                position: 0,
                start_cluster: new_cluster,
                current_cluster: new_cluster,
                sector_in_cluster: 0,
                byte_in_sector: 0,
                flags,
                dir_sector: dest_cluster,
                dir_offset: (dest_off / 32) as u16,
                modified: true,
                error: FError::Ok,
            })
        }
    })
}

pub fn f_read(file: &mut File, buf: &mut [u8]) -> FResult<usize> {
    if file.flags & FA_READ == 0 {
        file.error = FError::Denied;
        return Err(FError::Denied);
    }

    let remaining = (file.size - file.position) as usize;
    let to_read = buf.len().min(remaining);
    if to_read == 0 {
        return Ok(0);
    }

    let cluster_size = match with_vol(|vol| Ok(vol.sectors_per_cluster as usize * SECTOR_SIZE)) {
        Ok(s) => s,
        Err(e) => { file.error = e; return Err(e); }
    };
    let mut read = 0;

    while read < to_read {
        // Read current sector
        let mut sector_buf = [0u8; SECTOR_SIZE];
        if let Err(e) = with_vol(|vol| {
            let lba = vol.cluster_to_lba(file.current_cluster) + file.sector_in_cluster as u32;
            if !unsafe { (&*vol.dev).read_sector(lba, &mut sector_buf) } {
                return Err(FError::DiskErr);
            }
            Ok(())
        }) {
            file.error = e;
            return Err(e);
        }

        let to_copy = (SECTOR_SIZE - file.byte_in_sector as usize)
            .min(to_read - read);
        buf[read..read + to_copy]
            .copy_from_slice(&sector_buf[file.byte_in_sector as usize..file.byte_in_sector as usize + to_copy]);

        read += to_copy;
        file.byte_in_sector += to_copy as u16;
        file.position += to_copy as u32;

        // Advance to next sector/cluster
        if file.byte_in_sector >= SECTOR_SIZE as u16 {
            file.byte_in_sector = 0;
            file.sector_in_cluster += 1;

            if file.sector_in_cluster >= match with_vol(|vol| Ok(vol.sectors_per_cluster)) {
                Ok(spc) => spc,
                Err(e) => { file.error = e; return Err(e); }
            } {
                file.sector_in_cluster = 0;
                let next = match with_vol(|vol| vol.read_fat_entry(file.current_cluster)) {
                    Ok(n) => n,
                    Err(e) => { file.error = e; return Err(e); }
                };
                if next >= 0xFFFFFF8 {
                    file.current_cluster = 0;
                    break;
                }
                file.current_cluster = next;
            }
        }
    }

    Ok(read)
}

pub fn f_write(file: &mut File, buf: &[u8]) -> FResult<usize> {
    if file.flags & FA_WRITE == 0 {
        file.error = FError::Denied;
        return Err(FError::Denied);
    }
    if buf.is_empty() {
        return Ok(0);
    }

    // Allocate first cluster if needed
    if file.current_cluster == 0 {
        let new_clu = match with_vol(|vol| vol.find_free_cluster()) {
            Ok(c) => c,
            Err(e) => { file.error = e; return Err(e); }
        };
        if let Err(e) = with_vol(|vol| vol.write_fat_entry(new_clu, 0x0FFFFFFF)) {
            file.error = e; return Err(e);
        }
        file.start_cluster = new_clu;
        file.current_cluster = new_clu;
    }

    let to_write = buf.len();
    let mut written = 0;

    while written < to_write {
        // Read-modify-write using a stack buffer
        let mut sector_buf = [0u8; SECTOR_SIZE];
        if let Err(e) = with_vol(|vol| {
            let lba = vol.cluster_to_lba(file.current_cluster) + file.sector_in_cluster as u32;
            if !vol.dev().read_sector(lba, &mut sector_buf) {
                return Err(FError::DiskErr);
            }
            Ok(())
        }) {
            file.error = e;
            return Err(e);
        }

        let space = (SECTOR_SIZE - file.byte_in_sector as usize)
            .min(to_write - written);
        sector_buf[file.byte_in_sector as usize..file.byte_in_sector as usize + space]
            .copy_from_slice(&buf[written..written + space]);

        if let Err(e) = with_vol(|vol| {
            let lba = vol.cluster_to_lba(file.current_cluster) + file.sector_in_cluster as u32;
            if !vol.dev().write_sector(lba, &sector_buf) {
                return Err(FError::DiskErr);
            }
            Ok(())
        }) {
            file.error = e;
            return Err(e);
        }

        written += space;
        file.byte_in_sector += space as u16;
        file.position += space as u32;
        if file.position > file.size {
            file.size = file.position;
        }
        file.modified = true;

        // Advance to next sector/cluster
        if file.byte_in_sector >= SECTOR_SIZE as u16 {
            file.byte_in_sector = 0;
            file.sector_in_cluster += 1;

            let spc = match with_vol(|vol| Ok(vol.sectors_per_cluster)) {
                Ok(s) => s,
                Err(e) => { file.error = e; return Err(e); }
            };
            if file.sector_in_cluster >= spc {
                file.sector_in_cluster = 0;
                let next = match with_vol(|vol| vol.read_fat_entry(file.current_cluster)) {
                    Ok(n) => n,
                    Err(e) => { file.error = e; return Err(e); }
                };
                if next >= 0xFFFFFF8 {
                    // Allocate a new cluster
                    let new_clu = match with_vol(|vol| vol.find_free_cluster()) {
                        Ok(c) => c,
                        Err(e) => { file.error = e; return Err(e); }
                    };
                    if let Err(e) = with_vol(|vol| vol.write_fat_entry(new_clu, 0x0FFFFFFF)) {
                        file.error = e; return Err(e);
                    }
                    if let Err(e) = with_vol(|vol| vol.write_fat_entry(file.current_cluster, new_clu)) {
                        file.error = e; return Err(e);
                    }
                    file.current_cluster = new_clu;
                } else {
                    file.current_cluster = next;
                }
            }
        }
    }

    Ok(written)
}

pub fn f_lseek(file: &mut File, offset: u32) -> FResult<()> {
    if offset > file.size && file.flags & FA_WRITE == 0 {
        file.error = FError::Denied;
        return Err(FError::Denied);
    }
    if offset == file.position {
        return Ok(());
    }

    let cluster_size = match with_vol(|vol| Ok(vol.sectors_per_cluster as usize * SECTOR_SIZE)) {
        Ok(s) => s,
        Err(e) => { file.error = e; return Err(e); }
    };
    let target_cluster_idx = offset / cluster_size as u32;
    let sectors_per_cluster = match with_vol(|vol| Ok(vol.sectors_per_cluster)) {
        Ok(s) => s,
        Err(e) => { file.error = e; return Err(e); }
    };

    // Walk the cluster chain to find the target cluster
    let mut clu = file.start_cluster;
    for _ in 0..target_cluster_idx {
        clu = match with_vol(|vol| vol.read_fat_entry(clu)) {
            Ok(c) => c,
            Err(e) => { file.error = e; return Err(e); }
        };
        if clu >= 0xFFFFFF8 {
            // Chain ended early — position is past actual allocation
            let cluster_bytes = cluster_size as u32;
            let actual_position = (offset / cluster_bytes).min(
                // Calculate max reachable position
                u32::MAX
            );
            file.current_cluster = 0;
            file.sector_in_cluster = 0;
            file.byte_in_sector = 0;
            file.position = actual_position;
            return Ok(());
        }
    }

    let offset_in_cluster = offset % cluster_size as u32;
    file.current_cluster = clu;
    file.sector_in_cluster = (offset_in_cluster / SECTOR_SIZE as u32) as u16;
    file.byte_in_sector = (offset_in_cluster % SECTOR_SIZE as u32) as u16;
    file.position = offset;
    Ok(())
}

pub fn f_truncate(file: &mut File) -> FResult<()> {
    if file.flags & FA_WRITE == 0 {
        file.error = FError::Denied;
        return Err(FError::Denied);
    }

    // Walk the chain past current_cluster, freeing each
    let mut clu = match with_vol(|vol| vol.read_fat_entry(file.current_cluster)) {
        Ok(c) => c,
        Err(e) => { file.error = e; return Err(e); }
    };
    while clu >= 2 && clu < 0xFFFFFF8 {
        let next = match with_vol(|vol| vol.read_fat_entry(clu)) {
            Ok(c) => c,
            Err(e) => { file.error = e; return Err(e); }
        };
        if let Err(e) = with_vol(|vol| vol.write_fat_entry(clu, 0)) {
            file.error = e; return Err(e);
        }
        clu = next;
    }

    // Mark current cluster as end-of-chain
    if file.current_cluster >= 2 {
        if let Err(e) = with_vol(|vol| vol.write_fat_entry(file.current_cluster, 0x0FFFFFFF)) {
            file.error = e; return Err(e);
        }
    }

    file.size = file.position;
    file.modified = true;
    Ok(())
}

pub fn f_expand(file: &mut File, size: u32) -> FResult<()> {
    if file.flags & FA_WRITE == 0 {
        file.error = FError::Denied;
        return Err(FError::Denied);
    }

    let cluster_size = match with_vol(|vol| Ok(vol.sectors_per_cluster as usize * SECTOR_SIZE)) {
        Ok(s) => s,
        Err(e) => { file.error = e; return Err(e); }
    } as u32;

    if size <= file.size {
        return Ok(());
    }

    let current_clusters = if file.start_cluster == 0 { 0 } else {
        // Count existing chain
        let mut count = 1;
        let mut clu = file.start_cluster;
        loop {
            let next = match with_vol(|vol| vol.read_fat_entry(clu)) {
                Ok(c) => c,
                Err(e) => { file.error = e; return Err(e); }
            };
            if next >= 0xFFFFFF8 { break; }
            clu = next;
            count += 1;
        }
        count
    };

    let needed_clusters = ((size + cluster_size - 1) / cluster_size).max(1);
    let to_add = needed_clusters.saturating_sub(current_clusters);

    if to_add == 0 {
        return Ok(());
    }

    // Walk to the end of the current chain
    let mut tail = file.start_cluster;
    if tail == 0 {
        // No clusters allocated yet — allocate first
        let new_clu = match with_vol(|vol| vol.find_free_cluster()) {
            Ok(c) => c,
            Err(e) => { file.error = e; return Err(e); }
        };
        if let Err(e) = with_vol(|vol| vol.write_fat_entry(new_clu, 0x0FFFFFFF)) {
            file.error = e; return Err(e);
        }
        file.start_cluster = new_clu;
        file.current_cluster = new_clu;
        tail = new_clu;
        // We need to_add - 1 more
        for _ in 1..to_add {
            let next = match with_vol(|vol| vol.find_free_cluster()) {
                Ok(c) => c,
                Err(e) => { file.error = e; return Err(e); }
            };
            if let Err(e) = with_vol(|vol| vol.write_fat_entry(next, 0x0FFFFFFF)) {
                file.error = e; return Err(e);
            }
            if let Err(e) = with_vol(|vol| vol.write_fat_entry(tail, next)) {
                file.error = e; return Err(e);
            }
            tail = next;
        }
    } else {
        // Walk to end of chain
        loop {
            let next = match with_vol(|vol| vol.read_fat_entry(tail)) {
                Ok(c) => c,
                Err(e) => { file.error = e; return Err(e); }
            };
            if next >= 0xFFFFFF8 { break; }
            tail = next;
        }
        // Append new clusters
        for _ in 0..to_add {
            let new_clu = match with_vol(|vol| vol.find_free_cluster()) {
                Ok(c) => c,
                Err(e) => { file.error = e; return Err(e); }
            };
            if let Err(e) = with_vol(|vol| vol.write_fat_entry(new_clu, 0x0FFFFFFF)) {
                file.error = e; return Err(e);
            }
            if let Err(e) = with_vol(|vol| vol.write_fat_entry(tail, new_clu)) {
                file.error = e; return Err(e);
            }
            tail = new_clu;
        }
    }

    file.size = size;
    file.modified = true;
    Ok(())
}

pub fn f_close(_file: File) -> FResult<()> {
    Ok(())
}

pub fn f_tell(file: &File) -> u32 {
    file.position
}

pub fn f_eof(file: &File) -> bool {
    file.position >= file.size
}

pub fn f_size(file: &File) -> u32 {
    file.size
}

pub fn f_error(file: &File) -> bool {
    file.error != FError::Ok
}

pub fn f_stat(path: &str) -> FResult<FileInfo> {
    with_vol(|vol| {
        let (parent, name) = resolve_path(vol, path)?;
        if name.is_empty() {
            // Root directory
            return Ok(FileInfo {
                size: 0,
                attributes: ATTR_DIRECTORY,
                start_cluster: vol.root_cluster,
                write_time: 0,
                write_date: 0,
            });
        }
        let (entry_cluster, entry_idx) = vol.find_entry_in_dir(parent, name.as_bytes())?
            .ok_or(FError::NoFile)?;
        let entry = vol.read_entry_data(entry_cluster, entry_idx)?;
        Ok(FileInfo {
            size: u32::from_le_bytes([entry[0x1C], entry[0x1D], entry[0x1E], entry[0x1F]]),
            attributes: entry[0x0B],
            start_cluster: u32::from_le_bytes([
                entry[0x1A], entry[0x1B], entry[0x14], entry[0x15],
            ]),
            write_time: u16::from_le_bytes([entry[0x16], entry[0x17]]),
            write_date: u16::from_le_bytes([entry[0x18], entry[0x19]]),
        })
    })
}

pub fn f_sync(file: &mut File) -> FResult<()> {
    if !file.modified {
        return Ok(());
    }

    let (lba, off) = match with_vol(|vol| {
        let l = vol.cluster_to_lba(file.dir_sector);
        let o = file.dir_offset as usize * 32;
        Ok((l, o))
    }) {
        Ok(r) => r,
        Err(e) => { file.error = e; return Err(e); }
    };

    static mut SC: [u8; SECTOR_SIZE] = [0u8; SECTOR_SIZE];
    let sc = unsafe { &mut SC };

    // Disable interrupts around ATA I/O to prevent timer IRQ from interfering
    unsafe { core::arch::asm!("cli", options(nomem, nostack)); }

    let ok = with_vol(|vol| Ok(vol.dev().read_sector(lba, sc)))?;
    if !ok { file.error = FError::DiskErr; unsafe { core::arch::asm!("sti", options(nomem, nostack)); } return Err(FError::DiskErr); }

    let sz = file.size.to_le_bytes();
    sc[off + 0x1C..off + 0x20].copy_from_slice(&sz);

    let cl = (file.start_cluster & 0xFFFF) as u16;
    let ch = (file.start_cluster >> 16) as u16;
    sc[off + 0x1A..off + 0x1C].copy_from_slice(&cl.to_le_bytes());
    sc[off + 0x14..off + 0x16].copy_from_slice(&ch.to_le_bytes());

    let wrote = with_vol(|vol| Ok(vol.dev().write_sector(lba, &*sc)))?;
    if !wrote { file.error = FError::DiskErr; unsafe { core::arch::asm!("sti", options(nomem, nostack)); } return Err(FError::DiskErr); }

    unsafe { core::arch::asm!("sti", options(nomem, nostack)); }
    file.modified = false;
    Ok(())
}

// ── String I/O ────────────────────────────────────────────────

const FMT_BUF_SIZE: usize = 64;

pub fn f_gets(buf: &mut [u8], n: usize, file: &mut File) -> FResult<usize> {
    let mut i = 0usize;
    let max = (n - 1).min(buf.len().saturating_sub(1));
    while i < max {
        let mut c = [0u8; 1];
        let r = f_read(file, &mut c)?;
        if r == 0 {
            break;
        }
        if c[0] == b'\r' {
            continue;
        }
        buf[i] = c[0];
        i += 1;
        if c[0] == b'\n' {
            break;
        }
    }
    if i < buf.len() {
        buf[i] = 0;
    }
    Ok(i)
}

pub fn f_putc(c: u8, file: &mut File) -> FResult<usize> {
    let buf = [c];
    f_write(file, &buf)
}

pub fn f_puts(s: &[u8], file: &mut File) -> FResult<usize> {
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    if end > 0 {
        f_write(file, &s[..end])
    } else {
        Ok(0)
    }
}

pub enum FmtArg<'a> {
    Char(u8),
    Str(&'a [u8]),
    Int(i32),
    Hex(u32),
}

pub fn f_printf(file: &mut File, fmt: &[u8], args: &[FmtArg]) -> FResult<usize> {
    let mut written = 0usize;
    let mut arg_idx = 0;
    let mut i = 0;
    let mut tmp = [0u8; FMT_BUF_SIZE];

    while i < fmt.len() {
        if fmt[i] != b'%' {
            written += f_write(file, &fmt[i..i + 1])?;
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        match fmt[i] {
            b'c' => {
                if let Some(FmtArg::Char(c)) = args.get(arg_idx) {
                    written += f_putc(*c, file)?;
                }
                arg_idx += 1;
            }
            b's' => {
                if let Some(FmtArg::Str(s)) = args.get(arg_idx) {
                    written += f_puts(s, file)?;
                }
                arg_idx += 1;
            }
            b'd' => {
                if let Some(FmtArg::Int(val)) = args.get(arg_idx) {
                    let negative = *val < 0;
                    let mut n = val.unsigned_abs();
                    let mut pos = FMT_BUF_SIZE;
                    loop {
                        pos -= 1;
                        tmp[pos] = (n % 10) as u8 + b'0';
                        n /= 10;
                        if n == 0 { break; }
                    }
                    if negative {
                        pos -= 1;
                        tmp[pos] = b'-';
                    }
                    written += f_write(file, &tmp[pos..])?;
                }
                arg_idx += 1;
            }
            b'x' | b'X' => {
                if let Some(FmtArg::Hex(val)) = args.get(arg_idx) {
                    let mut n = *val;
                    let mut pos = FMT_BUF_SIZE;
                    let hex = b"0123456789ABCDEF";
                    loop {
                        pos -= 1;
                        tmp[pos] = hex[(n & 0xF) as usize];
                        n >>= 4;
                        if n == 0 { break; }
                    }
                    written += f_write(file, &tmp[pos..])?;
                }
                arg_idx += 1;
            }
            b'%' => {
                written += f_write(file, &[b'%'])?;
            }
            _ => {
                written += f_write(file, &[b'%', fmt[i]])?;
            }
        }
        i += 1;
    }

    Ok(written)
}

// ── Directory Operations ──────────────────────────────────────

fn match_pattern(name: &[u8; 13], pattern: &[u8; 11], pat_len: u8) -> bool {
    if pat_len == 0 {
        return true;
    }
    let pat = &pattern[..pat_len as usize];
    let mut ni = 0;
    let mut pi = 0;
    while pi < pat.len() {
        match pat[pi] {
            b'*' => {
                // Try matching rest at every position
                pi += 1;
                while ni < 11 && name[ni] != 0 {
                    if match_pattern_inner(name, ni, pat, pi) {
                        return true;
                    }
                    ni += 1;
                }
                return match_pattern_inner(name, ni, pat, pi);
            }
            b'?' => {
                if name[ni] == 0 || name[ni] == b' ' { return false; }
                ni += 1;
                pi += 1;
            }
            c => {
                if name[ni] != c { return false; }
                ni += 1;
                pi += 1;
            }
        }
    }
    // Pattern consumed — name should be at end or space/null
    name[ni] == 0 || name[ni] == b' '
}

fn match_pattern_inner(name: &[u8; 13], ni: usize, pat: &[u8], pi: usize) -> bool {
    let mut n = ni;
    let mut p = pi;
    while p < pat.len() {
        match pat[p] {
            b'*' => {
                p += 1;
                while n < 11 && name[n] != 0 {
                    if match_pattern_inner(name, n, pat, p) {
                        return true;
                    }
                    n += 1;
                }
                return match_pattern_inner(name, n, pat, p);
            }
            b'?' => {
                if name[n] == 0 || name[n] == b' ' { return false; }
                n += 1;
                p += 1;
            }
            c => {
                if name[n] != c { return false; }
                n += 1;
                p += 1;
            }
        }
    }
    name[n] == 0 || name[n] == b' '
}

pub fn f_opendir(path: &str) -> FResult<Dir> {
    with_vol(|vol| {
        let (dir_cluster, _) = resolve_path(vol, path)?;
        Ok(Dir {
            start_cluster: dir_cluster,
            current_cluster: dir_cluster,
            sector_in_cluster: 0,
            entry_index: 0,
            ended: false,
            pattern: [0u8; 11],
            pat_len: 0,
        })
    })
}

pub fn f_readdir(dir: &mut Dir, out_name: &mut [u8; 13]) -> FResult<bool> {
    with_vol(|vol| {
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];

        loop {
            if dir.ended {
                return Ok(false);
            }

            // If we've exhausted current cluster, move to next
            let max_entries = cluster_size / 32;
            if dir.entry_index as usize >= max_entries {
                dir.entry_index = 0;
                dir.sector_in_cluster = 0;
                let next = vol.read_fat_entry(dir.current_cluster)?;
                if next >= 0xFFFFFF8 {
                    dir.ended = true;
                    return Ok(false);
                }
                dir.current_cluster = next;
            }

            vol.read_cluster(dir.current_cluster, &mut buf)?;

            let offset = dir.entry_index as usize * 32;
            dir.entry_index += 1;

            let entry = &buf[offset..offset + 32];
            if entry[0] == 0 {
                dir.ended = true;
                return Ok(false);
            }
            if entry[0] == 0xE5 || entry[0x0B] == ATTR_LFN || entry[0x0B] & ATTR_VOLUME != 0 {
                continue;
            }

            if let Some(name_bytes) = vol.read_dir_entry_name(entry.try_into().unwrap()) {
                if dir.pat_len == 0 || match_pattern(&name_bytes, &dir.pattern, dir.pat_len) {
                    *out_name = name_bytes;
                    return Ok(true);
                }
                continue;
            }
        }
    })
}

pub fn f_closedir(_dir: Dir) -> FResult<()> {
    Ok(())
}

// ── File / Directory Management ──────────────────────────────

pub fn f_findfirst(path: &str, pattern: &[u8]) -> FResult<Dir> {
    let mut dir = f_opendir(path)?;
    let pat_len = pattern.len().min(11);
    dir.pattern[..pat_len].copy_from_slice(&pattern[..pat_len]);
    dir.pat_len = pat_len as u8;
    Ok(dir)
}

pub fn f_findnext(dir: &mut Dir, out_name: &mut [u8; 13]) -> FResult<bool> {
    f_readdir(dir, out_name)
}

pub fn f_mkdir(path: &str) -> FResult<()> {
    with_vol(|vol| {
        let (parent_cluster, name) = resolve_path(vol, path)?;
        if name.is_empty() {
            return Err(FError::Invalid);
        }
        let name_bytes = name.as_bytes();
        if name_bytes.len() > 11 {
            return Err(FError::Invalid);
        }

        // Check if name already exists
        if vol.find_entry_in_dir(parent_cluster, name_bytes)?.is_some() {
            return Err(FError::Exists);
        }

        // Allocate a cluster for the new directory
        let new_cluster = vol.find_free_cluster()?;
        vol.write_fat_entry(new_cluster, 0x0FFFFFFF)?;

        // Initialize directory with "." and ".."
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];

        // "." entry
        buf[0] = b'.';
        buf[1..11].fill(b' ');
        buf[0x0B] = ATTR_DIRECTORY;
        let low = (new_cluster & 0xFFFF) as u16;
        let high = (new_cluster >> 16) as u16;
        buf[0x1A..0x1C].copy_from_slice(&low.to_le_bytes());
        buf[0x14..0x16].copy_from_slice(&high.to_le_bytes());

        // ".." entry
        buf[32] = b'.';
        buf[33] = b'.';
        buf[34..43].fill(b' ');
        buf[32 + 0x0B] = ATTR_DIRECTORY;
        let plow = (parent_cluster & 0xFFFF) as u16;
        let phigh = (parent_cluster >> 16) as u16;
        buf[32 + 0x1A..32 + 0x1C].copy_from_slice(&plow.to_le_bytes());
        buf[32 + 0x14..32 + 0x16].copy_from_slice(&phigh.to_le_bytes());

        vol.write_cluster(new_cluster, &buf)?;

        // Create the directory entry in the parent
        let parent_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut parent_buf = alloc::vec![0u8; parent_size];
        // Find a free slot in the parent directory
        let mut slot_cluster = parent_cluster;
        let mut slot_offset = None;
        loop {
            vol.read_cluster(slot_cluster, &mut parent_buf)?;
            for off in (0..parent_size).step_by(32) {
                if parent_buf[off] == 0 || parent_buf[off] == 0xE5 {
                    slot_offset = Some(off);
                    break;
                }
            }
            if slot_offset.is_some() {
                break;
            }
            let next = vol.read_fat_entry(slot_cluster)?;
            if next >= 0xFFFFFF8 {
                return Err(FError::DiskErr); // No free slot
            }
            slot_cluster = next;
        }
        let off = slot_offset.unwrap();

        // Write the 8.3 name
        let mut i = 0;
        while i < name_bytes.len() && i < 8 && name_bytes[i] != b'.' {
            parent_buf[off + i] = name_bytes[i];
            i += 1;
        }
        if name_bytes.len() > 8 {
            // Has extension
            parent_buf[off + 8] = b' '; // extension start (space-filled if no ext)
            parent_buf[off + 9] = b' ';
            parent_buf[off + 10] = b' ';
        }
        // Fill remaining name bytes with spaces
        for j in i..8 {
            parent_buf[off + j] = b' ';
        }
        // Write extension if present
        if let Some(dot) = name_bytes.iter().position(|&b| b == b'.') {
            let ext_start = dot + 1;
            for j in 0..3 {
                if ext_start + j < name_bytes.len() {
                    parent_buf[off + 8 + j] = name_bytes[ext_start + j];
                } else {
                    parent_buf[off + 8 + j] = b' ';
                }
            }
        }

        parent_buf[off + 0x0B] = ATTR_DIRECTORY;
        let low = (new_cluster & 0xFFFF) as u16;
        let high = (new_cluster >> 16) as u16;
        parent_buf[off + 0x1A..off + 0x1C].copy_from_slice(&low.to_le_bytes());
        parent_buf[off + 0x14..off + 0x16].copy_from_slice(&high.to_le_bytes());

        vol.write_cluster(slot_cluster, &mut parent_buf)?;

        Ok(())
    })
}

pub fn f_unlink(path: &str) -> FResult<()> {
    with_vol(|vol| {
        let (parent_cluster, name) = resolve_path(vol, path)?;
        if name.is_empty() {
            return Err(FError::Invalid);
        }
        let name_bytes = name.as_bytes();

        let (entry_cluster, entry_idx) = match vol.find_entry_in_dir(parent_cluster, name_bytes)? {
            Some(x) => x,
            None => return Err(FError::NoFile),
        };

        // Read the cluster containing the entry
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        vol.read_cluster(entry_cluster, &mut buf)?;

        let off = entry_idx as usize * 32;

        // Read the file's start_cluster before deleting
        let start_cluster = u32::from_le_bytes([
            buf[off + 0x1A], buf[off + 0x1B],
            buf[off + 0x14], buf[off + 0x15],
        ]);

        // Mark the entry as deleted
        buf[off] = 0xE5;
        vol.write_cluster(entry_cluster, &mut buf)?;

        // Free the cluster chain
        let mut clu = start_cluster;
        while clu >= 2 && clu < 0xFFFFFF8 {
            let next = vol.read_fat_entry(clu)?;
            vol.write_fat_entry(clu, 0)?;
            clu = next;
        }

        Ok(())
    })
}

pub fn f_rename(old_path: &str, new_path: &str) -> FResult<()> {
    with_vol(|vol| {
        // Resolve old path
        let (old_parent, old_name) = resolve_path(vol, old_path)?;
        let old_bytes = old_name.as_bytes();
        let (entry_cluster, entry_idx) = match vol.find_entry_in_dir(old_parent, old_bytes)? {
            Some(x) => x,
            None => return Err(FError::NoFile),
        };

        // Read the old entry data
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        vol.read_cluster(entry_cluster, &mut buf)?;
        let mut entry_data = [0u8; 32];
        entry_data.copy_from_slice(&buf[entry_idx as usize * 32..][..32]);

        // Resolve new path
        let (new_parent, new_name) = resolve_path(vol, new_path)?;
        let new_bytes = new_name.as_bytes();
        if new_bytes.is_empty() {
            return Err(FError::Invalid);
        }

        // Check new name doesn't already exist
        if vol.find_entry_in_dir(new_parent, new_bytes)?.is_some() {
            return Err(FError::Exists);
        }

        // Find a free slot in new parent
        let mut slot_cluster = new_parent;
        let mut slot_offset = None;
        loop {
            let ps = vol.sectors_per_cluster as usize * SECTOR_SIZE;
            let mut pb = alloc::vec![0u8; ps];
            vol.read_cluster(slot_cluster, &mut pb)?;
            for off in (0..ps).step_by(32) {
                if pb[off] == 0 || pb[off] == 0xE5 {
                    slot_offset = Some((slot_cluster, off, pb));
                    break;
                }
            }
            if slot_offset.is_some() {
                break;
            }
            let next = vol.read_fat_entry(slot_cluster)?;
            if next >= 0xFFFFFF8 {
                return Err(FError::DiskErr);
            }
            slot_cluster = next;
        }

        let (dest_cluster, dest_off, mut dest_buf) = slot_offset.unwrap();

        // Write name into entry (proper 8.3 encoding)
        dest_buf[dest_off..dest_off + 11].fill(b' ');
        let dot = new_bytes.iter().position(|&b| b == b'.');
        let name_end = dot.unwrap_or(new_bytes.len()).min(8);
        for j in 0..name_end {
            dest_buf[dest_off + j] = new_bytes[j];
        }
        if let Some(d) = dot {
            let ext_end = (d + 1 + 3).min(new_bytes.len());
            for j in 0..ext_end - d - 1 {
                dest_buf[dest_off + 8 + j] = new_bytes[d + 1 + j];
            }
        }
        // Copy the rest of the entry data
        dest_buf[dest_off + 0x0B..dest_off + 32].copy_from_slice(&entry_data[0x0B..]);

        vol.write_cluster(dest_cluster, &mut dest_buf)?;

        // Mark old entry as deleted
        let mut old_buf = alloc::vec![0u8; cluster_size];
        vol.read_cluster(entry_cluster, &mut old_buf)?;
        old_buf[entry_idx as usize * 32] = 0xE5;
        vol.write_cluster(entry_cluster, &mut old_buf)?;

        Ok(())
    })
}

pub fn f_chmod(path: &str, attrs: u8) -> FResult<()> {
    with_vol(|vol| {
        let (parent, name) = resolve_path(vol, path)?;
        let (entry_cluster, entry_idx) = vol.find_entry_in_dir(parent, name.as_bytes())?
            .ok_or(FError::NoFile)?;

        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        vol.read_cluster(entry_cluster, &mut buf)?;
        let off = entry_idx as usize * 32;
        buf[off + 0x0B] = attrs & !ATTR_LFN; // Never set LFN attribute
        vol.write_cluster(entry_cluster, &mut buf)?;
        Ok(())
    })
}

pub fn f_utime(path: &str, time: u16, date: u16) -> FResult<()> {
    with_vol(|vol| {
        let (parent, name) = resolve_path(vol, path)?;
        let (entry_cluster, entry_idx) = vol.find_entry_in_dir(parent, name.as_bytes())?
            .ok_or(FError::NoFile)?;

        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        vol.read_cluster(entry_cluster, &mut buf)?;
        let off = entry_idx as usize * 32;
        buf[off + 0x16..off + 0x18].copy_from_slice(&time.to_le_bytes());
        buf[off + 0x18..off + 0x1A].copy_from_slice(&date.to_le_bytes());
        vol.write_cluster(entry_cluster, &mut buf)?;
        Ok(())
    })
}

// ── Volume operations ────────────────────────────────────────

pub fn f_getfree() -> FResult<(u32, u32)> {
    with_vol(|vol| {
        let max_cluster = vol.sectors_per_fat * SECTOR_SIZE as u32 / 4;
        let free = vol.count_free_clusters()?;
        Ok((free, max_cluster - 2))
    })
}

pub fn f_getlabel(label: &mut [u8; 12]) -> FResult<()> {
    with_vol(|vol| {
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        let mut clu = vol.root_cluster;
        loop {
            vol.read_cluster(clu, &mut buf)?;
            for off in (0..cluster_size).step_by(32) {
                if buf[off] == 0 {
                    return Ok(()); // no volume label found
                }
                if buf[off] == 0xE5 {
                    continue;
                }
                if buf[off + 0x0B] == ATTR_VOLUME {
                    let mut i = 0;
                    for j in 0..11 {
                        if buf[off + j] != b' ' {
                            label[i] = buf[off + j];
                            i += 1;
                        }
                    }
                    label[i..].fill(0);
                    return Ok(());
                }
            }
            let next = vol.read_fat_entry(clu)?;
            if next >= 0xFFFFFF8 { break; }
            clu = next;
        }
        label.fill(0);
        Ok(())
    })
}

pub fn f_setlabel(label: &[u8]) -> FResult<()> {
    with_vol(|vol| {
        let cluster_size = vol.sectors_per_cluster as usize * SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; cluster_size];
        let mut clu = vol.root_cluster;
        let mut found_slot = false;
        loop {
            vol.read_cluster(clu, &mut buf)?;
            for off in (0..cluster_size).step_by(32) {
                if buf[off] == 0 && !found_slot {
                    // Create new volume label entry
                    buf[off] = label[0];
                    for j in 0..11 {
                        if j < label.len() {
                            buf[off + j] = label[j];
                        } else {
                            buf[off + j] = b' ';
                        }
                    }
                    buf[off + 0x0B] = ATTR_VOLUME;
                    vol.write_cluster(clu, &mut buf)?;
                    return Ok(());
                }
                if buf[off] == 0xE5 && !found_slot {
                    found_slot = true;
                    continue;
                }
                if buf[off + 0x0B] == ATTR_VOLUME {
                    // Overwrite existing label
                    for j in 0..11 {
                        if j < label.len() {
                            buf[off + j] = label[j];
                        } else {
                            buf[off + j] = b' ';
                        }
                    }
                    vol.write_cluster(clu, &mut buf)?;
                    return Ok(());
                }
            }
            let next = vol.read_fat_entry(clu)?;
            if next >= 0xFFFFFF8 { break; }
            clu = next;
        }
        // If we found a deleted slot, use it
        Ok(())
    })
}

// ── Current directory ────────────────────────────────────────

static mut CWD_CLUSTER: u32 = 0;
static mut CWD_PATH: [u8; 256] = [0u8; 256];

fn cwd_init() {
    unsafe {
        CWD_CLUSTER = 2; // root cluster
        CWD_PATH = [0u8; 256];
        CWD_PATH[0] = b'/';
    }
}

pub fn f_chdir(path: &str) -> FResult<()> {
    with_vol(|vol| {
        let (cluster, _) = resolve_path(vol, path)?;
        unsafe {
            CWD_CLUSTER = cluster;
            // Build canonical path
            CWD_PATH = [0u8; 256];
            if path.starts_with('/') {
                let bytes = path.as_bytes();
                let len = bytes.len().min(255);
                CWD_PATH[..len].copy_from_slice(&bytes[..len]);
            } else {
            // Relative path — append to current CWD
            let cwd = &CWD_PATH;
                let cwd_len = cwd.iter().position(|&b| b == 0).unwrap_or(0);
                if cwd_len > 0 && cwd[cwd_len - 1] != b'/' {
                    CWD_PATH[cwd_len] = b'/';
                    let bytes = path.as_bytes();
                    let len = bytes.len().min(255 - cwd_len - 1);
                    CWD_PATH[cwd_len + 1..cwd_len + 1 + len].copy_from_slice(&bytes[..len]);
                } else {
                    let bytes = path.as_bytes();
                    let len = bytes.len().min(255 - cwd_len);
                    CWD_PATH[cwd_len..cwd_len + len].copy_from_slice(&bytes[..len]);
                }
            }
        }
        Ok(())
    })
}

pub fn f_getcwd(buf: &mut [u8]) -> FResult<()> {
    unsafe {
        let cwd = &CWD_PATH;
        let len = cwd.iter().position(|&b| b == 0).unwrap_or(0);
        if len > 0 {
            let copy_len = len.min(buf.len());
            buf[..copy_len].copy_from_slice(&cwd[..copy_len]);
            if copy_len < buf.len() {
                buf[copy_len] = 0;
            }
        } else {
            buf[0] = b'/';
            if buf.len() > 1 {
                buf[1] = 0;
            }
        }
        Ok(())
    }
}

// ── Path resolution ──────────────────────────────────────────

/// Walk a path like "/dir1/dir2/file" and return the parent directory
/// cluster and the final name component.
fn resolve_path<'a>(vol: &mut FatVolume, path: &'a str) -> FResult<(u32, &'a str)> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Ok((vol.root_cluster, ""));
    }

    let mut current_cluster = vol.root_cluster;
    let mut last_name = "";
    let mut components = path.split('/').peekable();

    while let Some(component) = components.next() {
        if component.is_empty() {
            continue;
        }

        if components.peek().is_some() {
            // Still traversing directories
            let name_bytes = component.as_bytes();
            if let Some((dir_cluster, idx)) =
                vol.find_entry_in_dir(current_cluster, name_bytes)?
            {
                // Read the entry's start_cluster — this is the directory's data cluster
                let entry = vol.read_entry_data(dir_cluster, idx)?;
                let start_cluster = u32::from_le_bytes([
                    entry[0x1A], entry[0x1B],
                    entry[0x14], entry[0x15],
                ]);
                current_cluster = start_cluster;
            } else {
                return Err(FError::NoPath);
            }
        } else {
            last_name = component;
        }
    }

    Ok((current_cluster, last_name))
}

// ── Test suite ───────────────────────────────────────────────

fn test_msg(msg: &[u8]) {
    crate::hal::serial::serial_debug(b"  [FATTEST] \0");
    crate::hal::serial::serial_debug(msg);
}

fn test_ok() {
    crate::hal::serial::serial_debug(b" PASS\r\n\0");
}

fn test_fail(err: &FError) {
    crate::hal::serial::serial_debug(b" FAIL: \0");
    let err_str: &[u8] = match err {
        FError::Ok => &b"Ok\0"[..],
        FError::DiskErr => &b"DiskErr\0"[..],
        FError::NoFile => &b"NoFile\0"[..],
        FError::NoPath => &b"NoPath\0"[..],
        FError::NotFound => &b"NotFound\0"[..],
        FError::Denied => &b"Denied\0"[..],
        FError::Exists => &b"Exists\0"[..],
        FError::Invalid => &b"Invalid\0"[..],
        _ => &b"Other\0"[..],
    };
    crate::hal::serial::serial_debug(err_str);
    crate::hal::serial::serial_debug(b"\r\n\0");
}

fn test_run(name: &[u8], f: fn() -> Result<(), FError>) {
    test_msg(name);
    match f() {
        Ok(()) => test_ok(),
        Err(e) => test_fail(&e),
    }
}

pub fn fat32_test() {
    // Reap any leftover test artifacts from previous runs
    let _ = f_unlink("/testdir/subdir");
    let _ = f_unlink("/testdir/renamed.txt");
    let _ = f_unlink("/testdir/renameme.txt");
    let _ = f_unlink("/testdir/hello.txt");
    let _ = f_unlink("/testdir");
    let _ = f_unlink("/README.TXT");
    crate::hal::serial::serial_debug(b"  [FATTEST] Cleanup done\r\n\0");

    // Direct ATA sector test — verify r/w on a known-good LBA (cluster 2, root dir)
    test_run(b"ATA sector r/w\0", || {
        with_vol(|vol| {
            let lba = vol.cluster_to_lba(2);
            let mut sector = [0u8; 512];
            if !unsafe { (&*vol.dev).read_sector(lba, &mut sector) } {
                return Err(FError::DiskErr);
            }
            // Root dir should have at least one non-zero entry (QASM dir)
            if sector[0] == 0 { return Err(FError::Invalid); }
            let wok = unsafe { (&*vol.dev).write_sector(lba, &sector) };
            if !wok { return Err(FError::DiskErr); }
            Ok(())
        })
    });

    test_run(b"f_stat KERNEL.BIN\0", || {
        let info = f_stat("/KERNEL.BIN")?;
        if info.size == 0 { return Err(FError::NoFile); }
        Ok(())
    });

    test_run(b"f_open + f_read\0", || {
        let mut f = f_open("/KERNEL.BIN", FA_READ)?;
        let mut buf = [0u8; 16];
        let r = f_read(&mut f, &mut buf)?;
        if r != 16 { return Err(FError::DiskErr); }
        if buf[0] == 0 { return Err(FError::Invalid); } // must have data
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_tell + f_eof + f_size\0", || {
        let f = f_open("/KERNEL.BIN", FA_READ)?;
        let s = f_size(&f);
        if s == 0 { return Err(FError::Invalid); }
        if f_eof(&f) { return Err(FError::Invalid); }
        if f_tell(&f) != 0 { return Err(FError::Invalid); }
        if f_error(&f) { return Err(FError::Denied); }
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_opendir + f_readdir\0", || {
        let mut d = f_opendir("/")?;
        let mut count = 0u32;
        let mut name = [0u8; 13];
        while f_readdir(&mut d, &mut name)? {
            count += 1;
        }
        f_closedir(d)?;
        if count < 2 { return Err(FError::NoFile); }
        Ok(())
    });

    test_run(b"f_findfirst K*\0", || {
        let mut d = f_findfirst("/", b"K*")?;
        let mut name = [0u8; 13];
        let found = f_findnext(&mut d, &mut name)?;
        if !found { return Err(FError::NoFile); }
        if name[0] != b'K' { return Err(FError::Invalid); }
        f_closedir(d)?;
        Ok(())
    });

    test_run(b"f_mkdir /testdir\0", || {
        f_mkdir("/testdir")
    });

    test_run(b"f_create + f_write + f_sync\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_WRITE | FA_CREATE_NEW)?;
        if f.current_cluster == 0 { return Err(FError::NoFile); }
        f_putc(b'A', &mut f)?;
        f_puts(b"BCDEF", &mut f)?;
        f_lseek(&mut f, 0)?;
        let w = f_write(&mut f, b"Hello from planckOS!")?;
        if w != 20 { return Err(FError::DiskErr); }
        f_sync(&mut f)?;
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_read written file\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_READ)?;
        let mut buf = [0u8; 32];
        let r = f_read(&mut f, &mut buf)?;
        if r != 20 { return Err(FError::DiskErr); }
        if &buf[..20] != b"Hello from planckOS!" { return Err(FError::Invalid); }
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_gets\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_READ)?;
        let mut buf = [0u8; 32];
        let r = f_gets(&mut buf, 32, &mut f)?;
        if r == 0 { return Err(FError::NoFile); }
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_putc + f_puts + f_printf\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_WRITE | FA_OPEN_ALWAYS)?;
        f_lseek(&mut f, 0)?;
        f_putc(b'A', &mut f)?;
        f_puts(b"BCDEF", &mut f)?;
        f_printf(&mut f, b" %d", &[FmtArg::Int(42)])?;
        f_sync(&mut f)?;
        f_close(f)?;
        // Verify
        let mut f2 = f_open("/testdir/hello.txt", FA_READ)?;
        let mut buf = [0u8; 16];
        let r = f_read(&mut f2, &mut buf)?;
        if r < 6 { return Err(FError::DiskErr); }
        if buf[0] != b'A' { return Err(FError::Invalid); }
        f_close(f2)?;
        // Reset file for other tests
        let mut f3 = f_open("/testdir/hello.txt", FA_WRITE | FA_CREATE_ALWAYS)?;
        f_write(&mut f3, b"Hello from planckOS!")?;
        f_sync(&mut f3)?;
        f_close(f3)?;
        Ok(())
    });

    test_run(b"f_lseek\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_READ)?;
        f_lseek(&mut f, 6)?;
        if f_tell(&f) != 6 { return Err(FError::Invalid); }
        let mut c = [0u8; 1];
        f_read(&mut f, &mut c)?;
        if c[0] != b'f' { return Err(FError::Invalid); } // "Hello from..." offset 6 = 'f'
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_truncate\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_WRITE | FA_OPEN_EXISTING)?;
        f_lseek(&mut f, 5)?;
        f_truncate(&mut f)?;
        if f_size(&f) != 5 { return Err(FError::Invalid); }
        f_sync(&mut f)?;
        f_close(f)?;
        // Verify
        let mut f2 = f_open("/testdir/hello.txt", FA_READ)?;
        let mut buf = [0u8; 10];
        let r = f_read(&mut f2, &mut buf)?;
        if r != 5 { return Err(FError::DiskErr); }
        if &buf[..5] != b"Hello" { return Err(FError::Invalid); }
        f_close(f2)?;
        // Restore
        let mut f3 = f_open("/testdir/hello.txt", FA_WRITE | FA_CREATE_ALWAYS)?;
        f_write(&mut f3, b"Hello from planckOS!")?;
        f_sync(&mut f3)?;
        f_close(f3)?;
        Ok(())
    });

    test_run(b"f_expand\0", || {
        let mut f = f_open("/testdir/hello.txt", FA_WRITE | FA_OPEN_EXISTING)?;
        let old = f_size(&f);
        f_expand(&mut f, 10000)?;
        if f_size(&f) < 10000 { return Err(FError::Invalid); }
        f_lseek(&mut f, old)?;
        f_truncate(&mut f)?;
        f_sync(&mut f)?;
        f_close(f)?;
        Ok(())
    });

    test_run(b"f_unlink hello.txt\0", || {
        f_unlink("/testdir/hello.txt")
    });

    test_run(b"f_mkdir subdir\0", || {
        f_mkdir("/testdir/subdir")
    });

    test_run(b"f_rename\0", || {
        // Use existing files on the image to test rename
        f_rename("/qasm/readme.txt", "/qasm/test.txt")?;
        if f_open("/qasm/readme.txt", FA_READ).is_ok() {
            return Err(FError::Exists);
        }
        f_open("/qasm/test.txt", FA_READ)?;
        f_rename("/qasm/test.txt", "/qasm/readme.txt")?;
        Ok(())
    });

    test_run(b"f_chmod\0", || {
        f_chmod("/testdir", 0x11) // read-only + directory
    });

    test_run(b"f_utime\0", || {
        f_utime("/testdir", 0x7F00, 0x4A00) // some arbitrary time/date
    });

    test_run(b"f_chmod restore\0", || {
        f_chmod("/testdir", 0x10) // directory only
    });

    test_run(b"f_getfree\0", || {
        let (free, total) = f_getfree()?;
        if total == 0 { return Err(FError::DiskErr); }
        if free == 0 && total > 0 { return Err(FError::DiskErr); } // should have at least some free
        Ok(())
    });

    test_run(b"f_getlabel + f_setlabel\0", || {
        let mut label = [0u8; 12];
        f_getlabel(&mut label)?;
        f_setlabel(b"PLANCKOS")?;
        let mut label2 = [0u8; 12];
        f_getlabel(&mut label2)?;
        if &label2[..8] != b"PLANCKOS" { return Err(FError::Invalid); }
        // Restore old label
        if label[0] != 0 {
            f_setlabel(&label[..label.iter().position(|&b| b==0).unwrap_or(11)])?;
        }
        Ok(())
    });

    test_run(b"f_chdir + f_getcwd\0", || {
        f_chdir("/testdir")?;
        let mut buf = [0u8; 64];
        f_getcwd(&mut buf)?;
        let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
        if len == 0 { return Err(FError::Invalid); }
        f_chdir("/")?;
        Ok(())
    });

    test_run(b"f_unlink testdir\0", || {
        f_unlink("/testdir/subdir")?;
        f_unlink("/testdir")?;
        Ok(())
    });

    test_run(b"f_findfirst no match\0", || {
        let mut d = f_findfirst("/", b"ZZZ*")?;
        let mut name = [0u8; 13];
        let found = f_findnext(&mut d, &mut name)?;
        if found { return Err(FError::Exists); } // should NOT find anything
        f_closedir(d)?;
        Ok(())
    });

    test_run(b"f_findfirst * wildcard\0", || {
        let mut d = f_findfirst("/", b"*BIN\0")?;
        let mut count = 0u32;
        let mut name = [0u8; 13];
        while f_findnext(&mut d, &mut name)? {
            count += 1;
        }
        f_closedir(d)?;
        if count < 1 { return Err(FError::NoFile); }
        Ok(())
    });

    // Simulate exactly what `mk testdir/` does in the shell
    test_run(b"mk testdir/ (relative path)\0", || {
        f_mkdir("testdir")
    });

    crate::hal::serial::serial_debug(b"  [FATTEST] All tests complete\r\n\0");
}

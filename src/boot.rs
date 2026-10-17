//! The MBR and the exFAT boot sector: parsing (untrusted) and rendering.

use crate::SECTOR_SIZE;
use crate::geometry::Layout;

/// Fields of an exFAT boot sector (spec §3.1), as read or as rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct BootSector {
    pub partition_offset: u64,
    pub volume_length: u64,
    pub fat_offset: u32,
    pub fat_length: u32,
    pub cluster_heap_offset: u32,
    pub cluster_count: u32,
    pub first_cluster_of_root: u32,
    pub volume_serial: u32,
    pub revision: u16,
    pub volume_flags: u16,
    pub bytes_per_sector_shift: u8,
    pub sectors_per_cluster_shift: u8,
    pub number_of_fats: u8,
    pub drive_select: u8,
    pub percent_in_use: u8,
}

/// `VolumeFlags` bit: the volume was not cleanly unmounted.
pub const FLAG_VOLUME_DIRTY: u16 = 1 << 1;
/// `VolumeFlags` bit: the media reported failures.
pub const FLAG_MEDIA_FAILURE: u16 = 1 << 2;

const JUMP_BOOT: [u8; 3] = [0xEB, 0x76, 0x90];
const FS_NAME: &[u8; 8] = b"EXFAT   ";

impl BootSector {
    /// Parse `sector` if it looks like an exFAT boot sector (jump code, file system name, signature);
    /// the fields are not validated here.
    pub fn parse(sector: &[u8; SECTOR_SIZE]) -> Option<BootSector> {
        if sector[0..3] != JUMP_BOOT
            || &sector[3..11] != FS_NAME
            || sector[510..512] != [0x55, 0xAA]
        {
            return None;
        }
        if sector[11..64].iter().any(|&b| b != 0) {
            return None;
        }
        Some(BootSector {
            partition_offset: le64(sector, 64),
            volume_length: le64(sector, 72),
            fat_offset: le32(sector, 80),
            fat_length: le32(sector, 84),
            cluster_heap_offset: le32(sector, 88),
            cluster_count: le32(sector, 92),
            first_cluster_of_root: le32(sector, 96),
            volume_serial: le32(sector, 100),
            revision: le16(sector, 104),
            volume_flags: le16(sector, 106),
            bytes_per_sector_shift: sector[108],
            sectors_per_cluster_shift: sector[109],
            number_of_fats: sector[110],
            drive_select: sector[111],
            percent_in_use: sector[112],
        })
    }

    /// The boot sector for a freshly formatted layout.
    pub fn for_layout(layout: &Layout, serial: u32, percent_in_use: u8) -> BootSector {
        BootSector {
            partition_offset: layout.partition_lba,
            volume_length: layout.volume_length,
            fat_offset: layout.fat_offset,
            fat_length: layout.fat_length,
            cluster_heap_offset: layout.cluster_heap_offset,
            cluster_count: layout.cluster_count,
            first_cluster_of_root: layout.root_cluster,
            volume_serial: serial,
            revision: 0x0100,
            volume_flags: 0,
            bytes_per_sector_shift: 9,
            sectors_per_cluster_shift: layout.geometry.sectors_per_cluster_shift,
            number_of_fats: 1,
            drive_select: 0x80,
            percent_in_use,
        }
    }

    /// Render into `out` (sector 0 of the boot region).
    pub fn render(&self, out: &mut [u8; SECTOR_SIZE]) {
        out.fill(0);
        out[0..3].copy_from_slice(&JUMP_BOOT);
        out[3..11].copy_from_slice(FS_NAME);
        put64(out, 64, self.partition_offset);
        put64(out, 72, self.volume_length);
        put32(out, 80, self.fat_offset);
        put32(out, 84, self.fat_length);
        put32(out, 88, self.cluster_heap_offset);
        put32(out, 92, self.cluster_count);
        put32(out, 96, self.first_cluster_of_root);
        put32(out, 100, self.volume_serial);
        put16(out, 104, self.revision);
        put16(out, 106, self.volume_flags);
        out[108] = self.bytes_per_sector_shift;
        out[109] = self.sectors_per_cluster_shift;
        out[110] = self.number_of_fats;
        out[111] = self.drive_select;
        out[112] = self.percent_in_use;
        out[510] = 0x55;
        out[511] = 0xAA;
    }
}

/// Running boot checksum (spec §3.4) over one sector of the boot region: `index` is the sector's index in the
/// region (0..=10); bytes 106, 107 and 112 of sector 0 are skipped.
pub fn boot_checksum_step(mut sum: u32, index: u32, sector: &[u8; SECTOR_SIZE]) -> u32 {
    for (i, &b) in sector.iter().enumerate() {
        if index == 0 && (i == 106 || i == 107 || i == 112) {
            continue;
        }
        sum = (if sum & 1 != 0 { 0x8000_0000u32 } else { 0 })
            .wrapping_add(sum >> 1)
            .wrapping_add(b as u32);
    }
    sum
}

/// Render sector `index` (0..12) of a boot region: 0 the boot sector, 1..=8 the extended boot sectors, 9 OEM
/// parameters (none), 10 reserved, 11 the checksum sector. The checksum must be known for 11.
pub fn render_boot_region_sector(
    boot: &BootSector,
    index: u32,
    checksum: u32,
    out: &mut [u8; SECTOR_SIZE],
) {
    match index {
        0 => boot.render(out),
        1..=8 => {
            out.fill(0);
            out[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
        }
        9 | 10 => out.fill(0),
        11 => {
            for chunk in out.as_chunks_mut::<4>().0 {
                *chunk = checksum.to_le_bytes();
            }
        }
        _ => out.fill(0),
    }
}

/// Checksum of a freshly rendered boot region.
pub fn boot_region_checksum(boot: &BootSector) -> u32 {
    let mut sum = 0;
    let mut sector = [0u8; SECTOR_SIZE];
    for i in 0..11 {
        render_boot_region_sector(boot, i, 0, &mut sector);
        sum = boot_checksum_step(sum, i, &sector);
    }
    sum
}

/// Partition type byte for exFAT (and NTFS/HPFS) in an MBR.
pub const PARTITION_TYPE_EXFAT: u8 = 0x07;

/// Render an MBR with one exFAT partition at `partition_lba` of `sectors` sectors (capped at `u32::MAX`).
pub fn render_mbr(
    partition_lba: u64,
    sectors: u64,
    disk_signature: u32,
    out: &mut [u8; SECTOR_SIZE],
) {
    out.fill(0);
    put32(out, 440, disk_signature);
    let e = &mut out[446..462];
    e[0] = 0x00; // not bootable
    e[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS: LBA only
    e[4] = PARTITION_TYPE_EXFAT;
    e[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[8..12].copy_from_slice(&(partition_lba.min(u32::MAX as u64) as u32).to_le_bytes());
    e[12..16].copy_from_slice(&(sectors.min(u32::MAX as u64) as u32).to_le_bytes());
    out[510] = 0x55;
    out[511] = 0xAA;
}

/// The first exFAT-typed partition of an MBR: `(start LBA, sectors)`; `None` when `sector` is no MBR or has
/// no such partition.
pub fn parse_mbr(sector: &[u8; SECTOR_SIZE]) -> Option<(u64, u64)> {
    if sector[510..512] != [0x55, 0xAA] {
        return None;
    }
    (0..4).map(|i| 446 + i * 16).find_map(|at| {
        let e = &sector[at..at + 16];
        let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
        let len = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
        (e[4] == PARTITION_TYPE_EXFAT && start != 0 && len != 0).then_some((start, len))
    })
}

pub(crate) fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(crate) fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub(crate) fn le64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

pub(crate) fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Geometry;

    #[test]
    fn boot_sector_round_trip() {
        let sectors = 64 * 1024 * 1024 / 512;
        let layout = Layout::compute(sectors, Geometry::for_capacity(sectors), true).unwrap();
        let boot = BootSector::for_layout(&layout, 0xDEAD_BEEF, 3);
        let mut s = [0u8; 512];
        boot.render(&mut s);
        assert_eq!(BootSector::parse(&s), Some(boot));
        s[5] = b'x';
        assert_eq!(BootSector::parse(&s), None);
    }

    #[test]
    fn mbr_round_trip() {
        let mut s = [0u8; 512];
        render_mbr(8192, 100_000, 1, &mut s);
        assert_eq!(parse_mbr(&s), Some((8192, 100_000)));
        assert_eq!(parse_mbr(&[0u8; 512]), None);
    }
}

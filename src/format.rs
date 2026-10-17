//! Formatting: MBR + exFAT metadata rendered sector by sector, written through a caller-provided scratch
//! buffer (any multiple of 512 bytes; bigger means fewer, larger writes).

use crate::boot::{BootSector, boot_region_checksum, render_boot_region_sector, render_mbr};
use crate::device::{BlockDevice, SECTOR_SIZE, Sector};
use crate::dir::{
    ENTRY_SIZE, MAX_LABEL_UNITS, render_bitmap_entry, render_label_entry, render_upcase_entry,
};
use crate::error::Error;
use crate::geometry::{BOOT_REGION_SECTORS, Geometry, Layout};
use crate::upcase::{UPCASE_CHECKSUM, UPCASE_TABLE_LEN, render_sector as render_upcase_sector};

/// What to format with.
#[derive(Debug, Clone, Copy)]
pub struct FormatParams<'a> {
    /// Cluster size and boundary unit.
    pub geometry: Geometry,
    /// Volume label, up to 11 UTF-16 units; empty for none.
    pub label: &'a str,
    /// `VolumeSerialNumber`, also the MBR disk signature. A device takes it from its RNG or clock.
    pub serial: u32,
    /// Write an MBR at sector 0 with one exFAT partition starting at the first boundary unit (the SD layout);
    /// `false` puts the boot sector at sector 0 (a bare volume, like `mkfs.exfat` on an image).
    pub with_mbr: bool,
}

impl<'a> FormatParams<'a> {
    /// SD layout: an MBR, the given geometry, label and serial.
    pub const fn new(geometry: Geometry, label: &'a str, serial: u32) -> FormatParams<'a> {
        FormatParams {
            geometry,
            label,
            serial,
            with_mbr: true,
        }
    }

    /// A bare volume from sector 0 instead of a partition.
    pub const fn without_mbr(mut self) -> FormatParams<'a> {
        self.with_mbr = false;
        self
    }
}

/// Renders every metadata sector of a fresh volume.
struct Renderer {
    layout: Layout,
    boot: BootSector,
    checksum: u32,
    serial: u32,
    label: [u16; MAX_LABEL_UNITS],
    label_len: u8,
}

impl Renderer {
    /// Fill `out` with what sector `lba` holds on the fresh volume (zeros where nothing is).
    fn render(&self, lba: u64, out: &mut Sector) {
        let l = &self.layout;
        if lba == 0 && l.partition_lba != 0 {
            render_mbr(l.partition_lba, l.volume_length, self.serial, out);
            return;
        }
        let Some(rel) = lba.checked_sub(l.partition_lba) else {
            out.fill(0);
            return;
        };
        if rel < BOOT_REGION_SECTORS as u64 {
            render_boot_region_sector(&self.boot, (rel % 12) as u32, self.checksum, out);
        } else if rel >= l.fat_offset as u64 && rel < l.fat_offset as u64 + l.fat_length as u64 {
            self.render_fat(rel - l.fat_offset as u64, out);
        } else if rel >= l.cluster_heap_offset as u64 {
            self.render_heap(rel - l.cluster_heap_offset as u64, out);
        } else {
            out.fill(0);
        }
    }

    fn render_fat(&self, fat_sector: u64, out: &mut Sector) {
        out.fill(0);
        let l = &self.layout;
        let first_entry = fat_sector * (SECTOR_SIZE as u64 / 4);
        let bitmap_end = 2 + l.bitmap_clusters; // exclusive
        let upcase_end = bitmap_end + l.upcase_clusters;
        for i in 0..SECTOR_SIZE / 4 {
            let entry = first_entry + i as u64;
            let value: u32 = match entry {
                0 => 0xFFFF_FFF8,
                1 => 0xFFFF_FFFF,
                c if c == l.root_cluster as u64 => 0xFFFF_FFFF,
                c if c < bitmap_end as u64 => {
                    if c + 1 == bitmap_end as u64 {
                        0xFFFF_FFFF
                    } else {
                        (c + 1) as u32
                    }
                }
                c if c < upcase_end as u64 => {
                    if c + 1 == upcase_end as u64 {
                        0xFFFF_FFFF
                    } else {
                        (c + 1) as u32
                    }
                }
                _ => break,
            };
            out[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    fn render_heap(&self, heap_sector: u64, out: &mut Sector) {
        out.fill(0);
        let l = &self.layout;
        let spc = l.geometry.sectors_per_cluster() as u64;
        let cluster = 2 + heap_sector / spc;
        let in_cluster = heap_sector % spc;
        let bitmap_end = 2 + l.bitmap_clusters as u64;
        let upcase_end = bitmap_end + l.upcase_clusters as u64;
        if cluster < bitmap_end {
            // Bit n of the bitmap is cluster n + 2; the metadata clusters are in use.
            let bitmap_sector = (cluster - 2) * spc + in_cluster;
            let first_bit = bitmap_sector * (SECTOR_SIZE as u64 * 8);
            let used = l.metadata_clusters() as u64;
            for bit in first_bit..(first_bit + SECTOR_SIZE as u64 * 8).min(used) {
                let i = (bit - first_bit) as usize;
                out[i / 8] |= 1 << (i % 8);
            }
        } else if cluster < upcase_end {
            let table_sector = (cluster - bitmap_end) * spc + in_cluster;
            if table_sector < u32::MAX as u64 {
                render_upcase_sector(table_sector as u32, out);
            }
        } else if cluster == l.root_cluster as u64 && in_cluster == 0 {
            let mut e = [0u8; ENTRY_SIZE];
            render_label_entry(&self.label[..self.label_len as usize], &mut e);
            out[0..ENTRY_SIZE].copy_from_slice(&e);
            render_bitmap_entry(2, l.bitmap_bytes() as u64, &mut e);
            out[ENTRY_SIZE..2 * ENTRY_SIZE].copy_from_slice(&e);
            render_upcase_entry(
                UPCASE_CHECKSUM,
                bitmap_end as u32,
                UPCASE_TABLE_LEN as u64,
                &mut e,
            );
            out[2 * ENTRY_SIZE..3 * ENTRY_SIZE].copy_from_slice(&e);
        }
    }
}

/// Format `dev`: writes the MBR (if asked), both boot regions, the whole FAT, the allocation bitmap, the
/// up-case table and the root directory (label, bitmap and up-case entries). Nothing else on the device is
/// touched: old data stays in the free clusters until overwritten. Returns the layout written.
///
/// `scratch` must hold at least one sector; metadata is rendered into it and written in chunks of its size.
/// Errors: `InvalidArgument` for a geometry, label or scratch buffer that can't be used, or a device too small.
pub async fn format<D: BlockDevice>(
    dev: &mut D,
    params: &FormatParams<'_>,
    scratch: &mut [u8],
) -> Result<Layout, Error<D::Error>> {
    let chunk_sectors = scratch.len() / SECTOR_SIZE;
    if chunk_sectors == 0 {
        return Err(Error::InvalidArgument);
    }
    let scratch = &mut scratch[..chunk_sectors * SECTOR_SIZE];
    let layout = Layout::compute(dev.sector_count(), params.geometry, params.with_mbr)
        .ok_or(Error::InvalidArgument)?;
    let mut label = [0u16; MAX_LABEL_UNITS];
    let mut label_len = 0u8;
    for unit in params.label.encode_utf16() {
        if label_len as usize >= MAX_LABEL_UNITS {
            return Err(Error::InvalidArgument);
        }
        label[label_len as usize] = unit;
        label_len += 1;
    }
    let percent = (layout.metadata_clusters() as u64 * 100 / layout.cluster_count as u64) as u8;
    let boot = BootSector::for_layout(&layout, params.serial, percent);
    let checksum = boot_region_checksum(&boot);
    let r = Renderer {
        layout,
        boot,
        checksum,
        serial: params.serial,
        label,
        label_len,
    };

    let p = layout.partition_lba;
    let spc = layout.geometry.sectors_per_cluster() as u64;
    let heap = layout.heap_lba();
    let regions: [(u64, u64); 4] = [
        (0, if params.with_mbr { 1 } else { 0 }),
        (p, BOOT_REGION_SECTORS as u64),
        (p + layout.fat_offset as u64, layout.fat_length as u64),
        (heap, layout.metadata_clusters() as u64 * spc),
    ];
    for (start, len) in regions {
        let mut at = start;
        let end = start + len;
        while at < end {
            let n = ((end - at) as usize).min(chunk_sectors);
            for (i, sector) in scratch
                .as_chunks_mut::<SECTOR_SIZE>()
                .0
                .iter_mut()
                .take(n)
                .enumerate()
            {
                r.render(at + i as u64, sector);
            }
            dev.write(at, &scratch[..n * SECTOR_SIZE])
                .await
                .map_err(Error::Device)?;
            at += n as u64;
        }
    }
    dev.flush().await.map_err(Error::Device)?;
    Ok(layout)
}

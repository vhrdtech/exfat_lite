//! Volume geometry: cluster size and boundary unit per the SD Association's file system specification, and the
//! layout of a formatted volume computed from them.

use crate::SECTOR_SIZE;
use crate::upcase::UPCASE_TABLE_LEN;

/// Cluster size and alignment unit of a volume.
///
/// The boundary unit (BU) is the card's allocation unit: the partition, the cluster heap and (through
/// [`crate::Volume::create_contiguous`]'s `align_sectors`) big files start on a multiple of it, so the card's
/// flash controller sees whole-unit writes. A card that reports its AU (SD Status `AU_SIZE`) can pass it here;
/// [`Geometry::for_capacity`] gives the spec's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Cluster size as a power of two of sectors: a cluster is `512 << sectors_per_cluster_shift` bytes.
    /// At most 16 (32 MB clusters, the exFAT maximum).
    pub sectors_per_cluster_shift: u8,
    /// Boundary unit in sectors, a power of two, at least one cluster.
    pub boundary_unit_sectors: u32,
}

impl Geometry {
    /// Defaults by capacity, from the SD Association File System Specification's exFAT table (SDXC) and its
    /// SDHC row for cards up to 32 GB (the spec formats those as FAT32; the same boundary unit and cluster
    /// size are used here for exFAT).
    ///
    /// | capacity | boundary unit | cluster |
    /// |---|---|---|
    /// | up to 32 GB | 4 MB | 32 KB |
    /// | 32 GB to 128 GB | 16 MB | 128 KB |
    /// | 128 GB to 512 GB | 32 MB | 256 KB |
    /// | over 512 GB | 64 MB | 512 KB |
    pub const fn for_capacity(sectors: u64) -> Geometry {
        const GB: u64 = 1 << 30;
        let bytes = sectors * SECTOR_SIZE as u64;
        if bytes <= 32 * GB {
            Geometry {
                sectors_per_cluster_shift: 6,
                boundary_unit_sectors: 8192,
            }
        } else if bytes <= 128 * GB {
            Geometry {
                sectors_per_cluster_shift: 8,
                boundary_unit_sectors: 32768,
            }
        } else if bytes <= 512 * GB {
            Geometry {
                sectors_per_cluster_shift: 9,
                boundary_unit_sectors: 65536,
            }
        } else {
            Geometry {
                sectors_per_cluster_shift: 10,
                boundary_unit_sectors: 131072,
            }
        }
    }

    /// Sectors per cluster.
    pub const fn sectors_per_cluster(&self) -> u32 {
        1 << self.sectors_per_cluster_shift
    }

    /// Bytes per cluster.
    pub const fn cluster_bytes(&self) -> u32 {
        (SECTOR_SIZE as u32) << self.sectors_per_cluster_shift
    }

    /// `true` when the fields are a geometry this crate can format with.
    pub const fn is_valid(&self) -> bool {
        self.sectors_per_cluster_shift <= 16
            && self.boundary_unit_sectors.is_power_of_two()
            && self.boundary_unit_sectors >= self.sectors_per_cluster()
    }
}

/// Where everything is on a volume this crate formats: all offsets in sectors from the start of the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The geometry it was computed from.
    pub geometry: Geometry,
    /// First sector of the partition (the exFAT boot sector); 0 when there is no MBR.
    pub partition_lba: u64,
    /// Sectors in the volume (`VolumeLength`).
    pub volume_length: u64,
    /// FAT start, relative to the partition (`FatOffset`).
    pub fat_offset: u32,
    /// FAT length in sectors (`FatLength`).
    pub fat_length: u32,
    /// Cluster heap start, relative to the partition (`ClusterHeapOffset`).
    pub cluster_heap_offset: u32,
    /// Clusters in the heap (`ClusterCount`); cluster numbers run from 2 to `cluster_count + 1`.
    pub cluster_count: u32,
    /// Clusters the allocation bitmap takes, from cluster 2.
    pub bitmap_clusters: u32,
    /// Clusters the up-case table takes, right after the bitmap.
    pub upcase_clusters: u32,
    /// The root directory's cluster, right after the up-case table.
    pub root_cluster: u32,
}

/// Sectors of the boot region (main) and its backup together.
pub const BOOT_REGION_SECTORS: u32 = 24;

/// The largest cluster count exFAT allows.
pub const MAX_CLUSTER_COUNT: u32 = 0xFFFF_FFF5;

impl Layout {
    /// Lay out a volume on a device of `sector_count` sectors with an MBR (`with_mbr`, the partition then starts
    /// at the first boundary unit) or as a bare volume from sector 0. `None` when the device is too small for
    /// the metadata plus one data cluster, or the geometry invalid.
    pub fn compute(sector_count: u64, geometry: Geometry, with_mbr: bool) -> Option<Layout> {
        if !geometry.is_valid() {
            return None;
        }
        let bu = geometry.boundary_unit_sectors as u64;
        let spc = geometry.sectors_per_cluster() as u64;
        let partition_lba = if with_mbr { bu } else { 0 };
        let volume_length = sector_count.checked_sub(partition_lba)?;
        // The FAT follows the boot regions (4 KB-aligned by construction: 24 sectors); the cluster heap starts on the
        // next boundary unit after it. The FAT is sized from an upper bound of the cluster count, then the exact
        // count is derived from the heap that follows.
        let fat_offset = BOOT_REGION_SECTORS as u64;
        let max_clusters = volume_length / spc;
        let fat_length = (max_clusters.checked_add(2)? * 4).div_ceil(SECTOR_SIZE as u64);
        let cluster_heap_offset = (fat_offset + fat_length).div_ceil(bu) * bu;
        let cluster_count = volume_length
            .checked_sub(cluster_heap_offset)?
            .checked_div(spc)?
            .min(MAX_CLUSTER_COUNT as u64);
        // With the real cluster count the FAT may be shorter; keep the heap where it is.
        let fat_length = ((cluster_count + 2) * 4).div_ceil(SECTOR_SIZE as u64);
        if fat_offset + fat_length > cluster_heap_offset || cluster_heap_offset > u32::MAX as u64 {
            return None;
        }
        let cluster_bytes = geometry.cluster_bytes() as u64;
        let bitmap_clusters = (cluster_count.div_ceil(8)).div_ceil(cluster_bytes) as u32;
        let upcase_clusters = (UPCASE_TABLE_LEN as u64).div_ceil(cluster_bytes) as u32;
        let root_cluster = 2 + bitmap_clusters + upcase_clusters;
        // Metadata plus at least one free cluster.
        if cluster_count < (bitmap_clusters + upcase_clusters + 2) as u64 {
            return None;
        }
        Some(Layout {
            geometry,
            partition_lba,
            volume_length,
            fat_offset: fat_offset as u32,
            fat_length: fat_length as u32,
            cluster_heap_offset: cluster_heap_offset as u32,
            cluster_count: cluster_count as u32,
            bitmap_clusters,
            upcase_clusters,
            root_cluster,
        })
    }

    /// Absolute LBA of the cluster heap's first sector.
    pub fn heap_lba(&self) -> u64 {
        self.partition_lba + self.cluster_heap_offset as u64
    }

    /// Absolute LBA of the FAT's first sector.
    pub fn fat_lba(&self) -> u64 {
        self.partition_lba + self.fat_offset as u64
    }

    /// Absolute LBA of cluster `cluster` (2-based). Not checked against the heap.
    pub fn cluster_lba(&self, cluster: u32) -> u64 {
        self.heap_lba()
            + ((cluster as u64).wrapping_sub(2) << self.geometry.sectors_per_cluster_shift)
    }

    /// Clusters the metadata (bitmap, up-case table, root directory) takes.
    pub fn metadata_clusters(&self) -> u32 {
        self.bitmap_clusters + self.upcase_clusters + 1
    }

    /// Bytes of the allocation bitmap.
    pub fn bitmap_bytes(&self) -> u32 {
        self.cluster_count.div_ceil(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn capacity_table() {
        assert_eq!(
            Geometry::for_capacity(GB / 512),
            Geometry {
                sectors_per_cluster_shift: 6,
                boundary_unit_sectors: 8192
            }
        );
        assert_eq!(
            Geometry::for_capacity(32 * GB / 512).sectors_per_cluster_shift,
            6
        );
        assert_eq!(
            Geometry::for_capacity(64 * GB / 512),
            Geometry {
                sectors_per_cluster_shift: 8,
                boundary_unit_sectors: 32768
            }
        );
        assert_eq!(
            Geometry::for_capacity(128 * GB / 512).sectors_per_cluster_shift,
            8
        );
        assert_eq!(
            Geometry::for_capacity(256 * GB / 512),
            Geometry {
                sectors_per_cluster_shift: 9,
                boundary_unit_sectors: 65536
            }
        );
        assert_eq!(
            Geometry::for_capacity(1024 * GB / 512),
            Geometry {
                sectors_per_cluster_shift: 10,
                boundary_unit_sectors: 131072
            }
        );
    }

    #[test]
    fn layout_64mb() {
        let sectors = 64 * 1024 * 1024 / 512;
        let l = Layout::compute(sectors, Geometry::for_capacity(sectors), true).unwrap();
        assert_eq!(l.partition_lba, 8192);
        assert_eq!(l.fat_offset, 24);
        assert_eq!(l.cluster_heap_offset, 8192);
        assert_eq!(l.volume_length, sectors - 8192);
        // (64 MB - 4 MB - 4 MB) / 32 KB
        assert_eq!(l.cluster_count, 56 * 32);
        assert_eq!(l.bitmap_clusters, 1);
        assert_eq!(l.upcase_clusters, 1);
        assert_eq!(l.root_cluster, 4);
        assert_eq!(l.cluster_lba(2), 8192 + 8192);
        assert_eq!(l.cluster_lba(3), 8192 + 8192 + 64);
    }

    #[test]
    fn layout_128gb_fat_fits_before_heap() {
        let sectors = 128 * GB / 512;
        let l = Layout::compute(sectors, Geometry::for_capacity(sectors), true).unwrap();
        assert_eq!(l.partition_lba, 32768);
        // 128 GB / 128 KB = 1 M clusters -> FAT 4 MB = 8192 sectors (+2 entries); heap at 16 MB.
        assert_eq!(l.cluster_heap_offset, 32768);
        assert!((8189..=8193).contains(&l.fat_length), "{}", l.fat_length);
        assert_eq!(l.bitmap_clusters, 1); // 128 KB bitmap in a 128 KB cluster
        assert_eq!(l.upcase_clusters, 1);
    }

    #[test]
    fn layout_rejects_tiny_devices() {
        let g = Geometry {
            sectors_per_cluster_shift: 3,
            boundary_unit_sectors: 128,
        };
        assert!(Layout::compute(100, g, true).is_none());
        assert!(Layout::compute(128 + 24 + 100, g, true).is_none());
        assert!(Layout::compute(2048, g, true).is_some());
        assert!(
            !Geometry {
                sectors_per_cluster_shift: 17,
                boundary_unit_sectors: 1 << 17
            }
            .is_valid()
        );
        assert!(
            !Geometry {
                sectors_per_cluster_shift: 6,
                boundary_unit_sectors: 100
            }
            .is_valid()
        );
        assert!(
            !Geometry {
                sectors_per_cluster_shift: 6,
                boundary_unit_sectors: 32
            }
            .is_valid()
        );
    }
}

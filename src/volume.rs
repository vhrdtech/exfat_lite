//! A mounted volume: directory iteration, lookup, a bounded walk, consistency checks, contiguous file
//! creation and deletion, file reads. One sector buffer, no allocation.

use crate::boot::{BootSector, boot_checksum_step, le32, parse_mbr, put32};
use crate::device::{BlockDevice, SECTOR_SIZE, Sector};
use crate::dir::{
    ENTRY_SIZE, EntryLocation, EntrySet, FileInfo, MAX_LABEL_UNITS, MAX_SECONDARIES,
    MAX_SET_ENTRIES, Name, TYPE_BITMAP, TYPE_END, TYPE_FILE, TYPE_IN_USE, TYPE_LABEL,
    TYPE_SECONDARY, TYPE_UPCASE, Timestamp, attr,
};
use crate::error::{BootProblem, Corrupt, Error, Limit, Unsupported};
use crate::geometry::{BOOT_REGION_SECTORS, MAX_CLUSTER_COUNT};
use crate::upcase::checksum_step;

/// Deepest directory nesting a walk follows (the walk's cursor stack is this big).
pub const MAX_WALK_DEPTH: usize = 8;

/// Root directory entries looked at for the bitmap, up-case and label entries at mount. Every formatter puts
/// them first; this bound keeps a crafted root directory from turning a mount into a scan of the whole card.
const MOUNT_ROOT_SCAN_ENTRIES: u32 = 4096;

/// FAT entry: end of chain.
const FAT_EOF: u32 = 0xFFFF_FFFF;
/// FAT entry: bad cluster.
const FAT_BAD: u32 = 0xFFFF_FFF7;

/// The volume label, up to 11 UTF-16 units.
#[derive(Clone, Copy, Default)]
pub struct Label {
    units: [u16; MAX_LABEL_UNITS],
    len: u8,
}

impl Label {
    /// The UTF-16 units.
    pub fn units(&self) -> &[u16] {
        &self.units[..self.len as usize]
    }

    /// The characters, lone surrogates as U+FFFD.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        char::decode_utf16(self.units().iter().copied())
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// `true` when the volume has no label.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl core::fmt::Debug for Label {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("\"")?;
        for c in self.chars() {
            core::fmt::Write::write_char(f, c)?;
        }
        f.write_str("\"")
    }
}

impl core::fmt::Display for Label {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.chars() {
            core::fmt::Write::write_char(f, c)?;
        }
        Ok(())
    }
}

/// What the boot sector and the root directory say about a mounted volume.
#[derive(Debug, Clone, Copy)]
pub struct VolumeInfo {
    /// First sector of the volume on the device (0 without an MBR).
    pub partition_lba: u64,
    /// Sectors in the volume.
    pub volume_length: u64,
    /// FAT start, relative to the volume.
    pub fat_offset: u32,
    /// FAT length in sectors.
    pub fat_length: u32,
    /// Cluster heap start, relative to the volume.
    pub cluster_heap_offset: u32,
    /// Clusters in the heap; cluster numbers run from 2 to `cluster_count + 1`.
    pub cluster_count: u32,
    /// The root directory's first cluster.
    pub root_cluster: u32,
    /// `VolumeSerialNumber`.
    pub serial: u32,
    /// A cluster is `512 << sectors_per_cluster_shift` bytes.
    pub sectors_per_cluster_shift: u8,
    /// `VolumeFlags` as found (see [`VolumeInfo::is_dirty`]).
    pub volume_flags: u16,
    /// `PercentInUse` as found (0xFF: unknown).
    pub percent_in_use: u8,
    /// The allocation bitmap's first cluster and length in bytes.
    pub bitmap_cluster: u32,
    /// Allocation bitmap length in bytes.
    pub bitmap_length: u64,
    /// The up-case table's first cluster, length in bytes and checksum, from its entry.
    pub upcase_cluster: u32,
    /// Up-case table length in bytes.
    pub upcase_length: u64,
    /// Up-case table checksum as recorded in its entry.
    pub upcase_checksum: u32,
    /// The volume label.
    pub label: Label,
}

impl VolumeInfo {
    /// Sectors per cluster.
    pub fn sectors_per_cluster(&self) -> u32 {
        1 << self.sectors_per_cluster_shift
    }

    /// Bytes per cluster.
    pub fn cluster_bytes(&self) -> u32 {
        (SECTOR_SIZE as u32) << self.sectors_per_cluster_shift
    }

    /// Absolute LBA of the cluster heap.
    pub fn heap_lba(&self) -> u64 {
        self.partition_lba + self.cluster_heap_offset as u64
    }

    /// Absolute LBA of the FAT.
    pub fn fat_lba(&self) -> u64 {
        self.partition_lba + self.fat_offset as u64
    }

    /// `true` when `cluster` is a cluster of the heap.
    pub fn cluster_in_range(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster - 2 < self.cluster_count
    }

    /// Absolute LBA of `cluster`'s first sector; `None` outside the heap.
    pub fn cluster_lba(&self, cluster: u32) -> Option<u64> {
        self.cluster_in_range(cluster)
            .then(|| self.heap_lba() + ((cluster as u64 - 2) << self.sectors_per_cluster_shift))
    }

    /// The volume was not cleanly unmounted (`VolumeDirty` flag).
    pub fn is_dirty(&self) -> bool {
        self.volume_flags & crate::boot::FLAG_VOLUME_DIRTY != 0
    }

    /// Clusters an allocation of `bytes` takes.
    pub fn clusters_for(&self, bytes: u64) -> u64 {
        bytes.div_ceil(self.cluster_bytes() as u64)
    }
}

/// A position in a directory, for iterating it with [`Volume::next`] / [`Volume::next_entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirCursor {
    first_cluster: u32,
    no_fat_chain: bool,
    /// Directory size in bytes; `u64::MAX` for the root, whose size only the FAT chain knows.
    size: u64,
    /// Current cluster, 0 once exhausted.
    cluster: u32,
    /// Byte offset in the current cluster.
    offset: u32,
    /// Bytes consumed from the start of the directory.
    index: u64,
    clusters_visited: u32,
    /// The last cluster that was current (for growing the root directory).
    last_cluster: u32,
}

impl DirCursor {
    /// Cursor at the start of the directory `info` describes; `None` when it is not a directory.
    pub fn for_dir(info: &FileInfo) -> Option<DirCursor> {
        if !info.is_dir() {
            return None;
        }
        Some(DirCursor {
            first_cluster: info.first_cluster,
            no_fat_chain: info.no_fat_chain,
            size: info.data_length,
            cluster: info.first_cluster,
            offset: 0,
            index: 0,
            clusters_visited: 0,
            last_cluster: info.first_cluster,
        })
    }

    /// Rewind to the directory's start.
    pub fn rewind(&mut self) {
        self.cluster = self.first_cluster;
        self.offset = 0;
        self.index = 0;
        self.clusters_visited = 0;
        self.last_cluster = self.first_cluster;
    }

    /// The directory's first cluster.
    pub fn first_cluster(&self) -> u32 {
        self.first_cluster
    }

    /// `true` for the root directory.
    pub fn is_root(&self) -> bool {
        self.size == u64::MAX
    }

    /// A cursor positioned at `location` within this directory.
    fn at(&self, location: EntryLocation) -> DirCursor {
        DirCursor {
            cluster: location.cluster,
            offset: location.offset,
            index: location.index,
            clusters_visited: 0,
            last_cluster: location.cluster,
            ..*self
        }
    }
}

/// What the caller of [`Volume::walk`] wants after each entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkControl {
    /// Go on; a directory is entered (within the depth limit).
    Continue,
    /// Go on, but don't enter this directory.
    SkipDir,
    /// End the walk now.
    Stop,
}

/// Bounds for [`Volume::walk`] and [`Volume::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkLimits {
    /// Deepest directory level entered (0: only the start directory). Capped at [`MAX_WALK_DEPTH`] - 1.
    pub max_depth: u8,
    /// Most directory entries (of every kind, used or not) looked at in all.
    pub max_entries: u32,
}

impl WalkLimits {
    /// The whole tree to [`MAX_WALK_DEPTH`], up to a million entries.
    pub const DEFAULT: WalkLimits = WalkLimits {
        max_depth: MAX_WALK_DEPTH as u8 - 1,
        max_entries: 1 << 20,
    };
}

/// Counts from a [`Volume::walk`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Files seen.
    pub files: u32,
    /// Directories seen.
    pub dirs: u32,
    /// Raw directory entries looked at.
    pub entries: u32,
    /// Entry sets that were torn or had a bad checksum (skipped).
    pub damaged: u32,
    /// Directories not entered because of the depth limit.
    pub too_deep: u32,
}

/// What [`Volume::check`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Walk counts.
    pub walk: WalkStats,
    /// The backup boot region's checksum sector matches the main one.
    pub backup_boot_ok: bool,
    /// The up-case table on the volume matches the checksum in its entry.
    pub upcase_ok: bool,
    /// Clusters whose bit is set in the allocation bitmap.
    pub bitmap_set: u64,
    /// Clusters reached from the directory tree, metadata included.
    pub clusters_reached: u64,
    /// Files whose clusters are not all marked in the bitmap, or run out of the heap, or whose chain is broken.
    pub bad_allocations: u32,
    /// `true` when everything checked agrees.
    pub clean: bool,
}

/// One directory entry as [`Volume::next_entry`] yields it.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)] // a FileInfo carries its name inline: no alloc to box it
pub enum Entry {
    /// A file or directory (a complete, checksummed entry set).
    File(FileInfo),
    /// The allocation bitmap entry.
    Bitmap {
        /// First cluster.
        first_cluster: u32,
        /// Bytes.
        length: u64,
    },
    /// The up-case table entry.
    Upcase {
        /// Checksum of the table.
        checksum: u32,
        /// First cluster.
        first_cluster: u32,
        /// Bytes.
        length: u64,
    },
    /// The volume label entry.
    Label(Label),
    /// A file entry set that is torn or has a bad checksum; `location` is where it started.
    Damaged(EntryLocation),
    /// Another in-use entry type (volume GUID, vendor entries...).
    Other(u8),
}

/// A file to create: contiguous (`NoFatChain`), preallocated to `size` bytes, optionally with initial data.
#[derive(Debug, Clone, Copy)]
pub struct NewFile<'a> {
    /// The name (UTF-8; stored as UTF-16, case kept).
    pub name: &'a str,
    /// `attr::*` bits.
    pub attributes: u16,
    /// Bytes to preallocate; with `data`, at least `data.len()` is allocated.
    pub size: u64,
    /// Initial contents; the rest of the allocation is written as zeros. Without it, nothing is written to the
    /// clusters (the caller fills them later) and the file reads whatever the clusters held.
    pub data: Option<&'a [u8]>,
    /// Start the file on a sector that is a multiple of this (0: none). A power of two, a multiple of the
    /// cluster size: the SD boundary unit, to keep a big file on allocation unit boundaries.
    pub align_sectors: u32,
    /// Put the file at this cluster (which must be free, with the following ones) instead of searching.
    pub at_cluster: Option<u32>,
    /// Creation and modification time.
    pub time: Timestamp,
}

impl<'a> NewFile<'a> {
    /// An empty archive file with the default timestamp.
    pub const fn new(name: &'a str) -> NewFile<'a> {
        NewFile {
            name,
            attributes: attr::ARCHIVE,
            size: 0,
            data: None,
            align_sectors: 0,
            at_cluster: None,
            time: Timestamp::DEFAULT,
        }
    }

    /// Preallocate `size` bytes.
    pub const fn size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Write `data` at the start.
    pub const fn with_data(mut self, data: &'a [u8]) -> Self {
        self.data = Some(data);
        self
    }

    /// Align the first cluster to a multiple of `sectors`.
    pub const fn align_sectors(mut self, sectors: u32) -> Self {
        self.align_sectors = sectors;
        self
    }

    /// `attr::*` bits.
    pub const fn attributes(mut self, attributes: u16) -> Self {
        self.attributes = attributes;
        self
    }

    /// Place the file at `cluster`.
    pub const fn at_cluster(mut self, cluster: u32) -> Self {
        self.at_cluster = Some(cluster);
        self
    }

    /// Timestamp for creation and modification.
    pub const fn time(mut self, time: Timestamp) -> Self {
        self.time = time;
        self
    }
}

/// A mounted exFAT volume on a [`BlockDevice`].
pub struct Volume<D: BlockDevice> {
    dev: D,
    info: VolumeInfo,
    buf: Sector,
    cached: Option<u64>,
    /// Most entries a single directory scan ([`Volume::find`], [`Volume::create`]) looks at.
    max_dir_entries: u32,
}

/// Result alias with the device's error.
type Res<T, D> = Result<T, Error<<D as BlockDevice>::Error>>;

impl<D: BlockDevice> Volume<D> {
    /// Default for [`Volume::set_max_dir_entries`].
    pub const DEFAULT_MAX_DIR_ENTRIES: u32 = 1 << 16;

    /// Mount: find the volume (a boot sector at sector 0, else the first exFAT partition of an MBR), validate
    /// the boot sector against the device and itself, verify the main boot region's checksum, and locate the
    /// allocation bitmap, up-case table and label in the root directory. Reads a few dozen sectors.
    pub async fn mount(mut dev: D) -> Res<Self, D> {
        let sectors = dev.sector_count();
        let mut buf = [0u8; SECTOR_SIZE];
        if sectors == 0 {
            return Err(Error::NotExfat);
        }
        dev.read(0, &mut buf).await.map_err(Error::Device)?;
        let partition_lba = if BootSector::parse(&buf).is_some() {
            0
        } else if let Some((start, _len)) = parse_mbr(&buf) {
            if start >= sectors {
                return Err(Error::NotExfat);
            }
            dev.read(start, &mut buf).await.map_err(Error::Device)?;
            start
        } else {
            return Err(Error::NotExfat);
        };
        let boot = BootSector::parse(&buf).ok_or(Error::NotExfat)?;
        if boot.revision >> 8 != 1 {
            return Err(Error::BootSector(BootProblem::Revision));
        }
        if boot.bytes_per_sector_shift != 9 {
            return Err(Error::Unsupported(Unsupported::SectorSize));
        }
        if boot.sectors_per_cluster_shift > 16 {
            return Err(Error::Unsupported(Unsupported::ClusterSize));
        }
        match boot.number_of_fats {
            1 => {}
            2 => return Err(Error::Unsupported(Unsupported::TexFat)),
            _ => return Err(Error::BootSector(BootProblem::Geometry)),
        }
        if boot.volume_length > sectors - partition_lba {
            return Err(Error::BootSector(BootProblem::VolumeTooLarge));
        }
        let spc = 1u64 << boot.sectors_per_cluster_shift;
        let fat_end = boot.fat_offset as u64 + boot.fat_length as u64;
        let heap_end = boot.cluster_heap_offset as u64 + (boot.cluster_count as u64) * spc;
        if boot.fat_offset < BOOT_REGION_SECTORS
            || boot.fat_length == 0
            || fat_end > boot.cluster_heap_offset as u64
            || boot.cluster_count == 0
            || boot.cluster_count > MAX_CLUSTER_COUNT
            || (boot.cluster_count as u64 + 2) * 4 > boot.fat_length as u64 * SECTOR_SIZE as u64
            || heap_end > boot.volume_length
        {
            return Err(Error::BootSector(BootProblem::Geometry));
        }
        let mut info = VolumeInfo {
            partition_lba,
            volume_length: boot.volume_length,
            fat_offset: boot.fat_offset,
            fat_length: boot.fat_length,
            cluster_heap_offset: boot.cluster_heap_offset,
            cluster_count: boot.cluster_count,
            root_cluster: boot.first_cluster_of_root,
            serial: boot.volume_serial,
            sectors_per_cluster_shift: boot.sectors_per_cluster_shift,
            volume_flags: boot.volume_flags,
            percent_in_use: boot.percent_in_use,
            bitmap_cluster: 0,
            bitmap_length: 0,
            upcase_cluster: 0,
            upcase_length: 0,
            upcase_checksum: 0,
            label: Label::default(),
        };
        if !info.cluster_in_range(info.root_cluster) {
            return Err(Error::BootSector(BootProblem::RootCluster));
        }
        let mut vol = Volume {
            dev,
            info,
            buf,
            cached: Some(partition_lba),
            max_dir_entries: Self::DEFAULT_MAX_DIR_ENTRIES,
        };
        if !vol.boot_region_checksum_ok(partition_lba).await? {
            return Err(Error::BootSector(BootProblem::Checksum));
        }
        // The metadata entries of the root directory.
        let mut cur = vol.root();
        let mut found_bitmap = false;
        let mut found_upcase = false;
        let mut seen = 0u32;
        while seen < MOUNT_ROOT_SCAN_ENTRIES && !(found_bitmap && found_upcase) {
            let before = cur.index;
            let Some(entry) = vol.next_entry(&mut cur).await? else {
                break;
            };
            seen += ((cur.index - before) / ENTRY_SIZE as u64) as u32;
            match entry {
                Entry::Bitmap {
                    first_cluster,
                    length,
                } if !found_bitmap => {
                    found_bitmap = true;
                    info.bitmap_cluster = first_cluster;
                    info.bitmap_length = length;
                }
                Entry::Upcase {
                    checksum,
                    first_cluster,
                    length,
                } if !found_upcase => {
                    found_upcase = true;
                    info.upcase_cluster = first_cluster;
                    info.upcase_length = length;
                    info.upcase_checksum = checksum;
                }
                Entry::Label(label) => info.label = label,
                _ => {}
            }
        }
        let needed = info.cluster_count.div_ceil(8) as u64;
        if !found_bitmap
            || info.bitmap_length < needed
            || !info.cluster_in_range(info.bitmap_cluster)
            || info.clusters_for(info.bitmap_length)
                > (info.cluster_count - (info.bitmap_cluster - 2)) as u64
        {
            return Err(Error::Corrupt(Corrupt::Bitmap));
        }
        if !found_upcase
            || !info.cluster_in_range(info.upcase_cluster)
            || info.upcase_length > 2 * 65536
        {
            return Err(Error::Corrupt(Corrupt::UpcaseTable));
        }
        vol.info = info;
        Ok(vol)
    }

    /// What the volume says about itself.
    pub fn info(&self) -> &VolumeInfo {
        &self.info
    }

    /// The device underneath (reads and writes through it bypass this volume's sector cache: call
    /// [`Volume::invalidate`] after writing metadata sectors through it).
    pub fn device(&mut self) -> &mut D {
        &mut self.dev
    }

    /// Forget the cached sector.
    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    /// Give the device back.
    pub fn into_device(self) -> D {
        self.dev
    }

    /// Most entries a single directory scan looks at before giving up with `Limit(Entries)`.
    pub fn set_max_dir_entries(&mut self, n: u32) {
        self.max_dir_entries = n;
    }

    /// Flush the device.
    pub async fn flush(&mut self) -> Res<(), D> {
        self.dev.flush().await.map_err(Error::Device)
    }

    /// A cursor at the start of the root directory.
    pub fn root(&self) -> DirCursor {
        DirCursor {
            first_cluster: self.info.root_cluster,
            no_fat_chain: false,
            size: u64::MAX,
            cluster: self.info.root_cluster,
            offset: 0,
            index: 0,
            clusters_visited: 0,
            last_cluster: self.info.root_cluster,
        }
    }

    // ---- sector cache -------------------------------------------------------------------------------------

    async fn load(&mut self, lba: u64) -> Res<(), D> {
        if self.cached != Some(lba) {
            self.cached = None;
            self.dev
                .read(lba, &mut self.buf)
                .await
                .map_err(Error::Device)?;
            self.cached = Some(lba);
        }
        Ok(())
    }

    async fn store(&mut self, lba: u64) -> Res<(), D> {
        self.dev
            .write(lba, &self.buf)
            .await
            .map_err(Error::Device)?;
        self.cached = Some(lba);
        Ok(())
    }

    async fn boot_region_checksum_ok(&mut self, region_lba: u64) -> Res<bool, D> {
        let mut sum = 0;
        for i in 0..11 {
            self.load(region_lba + i as u64).await?;
            sum = boot_checksum_step(sum, i, &self.buf);
        }
        self.load(region_lba + 11).await?;
        Ok(self
            .buf
            .as_chunks::<4>()
            .0
            .iter()
            .all(|c| u32::from_le_bytes(*c) == sum))
    }

    // ---- FAT and bitmap -----------------------------------------------------------------------------------

    /// The FAT entry of `cluster`.
    pub async fn fat_get(&mut self, cluster: u32) -> Res<u32, D> {
        if !self.info.cluster_in_range(cluster) {
            return Err(Error::Corrupt(Corrupt::ClusterOutOfRange));
        }
        let per_sector = (SECTOR_SIZE / 4) as u32;
        let sector = cluster / per_sector;
        if sector >= self.info.fat_length {
            return Err(Error::Corrupt(Corrupt::FatChain));
        }
        self.load(self.info.fat_lba() + sector as u64).await?;
        Ok(le32(&self.buf, (cluster % per_sector) as usize * 4))
    }

    async fn fat_set(&mut self, cluster: u32, value: u32) -> Res<(), D> {
        if !self.info.cluster_in_range(cluster) {
            return Err(Error::Corrupt(Corrupt::ClusterOutOfRange));
        }
        let per_sector = (SECTOR_SIZE / 4) as u32;
        let sector = cluster / per_sector;
        if sector >= self.info.fat_length {
            return Err(Error::Corrupt(Corrupt::FatChain));
        }
        let lba = self.info.fat_lba() + sector as u64;
        self.load(lba).await?;
        put32(&mut self.buf, (cluster % per_sector) as usize * 4, value);
        self.store(lba).await
    }

    /// The next cluster of a FAT chain after `cluster`: `None` at the end of the chain.
    pub async fn fat_next(&mut self, cluster: u32) -> Res<Option<u32>, D> {
        let next = self.fat_get(cluster).await?;
        if next == FAT_EOF {
            return Ok(None);
        }
        if next == FAT_BAD || !self.info.cluster_in_range(next) {
            return Err(Error::Corrupt(Corrupt::FatChain));
        }
        Ok(Some(next))
    }

    fn bitmap_lba(&self, cluster: u32) -> Option<u64> {
        if !self.info.cluster_in_range(cluster) {
            return None;
        }
        let bit = (cluster - 2) as u64;
        let sector = bit / (SECTOR_SIZE as u64 * 8);
        Some(self.info.cluster_lba(self.info.bitmap_cluster)? + sector)
    }

    /// Whether `cluster` is marked allocated.
    pub async fn bitmap_get(&mut self, cluster: u32) -> Res<bool, D> {
        let lba = self
            .bitmap_lba(cluster)
            .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?;
        self.load(lba).await?;
        let bit = ((cluster - 2) as usize) % (SECTOR_SIZE * 8);
        Ok(self.buf[bit / 8] & (1 << (bit % 8)) != 0)
    }

    /// Mark `count` clusters from `first` allocated or free.
    async fn bitmap_set_range(&mut self, first: u32, count: u64, value: bool) -> Res<(), D> {
        let mut cluster = first;
        let mut left = count;
        while left > 0 {
            let lba = self
                .bitmap_lba(cluster)
                .ok_or(Error::Corrupt(Corrupt::ExtentOutOfRange))?;
            self.load(lba).await?;
            let first_bit = ((cluster - 2) as usize) % (SECTOR_SIZE * 8);
            let in_sector = (SECTOR_SIZE * 8 - first_bit).min(left.min(u32::MAX as u64) as usize);
            let last_cluster = cluster as u64 + in_sector as u64 - 1;
            if last_cluster - 2 >= self.info.cluster_count as u64 {
                return Err(Error::Corrupt(Corrupt::ExtentOutOfRange));
            }
            for bit in first_bit..first_bit + in_sector {
                if value {
                    self.buf[bit / 8] |= 1 << (bit % 8);
                } else {
                    self.buf[bit / 8] &= !(1 << (bit % 8));
                }
            }
            self.store(lba).await?;
            cluster += in_sector as u32;
            left -= in_sector as u64;
        }
        Ok(())
    }

    /// `true` when all `count` clusters from `first` are inside the heap and marked allocated in the bitmap:
    /// what a caller that writes a preallocated file by raw sector address checks before trusting its extent.
    pub async fn is_allocated(&mut self, first: u32, count: u64) -> Res<bool, D> {
        if count == 0 {
            return Ok(true);
        }
        let Some(last) = (first as u64 + count - 1)
            .try_into()
            .ok()
            .filter(|&l| self.info.cluster_in_range(l))
        else {
            return Ok(false);
        };
        let _: u32 = last;
        for c in first..=last {
            if !self.bitmap_get(c).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Smallest cluster `>= cluster` whose first sector is a multiple of `align_sectors` (0 or 1: none).
    fn next_aligned(&self, cluster: u32, align_sectors: u32) -> Option<u32> {
        if align_sectors <= 1 {
            return Some(cluster);
        }
        let lba = self.info.cluster_lba(cluster)?;
        let rem = lba % align_sectors as u64;
        if rem == 0 {
            return Some(cluster);
        }
        let skip = (align_sectors as u64 - rem) >> self.info.sectors_per_cluster_shift;
        (cluster as u64 + skip).try_into().ok()
    }

    /// First-fit search for `count` free clusters in a row, the first one aligned to `align_sectors`.
    /// Bounded by one pass over the bitmap.
    pub async fn find_free_run(&mut self, count: u64, align_sectors: u32) -> Res<u32, D> {
        if count == 0 || count > self.info.cluster_count as u64 {
            return Err(Error::NoSpace);
        }
        if align_sectors > 1
            && (!align_sectors.is_power_of_two() || align_sectors < self.info.sectors_per_cluster())
        {
            return Err(Error::InvalidArgument);
        }
        let total = self.info.cluster_count;
        let bitmap_lba = self
            .info
            .cluster_lba(self.info.bitmap_cluster)
            .ok_or(Error::Corrupt(Corrupt::Bitmap))?;
        let sectors = (total as u64).div_ceil(SECTOR_SIZE as u64 * 8);
        let mut run_start: Option<u32> = None;
        for s in 0..sectors {
            self.load(bitmap_lba + s).await?;
            let first_bit = s * SECTOR_SIZE as u64 * 8;
            for (i, &byte) in self.buf.iter().enumerate() {
                let base = first_bit + i as u64 * 8;
                if base >= total as u64 {
                    break;
                }
                if byte == 0xFF {
                    run_start = None;
                    continue;
                }
                for bit in 0..8u32 {
                    let n = base + bit as u64;
                    if n >= total as u64 {
                        break;
                    }
                    let cluster = n as u32 + 2;
                    if byte & (1 << bit) != 0 {
                        run_start = None;
                        continue;
                    }
                    if run_start.is_none() {
                        run_start = self.next_aligned(cluster, align_sectors);
                        if run_start.is_none() {
                            return Err(Error::NoSpace);
                        }
                    }
                    let start = run_start.unwrap_or(cluster);
                    if cluster >= start && (cluster - start) as u64 + 1 >= count {
                        return Ok(start);
                    }
                }
            }
        }
        Err(Error::NoSpace)
    }

    // ---- directories --------------------------------------------------------------------------------------

    /// Advance `cur` to its next cluster; sets `cluster = 0` at the end of the directory.
    async fn next_cluster(&mut self, cur: &mut DirCursor) -> Res<(), D> {
        cur.clusters_visited += 1;
        if cur.clusters_visited >= self.info.cluster_count {
            return Err(Error::Corrupt(Corrupt::FatChain));
        }
        let next = if cur.no_fat_chain {
            let next = cur.cluster.wrapping_add(1);
            if cur.index >= cur.size || !self.info.cluster_in_range(next) {
                0
            } else {
                next
            }
        } else {
            self.fat_next(cur.cluster).await?.unwrap_or(0)
        };
        if cur.index >= cur.size {
            cur.cluster = 0;
        } else {
            cur.cluster = next;
        }
        if cur.cluster != 0 {
            cur.last_cluster = cur.cluster;
        }
        cur.offset = 0;
        Ok(())
    }

    /// The next raw 32-byte entry of a directory, with its location; `None` at the end of the directory's
    /// clusters (the type-0 end marker is returned like any entry).
    async fn next_raw(
        &mut self,
        cur: &mut DirCursor,
    ) -> Res<Option<(EntryLocation, [u8; ENTRY_SIZE])>, D> {
        if cur.cluster == 0 || cur.index >= cur.size {
            return Ok(None);
        }
        let lba = self
            .info
            .cluster_lba(cur.cluster)
            .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?
            + (cur.offset / SECTOR_SIZE as u32) as u64;
        self.load(lba).await?;
        let at = (cur.offset as usize) % SECTOR_SIZE;
        let mut entry = [0u8; ENTRY_SIZE];
        entry.copy_from_slice(&self.buf[at..at + ENTRY_SIZE]);
        let location = EntryLocation {
            cluster: cur.cluster,
            offset: cur.offset,
            index: cur.index,
        };
        cur.offset += ENTRY_SIZE as u32;
        cur.index += ENTRY_SIZE as u64;
        if cur.offset >= self.info.cluster_bytes() {
            self.next_cluster(cur).await?;
        }
        Ok(Some((location, entry)))
    }

    /// Write `entry` at `location`.
    async fn write_raw(&mut self, location: EntryLocation, entry: &[u8]) -> Res<(), D> {
        let lba = self
            .info
            .cluster_lba(location.cluster)
            .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?
            + (location.offset / SECTOR_SIZE as u32) as u64;
        self.load(lba).await?;
        let at = (location.offset as usize) % SECTOR_SIZE;
        self.buf[at..at + ENTRY_SIZE].copy_from_slice(entry);
        self.store(lba).await
    }

    /// The next in-use entry of a directory: files and directories as complete entry sets, the metadata
    /// entries, damaged sets flagged. `None` at the end of the directory. Unused entries are skipped.
    pub async fn next_entry(&mut self, cur: &mut DirCursor) -> Res<Option<Entry>, D> {
        loop {
            let Some((location, e)) = self.next_raw(cur).await? else {
                return Ok(None);
            };
            let kind = e[0];
            if kind == TYPE_END {
                cur.cluster = 0;
                return Ok(None);
            }
            if kind & TYPE_IN_USE == 0 {
                continue;
            }
            return Ok(Some(match kind {
                TYPE_FILE => self.read_set(cur, location, &e).await?,
                TYPE_BITMAP => Entry::Bitmap {
                    first_cluster: le32(&e, 20),
                    length: crate::boot::le64(&e, 24),
                },
                TYPE_UPCASE => Entry::Upcase {
                    checksum: le32(&e, 4),
                    first_cluster: le32(&e, 20),
                    length: crate::boot::le64(&e, 24),
                },
                TYPE_LABEL => {
                    let len = e[1].min(MAX_LABEL_UNITS as u8);
                    let mut units = [0u16; MAX_LABEL_UNITS];
                    for (i, unit) in units.iter_mut().enumerate().take(len as usize) {
                        *unit = crate::boot::le16(&e, 2 + i * 2);
                    }
                    Entry::Label(Label { units, len })
                }
                _ => Entry::Other(kind),
            }));
        }
    }

    /// Collect the secondaries of a file entry whose primary `primary` was just read at `location`.
    async fn read_set(
        &mut self,
        cur: &mut DirCursor,
        location: EntryLocation,
        primary: &[u8; ENTRY_SIZE],
    ) -> Res<Entry, D> {
        let mut set = EntrySet::new();
        set.push(primary);
        let secondaries = primary[1];
        if secondaries == 0 || secondaries > MAX_SECONDARIES {
            return Ok(Entry::Damaged(location));
        }
        for _ in 0..secondaries {
            let before = *cur;
            let Some((_, e)) = self.next_raw(cur).await? else {
                return Ok(Entry::Damaged(location));
            };
            if e[0] & (TYPE_IN_USE | TYPE_SECONDARY) != (TYPE_IN_USE | TYPE_SECONDARY) {
                // Not a secondary: leave it for the next call.
                *cur = before;
                return Ok(Entry::Damaged(location));
            }
            set.push(&e);
        }
        Ok(match set.parse_file(location) {
            Some(info) => Entry::File(info),
            None => Entry::Damaged(location),
        })
    }

    /// The next file or directory of a directory; metadata and damaged entries are skipped. `None` at the end.
    pub async fn next(&mut self, cur: &mut DirCursor) -> Res<Option<FileInfo>, D> {
        loop {
            match self.next_entry(cur).await? {
                None => return Ok(None),
                Some(Entry::File(info)) => return Ok(Some(info)),
                Some(_) => {}
            }
        }
    }

    /// Look `name` up in the directory at `dir` (case-insensitive). Bounded by [`Volume::set_max_dir_entries`].
    pub async fn lookup(&mut self, dir: DirCursor, name: &Name) -> Res<Option<FileInfo>, D> {
        let mut cur = dir;
        let hash = name.hash();
        let limit = self.max_dir_entries as u64 * ENTRY_SIZE as u64;
        loop {
            if cur.index > limit {
                return Err(Error::Limit(Limit::Entries));
            }
            let Some(entry) = self.next_entry(&mut cur).await? else {
                return Ok(None);
            };
            if let Entry::File(info) = entry
                && info.name.len() == name.len()
                && info.name.hash() == hash
                && info.name.eq_ignore_case(name)
            {
                return Ok(Some(info));
            }
        }
    }

    /// Find a file or directory by path from the root (`/` separated, case-insensitive). `Ok(None)` when a
    /// component does not exist; `NotADirectory` when a middle component is a file; `InvalidName` for a
    /// component exFAT can't hold.
    pub async fn find(&mut self, path: &str) -> Res<Option<FileInfo>, D> {
        let mut dir = self.root();
        let mut found: Option<FileInfo> = None;
        let mut depth = 0;
        for component in path.split('/').filter(|c| !c.is_empty()) {
            if let Some(info) = found {
                dir = DirCursor::for_dir(&info).ok_or(Error::NotADirectory)?;
            }
            depth += 1;
            if depth > MAX_WALK_DEPTH {
                return Err(Error::Limit(Limit::PathDepth));
            }
            let name = Name::new(component).ok_or(Error::InvalidName)?;
            match self.lookup(dir, &name).await? {
                Some(info) => found = Some(info),
                None => return Ok(None),
            }
        }
        Ok(found)
    }

    /// Walk the tree under `start` depth-first, calling `f` with the depth (0 for entries of `start`) and each
    /// file or directory. Bounded by `limits` and by [`MAX_WALK_DEPTH`]; damaged entry sets are counted and
    /// skipped.
    pub async fn walk<F>(
        &mut self,
        start: DirCursor,
        limits: WalkLimits,
        mut f: F,
    ) -> Res<WalkStats, D>
    where
        F: FnMut(u8, &FileInfo) -> WalkControl,
    {
        let mut stats = WalkStats::default();
        let max_depth = (limits.max_depth as usize).min(MAX_WALK_DEPTH - 1);
        let mut stack = [start; MAX_WALK_DEPTH];
        let mut depth = 0usize;
        loop {
            let cur = &mut stack[depth];
            let before = cur.index;
            let next = self.next_entry(cur).await?;
            stats.entries = stats
                .entries
                .saturating_add(((cur.index - before) / ENTRY_SIZE as u64) as u32);
            if stats.entries > limits.max_entries {
                return Err(Error::Limit(Limit::Entries));
            }
            match next {
                None => {
                    if depth == 0 {
                        return Ok(stats);
                    }
                    depth -= 1;
                }
                Some(Entry::Damaged(_)) => stats.damaged += 1,
                Some(Entry::File(info)) => {
                    if info.is_dir() {
                        stats.dirs += 1;
                    } else {
                        stats.files += 1;
                    }
                    match f(depth as u8, &info) {
                        WalkControl::Stop => return Ok(stats),
                        WalkControl::SkipDir => {}
                        WalkControl::Continue if info.is_dir() => {
                            if depth >= max_depth {
                                stats.too_deep += 1;
                            } else if let Some(sub) = DirCursor::for_dir(&info) {
                                if self.info.cluster_in_range(sub.first_cluster) {
                                    depth += 1;
                                    stack[depth] = sub;
                                } else {
                                    stats.damaged += 1;
                                }
                            }
                        }
                        WalkControl::Continue => {}
                    }
                }
                Some(_) => {}
            }
        }
    }

    // ---- checks -------------------------------------------------------------------------------------------

    /// Count the clusters of `info` and verify each is marked in the bitmap and inside the heap; `None` when
    /// the allocation is bad (a chain that breaks, a cluster out of range, an unmarked cluster).
    async fn file_clusters_ok(&mut self, info: &FileInfo) -> Res<Option<u64>, D> {
        let n = info.cluster_count(self.info.cluster_bytes());
        if n == 0 {
            return Ok(Some(0));
        }
        if !self.info.cluster_in_range(info.first_cluster) {
            return Ok(None);
        }
        if info.no_fat_chain {
            return Ok(self.is_allocated(info.first_cluster, n).await?.then_some(n));
        }
        // A chain longer than the heap loops: a bad allocation, not a hang.
        if n > self.info.cluster_count as u64 {
            return Ok(None);
        }
        let mut cluster = info.first_cluster;
        for i in 0..n {
            if !self.bitmap_get(cluster).await? {
                return Ok(None);
            }
            if i + 1 < n {
                match self.fat_next(cluster).await {
                    Ok(Some(next)) => cluster = next,
                    Ok(None) | Err(Error::Corrupt(_)) => return Ok(None),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(Some(n))
    }

    /// Check the volume: the backup boot region, the up-case table against its checksum, every file's and
    /// directory's allocation against the bitmap (a bounded walk), and the bitmap's count against the clusters
    /// reached. Read-only.
    pub async fn check(&mut self, limits: WalkLimits) -> Res<CheckReport, D> {
        let mut report = CheckReport::default();
        let p = self.info.partition_lba;
        report.backup_boot_ok = self.boot_region_checksum_ok(p + 12).await? && {
            self.load(p + 11).await?;
            let main = le32(&self.buf, 0);
            self.load(p + 23).await?;
            le32(&self.buf, 0) == main
        };
        // Up-case table.
        let mut sum = 0;
        let mut left = self.info.upcase_length;
        let mut lba = self
            .info
            .cluster_lba(self.info.upcase_cluster)
            .ok_or(Error::Corrupt(Corrupt::UpcaseTable))?;
        let upcase_end = self.info.cluster_lba(self.info.upcase_cluster).unwrap_or(0)
            + ((self.info.cluster_count as u64 - (self.info.upcase_cluster as u64 - 2))
                << self.info.sectors_per_cluster_shift);
        while left > 0 && lba < upcase_end {
            self.load(lba).await?;
            let n = left.min(SECTOR_SIZE as u64) as usize;
            sum = checksum_step(sum, &self.buf[..n]);
            left -= n as u64;
            lba += 1;
        }
        report.upcase_ok = left == 0 && sum == self.info.upcase_checksum;
        // Bitmap population.
        let bitmap_lba = self
            .info
            .cluster_lba(self.info.bitmap_cluster)
            .ok_or(Error::Corrupt(Corrupt::Bitmap))?;
        let total = self.info.cluster_count as u64;
        let sectors = total.div_ceil(SECTOR_SIZE as u64 * 8);
        for s in 0..sectors {
            self.load(bitmap_lba + s).await?;
            let first_bit = s * SECTOR_SIZE as u64 * 8;
            for (i, &byte) in self.buf.iter().enumerate() {
                let base = first_bit + i as u64 * 8;
                if base >= total {
                    break;
                }
                let valid = (total - base).min(8) as u32;
                let mask = if valid == 8 { 0xFF } else { (1u8 << valid) - 1 };
                report.bitmap_set += (byte & mask).count_ones() as u64;
            }
        }
        // Metadata clusters: bitmap, up-case table, root directory chain.
        let bitmap_clusters = self.info.clusters_for(self.info.bitmap_length);
        let upcase_clusters = self.info.clusters_for(self.info.upcase_length);
        let mut reached = 0u64;
        for (first, n) in [
            (self.info.bitmap_cluster, bitmap_clusters),
            (self.info.upcase_cluster, upcase_clusters),
        ] {
            if self.is_allocated(first, n).await? {
                reached += n;
            } else {
                report.bad_allocations += 1;
            }
        }
        let mut cluster = self.info.root_cluster;
        let mut root_clusters = 0u64;
        loop {
            if !self.bitmap_get(cluster).await? {
                report.bad_allocations += 1;
                break;
            }
            root_clusters += 1;
            if root_clusters > self.info.cluster_count as u64 {
                report.bad_allocations += 1;
                break;
            }
            match self.fat_next(cluster).await {
                Ok(Some(next)) => cluster = next,
                Ok(None) => break,
                Err(Error::Corrupt(_)) => {
                    report.bad_allocations += 1;
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        reached += root_clusters;
        // Every file and directory. The walk callback can't do async reads, so files are checked one at a
        // time by re-walking: a bounded, simple scheme that keeps the cursor stack in walk().
        let mut bad = 0u32;
        let mut file_clusters = 0u64;
        let mut stack = [self.root(); MAX_WALK_DEPTH];
        let mut depth = 0usize;
        let max_depth = (limits.max_depth as usize).min(MAX_WALK_DEPTH - 1);
        loop {
            let cur = &mut stack[depth];
            let before = cur.index;
            let next = self.next_entry(cur).await?;
            report.walk.entries = report
                .walk
                .entries
                .saturating_add(((cur.index - before) / ENTRY_SIZE as u64) as u32);
            if report.walk.entries > limits.max_entries {
                return Err(Error::Limit(Limit::Entries));
            }
            match next {
                None => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Some(Entry::Damaged(_)) => report.walk.damaged += 1,
                Some(Entry::File(info)) => {
                    if info.is_dir() {
                        report.walk.dirs += 1;
                    } else {
                        report.walk.files += 1;
                    }
                    match self.file_clusters_ok(&info).await? {
                        Some(n) => file_clusters += n,
                        None => bad += 1,
                    }
                    if info.is_dir() && info.first_cluster != 0 {
                        if depth >= max_depth {
                            report.walk.too_deep += 1;
                        } else if let Some(sub) = DirCursor::for_dir(&info)
                            && self.info.cluster_in_range(sub.first_cluster)
                        {
                            depth += 1;
                            stack[depth] = sub;
                        }
                    }
                }
                Some(_) => {}
            }
        }
        report.bad_allocations += bad;
        report.clusters_reached = reached + file_clusters;
        report.clean = report.backup_boot_ok
            && report.upcase_ok
            && report.bad_allocations == 0
            && report.walk.damaged == 0
            && report.walk.too_deep == 0
            && report.clusters_reached == report.bitmap_set;
        Ok(report)
    }

    // ---- files --------------------------------------------------------------------------------------------

    /// Read `buf.len()` bytes of `file` from `offset`; returns how many were read (short at the end of the
    /// file). Bytes past `valid_data_length` read as zeros, like an OS shows them.
    pub async fn read(&mut self, file: &FileInfo, offset: u64, buf: &mut [u8]) -> Res<usize, D> {
        if offset >= file.data_length {
            return Ok(0);
        }
        let len = buf
            .len()
            .min((file.data_length - offset).min(usize::MAX as u64) as usize);
        let cluster_bytes = self.info.cluster_bytes() as u64;
        let mut done = 0usize;
        // Walk to the cluster holding `offset`.
        let mut cluster_index = offset / cluster_bytes;
        let mut cluster = file.first_cluster;
        if !file.no_fat_chain {
            if cluster_index >= self.info.cluster_count as u64 {
                return Err(Error::Corrupt(Corrupt::FatChain));
            }
            for _ in 0..cluster_index {
                cluster = self
                    .fat_next(cluster)
                    .await?
                    .ok_or(Error::Corrupt(Corrupt::FatChain))?;
            }
        }
        while done < len {
            let at = offset + done as u64;
            let this_cluster = at / cluster_bytes;
            if this_cluster != cluster_index {
                cluster = if file.no_fat_chain {
                    cluster
                        .checked_add(1)
                        .ok_or(Error::Corrupt(Corrupt::ExtentOutOfRange))?
                } else {
                    self.fat_next(cluster)
                        .await?
                        .ok_or(Error::Corrupt(Corrupt::FatChain))?
                };
                cluster_index = this_cluster;
            }
            let in_cluster = at % cluster_bytes;
            let lba = if file.no_fat_chain {
                self.info
                    .cluster_lba(file.first_cluster)
                    .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?
                    .checked_add(at / SECTOR_SIZE as u64)
                    .ok_or(Error::Corrupt(Corrupt::ExtentOutOfRange))?
            } else {
                self.info
                    .cluster_lba(cluster)
                    .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?
                    + in_cluster / SECTOR_SIZE as u64
            };
            if file.no_fat_chain && !self.info.cluster_in_range(cluster) {
                return Err(Error::Corrupt(Corrupt::ExtentOutOfRange));
            }
            let in_sector = (at % SECTOR_SIZE as u64) as usize;
            let n = (SECTOR_SIZE - in_sector).min(len - done);
            if at >= file.valid_data_length {
                buf[done..done + n].fill(0);
            } else {
                self.load(lba).await?;
                buf[done..done + n].copy_from_slice(&self.buf[in_sector..in_sector + n]);
                if at + n as u64 > file.valid_data_length {
                    let keep = (file.valid_data_length - at) as usize;
                    buf[done + keep..done + n].fill(0);
                }
            }
            done += n;
        }
        Ok(done)
    }

    /// Create a contiguous, preallocated file in the root directory. See [`NewFile`] and
    /// [`Volume::create_in`].
    pub async fn create(&mut self, spec: &NewFile<'_>) -> Res<FileInfo, D> {
        let root = self.root();
        self.create_in(root, spec).await
    }

    /// Create a contiguous, preallocated (`NoFatChain`) file in the directory at `dir`: its clusters are
    /// marked in the bitmap, the FAT is untouched, `ValidDataLength = DataLength = size`. The root directory
    /// grows by a cluster when it has no room for the entry set; another directory gives `DirectoryFull`.
    pub async fn create_in(&mut self, dir: DirCursor, spec: &NewFile<'_>) -> Res<FileInfo, D> {
        let name = Name::new(spec.name).ok_or(Error::InvalidName)?;
        if self.lookup(dir, &name).await?.is_some() {
            return Err(Error::AlreadyExists);
        }
        let size = spec.size.max(spec.data.map_or(0, |d| d.len() as u64));
        let clusters = self.info.clusters_for(size);
        let set = EntrySet::for_file(&name, spec.attributes, 0, 0, true, spec.time);
        let slots = self.find_slots(dir, set.count as usize).await?;
        let first = if clusters == 0 {
            0
        } else if let Some(c) = spec.at_cluster {
            if !self.info.cluster_in_range(c)
                || spec.align_sectors > 1 && self.next_aligned(c, spec.align_sectors) != Some(c)
            {
                return Err(Error::InvalidArgument);
            }
            if (c as u64 - 2) + clusters > self.info.cluster_count as u64 {
                return Err(Error::NoSpace);
            }
            for i in 0..clusters {
                if self.bitmap_get(c + i as u32).await? {
                    return Err(Error::NoSpace);
                }
            }
            c
        } else {
            self.find_free_run(clusters, spec.align_sectors).await?
        };
        if clusters > 0 {
            self.bitmap_set_range(first, clusters, true).await?;
        }
        let result = self
            .create_rest(&name, spec, first, size, clusters, &slots)
            .await;
        if result.is_err() && clusters > 0 {
            // Best effort: give the clusters back.
            let _ = self.bitmap_set_range(first, clusters, false).await;
        }
        result
    }

    async fn create_rest(
        &mut self,
        name: &Name,
        spec: &NewFile<'_>,
        first: u32,
        size: u64,
        clusters: u64,
        slots: &[EntryLocation],
    ) -> Res<FileInfo, D> {
        if let Some(data) = spec.data
            && clusters > 0
        {
            let lba = self
                .info
                .cluster_lba(first)
                .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?;
            let sectors = clusters << self.info.sectors_per_cluster_shift;
            self.cached = None;
            for s in 0..sectors {
                let start = (s as usize).saturating_mul(SECTOR_SIZE);
                let n = data.len().saturating_sub(start).min(SECTOR_SIZE);
                self.buf.fill(0);
                if n > 0 {
                    self.buf[..n].copy_from_slice(&data[start..start + n]);
                }
                self.dev
                    .write(lba + s, &self.buf)
                    .await
                    .map_err(Error::Device)?;
            }
        }
        let set = EntrySet::for_file(name, spec.attributes, first, size, true, spec.time);
        for (i, loc) in slots.iter().enumerate().take(set.count as usize) {
            self.write_raw(*loc, set.entry(i)).await?;
        }
        self.dev.flush().await.map_err(Error::Device)?;
        set.parse_file(slots[0])
            .ok_or(Error::Corrupt(Corrupt::EntrySet))
    }

    /// Find `n` consecutive free entry slots in `dir`, growing the root directory if needed.
    async fn find_slots(
        &mut self,
        dir: DirCursor,
        n: usize,
    ) -> Res<[EntryLocation; MAX_SET_ENTRIES], D> {
        let mut slots = [EntryLocation {
            cluster: 0,
            offset: 0,
            index: 0,
        }; MAX_SET_ENTRIES];
        let mut free = 0usize;
        let mut cur = dir;
        let limit = self.max_dir_entries as u64 * ENTRY_SIZE as u64;
        loop {
            if cur.index > limit {
                return Err(Error::Limit(Limit::Entries));
            }
            match self.next_raw(&mut cur).await? {
                Some((loc, e)) => {
                    if e[0] & TYPE_IN_USE == 0 {
                        slots[free] = loc;
                        free += 1;
                        if free == n {
                            return Ok(slots);
                        }
                    } else {
                        free = 0;
                    }
                }
                None => {
                    if !dir.is_root() {
                        return Err(Error::DirectoryFull);
                    }
                    let new = self.find_free_run(1, 0).await?;
                    self.bitmap_set_range(new, 1, true).await?;
                    let lba = self
                        .info
                        .cluster_lba(new)
                        .ok_or(Error::Corrupt(Corrupt::ClusterOutOfRange))?;
                    self.cached = None;
                    self.buf.fill(0);
                    for s in 0..self.info.sectors_per_cluster() as u64 {
                        self.dev
                            .write(lba + s, &self.buf)
                            .await
                            .map_err(Error::Device)?;
                    }
                    self.fat_set(new, FAT_EOF).await?;
                    self.fat_set(cur.last_cluster, new).await?;
                    cur.cluster = new;
                    cur.last_cluster = new;
                    cur.offset = 0;
                }
            }
        }
    }

    /// Delete a file of the root directory: its entry set is marked unused and its clusters freed in the
    /// bitmap. Directories are refused (`InvalidArgument`) unless empty of entries is not checked: only files.
    pub async fn delete(&mut self, file: &FileInfo) -> Res<(), D> {
        let root = self.root();
        self.delete_in(root, file).await
    }

    /// Delete a file whose entry set sits in the directory at `dir` (see [`Volume::delete`]).
    pub async fn delete_in(&mut self, dir: DirCursor, file: &FileInfo) -> Res<(), D> {
        if file.is_dir() {
            return Err(Error::InvalidArgument);
        }
        let mut cur = dir.at(file.location);
        for i in 0..=file.secondary_count as usize {
            let Some((loc, mut e)) = self.next_raw(&mut cur).await? else {
                return Err(Error::Corrupt(Corrupt::EntrySet));
            };
            if i == 0 && e[0] != TYPE_FILE {
                return Err(Error::Corrupt(Corrupt::EntrySet));
            }
            e[0] &= !TYPE_IN_USE;
            self.write_raw(loc, &e).await?;
        }
        let n = file.cluster_count(self.info.cluster_bytes());
        if n > 0 {
            if file.no_fat_chain {
                self.bitmap_set_range(file.first_cluster, n, false).await?;
            } else {
                let mut cluster = file.first_cluster;
                for i in 0..n.min(self.info.cluster_count as u64) {
                    self.bitmap_set_range(cluster, 1, false).await?;
                    if i + 1 < n {
                        match self.fat_next(cluster).await? {
                            Some(next) => cluster = next,
                            None => break,
                        }
                    }
                }
            }
        }
        self.dev.flush().await.map_err(Error::Device)
    }
}

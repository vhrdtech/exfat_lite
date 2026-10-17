//! Format, mount, create, find, walk, check and delete: in memory, on sparse image files checked with
//! `fsck.exfat -n` (exfatprogs) where it is installed, and against an image `mkfs.exfat` made.

#![cfg(feature = "std")]

use std::path::{Path, PathBuf};
use std::process::Command;

use exfat_lite::volume::WalkStats;
use exfat_lite::{
    BlockDevice, Error, FormatParams, Geometry, ImageFile, NewFile, SliceDevice, Volume,
    WalkControl, WalkLimits, attr, block_on, format,
};

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

/// `fsck.exfat -n` on `image`; `None` when exfatprogs is not installed.
fn fsck(image: &Path) -> Option<(bool, String)> {
    let out = Command::new("fsck.exfat")
        .arg("-n")
        .arg(image)
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some((out.status.success(), text))
}

fn fsck_clean(image: &Path) {
    match fsck(image) {
        Some((ok, text)) => assert!(
            ok && text.contains("clean"),
            "fsck.exfat -n on {}:\n{text}",
            image.display()
        ),
        None => eprintln!(
            "fsck.exfat not installed: skipping the exfatprogs check of {}",
            image.display()
        ),
    }
}

/// The image file holds exactly the exFAT partition, the MBR and the gap before the partition live in RAM:
/// `fsck.exfat` reads a volume from offset 0 of a file, and this way no copy of a sparse 128 GB image is needed.
struct PartitionView {
    file: ImageFile,
    offset: u64,
    mbr: [u8; 512],
}

impl PartitionView {
    fn create(path: &Path, total_sectors: u64, geometry: Geometry) -> PartitionView {
        let offset = geometry.boundary_unit_sectors as u64;
        PartitionView {
            file: ImageFile::create(path, total_sectors - offset).unwrap(),
            offset,
            mbr: [0; 512],
        }
    }
}

impl BlockDevice for PartitionView {
    type Error = std::io::Error;

    fn sector_count(&self) -> u64 {
        self.file.sector_count() + self.offset
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()> {
        for (i, sector) in buf.as_chunks_mut::<512>().0.iter_mut().enumerate() {
            let lba = lba + i as u64;
            if lba == 0 {
                sector.copy_from_slice(&self.mbr);
            } else if lba < self.offset {
                sector.fill(0);
            } else {
                self.file.read(lba - self.offset, sector).await?;
            }
        }
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> std::io::Result<()> {
        for (i, sector) in buf.as_chunks::<512>().0.iter().enumerate() {
            let lba = lba + i as u64;
            if lba == 0 {
                self.mbr.copy_from_slice(sector);
            } else if lba >= self.offset {
                self.file.write(lba - self.offset, sector).await?;
            }
        }
        Ok(())
    }
}

fn temp_image(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("exfat_lite-")
        .tempdir()
        .unwrap();
    let path = dir.path().join(name);
    (dir, path)
}

/// Format `dev`, then put the usual two files on it: a README with text and a big aligned ring file that
/// leaves `free_bytes` free.
fn populate<D: BlockDevice>(
    mut dev: D,
    geometry: Geometry,
    label: &str,
    free_bytes: u64,
) -> Volume<D> {
    let mut scratch = vec![0u8; 64 * 1024];
    let params = FormatParams::new(geometry, label, 0xC0FF_EE01);
    let layout = block_on(format(&mut dev, &params, &mut scratch)).unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    assert_eq!(vol.info().cluster_count, layout.cluster_count);
    let readme =
        b"This card belongs to a logring device.\nRead logring.wlr with the logring CLI.\n";
    let info = block_on(vol.create(&NewFile::new("README.txt").with_data(readme))).unwrap();
    assert_eq!(info.data_length, readme.len() as u64);
    let cluster_bytes = vol.info().cluster_bytes() as u64;
    let free_now =
        (vol.info().cluster_count as u64 - layout.metadata_clusters() as u64 - 1) * cluster_bytes;
    let ring_size =
        free_now.saturating_sub(free_bytes) / geometry.boundary_unit_sectors as u64 / 512
            * geometry.boundary_unit_sectors as u64
            * 512;
    let ring = NewFile::new("logring.wlr")
        .size(ring_size)
        .align_sectors(geometry.boundary_unit_sectors)
        .attributes(attr::READ_ONLY | attr::SYSTEM | attr::HIDDEN | attr::ARCHIVE);
    let ring = block_on(vol.create(&ring)).unwrap();
    assert!(ring.no_fat_chain);
    assert_eq!(ring.data_length, ring_size);
    let lba = vol.info().cluster_lba(ring.first_cluster).unwrap();
    assert_eq!(
        lba % geometry.boundary_unit_sectors as u64,
        0,
        "ring file not on a boundary unit"
    );
    vol
}

#[test]
fn in_memory_round_trip() {
    let mut bytes = vec![0u8; 64 * MB as usize];
    let dev = SliceDevice::new(&mut bytes);
    let geometry = Geometry::for_capacity(dev.sector_count());
    let mut vol = populate(dev, geometry, "LOGRING", 8 * MB);
    assert_eq!(vol.info().label.to_string(), "LOGRING");
    assert_eq!(vol.info().serial, 0xC0FF_EE01);
    assert!(!vol.info().is_dirty());

    let readme = block_on(vol.find("readme.TXT"))
        .unwrap()
        .expect("README.txt by another case");
    let mut buf = [0u8; 200];
    let n = block_on(vol.read(&readme, 0, &mut buf)).unwrap();
    assert!(
        core::str::from_utf8(&buf[..n])
            .unwrap()
            .starts_with("This card belongs")
    );
    let n = block_on(vol.read(&readme, 10, &mut buf[..5])).unwrap();
    assert_eq!(&buf[..n], b"belon");
    assert_eq!(
        block_on(vol.read(&readme, readme.data_length, &mut buf)).unwrap(),
        0
    );

    let ring = block_on(vol.find("/logring.wlr")).unwrap().unwrap();
    assert_eq!(ring.attributes & attr::READ_ONLY, attr::READ_ONLY);
    assert_eq!(ring.valid_data_length, ring.data_length);
    assert!(block_on(vol.find("nothing.here")).unwrap().is_none());
    assert_eq!(
        block_on(vol.find("README.txt/x")).unwrap_err(),
        Error::NotADirectory
    );
    assert_eq!(
        block_on(vol.find("bad:name")).unwrap_err(),
        Error::InvalidName
    );
    assert_eq!(
        block_on(vol.create(&NewFile::new("readme.txt"))).unwrap_err(),
        Error::AlreadyExists
    );
    assert_eq!(
        block_on(vol.create(&NewFile::new("huge").size(1 << 40))).unwrap_err(),
        Error::NoSpace
    );

    let mut names = Vec::new();
    let stats = block_on(vol.walk(vol.root(), WalkLimits::DEFAULT, |depth, info| {
        names.push((depth, info.name.to_string()));
        WalkControl::Continue
    }))
    .unwrap();
    assert_eq!(
        stats,
        WalkStats {
            files: 2,
            dirs: 0,
            entries: 10,
            damaged: 0,
            too_deep: 0
        }
    );
    assert_eq!(
        names,
        vec![
            (0, "README.txt".to_string()),
            (0, "logring.wlr".to_string())
        ]
    );

    let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
    assert!(report.clean, "{report:?}");
    assert_eq!(report.walk.files, 2);
    assert_eq!(report.clusters_reached, report.bitmap_set);

    // Delete the README: its clusters come back, the volume stays consistent, the ring file is where it was.
    block_on(vol.delete(&readme)).unwrap();
    assert!(block_on(vol.find("README.txt")).unwrap().is_none());
    let report2 = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
    assert!(report2.clean, "{report2:?}");
    assert_eq!(report2.bitmap_set + 1, report.bitmap_set);
    let again = block_on(vol.create(&NewFile::new("README.txt").with_data(b"again"))).unwrap();
    assert_eq!(again.location, readme.location, "the freed slot is reused");
    assert_eq!(
        block_on(vol.find("logring.wlr"))
            .unwrap()
            .unwrap()
            .first_cluster,
        ring.first_cluster
    );

    // Re-mount from the bytes.
    let dev = vol.into_device();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
}

#[test]
fn fsck_on_sd_sized_images() {
    for (name, bytes) in [
        ("1gb", GB),
        ("8gb", 8 * GB),
        ("32gb", 32 * GB),
        ("128gb", 128 * GB),
    ] {
        let (_dir, path) = temp_image(&format!("{name}.img"));
        let sectors = bytes / 512;
        let geometry = Geometry::for_capacity(sectors);
        let dev = PartitionView::create(&path, sectors, geometry);
        let mut vol = populate(dev, geometry, "LOGRING", 128 * MB);
        let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
        assert!(report.clean, "{name}: {report:?}");
        drop(vol);
        fsck_clean(&path);
        // And once more after a re-mount from the file, as a bare volume (what fsck saw).
        let dev = ImageFile::open(&path).unwrap();
        let mut vol = block_on(Volume::mount(dev)).unwrap();
        assert_eq!(vol.info().partition_lba, 0);
        let ring = block_on(vol.find("logring.wlr")).unwrap().unwrap();
        let expected = (bytes - 2 * geometry.boundary_unit_sectors as u64 * 512 - 128 * MB)
            / (geometry.boundary_unit_sectors as u64 * 512)
            * geometry.boundary_unit_sectors as u64
            * 512;
        assert!(
            ring.data_length >= expected - geometry.boundary_unit_sectors as u64 * 512 * 2,
            "{name}: ring {} expected about {expected}",
            ring.data_length
        );
        assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
    }
}

#[test]
fn bare_volume_without_mbr() {
    let (_dir, path) = temp_image("bare.img");
    let mut dev = ImageFile::create(&path, 256 * MB / 512).unwrap();
    let mut scratch = [0u8; 512];
    let params = FormatParams::new(Geometry::for_capacity(dev.sector_count()), "", 1).without_mbr();
    let layout = block_on(format(&mut dev, &params, &mut scratch)).unwrap();
    assert_eq!(layout.partition_lba, 0);
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    assert!(vol.info().label.is_empty());
    block_on(vol.create(&NewFile::new("a.txt").with_data(b"a"))).unwrap();
    assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
    drop(vol);
    fsck_clean(&path);
}

#[test]
fn root_directory_grows() {
    // 4 KB clusters: 128 entries per root cluster; 60 files of 3 entries each need two clusters.
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 128,
    };
    let (_dir, path) = temp_image("grow.img");
    let mut dev = PartitionView::create(&path, 4 * MB / 512, geometry);
    let mut scratch = [0u8; 2048];
    block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "GROW", 7),
        &mut scratch,
    ))
    .unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    for i in 0..60 {
        let name = format!("file-{i:03}.bin");
        block_on(vol.create(&NewFile::new(&name).size(if i % 3 == 0 { 0 } else { 5000 }))).unwrap();
    }
    for i in 0..60 {
        assert!(
            block_on(vol.find(&format!("FILE-{i:03}.bin")))
                .unwrap()
                .is_some()
        );
    }
    let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
    assert!(report.clean, "{report:?}");
    assert_eq!(report.walk.files, 60);
    // Ask for one entry too many through a tight limit.
    assert!(matches!(
        block_on(vol.check(WalkLimits {
            max_depth: 1,
            max_entries: 100
        })),
        Err(Error::Limit(exfat_lite::Limit::Entries))
    ));
    drop(vol);
    fsck_clean(&path);
}

#[test]
fn long_and_unicode_names() {
    let mut bytes = vec![0u8; 16 * MB as usize];
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 2048,
    };
    let mut dev = SliceDevice::new(&mut bytes);
    let mut scratch = [0u8; 512];
    block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "Ünïcode", 3),
        &mut scratch,
    ))
    .unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    assert_eq!(vol.info().label.to_string(), "Ünïcode");
    let long = format!("{}.txt", "x".repeat(251));
    block_on(vol.create(&NewFile::new(&long).with_data(b"long"))).unwrap();
    block_on(vol.create(&NewFile::new("Дневник éàü 日本.log").with_data(b"u"))).unwrap();
    assert_eq!(
        block_on(vol.create(&NewFile::new(&format!("{long}x")))).unwrap_err(),
        Error::InvalidName
    );
    assert!(block_on(vol.find(&long.to_uppercase())).unwrap().is_some());
    assert!(
        block_on(vol.find("дНЕВНИК ÉÀÜ 日本.LOG"))
            .unwrap()
            .is_some()
    );
    assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
}

#[test]
fn garbage_does_not_panic() {
    for len in [0usize, 100, 512, 4096, 1 << 20] {
        let mut zeros = vec![0u8; len];
        assert!(block_on(Volume::mount(SliceDevice::new(&mut zeros))).is_err());
        let mut noise: Vec<u8> = (0..len)
            .map(|i| (i as u32).wrapping_mul(2654435761).to_le_bytes()[1])
            .collect();
        assert!(block_on(Volume::mount(SliceDevice::new(&mut noise))).is_err());
    }
    // A formatted volume, then truncated or scribbled on, mounts with an error or mounts and checks dirty,
    // never panics.
    let mut bytes = vec![0u8; 16 * MB as usize];
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 2048,
    };
    let mut dev = SliceDevice::new(&mut bytes);
    let mut scratch = [0u8; 512];
    let layout = block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "G", 3),
        &mut scratch,
    ))
    .unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    block_on(vol.create(&NewFile::new("a.txt").with_data(b"a"))).unwrap();
    let good = bytes.clone();
    for cut in [
        1usize,
        512,
        2048 * 512,
        2048 * 512 + 24 * 512,
        (layout.heap_lba() as usize + 8) * 512,
        good.len() / 2,
    ] {
        let mut short = good[..cut].to_vec();
        let _ = block_on(Volume::mount(SliceDevice::new(&mut short)))
            .map(|mut v| block_on(v.check(WalkLimits::DEFAULT)));
    }
    let root_lba = layout.cluster_lba(layout.root_cluster) as usize * 512;
    for (at, value) in [
        (root_lba, 0x85u8),
        (root_lba + 1, 0xFF),
        (root_lba + 32 + 20, 0xFF),
        (root_lba + 32 + 24, 0xFF),
        (root_lba + 96, 0x85),
        (root_lba + 97, 0x02),
        (layout.fat_lba() as usize * 512 + 16, 0x04),
    ] {
        let mut copy = good.clone();
        copy[at] = value;
        if let Ok(mut v) = block_on(Volume::mount(SliceDevice::new(&mut copy))) {
            let _ = block_on(v.check(WalkLimits::DEFAULT));
            let _ = block_on(v.find("a.txt"));
            let _ = block_on(v.create(&NewFile::new("b.txt").size(10)));
        }
    }
}

/// A FAT chain that loops on itself with a huge file length: every chain follower must stop at the cluster
/// count instead of walking the loop for 2^40 bytes.
#[test]
fn cyclic_fat_chain_is_bounded() {
    use exfat_lite::dir::{ENTRY_SIZE, STREAM_NO_FAT_CHAIN, set_checksum};
    let mut bytes = vec![0u8; 4 * MB as usize];
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 128,
    };
    let mut dev = SliceDevice::new(&mut bytes);
    let mut scratch = [0u8; 512];
    let layout = block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "LOOP", 9),
        &mut scratch,
    ))
    .unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    let file = block_on(vol.create(&NewFile::new("loop.bin").with_data(b"x"))).unwrap();
    let first = file.first_cluster;
    // Patch the stream entry: FAT-chained, 1 TB long; fix the set checksum; FAT[first] = first.
    let root = layout.cluster_lba(layout.root_cluster) as usize * 512;
    let at = root + file.location.offset as usize;
    let n = (file.secondary_count as usize + 1) * ENTRY_SIZE;
    bytes[at + 32 + 1] &= !STREAM_NO_FAT_CHAIN;
    bytes[at + 32 + 24..at + 32 + 32].copy_from_slice(&(1u64 << 40).to_le_bytes());
    bytes[at + 32 + 8..at + 32 + 16].copy_from_slice(&(1u64 << 40).to_le_bytes());
    let sum = set_checksum(&bytes[at..at + n]);
    bytes[at + 2..at + 4].copy_from_slice(&sum.to_le_bytes());
    let fat = layout.fat_lba() as usize * 512 + first as usize * 4;
    bytes[fat..fat + 4].copy_from_slice(&first.to_le_bytes());

    let dev = SliceDevice::new(&mut bytes);
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    let file = block_on(vol.find("loop.bin")).unwrap().unwrap();
    assert!(!file.no_fat_chain && file.data_length == 1 << 40);
    let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
    assert!(!report.clean && report.bad_allocations == 1, "{report:?}");
    let mut buf = [0u8; 16];
    assert!(matches!(
        block_on(vol.read(&file, 1 << 39, &mut buf)),
        Err(Error::Corrupt(_))
    ));
    assert_eq!(block_on(vol.read(&file, 0, &mut buf)).unwrap(), 16);
    block_on(vol.delete(&file)).unwrap();
    assert!(block_on(vol.find("loop.bin")).unwrap().is_none());
}

#[test]
fn mounts_an_image_from_mkfs_exfat() {
    if Command::new("mkfs.exfat").arg("-V").output().is_err() {
        eprintln!("mkfs.exfat not installed: skipping");
        return;
    }
    let (_dir, path) = temp_image("mkfs.img");
    ImageFile::create(&path, 64 * MB / 512).unwrap();
    let out = Command::new("mkfs.exfat")
        .args(["-L", "FROMMKFS", "-c", "32K", "-b", "4M"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dev = ImageFile::open(&path).unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    assert_eq!(vol.info().label.to_string(), "FROMMKFS");
    assert_eq!(vol.info().partition_lba, 0);
    assert_eq!(vol.info().cluster_bytes(), 32 * 1024);
    let report = block_on(vol.check(WalkLimits::DEFAULT)).unwrap();
    assert!(report.clean, "{report:?}");
    block_on(vol.create(&NewFile::new("ours.bin").size(MB).align_sectors(8192))).unwrap();
    block_on(vol.create(&NewFile::new("README.txt").with_data(b"written by exfat_lite"))).unwrap();
    assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
    drop(vol);
    fsck_clean(&path);
}

#[test]
fn free_space_finds_the_longest_run() {
    let mut bytes = vec![0u8; 16 * MB as usize];
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 2048,
    };
    let mut dev = SliceDevice::new(&mut bytes);
    let mut scratch = [0u8; 512];
    block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "FREE", 5),
        &mut scratch,
    ))
    .unwrap();
    let mut vol = block_on(Volume::mount(dev)).unwrap();
    let fresh = block_on(vol.free_space()).unwrap();
    assert_eq!(fresh.free_clusters, fresh.largest_run_len);
    let cb = vol.info().cluster_bytes() as u64;
    // a file in the middle splits the free space in two runs; the longer one is reported
    let mid = fresh.largest_run_first + (fresh.largest_run_len / 3) as u32;
    let f = block_on(vol.create(&NewFile::new("mid.bin").size(4 * cb).at_cluster(mid))).unwrap();
    let split = block_on(vol.free_space()).unwrap();
    assert_eq!(split.free_clusters, fresh.free_clusters - 4);
    assert_eq!(split.largest_run_first, mid + 4);
    assert_eq!(
        split.largest_run_len,
        fresh.largest_run_first as u64 + fresh.largest_run_len - (mid as u64 + 4)
    );
    block_on(vol.delete(&f)).unwrap();
}

#[test]
fn set_attributes_rewrites_the_entry_set() {
    let mut bytes = vec![0u8; 16 * MB as usize];
    let dev = SliceDevice::new(&mut bytes);
    let geometry = Geometry {
        sectors_per_cluster_shift: 3,
        boundary_unit_sectors: 2048,
    };
    let mut vol = populate(dev, geometry, "ATTR", 2 * MB);
    let ring = block_on(vol.find("logring.wlr")).unwrap().unwrap();
    let plain = block_on(vol.set_attributes(&ring, attr::ARCHIVE)).unwrap();
    assert_eq!(plain.attributes, attr::ARCHIVE);
    let found = block_on(vol.find("logring.wlr")).unwrap().unwrap();
    assert_eq!(found.attributes, attr::ARCHIVE);
    assert_eq!(found.first_cluster, ring.first_cluster);
    assert!(block_on(vol.check(WalkLimits::DEFAULT)).unwrap().clean);
    assert_eq!(
        block_on(vol.set_attributes(&found, attr::DIRECTORY)).unwrap_err(),
        Error::InvalidArgument
    );
    let back = block_on(vol.set_attributes(&found, ring.attributes)).unwrap();
    assert_eq!(back.attributes, ring.attributes);
}

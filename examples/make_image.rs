//! Make an SD-style exFAT image: MBR, a volume with the SD geometry for its size, a README.txt and a
//! preallocated contiguous ring file that leaves 128 MB free.
//!
//! ```sh
//! cargo run --features std --example make_image -- card.img 1048576   # size in KB (here 1 GB)
//! cargo run --features std --example make_image -- tiny.img 64 1 4    # 64 KB, 1 KB clusters, 4 KB boundary unit
//! ```

use exfat_lite::{
    BlockDevice, FormatParams, Geometry, ImageFile, NewFile, Volume, WalkLimits, attr, block_on,
    format,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(kb)) = (args.next(), args.next().and_then(|s| s.parse::<u64>().ok()))
    else {
        eprintln!("usage: make_image <path> <size in KB> [cluster KB] [boundary unit KB]");
        std::process::exit(2);
    };
    let sectors = kb * 2;
    let mut dev = ImageFile::create(&path, sectors).expect("create the image");
    let mut geometry = Geometry::for_capacity(dev.sector_count());
    let (cluster_kb, bu_kb) = (
        args.next().and_then(|s| s.parse::<u64>().ok()),
        args.next().and_then(|s| s.parse::<u64>().ok()),
    );
    if let (Some(cluster_kb), Some(bu_kb)) = (cluster_kb, bu_kb) {
        geometry = Geometry {
            sectors_per_cluster_shift: (cluster_kb * 2).trailing_zeros() as u8,
            boundary_unit_sectors: (bu_kb * 2) as u32,
        };
        assert!(
            geometry.is_valid(),
            "cluster and boundary unit must be powers of two, the unit at least a cluster"
        );
    }
    let mut scratch = vec![0u8; 1 << 20];
    let layout = block_on(format(
        &mut dev,
        &FormatParams::new(geometry, "LOGRING", 0x1234_5678),
        &mut scratch,
    ))
    .expect("format");
    let mut vol = block_on(Volume::mount(dev)).expect("mount");
    let readme = "This card belongs to a logring device.\r\nlogring.wlr is its ring log; read it with the logring tool.\r\n";
    block_on(vol.create(&NewFile::new("README.txt").with_data(readme.as_bytes())))
        .expect("README.txt");
    let cluster_bytes = vol.info().cluster_bytes() as u64;
    let free =
        (vol.info().cluster_count as u64 - layout.metadata_clusters() as u64 - 1) * cluster_bytes;
    let bu = geometry.boundary_unit_sectors as u64 * 512;
    let ring_size = free.saturating_sub(128 << 20) / bu * bu;
    let ring = NewFile::new("logring.wlr")
        .size(ring_size)
        .align_sectors(geometry.boundary_unit_sectors)
        .attributes(attr::READ_ONLY | attr::SYSTEM | attr::HIDDEN | attr::ARCHIVE);
    let ring = block_on(vol.create(&ring)).expect("ring file");
    let report = block_on(vol.check(WalkLimits::DEFAULT)).expect("check");
    println!(
        "{path}: {kb} KB, partition at sector {}, cluster {} KB, {} clusters, ring file {} KB at cluster {} (sector {}), check clean: {}",
        layout.partition_lba,
        cluster_bytes / 1024,
        layout.cluster_count,
        ring.data_length >> 10,
        ring.first_cluster,
        vol.info().cluster_lba(ring.first_cluster).unwrap_or(0),
        report.clean
    );
}

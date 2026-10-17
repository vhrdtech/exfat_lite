//! A valid small image with files, patched by the input: `(offset u32 LE, len u8, bytes...)` records applied
//! in order. Reaches the directory, FAT and bitmap parsers with mostly-valid structures around the damage.

#![no_main]

mod common;

use std::sync::OnceLock;

use exfat_lite::{FormatParams, Geometry, NewFile, SliceDevice, Volume, block_on, format};
use libfuzzer_sys::fuzz_target;

fn baseline() -> &'static [u8] {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE.get_or_init(|| {
        // 4 KB clusters, 64 KB boundary unit: a 1 MB image with room for a few files.
        let geometry = Geometry { sectors_per_cluster_shift: 3, boundary_unit_sectors: 128 };
        let mut bytes = vec![0u8; 1 << 20];
        let mut dev = SliceDevice::new(&mut bytes);
        let mut scratch = [0u8; 4096];
        block_on(format(&mut dev, &FormatParams::new(geometry, "FUZZ", 42), &mut scratch)).unwrap();
        let mut vol = block_on(Volume::mount(dev)).unwrap();
        block_on(vol.create(&NewFile::new("README.txt").with_data(b"fuzz me\n"))).unwrap();
        block_on(vol.create(&NewFile::new("ring.bin").size(200 * 1024).align_sectors(128))).unwrap();
        block_on(vol.create(&NewFile::new("empty"))).unwrap();
        bytes
    })
}

fuzz_target!(|data: &[u8]| {
    let mut image = baseline().to_vec();
    let mut at = 0;
    while at + 5 <= data.len() {
        let offset = u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let len = data[at + 4] as usize;
        at += 5;
        let bytes = &data[at..(at + len).min(data.len())];
        at += len;
        if offset < image.len() {
            let n = bytes.len().min(image.len() - offset);
            image[offset..offset + n].copy_from_slice(&bytes[..n]);
        }
    }
    common::exercise(&mut image);
});

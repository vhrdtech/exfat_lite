//! Shared by the fuzz targets: everything the crate does with a mounted volume, on a RAM image.

use exfat_lite::{NewFile, SliceDevice, Volume, WalkControl, WalkLimits, block_on};

/// Bounds for one fuzz iteration: enough to cover a small image, small enough to stay fast.
const LIMITS: WalkLimits = WalkLimits { max_depth: 4, max_entries: 4096 };

/// Mount `image` and exercise every reader and writer; nothing here may panic, whatever the bytes.
pub fn exercise(image: &mut [u8]) {
    let dev = SliceDevice::new(image);
    let Ok(mut vol) = block_on(Volume::mount(dev)) else { return };
    vol.set_max_dir_entries(4096);
    let _ = block_on(vol.check(LIMITS));
    let mut first = None;
    let _ = block_on(vol.walk(vol.root(), LIMITS, |_, info| {
        if first.is_none() && !info.is_dir() {
            first = Some(*info);
        }
        WalkControl::Continue
    }));
    let _ = block_on(vol.find("README.txt"));
    let _ = block_on(vol.find("dir/sub/file"));
    if let Some(file) = first {
        let mut buf = [0u8; 1024];
        let _ = block_on(vol.read(&file, 0, &mut buf));
        let _ = block_on(vol.read(&file, file.data_length.saturating_sub(100), &mut buf));
        let _ = block_on(vol.delete(&file));
    }
    let _ = block_on(vol.create(&NewFile::new("fuzz.bin").size(3000).align_sectors(8)));
    let _ = block_on(vol.create(&NewFile::new("fuzz2.txt").with_data(b"data")));
    let _ = block_on(vol.check(LIMITS));
}

//! Mount an image (bare volume or MBR), print what the volume says, list the tree, run the consistency check.
//!
//! ```sh
//! cargo run --features std --example inspect -- card.img
//! ```

use exfat_lite::{ImageFile, Volume, WalkControl, WalkLimits, block_on};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: inspect <image>");
        std::process::exit(2);
    };
    let dev = ImageFile::open_read_only(&path).expect("open the image");
    let mut vol = match block_on(Volume::mount(dev)) {
        Ok(vol) => vol,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let info = *vol.info();
    println!(
        "label {:?} serial {:#010x} partition at {} volume {} sectors cluster {} KB clusters {} root cluster {} dirty {}",
        info.label,
        info.serial,
        info.partition_lba,
        info.volume_length,
        info.cluster_bytes() / 1024,
        info.cluster_count,
        info.root_cluster,
        info.is_dirty()
    );
    let stats = block_on(vol.walk(vol.root(), WalkLimits::DEFAULT, |depth, f| {
        let kind = if f.is_dir() { "dir " } else { "file" };
        let chain = if f.no_fat_chain {
            "contiguous"
        } else {
            "fat chain"
        };
        println!(
            "{:indent$}{kind} {:>12} bytes  cluster {:>8}  {chain:10}  attr {:#06x}  {}",
            "",
            f.data_length,
            f.first_cluster,
            f.attributes,
            f.name,
            indent = depth as usize * 2
        );
        WalkControl::Continue
    }))
    .expect("walk");
    println!("{stats:?}");
    let report = block_on(vol.check(WalkLimits::DEFAULT)).expect("check");
    println!("{report:?}");
    if !report.clean {
        std::process::exit(1);
    }
}

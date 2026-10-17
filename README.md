# exfat_lite

A small, `no_std`, no-`alloc` exFAT implementation for SD cards: format with SD Association geometry, mount and
check, find a file, create a preallocated contiguous (`NoFatChain`) file, and a bounded directory walk — just
enough to put a self-describing, OS-readable volume on a microcontroller's SD card. No allocation anywhere in the
default build, so it runs on devices too small for the existing `alloc`-based exFAT crates.

```sh
cargo add exfat_lite                  # no_std, no alloc
cargo add exfat_lite --features std   # adds an image-file backend and block_on, for tests and PC tools
```

```rust
use exfat_lite::{attr, BlockDevice, format, FormatParams, Geometry, NewFile, Volume, WalkLimits};

// `dev` implements BlockDevice: async read/write of 512-byte sectors (an SD driver, a RAM slice, an image).
let geometry = Geometry::for_capacity(dev.sector_count());          // boundary unit + cluster size by capacity
let mut scratch = [0u8; 4096];                                        // any multiple of 512: bigger, fewer writes
format(&mut dev, &FormatParams::new(geometry, "MYCARD", serial), &mut scratch).await?;

let mut vol = Volume::mount(dev).await?;                              // validates boot sector, checksum, bitmap, up-case
vol.create(&NewFile::new("README.txt").with_data(b"hello\n")).await?;
let ring = NewFile::new("logring.wlr")
    .size(32 << 20)                                                   // preallocated, contiguous, FAT untouched
    .align_sectors(geometry.boundary_unit_sectors)                    // starts on an allocation unit boundary
    .attributes(attr::READ_ONLY | attr::SYSTEM | attr::HIDDEN);
let ring = vol.create(&ring).await?;
let lba = vol.info().cluster_lba(ring.first_cluster);                 // write it by raw sector address from here

let found = vol.find("readme.TXT").await?;                            // case-insensitive, through the up-case table
let report = vol.check(WalkLimits::DEFAULT).await?;                   // bounded: depth and entry limits
assert!(report.clean);
```

What it does, and what it deliberately doesn't:

- **Format**: MBR with one exFAT partition on the first boundary unit (or a bare volume), main and backup boot
  regions, FAT, allocation bitmap, the canonical up-case table (checksum `0xE619D30D`), root directory, label.
  Rendered sector by sector, so a 2 TB card formats through a 512-byte buffer.
- **Mount**: finds the volume, validates every boot sector field against the device and the others, checks the
  boot checksum, locates the bitmap, up-case table and label. Every loop over card data is bounded by the volume's
  own geometry or a caller limit; a crafted card gets an error, never a panic or a hang.
- **Files**: find by path, read, create preallocated contiguous files (with optional initial data, attributes,
  alignment), delete. No general write, append or truncate (see FEATURES.md FS-3): the one big file is written by
  its owner by raw sector address, past the filesystem, which is the point.
- **Walk and check**: a depth- and count-bounded tree walk; `check()` verifies the backup boot region, the
  up-case table, each file's clusters against the bitmap and the bitmap against the tree.
- **Devices**: `BlockDevice` is an async trait (SD drivers under embassy are async; a synchronous driver's
  `async fn`s just return). `SliceDevice` wraps RAM; with `std`, `ImageFile` wraps a sparse image file and
  `block_on` drives the futures.

Verified with `fsck.exfat -n` (exfatprogs) on 1 to 128 GB images and a Linux kernel mount; see FEATURES.md for the
record. Windows and macOS checks are pending.

Built for [logring](https://github.com/vhrdtech/logring) (a power-cut-safe ring log store), which puts its ring
file on a normal exFAT volume so the card stays readable by any desktop OS; usable on its own by anyone who needs
a no-alloc exFAT for a microcontroller SD card.

See [AGENTS.md](AGENTS.md) for the design, [FEATURES.md](FEATURES.md) for what is implemented and planned, and
[CHANGELOG.md](CHANGELOG.md) for the history.

Licensed under MIT OR Apache-2.0.

//! `exfat_lite`: a small, no-alloc exFAT for SD cards.
//!
//! Only what [logring](https://github.com/vhrdtech/logring) needs, and nothing else assumed about the caller:
//! format with SD Association geometry, mount and check, find a file, create a preallocated contiguous
//! (`NoFatChain`) file, a bounded directory walk, and (`synth` module, later) a read-only virtual exFAT volume
//! rendered sector by sector from a directory model (ghostfat-style). Untrusted-input parser: no panics on a
//! garbage card. See `AGENTS.md` and `FEATURES.md`.
//!
//! The block device is [`BlockDevice`], an async trait over 512-byte sectors (SD drivers under embassy are
//! async; a synchronous driver implements it with `async fn`s that return at once). Nothing here allocates: a
//! [`Volume`] holds one sector buffer, a walk a fixed stack of [`MAX_WALK_DEPTH`] directory cursors.
//!
//! ```no_run
//! # async fn demo() -> Result<(), exfat_lite::Error<exfat_lite::OutOfRange>> {
//! use exfat_lite::{BlockDevice, format, FormatParams, Geometry, NewFile, SliceDevice, Volume};
//! let mut bytes = vec![0u8; 64 << 20];
//! let mut dev = SliceDevice::new(&mut bytes);
//! let mut scratch = [0u8; 4096];
//! let params = FormatParams::new(Geometry::for_capacity(dev.sector_count()), "MYCARD", 0x1234_5678);
//! format(&mut dev, &params, &mut scratch).await?;
//! let mut vol = Volume::mount(dev).await?;
//! let readme = vol.create(&NewFile::new("README.txt").with_data(b"hello\n")).await?;
//! let ring = vol.create(&NewFile::new("logring.wlr").size(32 << 20).align_sectors(8192)).await?;
//! assert!(vol.find("readme.TXT").await?.is_some());
//! # let _ = (readme, ring);
//! # Ok(()) }
//! ```
#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

pub mod boot;
pub mod device;
pub mod dir;
pub mod error;
pub mod format;
pub mod geometry;
#[cfg(feature = "std")]
pub mod image;
pub mod upcase;
pub mod volume;

pub use device::{BlockDevice, OutOfRange, SECTOR_SIZE, Sector, SliceDevice};
pub use dir::{EntryLocation, FileInfo, Name, Timestamp, attr};
pub use error::{BootProblem, Corrupt, Error, Limit, Unsupported};
pub use format::{FormatParams, format};
pub use geometry::{Geometry, Layout};
#[cfg(feature = "std")]
pub use image::{ImageFile, block_on};
pub use volume::{
    CheckReport, DirCursor, MAX_WALK_DEPTH, NewFile, Volume, VolumeInfo, WalkControl, WalkLimits,
};

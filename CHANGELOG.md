# Changelog

## [Unreleased]

### Added

- `Volume::free_space` (free clusters and the longest free run, one bitmap pass) and `Volume::set_attributes` /
  `set_attributes_in` (new attribute bits, set checksum rewritten) (FS-4, P2628#16oct-1001d).
- Format an SD card or image as MBR + exFAT with the SD Association geometry (boundary unit and cluster size by
  capacity, or the card's own allocation unit), both boot regions with checksums, FAT, allocation bitmap, the
  canonical up-case table (checksum `0xE619D30D`, kept as about 1 KB of range rules), root directory and volume
  label; everything rendered sector by sector through a caller-provided scratch buffer, no allocation (FS-1).
- Mount with checks: MBR or bare volume, boot sector fields against the device and each other, boot region
  checksum, bitmap and up-case entries located and validated; `check()` verifies the backup boot region, the
  up-case table checksum, every file's clusters against the bitmap and the bitmap's count against the tree (FS-1).
- Find a file by path (case-insensitive through the up-case table), read file data, a bounded directory walk
  (depth and entry limits, damaged entry sets counted and skipped), directory iteration (FS-1).
- Create a preallocated contiguous `NoFatChain` file with optional initial data, attributes, timestamp and
  alignment to a boundary unit (the ring file logring needs), delete a file; the root directory grows by a
  cluster when full (FS-1).
- `BlockDevice`: an async trait over 512-byte sectors, with `SliceDevice` (RAM) for `no_std` and, with `std`,
  `ImageFile` (sparse image files) and `block_on` (FS-1).
- Examples `make_image` (an SD-style image with README.txt and a ring file) and `inspect` (mount, list, check);
  fuzz targets `mount` (garbage images) and `mount_patched` (a valid image with byte patches), `just fuzz` (FS-1).
- Repo scaffold: crate skeleton (`no_std`, `std` feature), `just test`/`lint`/`install`/`deploy`, CI (fmt,
  clippy, tests, typos, cargo sort), licence (MIT OR Apache-2.0) (P2628#16oct-1016).
- FEATURES.md with the planned items and ideas of `prj/P2628-logring/design/plan.md`: area `FS` (FS-1..3), each
  bound to its P2628 task where one exists (P2628#16oct-1016).
- `just test --changed` runs only the workspace crates changed against main and their dependents (`tpm test changed`, nextest's `rdeps()`); plain `just test` runs everything, as `just test-all` does (tpm ops#22oct-2049).


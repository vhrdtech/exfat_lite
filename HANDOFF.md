# Handoff: exfat_lite FS-1 format-mount (P2628#16oct-1000b)

Session exfat-lite, branch `s/17oct2026-exfat-lite`, 17 Oct 2026.

## Done

- The whole FS-1 scope: format (MBR + exFAT, SD geometry, up-case table, bitmap, root, label), mount + checks, find by path, read, bounded walk, `check()`, create contiguous `NoFatChain` files (with data: README.txt), delete, root directory growth, `SliceDevice` / `ImageFile` / `block_on`, examples `make_image` and `inspect`, fuzz targets `mount` and `mount_patched` with seeds.
- Verified: `fsck.exfat -n` clean on 1/8/32/128 GB images and on a `mkfs.exfat` image this crate wrote files into; Linux kernel exFAT mount (udisks loop device, no root) read both files and wrote one that `check()` accepted; fuzzers ran minutes without a panic. See FEATURES.md FS-1.

## Next (for R6 logring-on-exfat, not this crate)

- `Volume::find`, `FileInfo` (first_cluster, data_length, valid_data_length, no_fat_chain, attributes), `VolumeInfo::serial`, `Volume::is_allocated(first, count)` and `VolumeInfo::cluster_lba` are the binding check inputs; `delete` + `create` with `at_cluster` re-create the ring file in place.
- Windows chkdsk and macOS Disk Utility checks on a real card are a person's step.

## Decisions

- `BlockDevice` is async (`async fn` in trait, `#[allow(async_fn_in_trait)]`): embassy SD drivers are async; a sync driver returns at once. `block_on` (std) spins with a noop waker: fine for devices whose futures never pend.
- Format renders every metadata sector from the layout (`format::Renderer`): no RAM for the bitmap or FAT, and the same shape FS-2 `synth` needs.
- The up-case table is 119 range rules (`upcase::SEGMENTS`, about 1 KB) extracted from the canonical table; the on-disk compressed stream (5836 bytes) is regenerated from them with the canonical identity runs (`upcase::RUNS`), and a test pins the checksum to `0xE619D30D`.
- Layout: partition at one boundary unit (BU), FAT at 24 sectors into the partition, cluster heap on the next BU after the FAT, so cluster boundaries that are BU multiples are BU-aligned on the card. `fsck.exfat` cannot read an image that starts with an MBR: the tests wrap the image so the file holds only the partition (`PartitionView` in `tests/volume.rs`).
- Geometry table: SD spec SDXC rows (16/32/64 MB BU with 128/256/512 KB clusters); up to 32 GB uses the SDHC row (4 MB BU, 32 KB clusters). Callers pass the card's own AU to override.
- Only the root directory grows when full; a subdirectory gives `DirectoryFull` (growing it means rewriting its stream entry in the parent, out of FS-1 scope). Subdirectory creation is not implemented either.
- `Entry` is a large enum (a `FileInfo` carries its 255-unit name inline): `large_enum_variant` allowed, no alloc to box it.

## Gotchas

- An empty file must have stream flags 0 (no `NoFatChain`): fsck flags "empty, but has no Fat chain" otherwise.
- Checksums use wrapping arithmetic: the spec's C code overflows by design; a non-wrapping `+` panics in debug builds.
- The walk's `entries` count includes the end-of-directory marker it read.
- `fsck.exfat -n` on a loop partition needs read access to the device node (root); use the image file (bare volume) instead.

# Features

Single source of truth for what exfat_lite does and what is planned (see [AGENTS.md](AGENTS.md)).

Status: ✅ done, 🧪 tested on fixtures/images only, 📋 planned (with a P2628 task), 💡 idea, not filed as a task yet.

## exFAT (`FS`)

- ✅ **FS-1 format-mount**: MBR + exFAT format with SD Association geometry, up-case table, allocation bitmap,
  root directory, volume label, files with content (`README.txt`); mount + checks (boot sector checksum,
  directory entry set checksum, bitmap consistency); find an entry by path; create a contiguous `NoFatChain`
  file, delete a file; a bounded directory walk; `std` image backend. Untrusted-input parser: no panics on
  garbage. P2628#16oct-1000b. Plan: `prj/P2628-logring/design/plan.md` R4.
  Verified 17 Oct 2026 on gpd-omarchy: `fsck.exfat -n` (exfatprogs 1.4.3) clean on 1, 8, 32 and 128 GB images
  with README.txt and an aligned ring file, and on an image `mkfs.exfat` made that this crate added files to;
  Linux 7.2 kernel exFAT mount (udisks loop device) of a 1 GB image listed both files with their sizes and
  attributes, read README.txt, and a file the host wrote passed this crate's `check()` afterwards; two
  cargo-fuzz targets (garbage images, patched valid images) ran without a panic. Not yet checked: Windows
  chkdsk, macOS Disk Utility (a person's step with a real card).
- 🧪 **FS-4 ring-file-upkeep**: what a device that writes a preallocated file by raw sector address needs to
  keep it: `Volume::free_space` (free clusters and the longest free run in one bitmap pass, to place a ring
  file on a card that already holds files) and `Volume::set_attributes` (rewrite a file's attribute bits and
  its set checksum, to restore read-only/system/hidden). For `logring`'s LOG-3. P2628#16oct-1001d.
- 📋 **FS-2 synth-virtual-volume**: `synth` — a read-only virtual exFAT volume rendered sector by sector from a
  directory model the caller provides (ghostfat-style), paired with `logring`'s VIEW-1. After FS-1. Needs
  `logring`'s LOG-2. P2628#16oct-1001f. Plan R8.
- 💡 **FS-3 rw-file-ops-ww-files**: device-side rw file operations (write, truncate, append in the small exFAT
  region) for `wire_weaver`'s `ww_files` (P2424 IF-8), needed once a firmware writes its own files (config,
  exports).

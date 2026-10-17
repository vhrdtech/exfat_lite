# Features

Single source of truth for what exfat_lite does and what is planned (see [AGENTS.md](AGENTS.md)).

Status: ✅ done, 🧪 tested on fixtures/images only, 📋 planned (with a P2628 task), 💡 idea, not filed as a task yet.

## exFAT (`FS`)

- 📋 **FS-1 format-mount**: MBR + exFAT format with SD Association geometry, up-case table, allocation bitmap,
  root directory, volume label, `README.txt`; mount + checks (boot sector checksum, directory entry set
  checksum, bitmap consistency); find an entry by path; create a contiguous `NoFatChain` file; a bounded
  directory walk; `std` image backend. Untrusted-input parser: no panics on garbage. P2628#16oct-1000b. Plan:
  `prj/P2628-logring/design/plan.md` R4.
- 📋 **FS-2 synth-virtual-volume**: `synth` — a read-only virtual exFAT volume rendered sector by sector from a
  directory model the caller provides (ghostfat-style), paired with `logring`'s VIEW-1. After FS-1. Needs
  `logring`'s LOG-2. P2628#16oct-1001f. Plan R8.
- 💡 **FS-3 rw-file-ops-ww-files**: device-side rw file operations (write, truncate, append in the small exFAT
  region) for `wire_weaver`'s `ww_files` (P2424 IF-8), needed once a firmware writes its own files (config,
  exports).

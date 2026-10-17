# Working on exfat_lite

Guidance for AI agents and contributors. Read this before changing code.

## What this is

`exfat_lite` is a small, `no_std`, no-`alloc` exFAT implementation for SD cards: just enough to format a card, walk
and check its directory, find a file, and create a preallocated contiguous (`NoFatChain`) file — the subset
[logring](https://github.com/vhrdtech/logring) needs to put its `logring.wlr` ring file on a normal exFAT volume so
the card stays readable by any desktop OS. It knows nothing about logring or `wire_weaver`: no existing Rust crate
covered this (`embedded-exfat` needs `alloc` and `spin`; `hadris-fat`'s exFAT support is an unstable preview,
checked 16 Oct 2026), so it is its own repo, usable by anyone who needs a no-alloc exFAT on a microcontroller.

A card is untrusted input: every parser here is bounded (no unbounded loops over a count a bad card can set
arbitrarily high) and never panics on garbage, by design, not by luck — see "Tests" below.

Sibling repos (tpm project P2628; design: `prj/P2628-logring/design/plan.md` in the tpm folder, not here):
[`logring`](https://github.com/vhrdtech/logring) (the ring log store that uses this crate),
[`usb_msc_device`](https://github.com/vhrdtech/usb_msc_device) (the USB disk class logring serves over).

## FEATURES.md is the source of truth

[FEATURES.md](FEATURES.md) lists every feature with its status, every known bug, and what is planned, area `FS`.

- **Name IDs with a short slug when talking to the user**: `FS-1 format-mount`, never a bare `FS-1`. Commit
  messages, CHANGELOG and code `TODO`s keep the bare ID.
- **New IDs**: `tpm work claim FS` prints the next free one. Never renumber or reuse IDs.
- Status ✅ verified against `fsck.exfat`/a real OS mount; 🧪 unit-tested/fuzzed on images only; 📋 planned with a
  P2628 task; 💡 an idea, not filed as a task yet.

## CHANGELOG.md records every change

Every change a user would notice gets an entry under `## [Unreleased]` in the same commit (`### Added`,
`### Changed`, `### Fixed`, `### Removed`), short and user-facing, with the feature ID in parentheses. Pure
refactors and typo fixes don't need one.

## Layout

Single crate at the repo root (`src/`), `no_std` by default, `std` feature for an image-file backend (used by
tests, by `logring`'s simulated card, and later by a `make-image` CLI step):

- format: MBR, boot region (main + backup), up-case table, allocation bitmap, root directory, volume label, the
  per-capacity geometry table from the SD Association spec;
- mount + checks: boot sector checksum, directory entry set checksum, bitmap consistency, bounded;
- find an entry by path; create a contiguous `NoFatChain` file (what logring's ring file needs);
- a bounded directory walk (depth and entry count capped, so a crafted card can't hang a caller);
- `synth` (later, R8): a read-only virtual exFAT volume rendered sector by sector from a directory model the
  caller provides (ghostfat-style), so a USB host sees files that don't exist as real exFAT clusters.

## Commands

```sh
just test    # cargo test --features std (host) + a no_std build check for the default build
just lint    # fmt --check, clippy -D warnings (std and no_std)
just install # library only: nothing to install on this PC
just deploy  # bring this PC up to main
```

Validating against real tools (`fsck.exfat` from exfatprogs, a Linux kernel mount, Windows chkdsk, macOS Disk
Utility) needs those tools on a PC; say in the report which checks ran against them versus only against this
crate's own parser round-tripping its own images.

## Dependencies

Pure Rust only, no `-sys` crates, no `alloc`. Check new dependencies with `cargo tree`.

## Code conventions

- Errors: a `no_std`-friendly typed enum (`thiserror` with `default-features = false` once there's a reason for
  it; until then plain enums). No `unwrap`/`expect` on anything read from the card.
- Every loop over card-controlled data (directory entries, FAT chain length, bitmap bits) has an explicit bound
  from the volume's own geometry, never from a count read off the card alone.

## Tests

- Unit tests for format/mount/walk against crafted and fuzzed images (`std` feature, in-memory or temp files).
- A fuzz target for mount (garbage boot sectors, corrupted directory entries, truncated images): no panics, no
  unbounded loops.
- `fsck.exfat -n` (exfatprogs) against generated images of a few capacities, run in CI once available; a real
  Linux/Windows/macOS mount is a person's step, recorded in FEATURES.md with what was checked.
- A bug fix starts with a failing test when the bug can be reproduced in one; a parser bug gets a fuzz corpus
  entry, not just a hand-written regression test.

## Commits

Conventional Commits with a scope: `feat(format): ...`, `fix(mount): ...`, scopes `format`, `mount`, `walk`,
`synth`, `build`. Short imperative summary, blank line, body with what and why; reference feature IDs.

Commit on your own initiative on a session branch (`tpm work new SLUG exfat_lite`), each finished step one commit
with FEATURES.md and CHANGELOG.md updated in it. When the work is done and the user agrees, `tpm land` puts it on
main.

## Versions

Landing bumps the version, not each work commit (tpm CLAUDE.md "Landing bumps the version"): session commits add
CHANGELOG entries under `[Unreleased]` without bumping. This repo's version is its next crates.io release
(`bump = "at-publish"` in tpm's repos.toml): `tpm land` never bumps or tags it on its own; `tpm release exfat_lite`
does, once there is something to publish.

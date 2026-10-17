# exfat_lite

A small, `no_std`, no-`alloc` exFAT implementation for SD cards: format with SD Association geometry, mount and
check, find a file, create a preallocated contiguous (`NoFatChain`) file, and a bounded directory walk — just
enough to put a self-describing, OS-readable volume on a microcontroller's SD card. No allocation anywhere in the
default build, so it runs on devices too small for the existing `alloc`-based exFAT crates.

```sh
cargo add exfat_lite              # no_std, no alloc
cargo add exfat_lite --features std   # adds an image-file backend, for tests and PC tools
```

Built for [logring](https://github.com/vhrdtech/logring) (a power-cut-safe ring log store), which puts its ring
file on a normal exFAT volume so the card stays readable by any desktop OS; usable on its own by anyone who needs
a no-alloc exFAT for a microcontroller SD card.

See [AGENTS.md](AGENTS.md) for the design, [FEATURES.md](FEATURES.md) for what is implemented and planned, and
[CHANGELOG.md](CHANGELOG.md) for the history.

Licensed under MIT OR Apache-2.0.

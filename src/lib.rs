//! `exfat_lite`: a small, no-alloc exFAT for SD cards.
//!
//! Only what [logring](https://github.com/vhrdtech/logring) needs, and nothing else assumed about the caller:
//! format with SD Association geometry, mount and check, find a file, create a preallocated contiguous
//! (`NoFatChain`) file, a bounded directory walk, and (`synth` module, later) a read-only virtual exFAT volume
//! rendered sector by sector from a directory model (ghostfat-style). Untrusted-input parser: no panics on a
//! garbage card. See `AGENTS.md` and `FEATURES.md`; the format and mount land in R4 (P2628#16oct-1000b).
#![cfg_attr(not(feature = "std"), no_std)]

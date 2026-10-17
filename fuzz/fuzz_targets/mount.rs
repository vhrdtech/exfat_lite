//! The input bytes are the whole image: garbage, truncated, tiny.

#![no_main]

mod common;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut image = data.to_vec();
    common::exercise(&mut image);
});

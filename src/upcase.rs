//! The up-case table: how exFAT compares file names case-insensitively, and the table a volume carries.
//!
//! exFAT stores every name as it was given (UTF-16, case kept) and compares names through the volume's up-case
//! table. This crate uses the table Microsoft's exFAT specification recommends (§7.2.5.1; the one every
//! Windows/Linux/macOS format tool writes, checksum `0xE619D30D`): for lookups it is kept as range rules below,
//! about 1 KB instead of the 128 KB table; the on-disk compressed form (5836 bytes) is regenerated from the same
//! rules when a volume is formatted, which [`UPCASE_CHECKSUM`] pins to the canonical bytes.

/// Checksum of the recommended up-case table in its compressed on-disk form (exFAT spec §7.2.5.1).
pub const UPCASE_CHECKSUM: u32 = 0xE619_D30D;

/// Size of the compressed on-disk table in bytes.
pub const UPCASE_TABLE_LEN: u32 = 5836;

/// Number of 512-byte sectors the on-disk table takes.
pub const UPCASE_TABLE_SECTORS: u32 = UPCASE_TABLE_LEN.div_ceil(512);

/// Number of 16-bit values in the compressed stream.
const STREAM_LEN: usize = UPCASE_TABLE_LEN as usize / 2;

/// A range of code units mapped by a constant offset; `step == 2` maps every other unit from `first`.
struct Seg {
    first: u16,
    last: u16,
    delta: i16,
    step: u8,
}

/// Every non-identity mapping of the recommended table, ascending by `first`, ranges disjoint.
#[rustfmt::skip]
static SEGMENTS: [Seg; 119] = [
    Seg { first: 0x0061, last: 0x007A, delta: -32, step: 1 },
    Seg { first: 0x00E0, last: 0x00F6, delta: -32, step: 1 },
    Seg { first: 0x00F8, last: 0x00FE, delta: -32, step: 1 },
    Seg { first: 0x00FF, last: 0x00FF, delta: 121, step: 1 },
    Seg { first: 0x0101, last: 0x012F, delta: -1, step: 2 },
    Seg { first: 0x0133, last: 0x0137, delta: -1, step: 2 },
    Seg { first: 0x013A, last: 0x0148, delta: -1, step: 2 },
    Seg { first: 0x014B, last: 0x0177, delta: -1, step: 2 },
    Seg { first: 0x017A, last: 0x017E, delta: -1, step: 2 },
    Seg { first: 0x0180, last: 0x0180, delta: 195, step: 1 },
    Seg { first: 0x0183, last: 0x0185, delta: -1, step: 2 },
    Seg { first: 0x0188, last: 0x0188, delta: -1, step: 1 },
    Seg { first: 0x018C, last: 0x018C, delta: -1, step: 1 },
    Seg { first: 0x0192, last: 0x0192, delta: -1, step: 1 },
    Seg { first: 0x0195, last: 0x0195, delta: 97, step: 1 },
    Seg { first: 0x0199, last: 0x0199, delta: -1, step: 1 },
    Seg { first: 0x019A, last: 0x019A, delta: 163, step: 1 },
    Seg { first: 0x019E, last: 0x019E, delta: 130, step: 1 },
    Seg { first: 0x01A1, last: 0x01A5, delta: -1, step: 2 },
    Seg { first: 0x01A8, last: 0x01A8, delta: -1, step: 1 },
    Seg { first: 0x01AD, last: 0x01AD, delta: -1, step: 1 },
    Seg { first: 0x01B0, last: 0x01B0, delta: -1, step: 1 },
    Seg { first: 0x01B4, last: 0x01B6, delta: -1, step: 2 },
    Seg { first: 0x01B9, last: 0x01B9, delta: -1, step: 1 },
    Seg { first: 0x01BD, last: 0x01BD, delta: -1, step: 1 },
    Seg { first: 0x01BF, last: 0x01BF, delta: 56, step: 1 },
    Seg { first: 0x01C6, last: 0x01C6, delta: -2, step: 1 },
    Seg { first: 0x01C9, last: 0x01C9, delta: -2, step: 1 },
    Seg { first: 0x01CC, last: 0x01CC, delta: -2, step: 1 },
    Seg { first: 0x01CE, last: 0x01DC, delta: -1, step: 2 },
    Seg { first: 0x01DD, last: 0x01DD, delta: -79, step: 1 },
    Seg { first: 0x01DF, last: 0x01EF, delta: -1, step: 2 },
    Seg { first: 0x01F3, last: 0x01F3, delta: -2, step: 1 },
    Seg { first: 0x01F5, last: 0x01F5, delta: -1, step: 1 },
    Seg { first: 0x01F9, last: 0x021F, delta: -1, step: 2 },
    Seg { first: 0x0223, last: 0x0233, delta: -1, step: 2 },
    Seg { first: 0x023A, last: 0x023A, delta: 10795, step: 1 },
    Seg { first: 0x023C, last: 0x023C, delta: -1, step: 1 },
    Seg { first: 0x023E, last: 0x023E, delta: 10792, step: 1 },
    Seg { first: 0x0242, last: 0x0242, delta: -1, step: 1 },
    Seg { first: 0x0247, last: 0x024F, delta: -1, step: 2 },
    Seg { first: 0x0253, last: 0x0253, delta: -210, step: 1 },
    Seg { first: 0x0254, last: 0x0254, delta: -206, step: 1 },
    Seg { first: 0x0256, last: 0x0257, delta: -205, step: 1 },
    Seg { first: 0x0259, last: 0x0259, delta: -202, step: 1 },
    Seg { first: 0x025B, last: 0x025B, delta: -203, step: 1 },
    Seg { first: 0x0260, last: 0x0260, delta: -205, step: 1 },
    Seg { first: 0x0263, last: 0x0263, delta: -207, step: 1 },
    Seg { first: 0x0268, last: 0x0268, delta: -209, step: 1 },
    Seg { first: 0x0269, last: 0x0269, delta: -211, step: 1 },
    Seg { first: 0x026B, last: 0x026B, delta: 10743, step: 1 },
    Seg { first: 0x026F, last: 0x026F, delta: -211, step: 1 },
    Seg { first: 0x0272, last: 0x0272, delta: -213, step: 1 },
    Seg { first: 0x0275, last: 0x0275, delta: -214, step: 1 },
    Seg { first: 0x027D, last: 0x027D, delta: 10727, step: 1 },
    Seg { first: 0x0280, last: 0x0280, delta: -218, step: 1 },
    Seg { first: 0x0283, last: 0x0283, delta: -218, step: 1 },
    Seg { first: 0x0288, last: 0x0288, delta: -218, step: 1 },
    Seg { first: 0x0289, last: 0x0289, delta: -69, step: 1 },
    Seg { first: 0x028A, last: 0x028B, delta: -217, step: 1 },
    Seg { first: 0x028C, last: 0x028C, delta: -71, step: 1 },
    Seg { first: 0x0292, last: 0x0292, delta: -219, step: 1 },
    Seg { first: 0x037B, last: 0x037D, delta: 130, step: 1 },
    Seg { first: 0x03AC, last: 0x03AC, delta: -38, step: 1 },
    Seg { first: 0x03AD, last: 0x03AF, delta: -37, step: 1 },
    Seg { first: 0x03B1, last: 0x03C1, delta: -32, step: 1 },
    Seg { first: 0x03C2, last: 0x03C2, delta: -31, step: 1 },
    Seg { first: 0x03C3, last: 0x03CB, delta: -32, step: 1 },
    Seg { first: 0x03CC, last: 0x03CC, delta: -64, step: 1 },
    Seg { first: 0x03CD, last: 0x03CE, delta: -63, step: 1 },
    Seg { first: 0x03D9, last: 0x03EF, delta: -1, step: 2 },
    Seg { first: 0x03F2, last: 0x03F2, delta: 7, step: 1 },
    Seg { first: 0x03F8, last: 0x03F8, delta: -1, step: 1 },
    Seg { first: 0x03FB, last: 0x03FB, delta: -1, step: 1 },
    Seg { first: 0x0430, last: 0x044F, delta: -32, step: 1 },
    Seg { first: 0x0450, last: 0x045F, delta: -80, step: 1 },
    Seg { first: 0x0461, last: 0x0481, delta: -1, step: 2 },
    Seg { first: 0x048B, last: 0x04BF, delta: -1, step: 2 },
    Seg { first: 0x04C2, last: 0x04CE, delta: -1, step: 2 },
    Seg { first: 0x04CF, last: 0x04CF, delta: -15, step: 1 },
    Seg { first: 0x04D1, last: 0x0513, delta: -1, step: 2 },
    Seg { first: 0x0561, last: 0x0586, delta: -48, step: 1 },
    Seg { first: 0x1D7D, last: 0x1D7D, delta: 3814, step: 1 },
    Seg { first: 0x1E01, last: 0x1E95, delta: -1, step: 2 },
    Seg { first: 0x1EA1, last: 0x1EF9, delta: -1, step: 2 },
    Seg { first: 0x1F00, last: 0x1F07, delta: 8, step: 1 },
    Seg { first: 0x1F10, last: 0x1F15, delta: 8, step: 1 },
    Seg { first: 0x1F20, last: 0x1F27, delta: 8, step: 1 },
    Seg { first: 0x1F30, last: 0x1F37, delta: 8, step: 1 },
    Seg { first: 0x1F40, last: 0x1F45, delta: 8, step: 1 },
    Seg { first: 0x1F51, last: 0x1F57, delta: 8, step: 2 },
    Seg { first: 0x1F60, last: 0x1F67, delta: 8, step: 1 },
    Seg { first: 0x1F70, last: 0x1F71, delta: 74, step: 1 },
    Seg { first: 0x1F72, last: 0x1F75, delta: 86, step: 1 },
    Seg { first: 0x1F76, last: 0x1F77, delta: 100, step: 1 },
    Seg { first: 0x1F78, last: 0x1F79, delta: 128, step: 1 },
    Seg { first: 0x1F7A, last: 0x1F7B, delta: 112, step: 1 },
    Seg { first: 0x1F7C, last: 0x1F7D, delta: 126, step: 1 },
    Seg { first: 0x1F80, last: 0x1F87, delta: 8, step: 1 },
    Seg { first: 0x1F90, last: 0x1F97, delta: 8, step: 1 },
    Seg { first: 0x1FA0, last: 0x1FA7, delta: 8, step: 1 },
    Seg { first: 0x1FB0, last: 0x1FB1, delta: 8, step: 1 },
    Seg { first: 0x1FB3, last: 0x1FB3, delta: 9, step: 1 },
    Seg { first: 0x1FCC, last: 0x1FCC, delta: -9, step: 1 },
    Seg { first: 0x1FD0, last: 0x1FD1, delta: 8, step: 1 },
    Seg { first: 0x1FE0, last: 0x1FE1, delta: 8, step: 1 },
    Seg { first: 0x1FE5, last: 0x1FE5, delta: 7, step: 1 },
    Seg { first: 0x1FFC, last: 0x1FFC, delta: -9, step: 1 },
    Seg { first: 0x214E, last: 0x214E, delta: -28, step: 1 },
    Seg { first: 0x2170, last: 0x217F, delta: -16, step: 1 },
    Seg { first: 0x2184, last: 0x2184, delta: -1, step: 1 },
    Seg { first: 0x24D0, last: 0x24E9, delta: -26, step: 1 },
    Seg { first: 0x2C30, last: 0x2C5E, delta: -48, step: 1 },
    Seg { first: 0x2C61, last: 0x2C61, delta: -1, step: 1 },
    Seg { first: 0x2C68, last: 0x2C6C, delta: -1, step: 2 },
    Seg { first: 0x2C76, last: 0x2C76, delta: -1, step: 1 },
    Seg { first: 0x2C81, last: 0x2CE3, delta: -1, step: 2 },
    Seg { first: 0x2D00, last: 0x2D25, delta: -7264, step: 1 },
    Seg { first: 0xFF41, last: 0xFF5A, delta: -32, step: 1 },
];

/// Map one UTF-16 code unit to its upper-case form per the recommended table (identity where it has none).
pub fn upcase(unit: u16) -> u16 {
    // Binary search for the last segment with first <= unit.
    let (mut lo, mut hi) = (0usize, SEGMENTS.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if SEGMENTS[mid].first <= unit {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return unit;
    }
    let seg = &SEGMENTS[lo - 1];
    if unit > seg.last || (seg.step == 2 && !(unit - seg.first).is_multiple_of(2)) {
        return unit;
    }
    (unit as i32 + seg.delta as i32) as u16
}

/// Identity runs the canonical compressed stream encodes as `(0xFFFF, count)`; everything else is literal.
/// `(first code unit, count)`.
const RUNS: [(u16, u16); 4] = [
    (0x0587, 6134),
    (0x2185, 843),
    (0x24EA, 1862),
    (0x2D26, 53787),
];

/// Fill `out` with 512-byte sector `index` (0-based) of the compressed on-disk table; the tail of the last
/// sector is zero. Returns `false` (and clears `out`) for an index past the table.
pub fn render_sector(index: u32, out: &mut [u8; 512]) -> bool {
    out.fill(0);
    if index >= UPCASE_TABLE_SECTORS {
        return false;
    }
    let first = index as usize * 256;
    let mut stream_pos = 0usize; // next value index in the compressed stream
    let mut unit = 0u32; // next code unit to encode
    let mut run = 0usize;
    while stream_pos < STREAM_LEN && stream_pos < first + 256 {
        let (value, advance_units, values) = if run < RUNS.len() && RUNS[run].0 as u32 == unit {
            let r = RUNS[run];
            run += 1;
            (None, r.1 as u32, 2usize)
        } else {
            (Some(upcase(unit as u16)), 1, 1)
        };
        for v in 0..values {
            if stream_pos + v >= first && stream_pos + v < first + 256 {
                let word = match value {
                    Some(w) => w,
                    None if v == 0 => 0xFFFF,
                    None => RUNS[run - 1].1,
                };
                let at = (stream_pos + v - first) * 2;
                out[at..at + 2].copy_from_slice(&word.to_le_bytes());
            }
        }
        stream_pos += values;
        unit += advance_units;
    }
    true
}

/// Running exFAT table checksum (spec §7.2.5.2) over `bytes`, continuing from `sum`.
pub fn checksum_step(mut sum: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        sum = (if sum & 1 != 0 { 0x8000_0000u32 } else { 0 })
            .wrapping_add(sum >> 1)
            .wrapping_add(b as u32);
    }
    sum
}

/// Checksum of the whole compressed table as this crate renders it.
pub fn table_checksum() -> u32 {
    let mut sum = 0;
    let mut sector = [0u8; 512];
    let mut left = UPCASE_TABLE_LEN as usize;
    for i in 0..UPCASE_TABLE_SECTORS {
        render_sector(i, &mut sector);
        let n = left.min(512);
        sum = checksum_step(sum, &sector[..n]);
        left -= n;
    }
    sum
}

/// Expand the compressed stream `bytes` (as read off a volume) into a callback per `(unit, upcased)` pair,
/// bounded: at most 65536 units are produced, whatever the stream says.
#[cfg(test)]
fn expand(bytes: &[u8], mut f: impl FnMut(u16, u16)) {
    let mut unit = 0u32;
    let mut i = 0;
    while i + 1 < bytes.len() && unit < 0x10000 {
        let v = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        i += 2;
        if v == 0xFFFF && i + 1 < bytes.len() {
            let n = u16::from_le_bytes([bytes[i], bytes[i + 1]]) as u32;
            i += 2;
            for u in unit..(unit + n).min(0x10000) {
                f(u as u16, u as u16);
            }
            unit += n;
        } else {
            f(unit as u16, v);
            unit += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_latin() {
        assert_eq!(upcase(b'a' as u16), b'A' as u16);
        assert_eq!(upcase(b'z' as u16), b'Z' as u16);
        assert_eq!(upcase(b'A' as u16), b'A' as u16);
        assert_eq!(upcase(b'0' as u16), b'0' as u16);
        assert_eq!(upcase(0x00E9), 0x00C9); // é -> É
        assert_eq!(upcase(0x00FF), 0x0178); // ÿ -> Ÿ
        assert_eq!(upcase(0x0101), 0x0100); // ā -> Ā (step 2)
        assert_eq!(upcase(0x0100), 0x0100);
        assert_eq!(upcase(0x0430), 0x0410); // а -> А
        assert_eq!(upcase(0xFF41), 0xFF21); // fullwidth a
        assert_eq!(upcase(0xFFFF), 0xFFFF);
    }

    #[test]
    fn rendered_table_has_the_canonical_checksum() {
        assert_eq!(table_checksum(), UPCASE_CHECKSUM);
    }

    #[test]
    fn rendered_table_expands_to_the_rules() {
        let mut bytes = vec![0u8; UPCASE_TABLE_SECTORS as usize * 512];
        for i in 0..UPCASE_TABLE_SECTORS {
            let mut s = [0u8; 512];
            assert!(render_sector(i, &mut s));
            bytes[i as usize * 512..][..512].copy_from_slice(&s);
        }
        let mut s = [0u8; 512];
        assert!(!render_sector(UPCASE_TABLE_SECTORS, &mut s));
        let mut count = 0u32;
        expand(&bytes[..UPCASE_TABLE_LEN as usize], |u, up| {
            assert_eq!(up, upcase(u), "unit {u:#06x}");
            count += 1;
        });
        assert_eq!(count, 65536);
    }
}

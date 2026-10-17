//! Directory entries (spec §6, §7): the 32-byte entry formats, entry sets, timestamps, names and hashes.

use crate::boot::{le16, le32, le64, put16, put32, put64};
use crate::upcase::upcase;

/// Size of one directory entry in bytes.
pub const ENTRY_SIZE: usize = 32;

/// Entry type: end of directory (every entry after it is unused too).
pub const TYPE_END: u8 = 0x00;
/// Entry type: allocation bitmap.
pub const TYPE_BITMAP: u8 = 0x81;
/// Entry type: up-case table.
pub const TYPE_UPCASE: u8 = 0x82;
/// Entry type: volume label.
pub const TYPE_LABEL: u8 = 0x83;
/// Entry type: file or directory (primary of a file entry set).
pub const TYPE_FILE: u8 = 0x85;
/// Entry type: volume GUID (benign primary).
pub const TYPE_VOLUME_GUID: u8 = 0xA0;
/// Entry type: stream extension (first secondary of a file entry set).
pub const TYPE_STREAM: u8 = 0xC0;
/// Entry type: file name (secondaries after the stream extension).
pub const TYPE_NAME: u8 = 0xC1;

/// Entry type bit: the entry is in use (clear: a deleted or reserved slot).
pub const TYPE_IN_USE: u8 = 0x80;
/// Entry type bit: a secondary entry.
pub const TYPE_SECONDARY: u8 = 0x40;

/// Longest file name in UTF-16 code units.
pub const MAX_NAME_UNITS: usize = 255;
/// Name units per file name entry.
pub const NAME_UNITS_PER_ENTRY: usize = 15;
/// Most entries in a file entry set: file + stream + 17 names.
pub const MAX_SET_ENTRIES: usize = 2 + MAX_NAME_UNITS.div_ceil(NAME_UNITS_PER_ENTRY);
/// Most secondaries in a file entry set.
pub const MAX_SECONDARIES: u8 = (MAX_SET_ENTRIES - 1) as u8;

/// Volume label length limit in UTF-16 units.
pub const MAX_LABEL_UNITS: usize = 11;

/// File attribute bits (`FileAttributes`).
#[allow(missing_docs)]
pub mod attr {
    pub const READ_ONLY: u16 = 0x01;
    pub const HIDDEN: u16 = 0x02;
    pub const SYSTEM: u16 = 0x04;
    pub const DIRECTORY: u16 = 0x10;
    pub const ARCHIVE: u16 = 0x20;
}

/// Stream extension flag: the file's clusters are contiguous, the FAT holds nothing for them.
pub const STREAM_NO_FAT_CHAIN: u8 = 0x02;
/// Stream extension flag: the file has an allocation.
pub const STREAM_ALLOCATION_POSSIBLE: u8 = 0x01;

/// A point in time as exFAT stores it: local date and time to 10 ms, with an optional UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    /// 1980..=2107.
    pub year: u16,
    /// 1..=12.
    pub month: u8,
    /// 1..=31.
    pub day: u8,
    /// 0..=23.
    pub hour: u8,
    /// 0..=59.
    pub minute: u8,
    /// 0..=59.
    pub second: u8,
    /// Offset from UTC in 15-minute steps (-48..=52), `None` when unknown (the OS shows the time as is).
    pub utc_offset_quarters: Option<i8>,
}

impl Timestamp {
    /// 2026-01-01 00:00:00, offset unknown: what a device with no clock writes.
    pub const DEFAULT: Timestamp = Timestamp {
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        utc_offset_quarters: None,
    };

    /// `(packed date-time, 10 ms increment, UTC offset byte)` as the entries store them; fields out of range
    /// are clamped.
    pub fn encode(&self) -> (u32, u8, u8) {
        let year = self.year.clamp(1980, 2107) as u32 - 1980;
        let month = self.month.clamp(1, 12) as u32;
        let day = self.day.clamp(1, 31) as u32;
        let hour = self.hour.min(23) as u32;
        let minute = self.minute.min(59) as u32;
        let second = self.second.min(59) as u32;
        let packed = (year << 25)
            | (month << 21)
            | (day << 16)
            | (hour << 11)
            | (minute << 5)
            | (second / 2);
        let inc = ((second % 2) * 100) as u8;
        let offset = match self.utc_offset_quarters {
            Some(q) => 0x80 | (q.clamp(-48, 52) as u8 & 0x7F),
            None => 0,
        };
        (packed, inc, offset)
    }

    /// The reverse of [`Timestamp::encode`]; no range checks beyond what the bit fields hold.
    pub fn decode(packed: u32, inc: u8, offset: u8) -> Timestamp {
        Timestamp {
            year: ((packed >> 25) & 0x7F) as u16 + 1980,
            month: ((packed >> 21) & 0x0F) as u8,
            day: ((packed >> 16) & 0x1F) as u8,
            hour: ((packed >> 11) & 0x1F) as u8,
            minute: ((packed >> 5) & 0x3F) as u8,
            second: ((packed & 0x1F) as u8 * 2).saturating_add(inc / 100),
            utc_offset_quarters: (offset & 0x80 != 0).then_some(((offset & 0x7F) << 1) as i8 >> 1),
        }
    }
}

/// A file name: up to 255 UTF-16 code units, as stored.
#[derive(Clone, Copy)]
pub struct Name {
    units: [u16; MAX_NAME_UNITS],
    len: u8,
}

impl Name {
    /// Build from a `&str`; `None` when empty, longer than 255 UTF-16 units, or containing a character exFAT
    /// forbids (control characters and `" * / : < > ? \ |`).
    pub fn new(s: &str) -> Option<Name> {
        let mut name = Name {
            units: [0; MAX_NAME_UNITS],
            len: 0,
        };
        for unit in s.encode_utf16() {
            if name.len as usize >= MAX_NAME_UNITS || !is_name_unit_valid(unit) {
                return None;
            }
            name.units[name.len as usize] = unit;
            name.len += 1;
        }
        (name.len > 0).then_some(name)
    }

    /// Build from raw UTF-16 units (as read off a volume), unchecked but capped at 255 units.
    pub fn from_units(units: &[u16]) -> Name {
        let mut name = Name {
            units: [0; MAX_NAME_UNITS],
            len: 0,
        };
        for &u in units.iter().take(MAX_NAME_UNITS) {
            name.units[name.len as usize] = u;
            name.len += 1;
        }
        name
    }

    /// The UTF-16 code units.
    pub fn units(&self) -> &[u16] {
        &self.units[..self.len as usize]
    }

    /// Number of UTF-16 code units.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// `true` when the name has no units (only possible for a damaged entry).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The characters, with lone surrogates as U+FFFD.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        char::decode_utf16(self.units().iter().copied())
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// Copy the name as UTF-8 into `out`; `None` when it doesn't fit (255 units need at most 765 bytes).
    pub fn to_str<'a>(&self, out: &'a mut [u8]) -> Option<&'a str> {
        let mut at = 0;
        for c in self.chars() {
            let n = c.len_utf8();
            if at + n > out.len() {
                return None;
            }
            c.encode_utf8(&mut out[at..]);
            at += n;
        }
        core::str::from_utf8(&out[..at]).ok()
    }

    /// Case-insensitive equality through the up-case table, as exFAT compares names.
    pub fn eq_ignore_case(&self, other: &Name) -> bool {
        self.len == other.len
            && self
                .units()
                .iter()
                .zip(other.units())
                .all(|(&a, &b)| upcase(a) == upcase(b))
    }

    /// Case-insensitive equality with a `&str`.
    pub fn eq_ignore_case_str(&self, s: &str) -> bool {
        let mut units = s.encode_utf16();
        let mut mine = self.units().iter();
        loop {
            match (mine.next(), units.next()) {
                (None, None) => return true,
                (Some(&a), Some(b)) if upcase(a) == upcase(b) => {}
                _ => return false,
            }
        }
    }

    /// `NameHash` (spec §7.6.5): over the up-cased UTF-16 units.
    pub fn hash(&self) -> u16 {
        let mut h: u16 = 0;
        for &u in self.units() {
            for b in upcase(u).to_le_bytes() {
                h = (if h & 1 != 0 { 0x8000u16 } else { 0 })
                    .wrapping_add(h >> 1)
                    .wrapping_add(b as u16);
            }
        }
        h
    }
}

impl core::fmt::Debug for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("\"")?;
        for c in self.chars() {
            core::fmt::Write::write_char(f, c)?;
        }
        f.write_str("\"")
    }
}

impl core::fmt::Display for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.chars() {
            core::fmt::Write::write_char(f, c)?;
        }
        Ok(())
    }
}

/// `true` for a UTF-16 unit exFAT allows in a file name.
pub fn is_name_unit_valid(unit: u16) -> bool {
    unit >= 0x20
        && !matches!(
            unit,
            0x22 | 0x2A | 0x2F | 0x3A | 0x3C | 0x3E | 0x3F | 0x5C | 0x7C
        )
}

/// Where a directory entry sits: its directory's cluster, the byte offset in that cluster, and the entry's
/// byte index from the start of the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryLocation {
    /// The directory cluster holding the entry.
    pub cluster: u32,
    /// Byte offset of the entry in that cluster.
    pub offset: u32,
    /// Byte index of the entry from the start of the directory.
    pub index: u64,
}

/// A file or directory, as read from its entry set.
#[derive(Debug, Clone, Copy)]
pub struct FileInfo {
    /// Where the set's primary (file) entry is, for later changes.
    pub location: EntryLocation,
    /// Entries in the set after the primary.
    pub secondary_count: u8,
    /// `attr::*` bits.
    pub attributes: u16,
    /// First cluster, 0 when nothing is allocated.
    pub first_cluster: u32,
    /// Allocated length in bytes.
    pub data_length: u64,
    /// Bytes written so far; beyond it the file reads as zeros.
    pub valid_data_length: u64,
    /// Contiguous clusters, nothing in the FAT.
    pub no_fat_chain: bool,
    /// Creation time.
    pub created: Timestamp,
    /// Last modification time.
    pub modified: Timestamp,
    /// The file name.
    pub name: Name,
}

impl FileInfo {
    /// `true` for a directory.
    pub fn is_dir(&self) -> bool {
        self.attributes & attr::DIRECTORY != 0
    }

    /// Clusters the file occupies for a given cluster size (0 for an empty file).
    pub fn cluster_count(&self, cluster_bytes: u32) -> u64 {
        if self.first_cluster == 0 {
            0
        } else {
            self.data_length.div_ceil(cluster_bytes as u64)
        }
    }
}

/// `SetChecksum` (spec §6.3.3) over the raw entries of a set (`bytes.len()` a multiple of 32); the checksum
/// field itself (bytes 2 and 3) is skipped.
pub fn set_checksum(bytes: &[u8]) -> u16 {
    let mut sum: u16 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        sum = (if sum & 1 != 0 { 0x8000u16 } else { 0 })
            .wrapping_add(sum >> 1)
            .wrapping_add(b as u16);
    }
    sum
}

/// An entry set in RAM: the primary and its secondaries, raw.
pub struct EntrySet {
    /// Raw entries, 32 bytes each.
    pub bytes: [u8; MAX_SET_ENTRIES * ENTRY_SIZE],
    /// Entries in use (primary included).
    pub count: u8,
}

impl EntrySet {
    /// An empty set.
    pub const fn new() -> EntrySet {
        EntrySet {
            bytes: [0; MAX_SET_ENTRIES * ENTRY_SIZE],
            count: 0,
        }
    }

    /// The set's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.count as usize * ENTRY_SIZE]
    }

    /// Entry `i` of the set.
    pub fn entry(&self, i: usize) -> &[u8] {
        &self.bytes[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE]
    }

    /// Append a raw entry; `false` when the set is full.
    pub fn push(&mut self, entry: &[u8]) -> bool {
        if self.count as usize >= MAX_SET_ENTRIES || entry.len() != ENTRY_SIZE {
            return false;
        }
        let at = self.count as usize * ENTRY_SIZE;
        self.bytes[at..at + ENTRY_SIZE].copy_from_slice(entry);
        self.count += 1;
        true
    }

    /// Build the set for a new file: file entry, stream extension, name entries; the checksum filled in.
    pub fn for_file(
        name: &Name,
        attributes: u16,
        first_cluster: u32,
        data_length: u64,
        no_fat_chain: bool,
        time: Timestamp,
    ) -> EntrySet {
        let mut set = EntrySet::new();
        let name_entries = name.len().div_ceil(NAME_UNITS_PER_ENTRY);
        let (packed, inc, offset) = time.encode();
        let mut e = [0u8; ENTRY_SIZE];
        e[0] = TYPE_FILE;
        e[1] = (1 + name_entries) as u8;
        put16(&mut e, 4, attributes);
        put32(&mut e, 8, packed);
        put32(&mut e, 12, packed);
        put32(&mut e, 16, packed);
        e[20] = inc;
        e[21] = inc;
        e[22] = offset;
        e[23] = offset;
        e[24] = offset;
        set.push(&e);
        let mut s = [0u8; ENTRY_SIZE];
        s[0] = TYPE_STREAM;
        // An empty file has no allocation and no flags (`NoFatChain` on an empty file is an fsck error).
        s[1] = if first_cluster == 0 {
            0
        } else {
            STREAM_ALLOCATION_POSSIBLE | if no_fat_chain { STREAM_NO_FAT_CHAIN } else { 0 }
        };
        s[3] = name.len() as u8;
        put16(&mut s, 4, name.hash());
        put64(&mut s, 8, data_length);
        put32(&mut s, 20, first_cluster);
        put64(&mut s, 24, data_length);
        set.push(&s);
        for chunk in name.units().chunks(NAME_UNITS_PER_ENTRY) {
            let mut n = [0u8; ENTRY_SIZE];
            n[0] = TYPE_NAME;
            for (i, &u) in chunk.iter().enumerate() {
                put16(&mut n, 2 + i * 2, u);
            }
            set.push(&n);
        }
        let sum = set_checksum(set.as_bytes());
        put16(&mut set.bytes, 2, sum);
        set
    }

    /// Parse a complete file entry set (primary 0x85 first) into a [`FileInfo`]; `None` when the structure or
    /// the checksum is wrong. `location` is where the primary entry sits.
    pub fn parse_file(&self, location: EntryLocation) -> Option<FileInfo> {
        if self.count < 3 {
            return None;
        }
        let primary = self.entry(0);
        if primary[0] != TYPE_FILE || primary[1] as usize + 1 != self.count as usize {
            return None;
        }
        if set_checksum(self.as_bytes()) != le16(primary, 2) {
            return None;
        }
        let stream = self.entry(1);
        if stream[0] != TYPE_STREAM {
            return None;
        }
        let name_len = stream[3] as usize;
        let name_entries = name_len.div_ceil(NAME_UNITS_PER_ENTRY);
        if name_len == 0 || name_entries > self.count as usize - 2 {
            return None;
        }
        let mut units = [0u16; MAX_NAME_UNITS];
        for (i, unit) in units.iter_mut().enumerate().take(name_len) {
            let e = self.entry(2 + i / NAME_UNITS_PER_ENTRY);
            if e[0] != TYPE_NAME {
                return None;
            }
            *unit = le16(e, 2 + (i % NAME_UNITS_PER_ENTRY) * 2);
        }
        Some(FileInfo {
            location,
            secondary_count: primary[1],
            attributes: le16(primary, 4),
            first_cluster: le32(stream, 20),
            data_length: le64(stream, 24),
            valid_data_length: le64(stream, 8),
            no_fat_chain: stream[1] & STREAM_NO_FAT_CHAIN != 0,
            created: Timestamp::decode(le32(primary, 8), primary[20], primary[22]),
            modified: Timestamp::decode(le32(primary, 12), primary[21], primary[23]),
            name: Name::from_units(&units[..name_len]),
        })
    }
}

impl Default for EntrySet {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a volume label entry (0x83); an empty label gives the "no label" form (type 0x03).
pub fn render_label_entry(label: &[u16], out: &mut [u8; ENTRY_SIZE]) {
    out.fill(0);
    let n = label.len().min(MAX_LABEL_UNITS);
    out[0] = if n == 0 {
        TYPE_LABEL & !TYPE_IN_USE
    } else {
        TYPE_LABEL
    };
    out[1] = n as u8;
    for (i, &u) in label.iter().take(n).enumerate() {
        put16(out, 2 + i * 2, u);
    }
}

/// Render an allocation bitmap entry (0x81) for the first bitmap.
pub fn render_bitmap_entry(first_cluster: u32, length: u64, out: &mut [u8; ENTRY_SIZE]) {
    out.fill(0);
    out[0] = TYPE_BITMAP;
    put32(out, 20, first_cluster);
    put64(out, 24, length);
}

/// Render an up-case table entry (0x82).
pub fn render_upcase_entry(
    checksum: u32,
    first_cluster: u32,
    length: u64,
    out: &mut [u8; ENTRY_SIZE],
) {
    out.fill(0);
    out[0] = TYPE_UPCASE;
    put32(out, 4, checksum);
    put32(out, 20, first_cluster);
    put64(out, 24, length);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_round_trip() {
        let t = Timestamp {
            year: 2026,
            month: 10,
            day: 17,
            hour: 13,
            minute: 45,
            second: 31,
            utc_offset_quarters: Some(8),
        };
        let (p, inc, off) = t.encode();
        assert_eq!(inc, 100);
        assert_eq!(off, 0x88);
        assert_eq!(Timestamp::decode(p, inc, off), t);
        let t = Timestamp {
            utc_offset_quarters: Some(-20),
            second: 30,
            ..t
        };
        let (p, inc, off) = t.encode();
        assert_eq!(Timestamp::decode(p, inc, off), t);
        let (p, inc, off) = Timestamp::DEFAULT.encode();
        assert_eq!(Timestamp::decode(p, inc, off), Timestamp::DEFAULT);
    }

    #[test]
    fn names() {
        assert!(Name::new("").is_none());
        assert!(Name::new("a/b").is_none());
        assert!(Name::new("a\tb").is_none());
        assert!(Name::new(&"a".repeat(255)).is_some());
        assert!(Name::new(&"a".repeat(256)).is_none());
        let a = Name::new("logring.wlr").unwrap();
        let b = Name::new("LogRing.WLR").unwrap();
        assert!(a.eq_ignore_case(&b));
        assert!(a.eq_ignore_case_str("LOGRING.wlr"));
        assert!(!a.eq_ignore_case_str("logring.wl"));
        assert!(!a.eq_ignore_case_str("logring.wlrx"));
        assert_eq!(a.hash(), b.hash());
        let mut buf = [0u8; 64];
        assert_eq!(a.to_str(&mut buf), Some("logring.wlr"));
        assert_eq!(a.to_str(&mut buf[..3]), None);
        let e = Name::new("é").unwrap();
        assert_eq!(e.units(), &[0xE9]);
        assert!(e.eq_ignore_case_str("É"));
    }

    #[test]
    fn file_set_round_trip() {
        let name = Name::new("README.txt").unwrap();
        let t = Timestamp::DEFAULT;
        let set = EntrySet::for_file(&name, attr::ARCHIVE, 5, 1234, true, t);
        assert_eq!(set.count, 3);
        let loc = EntryLocation {
            cluster: 4,
            offset: 96,
            index: 96,
        };
        let info = set.parse_file(loc).unwrap();
        assert_eq!(info.first_cluster, 5);
        assert_eq!(info.data_length, 1234);
        assert_eq!(info.valid_data_length, 1234);
        assert!(info.no_fat_chain);
        assert_eq!(info.attributes, attr::ARCHIVE);
        assert_eq!(info.created, t);
        assert!(info.name.eq_ignore_case_str("readme.TXT"));
        assert_eq!(info.location, loc);
        // A flipped bit breaks the checksum.
        let mut bad = EntrySet::new();
        for i in 0..3 {
            bad.push(set.entry(i));
        }
        bad.bytes[40] ^= 1;
        assert!(bad.parse_file(loc).is_none());
        // Long name: 17 name entries.
        let set = EntrySet::for_file(&Name::new(&"a".repeat(255)).unwrap(), 0, 0, 0, false, t);
        assert_eq!(set.count as usize, MAX_SET_ENTRIES);
        assert_eq!(set.parse_file(loc).unwrap().name.len(), 255);
    }
}

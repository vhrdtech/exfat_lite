//! Errors. Everything a card can get wrong is a value here, never a panic.

/// What can go wrong; `E` is the block device's own error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The block device failed.
    Device(E),
    /// Neither an exFAT boot sector at LBA 0 nor an MBR with an exFAT (type 0x07) partition.
    NotExfat,
    /// The boot sector is exFAT but a field is inconsistent with the volume or the device.
    BootSector(BootProblem),
    /// A valid exFAT volume this crate does not handle.
    Unsupported(Unsupported),
    /// On-card structures contradict each other (a bad checksum, a chain that loops, an entry out of range).
    Corrupt(Corrupt),
    /// A walk or scan hit the bound the caller gave it.
    Limit(Limit),
    /// A caller-side mistake: an argument that can't be honoured.
    InvalidArgument,
    /// The path does not exist.
    NotFound,
    /// A path component that is not a directory was used as one.
    NotADirectory,
    /// A file of that name (compared case-insensitively) already exists.
    AlreadyExists,
    /// The name is empty, too long (more than 255 UTF-16 units), or has a character exFAT forbids.
    InvalidName,
    /// Not enough free clusters (contiguous and aligned, where asked) for the file.
    NoSpace,
    /// The directory has no room for the new entry set and cannot be grown.
    DirectoryFull,
}

/// Boot sector field problems found at mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootProblem {
    /// The boot checksum sector does not match sectors 0..=10.
    Checksum,
    /// `VolumeLength` runs past the end of the device.
    VolumeTooLarge,
    /// `FatOffset`/`FatLength`/`ClusterHeapOffset`/`ClusterCount` overlap or run past the volume.
    Geometry,
    /// `FirstClusterOfRootDirectory` is outside the cluster heap.
    RootCluster,
    /// `FileSystemRevision` has a major version other than 1.
    Revision,
}

/// Valid exFAT this crate refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// `BytesPerSectorShift` is not 9 (sectors other than 512 bytes).
    SectorSize,
    /// `SectorsPerClusterShift` makes clusters larger than 32 MB.
    ClusterSize,
    /// `NumberOfFats` is 2 (TexFAT).
    TexFat,
}

/// Contradictions found in on-card structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corrupt {
    /// A FAT entry points outside the cluster heap, or a chain is longer than the heap.
    FatChain,
    /// A cluster number in a directory entry is outside the cluster heap.
    ClusterOutOfRange,
    /// A file's allocation runs past the end of the cluster heap.
    ExtentOutOfRange,
    /// A directory entry set is torn or its checksum is wrong.
    EntrySet,
    /// The root directory has no allocation bitmap entry, or its size does not fit the cluster count.
    Bitmap,
    /// The root directory has no up-case table entry, or the table does not match its checksum.
    UpcaseTable,
    /// A directory entry set spans more entries than exFAT allows.
    SecondaryCount,
}

/// Bounds this crate stops at on purpose, so a crafted card cannot keep it busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// More directory entries than the caller allowed.
    Entries,
    /// A directory deeper than the caller allowed (or deeper than [`crate::MAX_WALK_DEPTH`]).
    Depth,
    /// A path with more components than the walk stack holds.
    PathDepth,
}

impl<E> Error<E> {
    /// Map the device error type (for callers that wrap devices).
    pub fn map_device<F>(self, f: impl FnOnce(E) -> F) -> Error<F> {
        match self {
            Error::Device(e) => Error::Device(f(e)),
            Error::NotExfat => Error::NotExfat,
            Error::BootSector(p) => Error::BootSector(p),
            Error::Unsupported(u) => Error::Unsupported(u),
            Error::Corrupt(c) => Error::Corrupt(c),
            Error::Limit(l) => Error::Limit(l),
            Error::InvalidArgument => Error::InvalidArgument,
            Error::NotFound => Error::NotFound,
            Error::NotADirectory => Error::NotADirectory,
            Error::AlreadyExists => Error::AlreadyExists,
            Error::InvalidName => Error::InvalidName,
            Error::NoSpace => Error::NoSpace,
            Error::DirectoryFull => Error::DirectoryFull,
        }
    }
}

impl<E: core::fmt::Debug> core::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Device(e) => write!(f, "block device error: {e:?}"),
            Error::NotExfat => f.write_str("not an exFAT volume"),
            Error::BootSector(p) => write!(f, "bad boot sector: {p:?}"),
            Error::Unsupported(u) => write!(f, "unsupported exFAT volume: {u:?}"),
            Error::Corrupt(c) => write!(f, "corrupt volume: {c:?}"),
            Error::Limit(l) => write!(f, "limit reached: {l:?}"),
            Error::InvalidArgument => f.write_str("invalid argument"),
            Error::NotFound => f.write_str("not found"),
            Error::NotADirectory => f.write_str("not a directory"),
            Error::AlreadyExists => f.write_str("already exists"),
            Error::InvalidName => f.write_str("invalid file name"),
            Error::NoSpace => f.write_str("no space"),
            Error::DirectoryFull => f.write_str("directory full"),
        }
    }
}

#[cfg(feature = "std")]
impl<E: core::fmt::Debug> std::error::Error for Error<E> {}

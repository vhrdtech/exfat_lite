//! The block device a volume lives on: 512-byte sectors, addressed by LBA.

/// Bytes per sector. SD cards address 512-byte blocks and exFAT volumes formatted here use that sector size;
/// a volume with another `BytesPerSectorShift` is refused at mount.
pub const SECTOR_SIZE: usize = 512;

/// One sector's worth of bytes.
pub type Sector = [u8; SECTOR_SIZE];

/// A block device of 512-byte sectors.
///
/// The methods are `async` because the SD drivers this crate is meant for (embassy, `block-device-driver`) are;
/// a driver that completes synchronously implements them as `async fn` that return at once. `buf.len()` is
/// always a multiple of [`SECTOR_SIZE`] and `lba + buf.len() / 512 <= sector_count()` when this crate calls
/// these: it never reads or writes past the end it was told about.
#[allow(async_fn_in_trait)]
pub trait BlockDevice {
    /// The driver's own error type.
    type Error: core::fmt::Debug;

    /// Number of 512-byte sectors on the device.
    fn sector_count(&self) -> u64;

    /// Read `buf.len() / 512` sectors starting at `lba`.
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Write `buf.len() / 512` sectors starting at `lba`.
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Make everything written so far durable (a card's write cache). Default: nothing to do.
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A block device over a byte slice in RAM: tests, fuzzing, and small images on a host.
///
/// Trailing bytes that don't fill a sector are ignored.
pub struct SliceDevice<'a> {
    data: &'a mut [u8],
}

/// The only error a [`SliceDevice`] gives: an access past its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange;

impl<'a> SliceDevice<'a> {
    /// Wrap `data`; its length divided by 512 is the sector count.
    pub fn new(data: &'a mut [u8]) -> Self {
        Self { data }
    }

    /// The bytes behind the device.
    pub fn as_bytes(&self) -> &[u8] {
        self.data
    }

    fn range(&self, lba: u64, len: usize) -> Result<core::ops::Range<usize>, OutOfRange> {
        let sectors = (len / SECTOR_SIZE) as u64;
        if !len.is_multiple_of(SECTOR_SIZE)
            || lba
                .checked_add(sectors)
                .is_none_or(|end| end > self.sector_count())
        {
            return Err(OutOfRange);
        }
        let start = (lba as usize) * SECTOR_SIZE;
        Ok(start..start + len)
    }
}

impl BlockDevice for SliceDevice<'_> {
    type Error = OutOfRange;

    fn sector_count(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), OutOfRange> {
        let range = self.range(lba, buf.len())?;
        buf.copy_from_slice(&self.data[range]);
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), OutOfRange> {
        let range = self.range(lba, buf.len())?;
        self.data[range].copy_from_slice(buf);
        Ok(())
    }
}

impl<D: BlockDevice + ?Sized> BlockDevice for &mut D {
    type Error = D::Error;

    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        (**self).read(lba, buf).await
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        (**self).write(lba, buf).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        (**self).flush().await
    }
}

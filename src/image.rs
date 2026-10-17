//! Host side (`std` feature): an image file as a block device, and a `block_on` for the async API.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::device::{BlockDevice, SECTOR_SIZE};

/// A disk image file as a block device. [`ImageFile::create`] makes a sparse file, so a 128 GB image costs
/// only the sectors written.
pub struct ImageFile {
    file: File,
    sectors: u64,
}

impl ImageFile {
    /// Create (or truncate) `path` as a sparse image of `sectors` sectors.
    pub fn create(path: impl AsRef<Path>, sectors: u64) -> std::io::Result<ImageFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(sectors * SECTOR_SIZE as u64)?;
        Ok(ImageFile { file, sectors })
    }

    /// Open an existing image read-write; its size (rounded down to whole sectors) is the device size.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<ImageFile> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let sectors = file.metadata()?.len() / SECTOR_SIZE as u64;
        Ok(ImageFile { file, sectors })
    }

    /// Open an existing image read-only (writes fail).
    pub fn open_read_only(path: impl AsRef<Path>) -> std::io::Result<ImageFile> {
        let file = File::open(path)?;
        let sectors = file.metadata()?.len() / SECTOR_SIZE as u64;
        Ok(ImageFile { file, sectors })
    }

    /// The file underneath.
    pub fn file(&self) -> &File {
        &self.file
    }

    fn check(&self, lba: u64, len: usize) -> std::io::Result<u64> {
        let sectors = (len / SECTOR_SIZE) as u64;
        if !len.is_multiple_of(SECTOR_SIZE)
            || lba
                .checked_add(sectors)
                .is_none_or(|end| end > self.sectors)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sector range outside the image",
            ));
        }
        Ok(lba * SECTOR_SIZE as u64)
    }
}

impl BlockDevice for ImageFile {
    type Error = std::io::Error;

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let at = self.check(lba, buf.len())?;
        self.file.seek(SeekFrom::Start(at))?;
        self.file.read_exact(buf)
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> std::io::Result<()> {
        let at = self.check(lba, buf.len())?;
        self.file.seek(SeekFrom::Start(at))?;
        self.file.write_all(buf)
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Run a future to completion by polling it in a loop. Meant for devices whose futures never wait
/// ([`ImageFile`], [`crate::SliceDevice`]): a future that pends on a real waker spins until it is ready.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = core::pin::pin!(fut);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    loop {
        if let core::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

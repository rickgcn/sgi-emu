//! Private host-file storage adapter.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use se_device::storage::BlockStorage;

pub(crate) struct FileBlockStorage {
    file: File,
    size_bytes: u64,
}

impl FileBlockStorage {
    pub(crate) fn open_read_only(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        Self::from_file(file)
    }

    pub(crate) fn open_read_write(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Self::from_file(file)
    }

    fn from_file(file: File) -> io::Result<Self> {
        let size_bytes = file.metadata()?.len();
        Ok(Self { file, size_bytes })
    }

    pub(crate) fn boxed(self) -> Box<dyn BlockStorage> {
        Box::new(self)
    }
}

impl BlockStorage for FileBlockStorage {
    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buffer)
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        let byte_count = u64::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "write length overflow"))?;
        let end = offset
            .checked_add(byte_count)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write range overflow"))?;
        if end > self.size_bytes {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "write extends beyond fixed storage capacity",
            ));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_device::storage::BlockStorage;

    use super::FileBlockStorage;

    static NEXT_PATH_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(name: &str) -> PathBuf {
        let id = NEXT_PATH_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sgi-emu-storage-{name}-{}-{id}.img",
            std::process::id()
        ))
    }

    #[test]
    fn read_write_storage_persists_in_range_and_tail_writes() {
        let path = temporary_path("persist");
        fs::write(&path, [0, 1, 2, 3, 4, 5]).unwrap();

        {
            let mut storage = FileBlockStorage::open_read_write(&path).unwrap();
            storage.write_all_at(1, &[9, 8]).unwrap();
            storage.write_all_at(4, &[7, 6]).unwrap();
            assert_eq!(storage.size_bytes(), 6);
        }

        let mut reopened = FileBlockStorage::open_read_only(&path).unwrap();
        let mut bytes = [0; 6];
        reopened.read_exact_at(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0, 9, 8, 3, 7, 6]);
        drop(reopened);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn writes_cannot_overflow_or_extend_fixed_capacity() {
        let path = temporary_path("bounds");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut storage = FileBlockStorage::open_read_write(&path).unwrap();

        assert!(storage.write_all_at(3, &[8, 9]).is_err());
        assert!(storage.write_all_at(u64::MAX, &[8, 9]).is_err());
        drop(storage);
        assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 4]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn read_only_storage_rejects_writes() {
        let path = temporary_path("read-only");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut storage = FileBlockStorage::open_read_only(&path).unwrap();

        assert!(storage.write_all_at(0, &[9]).is_err());
        drop(storage);
        assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 4]);
        fs::remove_file(path).unwrap();
    }
}

//! Fixed-capacity host-file storage for normal resource preparation.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use se_core::storage::StorageMedium;

pub(crate) struct HostFileStorage {
    file: File,
    size_bytes: u64,
}

impl HostFileStorage {
    pub(crate) fn open_read_only(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_file(File::open(path)?)
    }

    pub(crate) fn open_read_write(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_file(OpenOptions::new().read(true).write(true).open(path)?)
    }

    fn from_file(file: File) -> io::Result<Self> {
        let size_bytes = file.metadata()?.len();
        Ok(Self { file, size_bytes })
    }
}

impl StorageMedium for HostFileStorage {
    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        check_range(offset, buffer.len(), self.size_bytes)?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buffer)
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        check_range(offset, data.len(), self.size_bytes)?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)
    }
}

fn check_range(offset: u64, length: usize, size_bytes: u64) -> io::Result<()> {
    let byte_count = u64::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "storage length overflow"))?;
    let end = offset
        .checked_add(byte_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "storage range overflow"))?;
    if end > size_bytes {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "storage range exceeds fixed storage capacity",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_core::storage::StorageMedium;

    use super::HostFileStorage;

    static NEXT_PATH_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(name: &str) -> PathBuf {
        let id = NEXT_PATH_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sgi-emu-session-storage-{name}-{}-{id}.img",
            std::process::id()
        ))
    }

    #[test]
    fn read_write_storage_persists_in_range_access() {
        let path = temporary_path("persist");
        fs::write(&path, [0, 1, 2, 3, 4, 5]).unwrap();

        let mut storage = HostFileStorage::open_read_write(&path).unwrap();
        assert_eq!(storage.size_bytes(), 6);
        let mut initial = [0; 3];
        storage.read_exact_at(1, &mut initial).unwrap();
        assert_eq!(initial, [1, 2, 3]);
        storage.write_all_at(2, &[9, 8]).unwrap();
        drop(storage);

        assert_eq!(fs::read(&path).unwrap(), [0, 1, 9, 8, 4, 5]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn read_only_storage_rejects_writes() {
        let path = temporary_path("read-only");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut storage = HostFileStorage::open_read_only(&path).unwrap();

        let mut bytes = [0; 2];
        storage.read_exact_at(1, &mut bytes).unwrap();
        assert_eq!(bytes, [2, 3]);
        assert!(storage.write_all_at(0, &[9]).is_err());
        drop(storage);

        assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 4]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn out_of_range_access_cannot_extend_fixed_capacity() {
        let path = temporary_path("bounds");
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut storage = HostFileStorage::open_read_write(&path).unwrap();

        assert!(storage.read_exact_at(3, &mut [0; 2]).is_err());
        assert!(storage.write_all_at(3, &[8, 9]).is_err());
        assert!(storage.write_all_at(u64::MAX, &[8, 9]).is_err());
        drop(storage);

        assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 4]);
        fs::remove_file(path).unwrap();
    }
}

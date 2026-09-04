//! Private host-file storage adapter.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use se_device::storage::BlockStorage;
use se_runtime::record::{DISK_PAGE_BYTES, MediaIdentity, RecordDisk, ReplayDisk};

enum FileStorageMode {
    Normal,
    Recording(RecordDisk),
    Replay(ReplayDisk),
}

pub(crate) struct FileBlockStorage {
    file: File,
    size_bytes: u64,
    mode: FileStorageMode,
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
        Ok(Self {
            file,
            size_bytes,
            mode: FileStorageMode::Normal,
        })
    }

    pub(crate) fn identity(&mut self, path: &Path) -> io::Result<MediaIdentity> {
        MediaIdentity::from_file(path, &mut self.file)
    }

    pub(crate) fn recording(mut self, disk: RecordDisk) -> Self {
        self.mode = FileStorageMode::Recording(disk);
        self
    }

    pub(crate) fn replay(mut self, disk: ReplayDisk) -> Self {
        self.mode = FileStorageMode::Replay(disk);
        self
    }

    pub(crate) fn replay_initial_identity(&mut self, path: &Path) -> io::Result<MediaIdentity> {
        let mut hasher = sha2::Sha256::new();
        let mut offset = 0;
        let mut buffer = vec![0; 128 * 1024];
        while offset < self.size_bytes {
            let count = usize::try_from((self.size_bytes - offset).min(buffer.len() as u64))
                .map_err(|_| io::Error::other("storage hash length does not fit usize"))?;
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut buffer[..count])?;
            if let FileStorageMode::Replay(replay) = &self.mode {
                replay.overlay_initial_read(offset, &mut buffer[..count])?;
            }
            use sha2::Digest;
            hasher.update(&buffer[..count]);
            offset += count as u64;
        }
        use sha2::Digest;
        Ok(MediaIdentity {
            path_hint: path.to_string_lossy().into_owned(),
            size_bytes: self.size_bytes,
            sha256: hasher.finalize().into(),
        })
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
        check_range(offset, buffer.len(), self.size_bytes)?;
        if let Err(error) = self
            .file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.file.read_exact(buffer))
        {
            self.report_storage_error(&error);
            return Err(error);
        }
        let result = match &self.mode {
            FileStorageMode::Normal | FileStorageMode::Recording(_) => Ok(()),
            FileStorageMode::Replay(replay) => replay.overlay_read(offset, buffer),
        };
        if let Err(error) = &result {
            self.report_storage_error(error);
        }
        result
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        check_range(offset, data.len(), self.size_bytes)?;
        match &self.mode {
            FileStorageMode::Normal => {
                self.file.seek(SeekFrom::Start(offset))?;
                self.file.write_all(data)
            }
            FileStorageMode::Recording(recording) => {
                let recording = recording.clone();
                let result = (|| {
                    capture_before_images(
                        &mut self.file,
                        self.size_bytes,
                        offset,
                        data.len(),
                        &recording,
                    )?;
                    self.file.seek(SeekFrom::Start(offset))?;
                    self.file.write_all(data)
                })();
                if let Err(error) = &result {
                    recording.report_storage_error(error);
                }
                result
            }
            FileStorageMode::Replay(replay) => {
                let replay = replay.clone();
                let result =
                    replay.write_all_at(offset, data, self.size_bytes, |page_offset, page| {
                        self.file.seek(SeekFrom::Start(page_offset))?;
                        self.file.read_exact(page)
                    });
                if let Err(error) = &result {
                    replay.report_storage_error(error);
                }
                result
            }
        }
    }
}

impl FileBlockStorage {
    fn report_storage_error(&self, error: &io::Error) {
        match &self.mode {
            FileStorageMode::Recording(recording) => recording.report_storage_error(error),
            FileStorageMode::Replay(replay) => replay.report_storage_error(error),
            FileStorageMode::Normal => {}
        }
    }
}

fn capture_before_images(
    file: &mut File,
    size_bytes: u64,
    offset: u64,
    length: usize,
    recording: &RecordDisk,
) -> io::Result<()> {
    if length == 0 {
        return Ok(());
    }
    let end = offset + length as u64;
    let first_page = offset / DISK_PAGE_BYTES as u64;
    let last_page = end.saturating_sub(1) / DISK_PAGE_BYTES as u64;
    for page_index in first_page..=last_page {
        let page_offset = page_index * DISK_PAGE_BYTES as u64;
        let page_length = usize::try_from((size_bytes - page_offset).min(DISK_PAGE_BYTES as u64))
            .map_err(|_| io::Error::other("disk page length does not fit usize"))?;
        recording.capture_before_image(page_index, || {
            let mut bytes = vec![0; page_length];
            file.seek(SeekFrom::Start(page_offset))?;
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })?;
    }
    Ok(())
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

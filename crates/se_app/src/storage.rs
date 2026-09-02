//! Private host-file storage adapter.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use se_device::storage::BlockStorage;

pub(crate) struct FileBlockStorage {
    file: File,
    size_bytes: u64,
}

impl FileBlockStorage {
    pub(crate) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
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
}

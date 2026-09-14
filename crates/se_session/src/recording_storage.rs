//! Read-only host backing with a resource-specific Recording overlay.

use std::io;

use se_core::storage::StorageMedium;
use se_runtime::record::RecordStorage;

use crate::file_storage::HostFileStorage;

pub(crate) struct RecordingStorageMedium {
    base: HostFileStorage,
    overlay: RecordStorage,
}

impl RecordingStorageMedium {
    pub(crate) fn new(base: HostFileStorage, overlay: RecordStorage) -> Self {
        Self { base, overlay }
    }
}

impl StorageMedium for RecordingStorageMedium {
    fn size_bytes(&self) -> u64 {
        self.base.size_bytes()
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let result = self
            .base
            .read_exact_at(offset, buffer)
            .and_then(|()| self.overlay.overlay_read(offset, buffer));
        if let Err(error) = &result {
            self.overlay.report_storage_error(error);
        }
        result
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        let result =
            self.overlay
                .write_all_at(offset, data, self.base.size_bytes(), |page_offset, page| {
                    self.base.read_exact_at(page_offset, page)
                });
        if let Err(error) = &result {
            self.overlay.report_storage_error(error);
        }
        result
    }
}

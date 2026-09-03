//! Functional SCSI direct-access disk target.

use crate::scsi::{ScsiCommandPlan, ScsiStatus, ScsiStorageSizeError, ScsiTarget, SenseData};

const BLOCK_BYTES: u32 = 512;

const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const MODE_SENSE_6: u8 = 0x1a;
const START_STOP_UNIT: u8 = 0x1b;
const READ_CAPACITY_10: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2a;

/// Software-visible state of one SCSI disk target.
pub struct ScsiDisk {
    block_count: u64,
    ready: bool,
    sense: SenseData,
}

impl ScsiDisk {
    /// Creates a ready target for a validated storage capacity.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiStorageSizeError`] when `storage_bytes` is zero, is not
    /// a multiple of 512 bytes, or cannot be represented by
    /// `READ CAPACITY(10)`.
    pub fn try_new(storage_bytes: u64) -> Result<Self, ScsiStorageSizeError> {
        if storage_bytes == 0
            || !storage_bytes.is_multiple_of(u64::from(BLOCK_BYTES))
            || storage_bytes / u64::from(BLOCK_BYTES) > u64::from(u32::MAX) + 1
        {
            return Err(ScsiStorageSizeError::new(storage_bytes));
        }
        Ok(Self {
            block_count: storage_bytes / u64::from(BLOCK_BYTES),
            ready: true,
            sense: SenseData::NONE,
        })
    }

    fn request_sense(&mut self, allocation_length: u8) -> ScsiCommandPlan {
        let sense = self.sense;
        self.sense = SenseData::NONE;
        complete_good(sense.fixed_response(allocation_length))
    }

    fn inquiry(&self, allocation_length: u8) -> ScsiCommandPlan {
        let mut data = vec![0; 36];
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x01;
        data[3] = 0x01;
        data[4] = 31;
        data[8..16].copy_from_slice(b"SGI-EMU ");
        data[16..32].copy_from_slice(b"VIRTUAL DISK    ");
        data[32..36].copy_from_slice(b"0001");
        data.truncate(usize::from(allocation_length));
        complete_good(data)
    }

    fn mode_sense(&self, allocation_length: u8) -> ScsiCommandPlan {
        let mut data = vec![0; 12];
        data[0] = 0x0b;
        data[3] = 8;
        data[9..12].copy_from_slice(&BLOCK_BYTES.to_be_bytes()[1..]);
        data.truncate(usize::from(allocation_length));
        complete_good(data)
    }

    fn read_capacity(&mut self) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let last_lba = (self.block_count - 1) as u32;
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&last_lba.to_be_bytes());
        data.extend_from_slice(&BLOCK_BYTES.to_be_bytes());
        complete_good(data)
    }

    fn read_10(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
        let block_count = u16::from_be_bytes([cdb[7], cdb[8]]);
        if block_count == 0 {
            return complete_good(Vec::new());
        }
        let end = u64::from(lba) + u64::from(block_count);
        if end > self.block_count {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        }
        ScsiCommandPlan::ReadStorage {
            offset: u64::from(lba) * u64::from(BLOCK_BYTES),
            byte_count: u64::from(block_count) * u64::from(BLOCK_BYTES),
        }
    }

    fn write_10(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
        let block_count = u16::from_be_bytes([cdb[7], cdb[8]]);
        if block_count == 0 {
            return complete_good(Vec::new());
        }
        let end = u64::from(lba) + u64::from(block_count);
        if end > self.block_count {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        }
        ScsiCommandPlan::WriteStorage {
            offset: u64::from(lba) * u64::from(BLOCK_BYTES),
            byte_count: u64::from(block_count) * u64::from(BLOCK_BYTES),
        }
    }

    fn check_condition(&mut self, sense: SenseData) -> ScsiCommandPlan {
        self.sense = sense;
        ScsiCommandPlan::Complete {
            status: ScsiStatus::CheckCondition,
            data_in: Vec::new(),
        }
    }
}

impl ScsiTarget for ScsiDisk {
    fn storage_size_bytes(&self) -> u64 {
        self.block_count * u64::from(BLOCK_BYTES)
    }

    /// Decodes one command descriptor block.
    fn execute(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        let Some(opcode) = cdb.first().copied() else {
            return self.check_condition(SenseData::INVALID_CDB_FIELD);
        };

        match opcode {
            TEST_UNIT_READY if cdb.len() >= 6 => {
                if self.ready {
                    complete_good(Vec::new())
                } else {
                    self.check_condition(SenseData::NOT_READY)
                }
            }
            REQUEST_SENSE if cdb.len() >= 6 => self.request_sense(cdb[4]),
            INQUIRY if cdb.len() >= 6 => self.inquiry(cdb[4]),
            MODE_SENSE_6 if cdb.len() >= 6 => self.mode_sense(cdb[4]),
            START_STOP_UNIT if cdb.len() >= 6 => {
                self.ready = cdb[4] & 1 != 0;
                complete_good(Vec::new())
            }
            READ_CAPACITY_10 if cdb.len() >= 10 => self.read_capacity(),
            READ_10 if cdb.len() >= 10 => self.read_10(cdb),
            WRITE_10 if cdb.len() >= 10 => self.write_10(cdb),
            TEST_UNIT_READY | REQUEST_SENSE | INQUIRY | MODE_SENSE_6 | START_STOP_UNIT
            | READ_CAPACITY_10 | READ_10 | WRITE_10 => {
                self.check_condition(SenseData::INVALID_CDB_FIELD)
            }
            _ => self.check_condition(SenseData::UNSUPPORTED_OPCODE),
        }
    }

    /// Completes storage-backed I/O and records a host failure as target sense
    /// data.
    fn complete_storage(&mut self, succeeded: bool) -> ScsiStatus {
        if succeeded {
            ScsiStatus::Good
        } else {
            self.sense = SenseData::HOST_IO_ERROR;
            ScsiStatus::CheckCondition
        }
    }
}

fn complete_good(data_in: Vec<u8>) -> ScsiCommandPlan {
    ScsiCommandPlan::Complete {
        status: ScsiStatus::Good,
        data_in,
    }
}

#[cfg(test)]
mod tests {
    use crate::scsi::{ScsiCommandPlan, ScsiStatus, ScsiTarget};

    use super::{BLOCK_BYTES, ScsiDisk};

    fn disk(block_count: u64) -> ScsiDisk {
        ScsiDisk::try_new(block_count * u64::from(BLOCK_BYTES)).unwrap()
    }

    #[test]
    fn inquiry_and_capacity_report_the_fixed_target_identity() {
        let mut disk = disk(0x1235);
        let ScsiCommandPlan::Complete { status, data_in } = disk.execute(&[0x12, 0, 0, 0, 36, 0])
        else {
            panic!("INQUIRY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(&data_in[8..16], b"SGI-EMU ");
        assert_eq!(&data_in[16..32], b"VIRTUAL DISK    ");

        let ScsiCommandPlan::Complete { status, data_in } =
            disk.execute(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        else {
            panic!("READ CAPACITY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(data_in, [0, 0, 0x12, 0x34, 0, 0, 2, 0]);
    }

    #[test]
    fn read_ten_validates_the_requested_range() {
        let mut disk = disk(16);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[5] = 14;
        cdb[8] = 2;
        assert_eq!(
            disk.execute(&cdb),
            ScsiCommandPlan::ReadStorage {
                offset: 14 * u64::from(BLOCK_BYTES),
                byte_count: 2 * u64::from(BLOCK_BYTES),
            }
        );

        cdb[5] = 15;
        assert!(matches!(
            disk.execute(&cdb),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));
    }

    #[test]
    fn request_sense_reports_and_clears_the_latest_failure() {
        let mut disk = disk(1);
        assert!(matches!(
            disk.execute(&[0xff]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));

        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (5, 0x20, 0));

        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (0, 0, 0));
    }

    #[test]
    fn stopped_state_applies_to_reads_and_writes() {
        let mut disk = disk(1);
        let _ = disk.execute(&[0x1b, 0, 0, 0, 0, 0]);
        for cdb in [
            &[0x00, 0, 0, 0, 0, 0][..],
            &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0][..],
            &[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0][..],
        ] {
            assert!(matches!(
                disk.execute(cdb),
                ScsiCommandPlan::Complete {
                    status: ScsiStatus::CheckCondition,
                    ..
                }
            ));
            let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
            else {
                panic!("REQUEST SENSE should complete immediately");
            };
            assert_eq!((data_in[2], data_in[12], data_in[13]), (2, 0x04, 0x02));
        }
    }

    #[test]
    fn mode_sense_and_write_ten_report_a_writable_fixed_disk() {
        let mut disk = disk(16);
        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x1a, 0, 0, 0, 12, 0])
        else {
            panic!("MODE SENSE should complete immediately");
        };
        assert_eq!(data_in[2] & 0x80, 0);

        let mut cdb = [0; 10];
        cdb[0] = 0x2a;
        cdb[5] = 14;
        cdb[8] = 2;
        assert_eq!(
            disk.execute(&cdb),
            ScsiCommandPlan::WriteStorage {
                offset: 14 * u64::from(BLOCK_BYTES),
                byte_count: 2 * u64::from(BLOCK_BYTES),
            }
        );

        cdb[5] = 15;
        assert!(matches!(
            disk.execute(&cdb),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));

        cdb[5] = 0;
        cdb[8] = 0;
        assert_eq!(
            disk.execute(&cdb),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::Good,
                data_in: Vec::new(),
            }
        );
    }

    #[test]
    fn storage_completion_preserves_sense_and_reports_host_failures() {
        let mut disk = disk(1);
        let _ = disk.execute(&[0xff]);
        assert_eq!(disk.complete_storage(true), ScsiStatus::Good);
        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (5, 0x20, 0));

        assert_eq!(disk.complete_storage(false), ScsiStatus::CheckCondition);
        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (4, 0x44, 0));
    }
}

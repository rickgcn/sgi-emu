//! Functional read-only SCSI direct-access disk target.

const BLOCK_BYTES: u32 = 512;
const FIXED_SENSE_BYTES: usize = 18;

const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const MODE_SENSE_6: u8 = 0x1a;
const START_STOP_UNIT: u8 = 0x1b;
const READ_CAPACITY_10: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2a;

/// The status returned by a SCSI target command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiStatus {
    /// The command completed successfully.
    Good,
    /// Sense data describes why the command could not complete.
    CheckCondition,
}

impl ScsiStatus {
    /// Returns the status byte placed on the SCSI bus.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Good => 0x00,
            Self::CheckCondition => 0x02,
        }
    }
}

/// Work requested by one decoded SCSI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScsiCommandPlan {
    /// The target can complete without consulting block storage.
    Complete {
        /// Target status.
        status: ScsiStatus,
        /// Bytes returned to the initiator.
        data_in: Vec<u8>,
    },
    /// The containing machine must read blocks from host storage.
    ReadBlocks {
        /// First logical block address.
        lba: u32,
        /// Number of 512-byte blocks.
        block_count: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SenseData {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl SenseData {
    const NONE: Self = Self {
        key: 0,
        asc: 0,
        ascq: 0,
    };
    const UNSUPPORTED_OPCODE: Self = Self {
        key: 5,
        asc: 0x20,
        ascq: 0,
    };
    const INVALID_CDB_FIELD: Self = Self {
        key: 5,
        asc: 0x24,
        ascq: 0,
    };
    const LBA_OUT_OF_RANGE: Self = Self {
        key: 5,
        asc: 0x21,
        ascq: 0,
    };
    const WRITE_PROTECTED: Self = Self {
        key: 7,
        asc: 0x27,
        ascq: 0,
    };
    const HOST_READ_ERROR: Self = Self {
        key: 4,
        asc: 0x44,
        ascq: 0,
    };
    const NOT_READY: Self = Self {
        key: 2,
        asc: 0x04,
        ascq: 0x02,
    };
}

/// Software-visible state of one read-only SCSI disk target.
pub struct ScsiDisk {
    block_count: u64,
    ready: bool,
    sense: SenseData,
}

impl ScsiDisk {
    /// Creates a ready target with the supplied nonzero block count.
    ///
    /// # Panics
    ///
    /// Panics when `block_count` is zero or cannot be represented by
    /// `READ CAPACITY(10)`.
    #[must_use]
    pub const fn new(block_count: u64) -> Self {
        assert!(block_count != 0);
        assert!(block_count <= u32::MAX as u64 + 1);
        Self {
            block_count,
            ready: true,
            sense: SenseData::NONE,
        }
    }

    /// Decodes one command descriptor block.
    #[must_use]
    pub fn execute(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
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
            WRITE_10 if cdb.len() >= 10 => self.check_condition(SenseData::WRITE_PROTECTED),
            TEST_UNIT_READY | REQUEST_SENSE | INQUIRY | MODE_SENSE_6 | START_STOP_UNIT
            | READ_CAPACITY_10 | READ_10 | WRITE_10 => {
                self.check_condition(SenseData::INVALID_CDB_FIELD)
            }
            _ => self.check_condition(SenseData::UNSUPPORTED_OPCODE),
        }
    }

    /// Completes a storage-backed read and records a host read failure as
    /// target sense data.
    #[must_use]
    pub fn complete_read(&mut self, succeeded: bool) -> ScsiStatus {
        if succeeded {
            ScsiStatus::Good
        } else {
            self.sense = SenseData::HOST_READ_ERROR;
            ScsiStatus::CheckCondition
        }
    }

    fn request_sense(&mut self, allocation_length: u8) -> ScsiCommandPlan {
        let sense = self.sense;
        self.sense = SenseData::NONE;
        let mut data = vec![0; FIXED_SENSE_BYTES];
        data[0] = 0x70;
        data[2] = sense.key;
        data[7] = 10;
        data[12] = sense.asc;
        data[13] = sense.ascq;
        data.truncate(usize::from(allocation_length));
        complete_good(data)
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
        data[2] = 0x80;
        data[3] = 8;
        data[9..12].copy_from_slice(&BLOCK_BYTES.to_be_bytes()[1..]);
        data.truncate(usize::from(allocation_length));
        complete_good(data)
    }

    fn read_capacity(&mut self) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let last_lba = u32::try_from(self.block_count - 1)
            .expect("validated SCSI disk capacity must fit READ CAPACITY(10)");
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&last_lba.to_be_bytes());
        data.extend_from_slice(&BLOCK_BYTES.to_be_bytes());
        complete_good(data)
    }

    fn read_10(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let lba = u32::from_be_bytes(cdb[2..6].try_into().expect("READ(10) CDB was validated"));
        let block_count =
            u16::from_be_bytes(cdb[7..9].try_into().expect("READ(10) CDB was validated"));
        if block_count == 0 {
            return complete_good(Vec::new());
        }
        let end = u64::from(lba) + u64::from(block_count);
        if end > self.block_count {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        }
        ScsiCommandPlan::ReadBlocks { lba, block_count }
    }

    fn check_condition(&mut self, sense: SenseData) -> ScsiCommandPlan {
        self.sense = sense;
        ScsiCommandPlan::Complete {
            status: ScsiStatus::CheckCondition,
            data_in: Vec::new(),
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
    use super::{ScsiCommandPlan, ScsiDisk, ScsiStatus};

    #[test]
    fn inquiry_and_capacity_report_the_fixed_target_identity() {
        let mut disk = ScsiDisk::new(0x1235);
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
        let mut disk = ScsiDisk::new(16);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[5] = 4;
        cdb[8] = 2;
        assert_eq!(
            disk.execute(&cdb),
            ScsiCommandPlan::ReadBlocks {
                lba: 4,
                block_count: 2
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
        let mut disk = ScsiDisk::new(1);
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
    fn stopped_and_read_only_states_report_distinct_sense() {
        let mut disk = ScsiDisk::new(1);
        let _ = disk.execute(&[0x1b, 0, 0, 0, 0, 0]);
        assert!(matches!(
            disk.execute(&[0x00, 0, 0, 0, 0, 0]),
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

        assert!(matches!(
            disk.execute(&[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));
        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (7, 0x27, 0));
    }

    #[test]
    fn host_read_failure_becomes_hardware_error_sense() {
        let mut disk = ScsiDisk::new(1);
        assert_eq!(disk.complete_read(false), ScsiStatus::CheckCondition);
        let ScsiCommandPlan::Complete { data_in, .. } = disk.execute(&[0x03, 0, 0, 0, 18, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!((data_in[2], data_in[12], data_in[13]), (4, 0x44, 0));
    }
}

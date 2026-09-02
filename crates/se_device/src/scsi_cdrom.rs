//! Functional read-only SCSI CD-ROM target backed by a raw ISO image.

use crate::scsi::{ScsiCommandPlan, ScsiStatus, SenseData};

const BLOCK_BYTES: u32 = 512;

const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const MODE_SENSE_6: u8 = 0x1a;
const START_STOP_UNIT: u8 = 0x1b;
const READ_CAPACITY_10: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2a;

/// Software-visible state of one read-only SCSI CD-ROM target.
pub struct ScsiCdrom {
    logical_block_count: u64,
    ready: bool,
    sense: SenseData,
}

impl ScsiCdrom {
    /// Creates a ready target with the supplied nonzero logical block count.
    ///
    /// # Panics
    ///
    /// Panics when `logical_block_count` is zero or cannot be represented by
    /// `READ CAPACITY(10)`.
    #[must_use]
    pub const fn new(logical_block_count: u64) -> Self {
        assert!(logical_block_count != 0);
        assert!(logical_block_count <= u32::MAX as u64 + 1);
        Self {
            logical_block_count,
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
            START_STOP_UNIT if cdb.len() >= 6 => self.start_stop(cdb[4]),
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
        complete_good(sense.fixed_response(allocation_length))
    }

    fn inquiry(&self, allocation_length: u8) -> ScsiCommandPlan {
        let mut data = vec![0; 36];
        data[0] = 0x05;
        data[1] = 0x80;
        data[2] = 0x01;
        data[3] = 0x01;
        data[4] = 31;
        data[8..16].copy_from_slice(b"SGI-EMU ");
        data[16..32].copy_from_slice(b"VIRTUAL CD-ROM  ");
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

    fn start_stop(&mut self, control: u8) -> ScsiCommandPlan {
        if control & 0x02 != 0 {
            return self.check_condition(SenseData::INVALID_CDB_FIELD);
        }
        self.ready = control & 1 != 0;
        complete_good(Vec::new())
    }

    fn read_capacity(&mut self) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        let last_lba = u32::try_from(self.logical_block_count - 1)
            .expect("validated SCSI CD-ROM capacity must fit READ CAPACITY(10)");
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
        if end > self.logical_block_count {
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
    use crate::scsi::{ScsiCommandPlan, ScsiStatus};

    use super::ScsiCdrom;

    fn sense(cdrom: &mut ScsiCdrom, allocation_length: u8) -> Vec<u8> {
        let ScsiCommandPlan::Complete { status, data_in } =
            cdrom.execute(&[0x03, 0, 0, 0, allocation_length, 0])
        else {
            panic!("REQUEST SENSE should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        data_in
    }

    #[test]
    fn inquiry_reports_cdrom_identity_and_honors_allocation_length() {
        let mut cdrom = ScsiCdrom::new(4);
        let ScsiCommandPlan::Complete { status, data_in } = cdrom.execute(&[0x12, 0, 0, 0, 36, 0])
        else {
            panic!("INQUIRY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(data_in[0], 5);
        assert_eq!(data_in[1], 0x80);
        assert_eq!(&data_in[8..16], b"SGI-EMU ");
        assert_eq!(&data_in[16..32], b"VIRTUAL CD-ROM  ");
        assert_eq!(&data_in[32..36], b"0001");

        let ScsiCommandPlan::Complete { data_in, .. } = cdrom.execute(&[0x12, 0, 0, 0, 7, 0])
        else {
            panic!("INQUIRY should complete immediately");
        };
        assert_eq!(data_in.len(), 7);
    }

    #[test]
    fn capacity_and_mode_sense_report_512_byte_logical_blocks() {
        let mut cdrom = ScsiCdrom::new(0x1235);
        let ScsiCommandPlan::Complete { status, data_in } =
            cdrom.execute(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        else {
            panic!("READ CAPACITY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(data_in, [0, 0, 0x12, 0x34, 0, 0, 2, 0]);

        let ScsiCommandPlan::Complete { data_in, .. } = cdrom.execute(&[0x1a, 0, 0, 0, 12, 0])
        else {
            panic!("MODE SENSE should complete immediately");
        };
        assert_eq!(&data_in[9..12], [0, 2, 0]);
    }

    #[test]
    fn read_ten_returns_logical_block_work_and_validates_the_range() {
        let mut cdrom = ScsiCdrom::new(8);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[5] = 1;
        cdb[8] = 2;
        assert_eq!(
            cdrom.execute(&cdb),
            ScsiCommandPlan::ReadBlocks {
                lba: 1,
                block_count: 2,
            }
        );

        cdb[5] = 7;
        assert!(matches!(
            cdrom.execute(&cdb),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));
        assert_eq!(
            &sense(&mut cdrom, 18)[2..14],
            [5, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0x21, 0]
        );
    }

    #[test]
    fn start_stop_changes_readiness_and_loej_is_rejected_without_state_change() {
        let mut cdrom = ScsiCdrom::new(4);
        let _ = cdrom.execute(&[0x1b, 0, 0, 0, 0, 0]);

        for cdb in [
            &[0x00, 0, 0, 0, 0, 0][..],
            &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
            &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0][..],
        ] {
            assert!(matches!(
                cdrom.execute(cdb),
                ScsiCommandPlan::Complete {
                    status: ScsiStatus::CheckCondition,
                    ..
                }
            ));
            let data = sense(&mut cdrom, 18);
            assert_eq!((data[2], data[12], data[13]), (2, 0x04, 0x02));
        }

        assert!(matches!(
            cdrom.execute(&[0x12, 0, 0, 0, 36, 0]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::Good,
                ..
            }
        ));
        assert!(matches!(
            cdrom.execute(&[0x1a, 0, 0, 0, 12, 0]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::Good,
                ..
            }
        ));

        let _ = cdrom.execute(&[0x1b, 0, 0, 0, 3, 0]);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (5, 0x24, 0));
        assert!(matches!(
            cdrom.execute(&[0x00, 0, 0, 0, 0, 0]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::CheckCondition,
                ..
            }
        ));
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (2, 0x04, 0x02));

        let _ = cdrom.execute(&[0x1b, 0, 0, 0, 1, 0]);
        assert!(matches!(
            cdrom.execute(&[0x00, 0, 0, 0, 0, 0]),
            ScsiCommandPlan::Complete {
                status: ScsiStatus::Good,
                ..
            }
        ));
    }

    #[test]
    fn command_failures_report_distinct_sense_and_success_does_not_clear_it() {
        let mut cdrom = ScsiCdrom::new(4);
        let _ = cdrom.execute(&[0xff]);
        let _ = cdrom.execute(&[0x12, 0, 0, 0, 36, 0]);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (5, 0x20, 0));

        let _ = cdrom.execute(&[]);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (5, 0x24, 0));

        let _ = cdrom.execute(&[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0]);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (7, 0x27, 0));
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (0, 0, 0));
    }

    #[test]
    fn host_read_failure_becomes_hardware_error_sense() {
        let mut cdrom = ScsiCdrom::new(4);
        assert_eq!(cdrom.complete_read(false), ScsiStatus::CheckCondition);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (4, 0x44, 0));
    }
}

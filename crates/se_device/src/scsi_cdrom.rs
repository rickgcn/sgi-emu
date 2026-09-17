//! Functional read-only SCSI CD-ROM target backed by a raw media image.

use serde::{Deserialize, Serialize};

use crate::scsi::{
    ScsiCommandPlan, ScsiStatus, ScsiStorageSizeError, ScsiTarget, ScsiTargetSnapshot, SenseData,
};

const INITIAL_LOGICAL_BLOCK_BYTES: u32 = 512;
const SUPPORTED_LOGICAL_BLOCK_BYTES: [u32; 2] = [INITIAL_LOGICAL_BLOCK_BYTES, 2048];
const STORAGE_ALIGNMENT_BYTES: u64 = 2048;

const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const READ_6: u8 = 0x08;
const INQUIRY: u8 = 0x12;
const MODE_SELECT_6: u8 = 0x15;
const MODE_SENSE_6: u8 = 0x1a;
const START_STOP_UNIT: u8 = 0x1b;
const PREVENT_ALLOW_MEDIUM_REMOVAL: u8 = 0x1e;
const READ_CAPACITY_10: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2a;
const SGI_HD_TO_CDROM: u8 = 0xc9;

/// Software-visible state of one read-only SCSI CD-ROM target.
#[derive(Clone, Deserialize, Serialize)]
pub struct ScsiCdrom {
    storage_bytes: u64,
    logical_block_bytes: u32,
    ready: bool,
    sense: SenseData,
}

impl ScsiCdrom {
    /// Creates a ready target for a validated raw-media capacity.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiStorageSizeError`] when `storage_bytes` is zero, is not
    /// a multiple of 2048 bytes, or cannot be represented as 512-byte logical
    /// blocks by `READ CAPACITY(10)`.
    pub fn try_new(storage_bytes: u64) -> Result<Self, ScsiStorageSizeError> {
        if storage_bytes == 0
            || !storage_bytes.is_multiple_of(STORAGE_ALIGNMENT_BYTES)
            || storage_bytes / u64::from(INITIAL_LOGICAL_BLOCK_BYTES) > u64::from(u32::MAX) + 1
        {
            return Err(ScsiStorageSizeError::new(storage_bytes));
        }
        Ok(Self {
            storage_bytes,
            logical_block_bytes: INITIAL_LOGICAL_BLOCK_BYTES,
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
        data[9..12].copy_from_slice(&self.logical_block_bytes.to_be_bytes()[1..]);
        data.truncate(usize::from(allocation_length));
        complete_good(data)
    }

    fn mode_select(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        if cdb[1] != 0 {
            return self.check_condition(SenseData::INVALID_CDB_FIELD);
        }
        ScsiCommandPlan::ReceiveDataOut {
            byte_count: u64::from(cdb[4]),
        }
    }

    fn complete_mode_select(&mut self, cdb: &[u8], data: &[u8]) -> ScsiStatus {
        let valid_length = cdb.len() >= 6 && usize::from(cdb[4]) == data.len();
        if !valid_length || data.len() != 12 || data[3] != 8 {
            return self.parameter_list_error();
        }

        let logical_block_bytes = u32::from_be_bytes([0, data[9], data[10], data[11]]);
        if !SUPPORTED_LOGICAL_BLOCK_BYTES.contains(&logical_block_bytes)
            || !self
                .storage_bytes
                .is_multiple_of(u64::from(logical_block_bytes))
        {
            return self.parameter_list_error();
        }

        self.logical_block_bytes = logical_block_bytes;
        ScsiStatus::Good
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
        let Some(last_lba) = self
            .logical_block_count()
            .checked_sub(1)
            .and_then(|last_lba| u32::try_from(last_lba).ok())
        else {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        };
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&last_lba.to_be_bytes());
        data.extend_from_slice(&self.logical_block_bytes.to_be_bytes());
        complete_good(data)
    }

    fn read_6(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
        // READ(6) carries a 21-bit LBA and encodes 256 blocks as length zero.
        let lba = u32::from_be_bytes([0, cdb[1] & 0x1f, cdb[2], cdb[3]]);
        let block_count = if cdb[4] == 0 { 256 } else { u16::from(cdb[4]) };
        self.read_blocks(lba, block_count)
    }

    fn read_blocks(&mut self, lba: u32, block_count: u16) -> ScsiCommandPlan {
        if !self.ready {
            return self.check_condition(SenseData::NOT_READY);
        }
        if block_count == 0 {
            return complete_good(Vec::new());
        }
        let Some(end) = u64::from(lba).checked_add(u64::from(block_count)) else {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        };
        if end > self.logical_block_count() {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        }
        let Some(offset) = u64::from(lba).checked_mul(u64::from(self.logical_block_bytes)) else {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        };
        let Some(byte_count) =
            u64::from(block_count).checked_mul(u64::from(self.logical_block_bytes))
        else {
            return self.check_condition(SenseData::LBA_OUT_OF_RANGE);
        };
        ScsiCommandPlan::ReadStorage { offset, byte_count }
    }

    fn logical_block_count(&self) -> u64 {
        self.storage_bytes / u64::from(self.logical_block_bytes)
    }

    fn accepts_cdrom_snapshot(&self, state: &Self) -> bool {
        state.storage_bytes == self.storage_bytes
            && SUPPORTED_LOGICAL_BLOCK_BYTES.contains(&state.logical_block_bytes)
            && state
                .storage_bytes
                .is_multiple_of(u64::from(state.logical_block_bytes))
    }

    fn parameter_list_error(&mut self) -> ScsiStatus {
        self.sense = SenseData::INVALID_PARAMETER_LIST;
        ScsiStatus::CheckCondition
    }

    fn check_condition(&mut self, sense: SenseData) -> ScsiCommandPlan {
        self.sense = sense;
        ScsiCommandPlan::Complete {
            status: ScsiStatus::CheckCondition,
            data_in: Vec::new(),
        }
    }
}

impl ScsiTarget for ScsiCdrom {
    fn storage_size_bytes(&self) -> u64 {
        self.storage_bytes
    }

    fn snapshot(&self) -> Option<ScsiTargetSnapshot> {
        Some(ScsiTargetSnapshot::Cdrom(self.clone()))
    }

    fn accepts_snapshot(&self, snapshot: &ScsiTargetSnapshot) -> bool {
        matches!(snapshot, ScsiTargetSnapshot::Cdrom(state) if self.accepts_cdrom_snapshot(state))
    }

    fn restore_snapshot(&mut self, snapshot: ScsiTargetSnapshot) -> bool {
        let ScsiTargetSnapshot::Cdrom(state) = snapshot else {
            return false;
        };
        if !self.accepts_cdrom_snapshot(&state) {
            return false;
        }
        *self = state;
        true
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
            READ_6 if cdb.len() >= 6 => self.read_6(cdb),
            INQUIRY if cdb.len() >= 6 => self.inquiry(cdb[4]),
            MODE_SELECT_6 if cdb.len() >= 6 => self.mode_select(cdb),
            MODE_SENSE_6 if cdb.len() >= 6 => self.mode_sense(cdb[4]),
            START_STOP_UNIT if cdb.len() >= 6 => self.start_stop(cdb[4]),
            PREVENT_ALLOW_MEDIUM_REMOVAL if cdb.len() >= 6 => complete_good(Vec::new()),
            READ_CAPACITY_10 if cdb.len() >= 10 => self.read_capacity(),
            READ_10 if cdb.len() >= 10 => self.read_blocks(
                u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]),
                u16::from_be_bytes([cdb[7], cdb[8]]),
            ),
            WRITE_10 if cdb.len() >= 10 => self.check_condition(SenseData::WRITE_PROTECTED),
            SGI_HD_TO_CDROM if cdb.len() >= 6 => complete_good(Vec::new()),
            TEST_UNIT_READY
            | REQUEST_SENSE
            | INQUIRY
            | MODE_SELECT_6
            | MODE_SENSE_6
            | START_STOP_UNIT
            | PREVENT_ALLOW_MEDIUM_REMOVAL
            | READ_CAPACITY_10
            | READ_6
            | READ_10
            | WRITE_10
            | SGI_HD_TO_CDROM => self.check_condition(SenseData::INVALID_CDB_FIELD),
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

    fn complete_data_out(&mut self, cdb: &[u8], data: &[u8]) -> ScsiStatus {
        if cdb.first() == Some(&MODE_SELECT_6) {
            self.complete_mode_select(cdb, data)
        } else {
            self.sense = SenseData::INVALID_CDB_FIELD;
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
    use crate::scsi::{ScsiCommandPlan, ScsiStatus, ScsiTarget, ScsiTargetSnapshot};

    use super::{INITIAL_LOGICAL_BLOCK_BYTES, SUPPORTED_LOGICAL_BLOCK_BYTES, ScsiCdrom};

    fn cdrom(logical_block_count: u64) -> ScsiCdrom {
        ScsiCdrom::try_new(logical_block_count * u64::from(INITIAL_LOGICAL_BLOCK_BYTES)).unwrap()
    }

    fn mode_select_payload(logical_block_bytes: u32) -> [u8; 12] {
        let mut data = [0; 12];
        data[3] = 8;
        data[9..12].copy_from_slice(&logical_block_bytes.to_be_bytes()[1..]);
        data
    }

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
        let mut cdrom = cdrom(4);
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
        let mut cdrom = cdrom(0x1238);
        assert_eq!(cdrom.logical_block_bytes, INITIAL_LOGICAL_BLOCK_BYTES);
        let ScsiCommandPlan::Complete { status, data_in } =
            cdrom.execute(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        else {
            panic!("READ CAPACITY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(data_in, [0, 0, 0x12, 0x37, 0, 0, 2, 0]);

        let ScsiCommandPlan::Complete { data_in, .. } = cdrom.execute(&[0x1a, 0, 0, 0, 12, 0])
        else {
            panic!("MODE SENSE should complete immediately");
        };
        assert_eq!(&data_in[9..12], [0, 2, 0]);
    }

    #[test]
    fn mode_select_switches_capacity_mode_sense_and_reads_to_2048_bytes() {
        let mut cdrom = ScsiCdrom::try_new(8192).unwrap();
        let cdb = [0x15, 0, 0, 0, 12, 0];
        let selected_block_bytes = SUPPORTED_LOGICAL_BLOCK_BYTES[1];
        assert_eq!(
            cdrom.execute(&cdb),
            ScsiCommandPlan::ReceiveDataOut { byte_count: 12 }
        );
        assert_eq!(
            cdrom.complete_data_out(&cdb, &mode_select_payload(selected_block_bytes)),
            ScsiStatus::Good
        );
        assert_eq!(cdrom.logical_block_bytes, selected_block_bytes);
        assert_eq!(cdrom.storage_size_bytes(), 8192);

        let ScsiCommandPlan::Complete { data_in, .. } = cdrom.execute(&[0x1a, 0, 0, 0, 12, 0])
        else {
            panic!("MODE SENSE should complete immediately");
        };
        assert_eq!(&data_in[9..12], [0, 8, 0]);

        let ScsiCommandPlan::Complete { status, data_in } =
            cdrom.execute(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        else {
            panic!("READ CAPACITY should complete immediately");
        };
        assert_eq!(status, ScsiStatus::Good);
        assert_eq!(data_in, [0, 0, 0, 3, 0, 0, 8, 0]);

        assert_eq!(
            cdrom.execute(&[0x28, 0, 0, 0, 0, 1, 0, 0, 1, 0]),
            ScsiCommandPlan::ReadStorage {
                offset: 2048,
                byte_count: 2048,
            }
        );

        assert_eq!(
            cdrom.complete_data_out(&cdb, &mode_select_payload(INITIAL_LOGICAL_BLOCK_BYTES)),
            ScsiStatus::Good
        );
        assert_eq!(cdrom.logical_block_bytes, INITIAL_LOGICAL_BLOCK_BYTES);
    }

    #[test]
    fn malformed_mode_select_data_preserves_the_logical_block_size() {
        let cdb = [0x15, 0, 0, 0, 12, 0];
        let mut cdrom = ScsiCdrom::try_new(8192).unwrap();
        let selected_block_bytes = SUPPORTED_LOGICAL_BLOCK_BYTES[1];
        let mut invalid_descriptor = mode_select_payload(selected_block_bytes);
        invalid_descriptor[3] = 0;
        let short_payload = &mode_select_payload(selected_block_bytes)[..11];
        let unsupported = mode_select_payload(1024);

        for data in [&invalid_descriptor[..], short_payload, &unsupported[..]] {
            assert_eq!(
                cdrom.complete_data_out(&cdb, data),
                ScsiStatus::CheckCondition
            );
            assert_eq!(cdrom.logical_block_bytes, INITIAL_LOGICAL_BLOCK_BYTES);
            let data = sense(&mut cdrom, 18);
            assert_eq!((data[2], data[12], data[13]), (5, 0x26, 0));
        }
    }

    #[test]
    fn snapshot_restores_a_dynamic_block_size_into_a_cold_target() {
        let mut source = ScsiCdrom::try_new(8192).unwrap();
        let cdb = [0x15, 0, 0, 0, 12, 0];
        let selected_block_bytes = SUPPORTED_LOGICAL_BLOCK_BYTES[1];
        assert_eq!(
            source.complete_data_out(&cdb, &mode_select_payload(selected_block_bytes)),
            ScsiStatus::Good
        );
        let snapshot = source.snapshot().unwrap();

        let mut restored = ScsiCdrom::try_new(8192).unwrap();
        assert!(restored.accepts_snapshot(&snapshot));
        assert!(restored.restore_snapshot(snapshot));
        assert_eq!(restored.logical_block_bytes, selected_block_bytes);
        assert_eq!(restored.storage_size_bytes(), 8192);

        let incompatible = ScsiTargetSnapshot::Cdrom(ScsiCdrom::try_new(4096).unwrap());
        assert!(!restored.accepts_snapshot(&incompatible));
    }

    #[test]
    fn prevent_allow_and_sgi_compatibility_commands_are_no_op_successes() {
        let mut cdrom = cdrom(4);
        for cdb in [&[0x1e, 0, 0, 0, 1, 0][..], &[0xc9, 0, 0, 0, 0, 0][..]] {
            assert_eq!(
                cdrom.execute(cdb),
                ScsiCommandPlan::Complete {
                    status: ScsiStatus::Good,
                    data_in: Vec::new(),
                }
            );
        }
    }

    #[test]
    fn read_six_decodes_lba_zero_length_and_media_boundaries() {
        let mut cdrom = cdrom(0x20_0000);
        for (cdb, lba, count) in [
            ([8, 0xe1, 0x23, 0x45, 2, 0], 0x1_2345, 2),
            ([8, 0xff, 0xff, 0xff, 1, 0], 0x1f_ffff, 1),
            ([8, 0x1f, 0xff, 0, 0, 0], 0x1f_ff00, 256),
        ] {
            assert_eq!(
                cdrom.execute(&cdb),
                ScsiCommandPlan::ReadStorage {
                    offset: lba * u64::from(INITIAL_LOGICAL_BLOCK_BYTES),
                    byte_count: count * u64::from(INITIAL_LOGICAL_BLOCK_BYTES)
                }
            );
        }
        cdrom.execute(&[8, 0x1f, 0xff, 1, 0, 0]);
        assert_eq!(sense(&mut cdrom, 18)[12], 0x21);
        cdrom.execute(&[0x1b, 0, 0, 0, 0, 0]);
        cdrom.execute(&[8, 0, 0, 0, 1, 0]);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (2, 4, 2));
    }

    #[test]
    fn truncated_read_six_reports_invalid_cdb_fields() {
        let mut cdrom = cdrom(4);
        for length in 1..6 {
            cdrom.execute(&[8, 0, 0, 0, 1, 0][..length]);
            let data = sense(&mut cdrom, 18);
            assert_eq!((data[2], data[12]), (5, 0x24));
        }
    }

    #[test]
    fn read_ten_returns_logical_block_work_and_validates_the_range() {
        let mut cdrom = cdrom(8);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[5] = 1;
        cdb[8] = 2;
        assert_eq!(
            cdrom.execute(&cdb),
            ScsiCommandPlan::ReadStorage {
                offset: u64::from(INITIAL_LOGICAL_BLOCK_BYTES),
                byte_count: 2 * u64::from(INITIAL_LOGICAL_BLOCK_BYTES),
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
        let mut cdrom = cdrom(4);
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
        let mut cdrom = cdrom(4);
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
    fn host_storage_failure_becomes_hardware_error_sense() {
        let mut cdrom = cdrom(4);
        assert_eq!(cdrom.complete_storage(false), ScsiStatus::CheckCondition);
        let data = sense(&mut cdrom, 18);
        assert_eq!((data[2], data[12], data[13]), (4, 0x44, 0));
    }
}

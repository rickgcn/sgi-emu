//! Shared types for functional SCSI targets.

const FIXED_SENSE_BYTES: usize = 18;

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
pub(crate) struct SenseData {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl SenseData {
    pub(crate) const NONE: Self = Self::new(0, 0, 0);
    pub(crate) const UNSUPPORTED_OPCODE: Self = Self::new(5, 0x20, 0);
    pub(crate) const INVALID_CDB_FIELD: Self = Self::new(5, 0x24, 0);
    pub(crate) const LBA_OUT_OF_RANGE: Self = Self::new(5, 0x21, 0);
    pub(crate) const WRITE_PROTECTED: Self = Self::new(7, 0x27, 0);
    pub(crate) const HOST_READ_ERROR: Self = Self::new(4, 0x44, 0);
    pub(crate) const NOT_READY: Self = Self::new(2, 0x04, 0x02);

    const fn new(key: u8, asc: u8, ascq: u8) -> Self {
        Self { key, asc, ascq }
    }

    pub(crate) fn fixed_response(self, allocation_length: u8) -> Vec<u8> {
        let mut data = vec![0; FIXED_SENSE_BYTES];
        data[0] = 0x70;
        data[2] = self.key;
        data[7] = 10;
        data[12] = self.asc;
        data[13] = self.ascq;
        data.truncate(usize::from(allocation_length));
        data
    }
}

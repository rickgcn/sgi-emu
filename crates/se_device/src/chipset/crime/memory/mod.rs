//! CRIME SDRAM device and memory-domain bus.

pub mod bus;
mod ecc;
pub(crate) mod framebuffer;

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
use se_core::role::BusDeviceRole;

use super::config::{CrimeMemoryConfig, CrimeSdramBankConfig};
use super::protocol::{
    CrimeBusError, CrimeByteEnableView, CrimeCompletionPayload, CrimeData, CrimeMemoryBankSelect,
    CrimeMemoryCompletion, CrimeMemoryDiagnostic, CrimeMemoryFault, CrimeMemoryOutcome,
    CrimeMemoryTransaction, CrimeSdramSignal, CrimeSynchronousMemoryTarget, CrimeTransferView,
};
use super::registers;

#[cfg(test)]
mod tests;

const PAGE_SIZE: usize = 4096;
const MAX_TRANSFER_BYTES: usize = 16 * 32;
const BANK_32_MIB_ADDRESS_SHIFT: u32 = 25;
const BANK_128_MIB_ADDRESS_SHIFT: u32 = 27;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum BankSelection {
    Populated { index: usize, offset: u64 },
    Unpopulated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SparseBank {
    config: CrimeSdramBankConfig,
    pages: Vec<Option<Box<SparsePage>>>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct SparseBankState {
    config: CrimeSdramBankConfig,
    pages: Vec<Option<Box<SparsePage>>>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct SparsePage {
    #[serde(with = "crate::common::serde_array")]
    data: [u8; PAGE_SIZE],
    #[serde(with = "crate::common::serde_array")]
    ecc: [u8; PAGE_SIZE / 8],
}

impl SparseBank {
    fn new(config: CrimeSdramBankConfig) -> Self {
        Self {
            config,
            pages: vec![None; config.size.bytes() as usize / PAGE_SIZE],
        }
    }

    #[cfg(test)]
    fn read_byte(&self, offset: u64) -> u8 {
        let page = offset as usize / PAGE_SIZE;
        let in_page = offset as usize % PAGE_SIZE;
        self.pages[page]
            .as_ref()
            .map_or(0, |contents| contents.data[in_page])
    }

    fn read_lane(&self, offset: u64) -> (u64, u8) {
        let aligned = offset & !7;
        let aligned = aligned as usize;
        let page = aligned / PAGE_SIZE;
        let in_page = aligned % PAGE_SIZE;
        let lane = in_page / 8;
        let Some(contents) = self.pages[page].as_ref() else {
            return (0, 0);
        };
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&contents.data[in_page..in_page + 8]);
        (u64::from_le_bytes(bytes), contents.ecc[lane])
    }

    fn write_lane(&mut self, offset: u64, data: u64, check: u8) {
        let aligned = offset & !7;
        let aligned = aligned as usize;
        let page = aligned / PAGE_SIZE;
        let in_page = aligned % PAGE_SIZE;
        let lane = in_page / 8;
        if data == 0 && check == 0 && self.pages[page].is_none() {
            return;
        }
        let contents = self.page_mut(page);
        contents.data[in_page..in_page + 8].copy_from_slice(&data.to_le_bytes());
        contents.ecc[lane] = check;
    }

    fn page_mut(&mut self, page: usize) -> &mut SparsePage {
        self.pages[page]
            .get_or_insert_with(|| {
                Box::new(SparsePage {
                    data: [0; PAGE_SIZE],
                    ecc: [0; PAGE_SIZE / 8],
                })
            })
            .as_mut()
    }

    fn clear(&mut self) {
        self.pages.fill(None);
    }
}

/// CRIME-attached sparse SDRAM with eight physical external banks.
///
/// Reads from a bank selected by its control register but lacking physical
/// DIMMs return zero-filled data. This is a deterministic functional-model
/// convention and does not claim to reproduce undriven electrical bus levels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeSdram {
    id: ComponentId,
    name: String,
    banks: [Option<SparseBank>; 8],
    bank_control: [u16; 8],
    ecc_enabled: bool,
    use_replacement: bool,
    replacement: u8,
}

/// Serializable sparse contents and programmable state of CRIME SDRAM.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct CrimeSdramState {
    id: ComponentId,
    banks: [Option<SparseBankState>; 8],
    bank_control: [u16; 8],
    ecc_enabled: bool,
    use_replacement: bool,
    replacement: u8,
}

impl CrimeSdram {
    /// Creates zero-filled SDRAM from an explicit physical topology.
    pub fn new(id: ComponentId, name: impl Into<String>, config: CrimeMemoryConfig) -> Self {
        let bank_control = reset_bank_control(config);
        Self {
            id,
            name: name.into(),
            banks: config.banks.map(|bank| bank.map(SparseBank::new)),
            bank_control,
            ecc_enabled: false,
            use_replacement: false,
            replacement: 0,
        }
    }

    /// Captures sparse memory contents and programmable controller-facing state.
    pub fn save_state(&self) -> CrimeSdramState {
        CrimeSdramState {
            id: self.id,
            banks: std::array::from_fn(|index| {
                self.banks[index].as_ref().map(|bank| SparseBankState {
                    config: bank.config,
                    pages: bank.pages.clone(),
                })
            }),
            bank_control: self.bank_control,
            ecc_enabled: self.ecc_enabled,
            use_replacement: self.use_replacement,
            replacement: self.replacement,
        }
    }

    /// Restores sparse contents after validating the physical bank topology.
    pub fn restore_state(&mut self, state: CrimeSdramState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        for (current, restored) in self.banks.iter().zip(&state.banks) {
            match (current, restored) {
                (Some(current), Some(restored)) if current.config == restored.config => {
                    let expected_pages = current.config.size.bytes() as usize / PAGE_SIZE;
                    if restored.pages.len() != expected_pages {
                        return Err(ComponentStateError::InvalidState {
                            component: self.id,
                            invariant: "SDRAM sparse page table length must match bank capacity",
                        });
                    }
                }
                (None, None) => {}
                _ => {
                    return Err(ComponentStateError::ConfigurationMismatch {
                        component: self.id,
                        field: "SDRAM physical bank topology",
                    });
                }
            }
        }
        if state
            .bank_control
            .iter()
            .any(|control| control & !registers::MEMORY_BANK_CONTROL_MASK != 0)
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "SDRAM bank controls must use implemented bit encodings",
            });
        }
        for (current, restored) in self.banks.iter_mut().zip(state.banks) {
            if let (Some(current), Some(restored)) = (current, restored) {
                current.pages = restored.pages;
            }
        }
        self.bank_control = state.bank_control;
        self.ecc_enabled = state.ecc_enabled;
        self.use_replacement = state.use_replacement;
        self.replacement = state.replacement;
        Ok(())
    }

    /// Returns total installed physical capacity.
    pub fn total_size_bytes(&self) -> u64 {
        self.banks
            .iter()
            .flatten()
            .map(|bank| bank.config.size.bytes())
            .sum()
    }

    /// Returns one programmable bank-control value.
    pub fn bank_control(&self, bank: usize) -> Option<u16> {
        self.bank_control.get(bank).copied()
    }

    /// Reads a side-effect-free code window and fingerprints its data, ECC,
    /// and programmable mapping state.
    pub fn stable_code_window(
        &self,
        address: u64,
        length: usize,
        no_ecc: bool,
    ) -> Option<(Vec<u8>, u64)> {
        if length == 0 || length > 128 {
            return None;
        }
        let _ = address.checked_add(length as u64)?;
        let mut output = Vec::with_capacity(length);
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        let mut position = 0;
        while position < length {
            let current = address + position as u64;
            let selection = self.decode(current)?;
            let (data, check, mapping) = match selection {
                BankSelection::Populated {
                    index: bank,
                    offset,
                } => {
                    let (data, check) = self.banks[bank]
                        .as_ref()
                        .expect("decoded bank exists")
                        .read_lane(offset & !7);
                    if self.ecc_enabled
                        && !no_ecc
                        && !matches!(ecc::check(data, check), ecc::EccCheck::Clean { .. })
                    {
                        return None;
                    }
                    (
                        data,
                        check,
                        (u64::from(self.bank_control[bank]) << 8) | bank as u64,
                    )
                }
                BankSelection::Unpopulated => (0, 0, u64::MAX),
            };
            for byte in data.to_le_bytes() {
                fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            fingerprint = (fingerprint ^ u64::from(check)).wrapping_mul(0x0000_0100_0000_01b3);
            fingerprint = (fingerprint ^ mapping).wrapping_mul(0x0000_0100_0000_01b3);
            let bytes = data.to_le_bytes();
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(length - position);
            output.extend_from_slice(&bytes[in_lane..in_lane + count]);
            position += count;
        }
        fingerprint ^= u64::from(self.ecc_enabled)
            | (u64::from(self.use_replacement) << 1)
            | (u64::from(self.replacement) << 8)
            | (u64::from(no_ecc) << 16);
        Some((output, fingerprint))
    }

    /// Reads raw data into `output` without ECC checking or side effects.
    ///
    /// Bytes in unpopulated or undecodable address ranges read as zero. This is
    /// the bulk counterpart of [`Self::stable_code_window`] for display-path
    /// consumers whose hardware reads do not participate in ECC diagnostics.
    pub fn read_raw_window(&self, address: u64, output: &mut [u8]) {
        output.fill(0);
        let mut position = 0;
        while position < output.len() {
            let Some(current) = address.checked_add(position as u64) else {
                return;
            };
            let Some(BankSelection::Populated { index, offset }) = self.decode(current) else {
                position += 1;
                continue;
            };
            let bank = self.banks[index].as_ref().expect("decoded bank exists");
            let bank_remaining = bank.config.size.bytes() - offset;
            let count = (output.len() - position).min(bank_remaining as usize);
            let mut consumed = 0;
            while consumed < count {
                let bank_offset = offset as usize + consumed;
                let page = bank_offset / PAGE_SIZE;
                let in_page = bank_offset % PAGE_SIZE;
                let chunk = (PAGE_SIZE - in_page).min(count - consumed);
                if let Some(contents) = bank.pages[page].as_ref() {
                    output[position + consumed..position + consumed + chunk]
                        .copy_from_slice(&contents.data[in_page..in_page + chunk]);
                }
                consumed += chunk;
            }
            position += count;
        }
    }

    /// Returns whether an immediate CPU transaction is free of exceptional
    /// ECC, address, and read-modify-write diagnostics.
    pub fn synchronous_transaction_ready(&self, transaction: &CrimeMemoryTransaction) -> bool {
        let length = transaction.transfer.length();
        if length == 0
            || length > MAX_TRANSFER_BYTES
            || !matches!(transaction.bank_select, CrimeMemoryBankSelect::Decode)
            || transaction
                .address
                .checked_add(length.saturating_sub(1) as u64)
                .is_none()
        {
            return false;
        }
        match transaction.transfer.view() {
            CrimeTransferView::Read { .. } => {
                self.synchronous_read_ready(transaction.address, length, transaction.no_ecc)
            }
            CrimeTransferView::Write { data, byte_enable } => {
                data.len() == byte_enable.len()
                    && self.synchronous_write_ready(transaction.address, data, byte_enable)
            }
        }
    }

    /// Completes one previously decoded synchronous CPU read.
    pub fn read_synchronous_cpu(
        &mut self,
        target: CrimeSynchronousMemoryTarget,
        length: usize,
    ) -> Option<u64> {
        if !matches!(length, 1 | 2 | 4 | 8) {
            return None;
        }
        let in_lane = target.address() as usize & 7;
        if in_lane + length <= 8 {
            target
                .address()
                .checked_add(length.saturating_sub(1) as u64)?;
            let selection = self.decode(target.address())?;
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                return Some(0);
            };
            let (data, stored_check) = self.banks[bank_index]
                .as_ref()
                .expect("decoded bank exists")
                .read_lane(bank_offset & !7);
            if self.ecc_enabled
                && !target.no_ecc()
                && !matches!(ecc::check(data, stored_check), ecc::EccCheck::Clean { .. })
            {
                return None;
            }
            let shifted = data >> (in_lane * 8);
            let mask = if length == 8 {
                u64::MAX
            } else {
                (1_u64 << (length * 8)) - 1
            };
            return Some(shifted & mask);
        }
        let mut output = [0; 8];
        let mut position = 0;
        while position < length {
            let current = target.address().checked_add(position as u64)?;
            let selection = self.decode(current)?;
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                position += (8 - (current as usize & 7)).min(length - position);
                continue;
            };
            let lane_offset = bank_offset & !7;
            let in_lane = bank_offset as usize & 7;
            let count = (8 - in_lane).min(length - position);
            let (data, stored_check) = self.banks[bank_index]
                .as_ref()
                .expect("decoded bank exists")
                .read_lane(lane_offset);
            if self.ecc_enabled
                && !target.no_ecc()
                && !matches!(ecc::check(data, stored_check), ecc::EccCheck::Clean { .. })
            {
                return None;
            }
            output[position..position + count]
                .copy_from_slice(&data.to_le_bytes()[in_lane..in_lane + count]);
            position += count;
        }
        Some(u64::from_le_bytes(output))
    }

    /// Completes one previously decoded synchronous CPU write.
    pub fn write_synchronous_cpu(
        &mut self,
        target: CrimeSynchronousMemoryTarget,
        length: usize,
        data: u64,
        byte_enable: u8,
    ) -> bool {
        if !matches!(length, 1 | 2 | 4 | 8) {
            return false;
        }
        let data = data.to_le_bytes();
        let in_lane = target.address() as usize & 7;
        if in_lane + length <= 8 {
            if target
                .address()
                .checked_add(length.saturating_sub(1) as u64)
                .is_none()
            {
                return false;
            }
            let Some(selection) = self.decode(target.address()) else {
                return false;
            };
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                return true;
            };
            let lane_offset = bank_offset & !7;
            let read_modify_write = in_lane != 0
                || length != 8
                || (0..length).any(|index| byte_enable & (1 << index) == 0);
            let mut bytes = if read_modify_write {
                let (old_data, stored_check) = self.banks[bank_index]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(lane_offset);
                if self.ecc_enabled
                    && !matches!(
                        ecc::check(old_data, stored_check),
                        ecc::EccCheck::Clean { .. }
                    )
                {
                    return false;
                }
                old_data.to_le_bytes()
            } else {
                [0; 8]
            };
            for index in 0..length {
                if byte_enable & (1 << index) != 0 {
                    bytes[in_lane + index] = data[index];
                }
            }
            let data = u64::from_le_bytes(bytes);
            let check = if self.use_replacement {
                self.replacement
            } else {
                ecc::generate(data)
            };
            self.banks[bank_index]
                .as_mut()
                .expect("decoded bank exists")
                .write_lane(lane_offset, data, check);
            return true;
        }
        let mut position = 0;
        while position < length {
            let Some(current) = target.address().checked_add(position as u64) else {
                return false;
            };
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(length - position);
            let read_modify_write = in_lane != 0
                || count != 8
                || (position..position + count).any(|index| byte_enable & (1 << index) == 0);
            let Some(selection) = self.decode(current) else {
                return false;
            };
            if let BankSelection::Populated {
                index: bank,
                offset,
            } = selection
                && read_modify_write
                && self.ecc_enabled
            {
                let (old_data, stored_check) = self.banks[bank]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(offset & !7);
                if !matches!(
                    ecc::check(old_data, stored_check),
                    ecc::EccCheck::Clean { .. }
                ) {
                    return false;
                }
            }
            position += count;
        }

        position = 0;
        while position < length {
            let current = target.address() + position as u64;
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(length - position);
            let selection = self
                .decode(current)
                .expect("the synchronous write was validated before mutation");
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                position += count;
                continue;
            };
            let lane_offset = bank_offset & !7;
            let read_modify_write = in_lane != 0
                || count != 8
                || (position..position + count).any(|index| byte_enable & (1 << index) == 0);
            let mut bytes = if read_modify_write {
                self.banks[bank_index]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(lane_offset)
                    .0
                    .to_le_bytes()
            } else {
                [0; 8]
            };
            for index in 0..count {
                if byte_enable & (1 << (position + index)) != 0 {
                    bytes[in_lane + index] = data[position + index];
                }
            }
            let data = u64::from_le_bytes(bytes);
            let check = if self.use_replacement {
                self.replacement
            } else {
                ecc::generate(data)
            };
            self.banks[bank_index]
                .as_mut()
                .expect("decoded bank exists")
                .write_lane(lane_offset, data, check);
            position += count;
        }
        true
    }

    fn synchronous_read_ready(&self, address: u64, length: usize, no_ecc: bool) -> bool {
        let mut position = 0;
        while position < length {
            let current = address + position as u64;
            let Some(selection) = self.decode(current) else {
                return false;
            };
            if let BankSelection::Populated {
                index: bank,
                offset,
            } = selection
                && self.ecc_enabled
                && !no_ecc
            {
                let (data, check) = self.banks[bank]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(offset & !7);
                if !matches!(ecc::check(data, check), ecc::EccCheck::Clean { .. }) {
                    return false;
                }
            }
            position += (8 - (current as usize & 7)).min(length - position);
        }
        true
    }

    fn synchronous_write_ready(
        &self,
        address: u64,
        data: &[u8],
        byte_enable: CrimeByteEnableView<'_>,
    ) -> bool {
        let mut position = 0;
        while position < data.len() {
            let current = address + position as u64;
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(data.len() - position);
            let read_modify_write = in_lane != 0
                || count != 8
                || (position..position + count).any(|index| {
                    !byte_enable
                        .is_enabled(index)
                        .expect("validated byte-enable length covers write data")
                });
            let Some(selection) = self.decode(current) else {
                return false;
            };
            if let BankSelection::Populated {
                index: bank,
                offset,
            } = selection
                && read_modify_write
                && self.ecc_enabled
            {
                let (old_data, stored_check) = self.banks[bank]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(offset & !7);
                if !matches!(
                    ecc::check(old_data, stored_check),
                    ecc::EccCheck::Clean { .. }
                ) {
                    return false;
                }
            }
            position += count;
        }
        true
    }

    /// Injects a data-bit fault without updating ECC.
    pub fn inject_data_bit(&mut self, address: u64, bit: u8) -> Result<(), CrimeBusError> {
        if bit >= 64 {
            return Err(CrimeBusError::Access);
        }
        let Some(BankSelection::Populated {
            index: bank,
            offset,
        }) = self.decode(address)
        else {
            return Err(CrimeBusError::Address);
        };
        let (data, check) = self.banks[bank]
            .as_ref()
            .expect("decoded bank exists")
            .read_lane(offset);
        let corrupted = data ^ (1_u64 << bit);
        let bank = self.banks[bank].as_mut().expect("decoded bank exists");
        bank.write_lane(offset, corrupted, check);
        Ok(())
    }

    fn accept_transaction(&mut self, transaction: CrimeMemoryTransaction) -> CrimeMemoryCompletion {
        CrimeMemoryCompletion {
            id: transaction.id,
            result: self.transfer(&transaction),
        }
    }

    fn transfer(
        &mut self,
        transaction: &CrimeMemoryTransaction,
    ) -> Result<CrimeMemoryOutcome, CrimeBusError> {
        let length = transaction.transfer.length();
        if length == 0 || length > MAX_TRANSFER_BYTES {
            return Err(CrimeBusError::Access);
        }
        match transaction.transfer.view() {
            CrimeTransferView::Read { .. }
                if matches!(transaction.bank_select, CrimeMemoryBankSelect::Decode) =>
            {
                self.read(transaction.address, length, transaction.no_ecc)
            }
            CrimeTransferView::Read { .. } => Ok(inhibited_read(transaction.address, length)),
            CrimeTransferView::Write { data, byte_enable } => {
                if byte_enable.len() != data.len() {
                    return Err(CrimeBusError::Access);
                }
                if matches!(transaction.bank_select, CrimeMemoryBankSelect::Decode) {
                    self.write(transaction.address, data, byte_enable)
                } else {
                    Ok(inhibited_write(transaction.address, data, byte_enable))
                }
            }
        }
    }

    fn read(
        &mut self,
        address: u64,
        length: usize,
        no_ecc: bool,
    ) -> Result<CrimeMemoryOutcome, CrimeBusError> {
        if length > 8
            && let Some(output) = self.read_clean_sparse_page(address, length, no_ecc)
        {
            return Ok(CrimeMemoryOutcome::new(
                CrimeCompletionPayload::ReadData(output),
                None,
                None,
            ));
        }
        let mut output = CrimeData::zeroed(length);
        let mut diagnostic = None;
        let mut fault = None;
        let mut position = 0;
        while position < length {
            let current = address + position as u64;
            let Some(selection) = self.decode(current) else {
                diagnostic.get_or_insert_with(|| address_diagnostic(current, false, false));
                fault.get_or_insert(CrimeMemoryFault::Address);
                position += (8 - (current as usize & 7)).min(length - position);
                continue;
            };
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                // An electrically undriven memory data bus is represented by
                // zeroes so the functional model remains deterministic.
                position += (8 - (current as usize & 7)).min(length - position);
                continue;
            };
            let lane_offset = bank_offset & !7;
            let in_lane = bank_offset as usize & 7;
            let count = (8 - in_lane).min(length - position);
            let (data, stored_check) = self.banks[bank_index]
                .as_ref()
                .expect("decoded bank exists")
                .read_lane(lane_offset);
            let readable = if no_ecc || !self.ecc_enabled {
                data
            } else {
                match ecc::check(data, stored_check) {
                    ecc::EccCheck::Clean { .. } => data,
                    ecc::EccCheck::Corrected {
                        data,
                        syndrome,
                        check,
                    } => {
                        diagnostic.get_or_insert_with(|| {
                            ecc_diagnostic(current, syndrome, check, true, false, false)
                        });
                        data
                    }
                    ecc::EccCheck::Uncorrectable { syndrome, check } => {
                        diagnostic.get_or_insert_with(|| {
                            ecc_diagnostic(current, syndrome, check, false, false, false)
                        });
                        fault.get_or_insert(CrimeMemoryFault::UncorrectableEcc);
                        data
                    }
                }
            };
            let bytes = readable.to_le_bytes();
            output[position..position + count].copy_from_slice(&bytes[in_lane..in_lane + count]);
            position += count;
        }
        Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(output),
            fault,
            diagnostic,
        ))
    }

    fn read_clean_sparse_page(
        &self,
        address: u64,
        length: usize,
        no_ecc: bool,
    ) -> Option<CrimeData> {
        let last_address = address.checked_add(length.checked_sub(1)? as u64)?;
        let BankSelection::Populated {
            index: first_bank,
            offset: first_offset,
        } = self.decode(address)?
        else {
            return None;
        };
        let BankSelection::Populated {
            index: last_bank,
            offset: last_offset,
        } = self.decode(last_address)?
        else {
            return None;
        };
        if first_bank != last_bank || last_offset != first_offset.checked_add(length as u64 - 1)? {
            return None;
        }
        let first_offset = first_offset as usize;
        let last_offset = last_offset as usize;
        let page_index = first_offset / PAGE_SIZE;
        if last_offset / PAGE_SIZE != page_index {
            return None;
        }
        let Some(page) = self.banks[first_bank]
            .as_ref()
            .expect("decoded bank exists")
            .pages[page_index]
            .as_ref()
        else {
            return Some(CrimeData::zeroed(length));
        };
        if self.ecc_enabled && !no_ecc {
            let first_lane = (first_offset % PAGE_SIZE) / 8;
            let last_lane = (last_offset % PAGE_SIZE) / 8;
            for lane in first_lane..=last_lane {
                let start = lane * 8;
                let data = u64::from_le_bytes(
                    page.data[start..start + 8]
                        .try_into()
                        .expect("one sparse ECC lane is eight bytes"),
                );
                if !matches!(
                    ecc::check(data, page.ecc[lane]),
                    ecc::EccCheck::Clean { .. }
                ) {
                    return None;
                }
            }
        }
        let in_page = first_offset % PAGE_SIZE;
        Some(
            page.data[in_page..in_page + length]
                .iter()
                .copied()
                .collect(),
        )
    }

    fn write(
        &mut self,
        address: u64,
        data: &[u8],
        byte_enable: CrimeByteEnableView<'_>,
    ) -> Result<CrimeMemoryOutcome, CrimeBusError> {
        let mut diagnostic = None;
        let mut fault = None;
        let mut position = 0;
        while position < data.len() {
            let current = address + position as u64;
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(data.len() - position);
            let read_modify_write = in_lane != 0
                || count != 8
                || (position..position + count).any(|index| {
                    !byte_enable
                        .is_enabled(index)
                        .expect("validated byte-enable length covers write data")
                });
            let Some(selection) = self.decode(current) else {
                diagnostic
                    .get_or_insert_with(|| address_diagnostic(current, true, read_modify_write));
                fault.get_or_insert(CrimeMemoryFault::Address);
                position += count;
                continue;
            };
            let BankSelection::Populated {
                index: bank_index,
                offset: bank_offset,
            } = selection
            else {
                // Writes to a selected but unpopulated external bank have no
                // storage side effect and still complete normally.
                position += count;
                continue;
            };
            let lane_offset = bank_offset & !7;
            let mut bytes = if read_modify_write {
                let (old_data, stored_check) = self.banks[bank_index]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(lane_offset);
                let old_data = if self.ecc_enabled {
                    match ecc::check(old_data, stored_check) {
                        ecc::EccCheck::Clean { .. } => old_data,
                        ecc::EccCheck::Corrected {
                            data,
                            syndrome,
                            check,
                        } => {
                            diagnostic.get_or_insert_with(|| {
                                ecc_diagnostic(current, syndrome, check, true, true, true)
                            });
                            data
                        }
                        ecc::EccCheck::Uncorrectable { syndrome, check } => {
                            diagnostic.get_or_insert_with(|| {
                                ecc_diagnostic(current, syndrome, check, false, true, true)
                            });
                            fault.get_or_insert(CrimeMemoryFault::UncorrectableEcc);
                            old_data
                        }
                    }
                } else {
                    old_data
                };
                old_data.to_le_bytes()
            } else {
                [0; 8]
            };
            for index in 0..count {
                if byte_enable
                    .is_enabled(position + index)
                    .expect("validated byte-enable length covers write data")
                {
                    bytes[in_lane + index] = data[position + index];
                }
            }
            let data = u64::from_le_bytes(bytes);
            let check = if self.use_replacement {
                self.replacement
            } else {
                ecc::generate(data)
            };
            let bank = self.banks[bank_index]
                .as_mut()
                .expect("decoded bank exists");
            bank.write_lane(lane_offset, data, check);
            position += count;
        }
        Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::WriteComplete,
            fault,
            diagnostic,
        ))
    }

    fn decode(&self, address: u64) -> Option<BankSelection> {
        for (index, control) in self.bank_control.iter().copied().enumerate() {
            let programmed = u64::from(control & registers::MEMORY_BANK_ADDRESS_MASK);
            let address_field = (address >> BANK_32_MIB_ADDRESS_SHIFT)
                & u64::from(registers::MEMORY_BANK_ADDRESS_MASK);
            let offset = if control & registers::MEMORY_BANK_SIZE_128_MIB != 0 {
                if address_field >> 2 != programmed >> 2 {
                    continue;
                }
                address & ((1_u64 << BANK_128_MIB_ADDRESS_SHIFT) - 1)
            } else {
                if address_field != programmed {
                    continue;
                }
                address & ((1_u64 << BANK_32_MIB_ADDRESS_SHIFT) - 1)
            };
            let Some(bank) = &self.banks[index] else {
                return Some(BankSelection::Unpopulated);
            };
            return Some(BankSelection::Populated {
                index,
                offset: offset % bank.config.size.bytes(),
            });
        }
        None
    }
}

impl Component for CrimeSdram {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        for bank in self.banks.iter_mut().flatten() {
            bank.clear();
        }
        for (index, control) in self.bank_control.iter_mut().enumerate() {
            *control = index as u16;
            if let Some(bank) = &self.banks[index] {
                *control |= bank.config.size.control_bit();
            }
        }
        self.ecc_enabled = false;
        self.use_replacement = false;
        self.replacement = 0;
    }
}

impl BusDeviceRole<CrimeMemoryTransaction> for CrimeSdram {
    type Response = CrimeMemoryCompletion;

    fn accept(&mut self, transaction: CrimeMemoryTransaction) -> Self::Response {
        self.accept_transaction(transaction)
    }
}

impl BusDeviceRole<CrimeSdramSignal> for CrimeSdram {
    type Response = ();

    fn accept(&mut self, signal: CrimeSdramSignal) {
        match signal {
            CrimeSdramSignal::SetBankControl { bank, value } => {
                if let Some(control) = self.bank_control.get_mut(usize::from(bank)) {
                    *control = value & registers::MEMORY_BANK_CONTROL_MASK;
                }
            }
            CrimeSdramSignal::SetEccControl {
                enabled,
                use_replacement,
                replacement,
            } => {
                self.ecc_enabled = enabled;
                self.use_replacement = use_replacement;
                self.replacement = replacement;
            }
            CrimeSdramSignal::PowerOn => self.reset(),
            CrimeSdramSignal::HardReset => {}
        }
    }
}

fn ecc_diagnostic(
    address: u64,
    syndrome: u8,
    check: u8,
    corrected: bool,
    write: bool,
    read_modify_write: bool,
) -> CrimeMemoryDiagnostic {
    let lane = ((address >> 3) & 3) as u32;
    CrimeMemoryDiagnostic {
        address: address & !31,
        syndrome: u32::from(syndrome) << (lane * 8),
        check: u32::from(check) << (lane * 8),
        corrected,
        write,
        read_modify_write,
    }
}

fn address_diagnostic(address: u64, write: bool, read_modify_write: bool) -> CrimeMemoryDiagnostic {
    CrimeMemoryDiagnostic {
        address,
        syndrome: 0,
        check: 0,
        corrected: false,
        write,
        read_modify_write,
    }
}

fn inhibited_read(address: u64, length: usize) -> CrimeMemoryOutcome {
    CrimeMemoryOutcome::new(
        CrimeCompletionPayload::ReadData(CrimeData::zeroed(length)),
        Some(CrimeMemoryFault::Address),
        Some(address_diagnostic(address, false, false)),
    )
}

fn inhibited_write(
    address: u64,
    data: &[u8],
    byte_enable: CrimeByteEnableView<'_>,
) -> CrimeMemoryOutcome {
    let in_lane = address as usize & 7;
    let count = (8 - in_lane).min(data.len());
    let read_modify_write = in_lane != 0
        || count != 8
        || (0..count).any(|index| {
            !byte_enable
                .is_enabled(index)
                .expect("validated byte-enable length covers inhibited write")
        });
    CrimeMemoryOutcome::new(
        CrimeCompletionPayload::WriteComplete,
        Some(CrimeMemoryFault::Address),
        Some(address_diagnostic(address, true, read_modify_write)),
    )
}

const fn reset_bank_control(config: CrimeMemoryConfig) -> [u16; 8] {
    let mut controls = [0; 8];
    let mut index = 0;
    while index < controls.len() {
        controls[index] = index as u16;
        if let Some(bank) = config.banks[index] {
            controls[index] |= bank.size.control_bit();
        }
        index += 1;
    }
    controls
}

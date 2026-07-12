//! CRIME SDRAM device and memory-domain bus.

pub mod bus;
mod ecc;

use std::collections::BTreeMap;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

use super::config::{CrimeMemoryConfig, CrimeSdramBankConfig};
use super::protocol::{
    CrimeBusError, CrimeCompletionPayload, CrimeMemoryCompletion, CrimeMemoryDiagnostic,
    CrimeMemoryTransaction, CrimeSdramSignal, CrimeTransfer,
};
use super::registers;

#[cfg(test)]
mod tests;

const PAGE_SIZE: usize = 4096;
const MAX_TRANSFER_BYTES: usize = 16 * 32;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SparseBank {
    config: CrimeSdramBankConfig,
    pages: BTreeMap<u64, Box<[u8; PAGE_SIZE]>>,
    ecc: BTreeMap<u64, u8>,
}

impl SparseBank {
    fn new(config: CrimeSdramBankConfig) -> Self {
        Self {
            config,
            pages: BTreeMap::new(),
            ecc: BTreeMap::new(),
        }
    }

    fn read_byte(&self, offset: u64) -> u8 {
        let page = offset / PAGE_SIZE as u64;
        let in_page = offset as usize % PAGE_SIZE;
        self.pages.get(&page).map_or(0, |bytes| bytes[in_page])
    }

    fn write_byte(&mut self, offset: u64, value: u8) {
        let page = offset / PAGE_SIZE as u64;
        let in_page = offset as usize % PAGE_SIZE;
        if value == 0 && !self.pages.contains_key(&page) {
            return;
        }
        self.pages
            .entry(page)
            .or_insert_with(|| Box::new([0; PAGE_SIZE]))[in_page] = value;
    }

    fn read_lane(&self, offset: u64) -> (u64, u8) {
        let aligned = offset & !7;
        let mut bytes = [0; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read_byte(aligned + index as u64);
        }
        let data = u64::from_le_bytes(bytes);
        let check = self.ecc.get(&(aligned / 8)).copied().unwrap_or(0);
        (data, check)
    }

    fn write_lane(&mut self, offset: u64, data: u64) {
        let aligned = offset & !7;
        for (index, byte) in data.to_le_bytes().into_iter().enumerate() {
            self.write_byte(aligned + index as u64, byte);
        }
        let check = ecc::generate(data);
        if check == 0 {
            self.ecc.remove(&(aligned / 8));
        } else {
            self.ecc.insert(aligned / 8, check);
        }
    }

    fn clear(&mut self) {
        self.pages.clear();
        self.ecc.clear();
    }
}

/// CRIME-attached sparse SDRAM with eight physical external banks.
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

    /// Injects a data-bit fault without updating ECC.
    pub fn inject_data_bit(&mut self, address: u64, bit: u8) -> Result<(), CrimeBusError> {
        if bit >= 64 {
            return Err(CrimeBusError::Access);
        }
        let (bank, offset) = self.decode(address)?;
        let (data, _) = self.banks[bank]
            .as_ref()
            .expect("decoded bank exists")
            .read_lane(offset);
        let corrupted = data ^ (1_u64 << bit);
        let bytes = corrupted.to_le_bytes();
        let bank = self.banks[bank].as_mut().expect("decoded bank exists");
        let aligned = offset & !7;
        for (index, byte) in bytes.into_iter().enumerate() {
            bank.write_byte(aligned + index as u64, byte);
        }
        Ok(())
    }

    fn accept_transaction(&mut self, transaction: CrimeMemoryTransaction) -> CrimeMemoryCompletion {
        let result = self.transfer(&transaction);
        match result {
            Ok((payload, diagnostic)) => CrimeMemoryCompletion {
                id: transaction.id,
                result: Ok(payload),
                diagnostic,
            },
            Err((error, diagnostic)) => CrimeMemoryCompletion {
                id: transaction.id,
                result: Err(error),
                diagnostic,
            },
        }
    }

    fn transfer(
        &mut self,
        transaction: &CrimeMemoryTransaction,
    ) -> Result<
        (CrimeCompletionPayload, Option<CrimeMemoryDiagnostic>),
        (CrimeBusError, Option<CrimeMemoryDiagnostic>),
    > {
        let length = transaction.transfer.length();
        if length == 0 || length > MAX_TRANSFER_BYTES {
            return Err((CrimeBusError::Access, None));
        }
        match &transaction.transfer {
            CrimeTransfer::Read { .. } => {
                self.read(transaction.address, length, transaction.no_ecc)
            }
            CrimeTransfer::Write { data, byte_enable } => {
                if byte_enable.len() != data.len() {
                    return Err((CrimeBusError::Access, None));
                }
                self.write(transaction.address, data, byte_enable)
            }
        }
    }

    fn read(
        &mut self,
        address: u64,
        length: usize,
        no_ecc: bool,
    ) -> Result<
        (CrimeCompletionPayload, Option<CrimeMemoryDiagnostic>),
        (CrimeBusError, Option<CrimeMemoryDiagnostic>),
    > {
        let mut output = vec![0; length];
        let mut diagnostic = None;
        let mut position = 0;
        while position < length {
            let current = address + position as u64;
            let (bank_index, bank_offset) = self
                .decode(current)
                .map_err(|error| (error, address_diagnostic(current, false, false)))?;
            let lane_offset = bank_offset & !7;
            let in_lane = bank_offset as usize & 7;
            let count = (8 - in_lane).min(length - position);
            let (data, stored_check) = self.banks[bank_index]
                .as_ref()
                .expect("decoded bank exists")
                .read_lane(lane_offset);
            let checked = if no_ecc || !self.ecc_enabled {
                ecc::EccCheck::Clean {
                    check: ecc::generate(data),
                }
            } else {
                ecc::check(data, stored_check)
            };
            let readable = match checked {
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
                    let diagnostic = ecc_diagnostic(current, syndrome, check, false, false, false);
                    return Err((CrimeBusError::UncorrectableEcc, Some(diagnostic)));
                }
            };
            let bytes = readable.to_le_bytes();
            output[position..position + count].copy_from_slice(&bytes[in_lane..in_lane + count]);
            position += count;
        }
        Ok((CrimeCompletionPayload::ReadData(output), diagnostic))
    }

    fn write(
        &mut self,
        address: u64,
        data: &[u8],
        byte_enable: &[bool],
    ) -> Result<
        (CrimeCompletionPayload, Option<CrimeMemoryDiagnostic>),
        (CrimeBusError, Option<CrimeMemoryDiagnostic>),
    > {
        let mut diagnostic = None;
        let mut position = 0;
        while position < data.len() {
            let current = address + position as u64;
            let in_lane = current as usize & 7;
            let count = (8 - in_lane).min(data.len() - position);
            let read_modify_write = in_lane != 0
                || count != 8
                || byte_enable[position..position + count]
                    .iter()
                    .any(|enabled| !enabled);
            let (bank_index, bank_offset) = self
                .decode(current)
                .map_err(|error| (error, address_diagnostic(current, true, read_modify_write)))?;
            let lane_offset = bank_offset & !7;
            let mut bytes = if read_modify_write {
                let (old_data, stored_check) = self.banks[bank_index]
                    .as_ref()
                    .expect("decoded bank exists")
                    .read_lane(lane_offset);
                let old_data = match if self.ecc_enabled {
                    ecc::check(old_data, stored_check)
                } else {
                    ecc::EccCheck::Clean {
                        check: ecc::generate(old_data),
                    }
                } {
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
                        let diagnostic =
                            ecc_diagnostic(current, syndrome, check, false, true, true);
                        return Err((CrimeBusError::UncorrectableEcc, Some(diagnostic)));
                    }
                };
                old_data.to_le_bytes()
            } else {
                [0; 8]
            };
            for index in 0..count {
                if byte_enable[position + index] {
                    bytes[in_lane + index] = data[position + index];
                }
            }
            let data = u64::from_le_bytes(bytes);
            let replacement = self.use_replacement.then_some(self.replacement);
            let bank = self.banks[bank_index]
                .as_mut()
                .expect("decoded bank exists");
            bank.write_lane(lane_offset, data);
            if let Some(check) = replacement {
                bank.ecc.insert(lane_offset / 8, check);
            }
            position += count;
        }
        Ok((CrimeCompletionPayload::WriteComplete, diagnostic))
    }

    fn decode(&self, address: u64) -> Result<(usize, u64), CrimeBusError> {
        for (index, control) in self.bank_control.iter().copied().enumerate() {
            let base = u64::from(control & registers::MEMORY_BANK_ADDRESS_MASK) << 25;
            let decode_size = if control & registers::MEMORY_BANK_SIZE_128_MIB != 0 {
                128 * 1024 * 1024
            } else {
                32 * 1024 * 1024
            };
            if address < base || address >= base + decode_size {
                continue;
            }
            let Some(bank) = &self.banks[index] else {
                return Err(CrimeBusError::Address);
            };
            return Ok((index, (address - base) % bank.config.size.bytes()));
        }
        Err(CrimeBusError::Address)
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

fn address_diagnostic(
    address: u64,
    write: bool,
    read_modify_write: bool,
) -> Option<CrimeMemoryDiagnostic> {
    Some(CrimeMemoryDiagnostic {
        address,
        syndrome: 0,
        check: 0,
        corrected: false,
        write,
        read_modify_write,
    })
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

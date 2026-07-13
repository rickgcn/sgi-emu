//! Reusable byte-addressed memory components.
//!
//! Memory transactions use offsets within a device rather than machine physical
//! addresses. Machine buses own address decoding and alias translation before
//! forwarding a transaction to one of these components.

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

pub mod ds2502;
pub mod flash;

/// Transaction accepted by a byte-addressed memory component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MemoryTransaction {
    /// Reads physical byte lanes beginning at a device offset.
    Read {
        /// First byte offset within the device.
        offset: u64,

        /// Number of bytes to read, from one through eight.
        size: u8,
    },

    /// Writes enabled physical byte lanes beginning at a device offset.
    Write {
        /// First byte offset within the device.
        offset: u64,

        /// Width of the write container, from one through eight bytes.
        size: u8,

        /// Physical byte-lane data, with the least significant byte first.
        data: u64,

        /// Enabled byte lanes relative to `offset`.
        byte_enable: u8,
    },
}

/// Response returned by a byte-addressed memory component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MemoryResponse {
    /// Read data in physical byte-lane order.
    ReadData(u64),

    /// A write completed successfully.
    WriteComplete,

    /// The transaction was outside the device or unsupported.
    AccessError,
}

/// Writable byte-addressed memory.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ram {
    id: ComponentId,
    name: String,
    bytes: Vec<u8>,
}

impl Ram {
    /// Creates zero-filled writable memory.
    pub fn new(id: ComponentId, name: impl Into<String>, size_bytes: usize) -> Self {
        Self {
            id,
            name: name.into(),
            bytes: vec![0; size_bytes],
        }
    }

    /// Returns the number of addressable bytes.
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the memory has no addressable bytes.
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the raw memory bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Component for Ram {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.bytes.fill(0);
    }
}

impl BusDeviceRole<MemoryTransaction> for Ram {
    type Response = MemoryResponse;

    fn accept(&mut self, transaction: MemoryTransaction) -> Self::Response {
        match transaction {
            MemoryTransaction::Read { offset, size } => read(&self.bytes, offset, size),
            MemoryTransaction::Write {
                offset,
                size,
                data,
                byte_enable,
            } => write(&mut self.bytes, offset, size, data, byte_enable),
        }
    }
}

/// Read-only byte-addressed memory.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Rom {
    id: ComponentId,
    name: String,
    bytes: Vec<u8>,
}

impl Rom {
    /// Creates read-only memory from an immutable image.
    pub fn new(id: ComponentId, name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            id,
            name: name.into(),
            bytes,
        }
    }

    /// Returns the number of addressable bytes.
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the memory has no addressable bytes.
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the immutable image bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Component for Rom {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

impl BusDeviceRole<MemoryTransaction> for Rom {
    type Response = MemoryResponse;

    fn accept(&mut self, transaction: MemoryTransaction) -> Self::Response {
        match transaction {
            MemoryTransaction::Read { offset, size } => read(&self.bytes, offset, size),
            MemoryTransaction::Write { .. } => MemoryResponse::AccessError,
        }
    }
}

fn read(bytes: &[u8], offset: u64, size: u8) -> MemoryResponse {
    let Some(range) = checked_range(bytes.len(), offset, size) else {
        return MemoryResponse::AccessError;
    };
    let mut data = 0_u64;
    for (lane, byte) in bytes[range].iter().copied().enumerate() {
        data |= u64::from(byte) << (lane * 8);
    }
    MemoryResponse::ReadData(data)
}

fn write(bytes: &mut [u8], offset: u64, size: u8, data: u64, byte_enable: u8) -> MemoryResponse {
    let Some(range) = checked_range(bytes.len(), offset, size) else {
        return MemoryResponse::AccessError;
    };
    for (lane, byte) in bytes[range].iter_mut().enumerate() {
        if byte_enable & (1 << lane) != 0 {
            *byte = (data >> (lane * 8)) as u8;
        }
    }
    MemoryResponse::WriteComplete
}

fn checked_range(length: usize, offset: u64, size: u8) -> Option<core::ops::Range<usize>> {
    if !(1..=8).contains(&size) {
        return None;
    }
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::from(size))?;
    (end <= length).then_some(start..end)
}

#[cfg(test)]
mod tests;

//! Compact owned byte-transfer storage shared by device protocols.

use core::ops::{Deref, DerefMut};
use smallvec::SmallVec;

const INLINE_BYTES: usize = 8;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CompactData(SmallVec<[u8; INLINE_BYTES]>);

impl CompactData {
    pub(crate) fn zeroed(length: usize) -> Self {
        Self(smallvec::smallvec![0; length])
    }

    pub(crate) fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

impl Deref for CompactData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CompactData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<u8>> for CompactData {
    fn from(value: Vec<u8>) -> Self {
        if value.len() <= INLINE_BYTES {
            Self(value.into_iter().collect())
        } else {
            Self(SmallVec::from_vec(value))
        }
    }
}

impl<const N: usize> From<[u8; N]> for CompactData {
    fn from(value: [u8; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<u8> for CompactData {
    fn from_iter<T: IntoIterator<Item = u8>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CompactByteEnable {
    length: usize,
    words: SmallVec<[u64; 1]>,
}

impl CompactByteEnable {
    pub(crate) fn enabled(length: usize) -> Self {
        Self::from_iter(core::iter::repeat_n(true, length))
    }

    pub(crate) fn len(&self) -> usize {
        self.length
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub(crate) fn is_enabled(&self, index: usize) -> Option<bool> {
        (index < self.length).then(|| self.words[index / 64] & (1_u64 << (index % 64)) != 0)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.length).map(|index| {
            self.is_enabled(index)
                .expect("byte-enable iterator stays in range")
        })
    }

    pub(crate) fn spilled(&self) -> bool {
        self.length > INLINE_BYTES
    }
}

impl FromIterator<bool> for CompactByteEnable {
    fn from_iter<T: IntoIterator<Item = bool>>(iter: T) -> Self {
        let values: SmallVec<[bool; INLINE_BYTES]> = iter.into_iter().collect();
        let mut words = SmallVec::from_elem(0, values.len().div_ceil(64));
        for (index, enabled) in values.iter().copied().enumerate() {
            if enabled {
                words[index / 64] |= 1_u64 << (index % 64);
            }
        }
        Self {
            length: values.len(),
            words,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompactBlockWrite {
    data: CompactData,
    byte_enable: CompactByteEnable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompactTransfer {
    Read {
        length: u16,
    },
    WriteInline {
        data_length: u8,
        enable_length: u8,
        data: [u8; INLINE_BYTES],
        enable_bits: u8,
    },
    WriteBlock(Box<CompactBlockWrite>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompactByteEnableView<'a> {
    length: usize,
    storage: CompactByteEnableViewStorage<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompactByteEnableViewStorage<'a> {
    Inline(u8),
    Block(&'a CompactByteEnable),
}

impl<'a> CompactByteEnableView<'a> {
    pub(crate) fn len(self) -> usize {
        self.length
    }

    pub(crate) fn is_empty(self) -> bool {
        self.length == 0
    }

    pub(crate) fn is_enabled(self, index: usize) -> Option<bool> {
        if index >= self.length {
            return None;
        }
        Some(match self.storage {
            CompactByteEnableViewStorage::Inline(bits) => bits & (1_u8 << index) != 0,
            CompactByteEnableViewStorage::Block(enable) => enable
                .is_enabled(index)
                .expect("block byte-enable view stays in range"),
        })
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = bool> + 'a {
        (0..self.length).map(move |index| {
            self.is_enabled(index)
                .expect("byte-enable view iterator stays in range")
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompactTransferView<'a> {
    Read {
        length: u16,
    },
    Write {
        data: &'a [u8],
        byte_enable: CompactByteEnableView<'a>,
    },
}

impl CompactTransfer {
    pub(crate) const fn read(length: u16) -> Self {
        Self::Read { length }
    }

    pub(crate) fn write(data: CompactData, byte_enable: CompactByteEnable) -> Self {
        if data.len() <= INLINE_BYTES && byte_enable.len() <= INLINE_BYTES {
            let mut inline_data = [0; INLINE_BYTES];
            inline_data[..data.len()].copy_from_slice(&data);
            let enable_bits = byte_enable
                .iter()
                .enumerate()
                .fold(0, |bits, (index, enabled)| {
                    bits | (u8::from(enabled) << index)
                });
            Self::WriteInline {
                data_length: data.len() as u8,
                enable_length: byte_enable.len() as u8,
                data: inline_data,
                enable_bits,
            }
        } else {
            Self::WriteBlock(Box::new(CompactBlockWrite { data, byte_enable }))
        }
    }

    pub(crate) fn length(&self) -> usize {
        match self {
            Self::Read { length } => usize::from(*length),
            Self::WriteInline { data_length, .. } => usize::from(*data_length),
            Self::WriteBlock(write) => write.data.len(),
        }
    }

    pub(crate) fn view(&self) -> CompactTransferView<'_> {
        match self {
            Self::Read { length } => CompactTransferView::Read { length: *length },
            Self::WriteInline {
                data_length,
                enable_length,
                data,
                enable_bits,
            } => CompactTransferView::Write {
                data: &data[..usize::from(*data_length)],
                byte_enable: CompactByteEnableView {
                    length: usize::from(*enable_length),
                    storage: CompactByteEnableViewStorage::Inline(*enable_bits),
                },
            },
            Self::WriteBlock(write) => CompactTransferView::Write {
                data: &write.data,
                byte_enable: CompactByteEnableView {
                    length: write.byte_enable.len(),
                    storage: CompactByteEnableViewStorage::Block(&write.byte_enable),
                },
            },
        }
    }
}

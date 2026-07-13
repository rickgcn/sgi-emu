//! MACE address decoding and CMI-facing access validation.

use super::registers;

/// Internal destination selected by the MACE primary decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MaceAddressTarget {
    Future,
    PciRegisters,
    VideoInput1,
    VideoInput2,
    VideoOutput,
    Ethernet,
    Peripheral,
    ExternalIsa,
    SystemFlash,
    PciIo,
    PciMemory,
    PciConfiguration,
}

/// Complete decoded address with a target-local byte offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceAddressResolution {
    pub target: MaceAddressTarget,
    pub offset: u64,
}

/// External ISA target selected by the MACE decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MaceExternalIsaTarget {
    Parallel,
    Serial1,
    Serial2,
    Rtc,
}

/// Decoded byte-register access on the external ISA island.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceExternalIsaResolution {
    pub target: MaceExternalIsaTarget,
    pub register: u32,
}

/// Decodes one complete CMI transfer.
pub fn resolve(address: u64, length: usize) -> Option<MaceAddressResolution> {
    let end = address.checked_add(length as u64)?;
    let in_window = |start, limit| address >= start && address < limit && end <= limit;
    let resolution = if in_window(registers::PROM_START, registers::PROM_END) {
        MaceAddressResolution {
            target: MaceAddressTarget::SystemFlash,
            offset: (address - registers::PROM_START) % registers::PROM_IMAGE_SIZE,
        }
    } else if in_window(registers::PCI_LOW_IO_START, registers::PCI_LOW_IO_END)
        || in_window(registers::PCI_HIGH_IO_START, registers::PCI_HIGH_IO_END)
    {
        MaceAddressResolution {
            target: MaceAddressTarget::PciIo,
            offset: address,
        }
    } else if in_window(
        registers::PCI_LOW_MEMORY_START,
        registers::PCI_LOW_MEMORY_END,
    ) || in_window(
        registers::PCI_HIGH_MEMORY_START,
        registers::PCI_HIGH_MEMORY_END,
    ) {
        MaceAddressResolution {
            target: MaceAddressTarget::PciMemory,
            offset: address,
        }
    } else if in_window(registers::PCI_CONFIG_START, registers::PCI_CONFIG_END) {
        MaceAddressResolution {
            target: MaceAddressTarget::PciConfiguration,
            offset: address - registers::PCI_CONFIG_START,
        }
    } else if in_window(registers::FUTURE_BASE, registers::PCI_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::Future,
            offset: address - registers::FUTURE_BASE,
        }
    } else if in_window(registers::PCI_BASE, registers::VIDEO_INPUT1_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::PciRegisters,
            offset: address - registers::PCI_BASE,
        }
    } else if in_window(registers::VIDEO_INPUT1_BASE, registers::VIDEO_INPUT2_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::VideoInput1,
            offset: (address - registers::VIDEO_INPUT1_BASE) & 0x7ffff,
        }
    } else if in_window(registers::VIDEO_INPUT2_BASE, registers::VIDEO_OUTPUT_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::VideoInput2,
            offset: (address - registers::VIDEO_INPUT2_BASE) & 0x7ffff,
        }
    } else if in_window(registers::VIDEO_OUTPUT_BASE, registers::ETHERNET_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::VideoOutput,
            offset: (address - registers::VIDEO_OUTPUT_BASE) & 0x7ffff,
        }
    } else if in_window(registers::ETHERNET_BASE, registers::PERIPHERAL_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::Ethernet,
            offset: (address - registers::ETHERNET_BASE) & 0x7ffff,
        }
    } else if in_window(registers::PERIPHERAL_BASE, registers::EXTERNAL_ISA_BASE) {
        MaceAddressResolution {
            target: MaceAddressTarget::Peripheral,
            offset: address - registers::PERIPHERAL_BASE,
        }
    } else if in_window(registers::EXTERNAL_ISA_BASE, registers::PRIMARY_END) {
        MaceAddressResolution {
            target: MaceAddressTarget::ExternalIsa,
            offset: address - registers::EXTERNAL_ISA_BASE,
        }
    } else {
        return None;
    };
    Some(resolution)
}

/// Converts an external ISA address in the Dallas window to its byte register.
pub fn rtc_register(offset: u64, length: usize) -> Option<u32> {
    if length != 1
        || !(registers::RTC_EXTERNAL_OFFSET..registers::RTC_EXTERNAL_END).contains(&offset)
    {
        return None;
    }
    let within = offset - registers::RTC_EXTERNAL_OFFSET;
    if within & 0xff != registers::EXTERNAL_VALID_BYTE_LANE {
        return None;
    }
    Some((within / registers::EXTERNAL_REGISTER_STRIDE) as u32)
}

/// Decodes one byte-lane-spaced external ISA register.
pub fn resolve_external_isa(offset: u64, length: usize) -> Option<MaceExternalIsaResolution> {
    if length != 1 || offset >= 0x40000 || offset & 0xff != registers::EXTERNAL_VALID_BYTE_LANE {
        return None;
    }
    let (target, base, register_mask) = match offset {
        0x00000..=0x0ffff => (MaceExternalIsaTarget::Parallel, 0x00000, 0x0f),
        0x10000..=0x17fff => (MaceExternalIsaTarget::Serial1, 0x10000, 0x07),
        0x18000..=0x1ffff => (MaceExternalIsaTarget::Serial2, 0x18000, 0x07),
        0x20000..=0x2ffff => (MaceExternalIsaTarget::Rtc, 0x20000, 0xff),
        _ => return None,
    };
    Some(MaceExternalIsaResolution {
        target,
        register: (((offset - base) / registers::EXTERNAL_REGISTER_STRIDE) as u32) & register_mask,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_prom_mirroring_and_dallas_lane() {
        assert_eq!(resolve(0x1fc8_0000, 4).unwrap().offset, 0);
        assert_eq!(rtc_register(0x23707, 1), Some(0x37));
        assert_eq!(rtc_register(0x23700, 1), None);
        assert_eq!(
            resolve_external_isa(0x10207, 1),
            Some(MaceExternalIsaResolution {
                target: MaceExternalIsaTarget::Serial1,
                register: 2,
            })
        );
    }
}

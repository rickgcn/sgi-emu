//! GBE Revision 1.1 software-visible register and on-chip RAM state.

use crate::chipset::crime::protocol::CrimeBusError;

pub(super) const GBE_BASE: u64 = 0x1600_0000;
pub(super) const CONTROL_STATUS: u64 = GBE_BASE;
pub(super) const DOT_CLOCK: u64 = GBE_BASE + 0x04;
pub(super) const CRT_DDC: u64 = GBE_BASE + 0x08;
pub(super) const SYSTEM_CLOCK: u64 = GBE_BASE + 0x0c;
pub(super) const FLAT_PANEL_DDC: u64 = GBE_BASE + 0x10;
pub(super) const DEVICE_ID: u64 = GBE_BASE + 0x14;

pub(super) const VT_START: u64 = GBE_BASE + 0x0001_0000;
pub(super) const VT_XY: usize = 0;
pub(super) const VT_XY_MAX: usize = 1;
pub(super) const VT_VSYNC: usize = 2;
pub(super) const VT_HSYNC: usize = 3;
pub(super) const VT_VBLANK: usize = 4;
pub(super) const VT_HBLANK: usize = 5;
pub(super) const VT_FLAGS: usize = 6;
pub(super) const VT_F2RF_LOCK: usize = 7;
pub(super) const VT_INTR01: usize = 8;
pub(super) const VT_INTR23: usize = 9;
pub(super) const FP_HDRIVE: usize = 10;
pub(super) const FP_VDRIVE: usize = 11;
pub(super) const FP_DATA_ENABLE: usize = 12;
pub(super) const VT_HPIXEN: usize = 13;
pub(super) const VT_VPIXEN: usize = 14;
pub(super) const VT_HCMAP: usize = 15;
pub(super) const VT_VCMAP: usize = 16;
pub(super) const DID_START_XY: usize = 17;
pub(super) const CURSOR_START_XY: usize = 18;
pub(super) const VIDEO_CAPTURE_START_XY: usize = 19;

pub(super) const OVERLAY_START: u64 = GBE_BASE + 0x0002_0000;
pub(super) const FRAME_START: u64 = GBE_BASE + 0x0003_0000;
pub(super) const DID_START: u64 = GBE_BASE + 0x0004_0000;
pub(super) const WID_START: u64 = GBE_BASE + 0x0004_8000;
pub(super) const COLOR_MAP_START: u64 = GBE_BASE + 0x0005_0000;
pub(super) const COLOR_MAP_END: u64 = COLOR_MAP_START + 4 * 4_608;
pub(super) const COLOR_MAP_FIFO: u64 = GBE_BASE + 0x0005_8000;
pub(super) const GAMMA_MAP_START: u64 = GBE_BASE + 0x0006_0000;
pub(super) const CURSOR_REGISTER_START: u64 = GBE_BASE + 0x0007_0000;
pub(super) const CURSOR_GLYPH_START: u64 = GBE_BASE + 0x0007_8000;
pub(super) const VIDEO_CAPTURE_START: u64 = GBE_BASE + 0x0008_0000;

pub(super) const CONTROL_STATUS_WRITABLE_MASK: u32 = 0x3fff_ffc0;
pub(super) const CONTROL_STATUS_RESET: u32 = 0x03ff_ffd1;
pub(super) const AUXILIARY_PIN_COUNT: usize = 10;
pub(super) const DEVICE_ID_VALUE: u32 = 0x0000_0666;
pub(super) const VT_XY_FREEZE: u32 = 1 << 31;
pub(super) const DOT_CLOCK_RUN: u32 = 1 << 20;

/// Side effect produced by one accepted register write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RegisterWrite {
    None,
    DotClock,
    Ddc {
        flat_panel: bool,
        clock_released: bool,
        data_released: bool,
    },
    Timing {
        counter_written: bool,
    },
    Shadow,
    FrameFifoReset,
    OverlayFifoReset,
    ColorMap {
        index: u16,
        value: u32,
    },
    Capture,
}

/// GBE Revision 1.1 register file and on-chip memories.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct GbeRegisters {
    pub control_status: u32,
    pub dot_clock: u32,
    pub crt_ddc: u32,
    pub system_clock: u32,
    pub flat_panel_ddc: u32,
    pub vt: [u32; 20],
    pub overlay: [u32; 3],
    pub frame: [u32; 4],
    pub did: [u32; 2],
    pub wid: [u32; 32],
    #[serde(with = "crate::common::serde_array")]
    pub color_map: [u32; 4_608],
    #[serde(with = "crate::common::serde_array")]
    pub gamma_map: [u32; 256],
    pub cursor: [u32; 5],
    #[serde(with = "crate::common::serde_array")]
    pub cursor_glyph: [u32; 64],
    pub video_capture: [u32; 9],
}

impl GbeRegisters {
    pub fn new() -> Self {
        let mut registers = Self {
            control_status: CONTROL_STATUS_RESET,
            dot_clock: 0,
            crt_ddc: 3,
            system_clock: 1 << 3,
            flat_panel_ddc: 3,
            vt: [0; 20],
            overlay: [0; 3],
            frame: [0; 4],
            did: [0; 2],
            wid: [0; 32],
            color_map: [0; 4_608],
            gamma_map: [0; 256],
            cursor: [0; 5],
            cursor_glyph: [0; 64],
            video_capture: [0; 9],
        };
        registers.vt[VT_XY] = VT_XY_FREEZE;
        registers.video_capture[2] = (1 << 1) | (1 << 2);
        registers.video_capture[3] = 1 << 4;
        registers
    }

    pub fn read(&self, address: u64) -> Result<u32, CrimeBusError> {
        let value = match address {
            CONTROL_STATUS => self.control_status,
            DOT_CLOCK => self.dot_clock_with_lock(),
            CRT_DDC => self.crt_ddc,
            SYSTEM_CLOCK => self.system_clock,
            FLAT_PANEL_DDC => self.flat_panel_ddc,
            DEVICE_ID => DEVICE_ID_VALUE,
            COLOR_MAP_FIFO => return Err(CrimeBusError::Unsupported),
            _ if in_words(address, VT_START, 20) => self.vt[word_index(address, VT_START)],
            _ if in_words(address, OVERLAY_START, 3) => {
                self.overlay[word_index(address, OVERLAY_START)]
            }
            _ if in_words(address, FRAME_START, 4) => self.frame[word_index(address, FRAME_START)],
            _ if in_words(address, DID_START, 2) => self.did[word_index(address, DID_START)],
            _ if in_words(address, WID_START, 32) => self.wid[word_index(address, WID_START)],
            _ if (COLOR_MAP_START..COLOR_MAP_END).contains(&address) => {
                self.color_map[word_index(address, COLOR_MAP_START)]
            }
            _ if in_words(address, GAMMA_MAP_START, 256) => {
                self.gamma_map[word_index(address, GAMMA_MAP_START)]
            }
            _ if in_words(address, CURSOR_REGISTER_START, 5) => {
                self.cursor[word_index(address, CURSOR_REGISTER_START)]
            }
            _ if in_words(address, CURSOR_GLYPH_START, 64) => {
                self.cursor_glyph[word_index(address, CURSOR_GLYPH_START)]
            }
            _ if in_words(address, VIDEO_CAPTURE_START, 8) => {
                self.video_capture[word_index(address, VIDEO_CAPTURE_START)]
            }
            _ => return Err(CrimeBusError::Unsupported),
        };
        Ok(value)
    }

    pub(super) fn control_status_with_auxiliary_inputs(
        &self,
        auxiliary_inputs: [bool; AUXILIARY_PIN_COUNT],
    ) -> u32 {
        let mut value = self.control_status;
        for (index, input_high) in auxiliary_inputs.into_iter().enumerate() {
            let data_mask = auxiliary_data_mask(index);
            if value & auxiliary_output_disable_mask(index) != 0 {
                value = (value & !data_mask) | if input_high { data_mask } else { 0 };
            }
        }
        value
    }

    pub fn write(&mut self, address: u64, value: u32) -> Result<RegisterWrite, CrimeBusError> {
        let effect = match address {
            CONTROL_STATUS => {
                let previous = self.control_status;
                self.control_status = (self.control_status & !CONTROL_STATUS_WRITABLE_MASK)
                    | (value & CONTROL_STATUS_WRITABLE_MASK);
                if (previous ^ self.control_status) & (3 << 28) != 0 {
                    RegisterWrite::DotClock
                } else {
                    RegisterWrite::None
                }
            }
            DOT_CLOCK => {
                self.dot_clock = value & 0x0310_ffff;
                RegisterWrite::DotClock
            }
            CRT_DDC | FLAT_PANEL_DDC => {
                let value = value & 3;
                if address == CRT_DDC {
                    self.crt_ddc = value;
                } else {
                    self.flat_panel_ddc = value;
                }
                RegisterWrite::Ddc {
                    flat_panel: address == FLAT_PANEL_DDC,
                    clock_released: value & 2 != 0,
                    data_released: value & 1 != 0,
                }
            }
            SYSTEM_CLOCK => {
                self.system_clock = value & 0x0000_7f0f;
                RegisterWrite::None
            }
            DEVICE_ID | COLOR_MAP_FIFO => RegisterWrite::None,
            _ if in_words(address, VT_START, 20) => {
                let index = word_index(address, VT_START);
                self.vt[index] = if index == VT_XY {
                    value & 0x80ff_ffff
                } else {
                    value & 0x00ff_ffff
                };
                RegisterWrite::Timing {
                    counter_written: index == VT_XY,
                }
            }
            _ if in_words(address, OVERLAY_START, 3) => {
                let index = word_index(address, OVERLAY_START);
                if index == 1 {
                    return Ok(RegisterWrite::None);
                }
                let previous = self.overlay[index];
                self.overlay[index] = if index == 0 {
                    value & 0x3fff
                } else {
                    value & 0xffff_ffe1
                };
                if index == 0 && previous & (1 << 13) == 0 && value & (1 << 13) != 0 {
                    RegisterWrite::OverlayFifoReset
                } else if index == 2 {
                    RegisterWrite::Shadow
                } else {
                    RegisterWrite::None
                }
            }
            _ if in_words(address, FRAME_START, 4) => {
                let index = word_index(address, FRAME_START);
                if index == 2 {
                    return Ok(RegisterWrite::None);
                }
                let previous = self.frame[index];
                self.frame[index] = match index {
                    0 => value & 0xffff,
                    1 => value & 0xffff_0000,
                    3 => value & 0xffff_ffe1,
                    _ => unreachable!("validated writable frame register"),
                };
                if index == 0 && previous & (1 << 15) == 0 && value & (1 << 15) != 0 {
                    RegisterWrite::FrameFifoReset
                } else if index == 3 {
                    RegisterWrite::Shadow
                } else {
                    RegisterWrite::None
                }
            }
            _ if in_words(address, DID_START, 2) => {
                let index = word_index(address, DID_START);
                if index == 0 {
                    RegisterWrite::None
                } else {
                    self.did[1] = value & 0x0001_ffff;
                    RegisterWrite::Shadow
                }
            }
            _ if in_words(address, WID_START, 32) => {
                self.wid[word_index(address, WID_START)] = value & 0x1fff;
                RegisterWrite::None
            }
            _ if (COLOR_MAP_START..COLOR_MAP_END).contains(&address) => RegisterWrite::ColorMap {
                index: word_index(address, COLOR_MAP_START) as u16,
                value: value & 0xffff_ff00,
            },
            _ if in_words(address, GAMMA_MAP_START, 256) => {
                self.gamma_map[word_index(address, GAMMA_MAP_START)] = value & 0xffff_ff00;
                RegisterWrite::None
            }
            _ if in_words(address, CURSOR_REGISTER_START, 5) => {
                let index = word_index(address, CURSOR_REGISTER_START);
                self.cursor[index] = match index {
                    0 => value,
                    1 => value & 3,
                    2..=4 => value & 0xffff_ff00,
                    _ => unreachable!("validated cursor-register index"),
                };
                RegisterWrite::None
            }
            _ if in_words(address, CURSOR_GLYPH_START, 64) => {
                self.cursor_glyph[word_index(address, CURSOR_GLYPH_START)] = value;
                RegisterWrite::None
            }
            _ if in_words(address, VIDEO_CAPTURE_START, 9) => {
                let index = word_index(address, VIDEO_CAPTURE_START);
                self.video_capture[index] = match index {
                    0 | 1 => value & 0x00ff_ffff,
                    2 => value & 0x1f,
                    3 => {
                        let oddeven = self.video_capture[3] & (1 << 4);
                        oddeven | (value & !(1 << 4))
                    }
                    4..=8 => value,
                    _ => unreachable!("validated video-capture register index"),
                };
                RegisterWrite::Capture
            }
            _ => return Err(CrimeBusError::Unsupported),
        };
        Ok(effect)
    }

    pub fn commit_shadow(&mut self) {
        self.overlay[1] = self.overlay[2];
        self.frame[2] = self.frame[3];
        self.did[0] = self.did[1];
    }

    pub fn set_ddc_levels(&mut self, flat_panel: bool, clock_high: bool, data_high: bool) {
        let register = if flat_panel {
            &mut self.flat_panel_ddc
        } else {
            &mut self.crt_ddc
        };
        *register = (*register & !3) | u32::from(data_high) | (u32::from(clock_high) << 1);
    }

    fn dot_clock_with_lock(&self) -> u32 {
        if self.dot_clock & DOT_CLOCK_RUN == 0 {
            (self.dot_clock & !(1 << 22)) | (1 << 23)
        } else {
            self.dot_clock & !((1 << 22) | (1 << 23))
        }
    }
}

fn in_words(address: u64, start: u64, words: usize) -> bool {
    (start..start + (words as u64) * 4).contains(&address)
}

fn word_index(address: u64, start: u64) -> usize {
    ((address - start) / 4) as usize
}

pub(super) const fn auxiliary_data_mask(index: usize) -> u32 {
    1 << (6 + index * 2)
}

pub(super) const fn auxiliary_output_disable_mask(index: usize) -> u32 {
    1 << (7 + index * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_one_reset_values_are_deterministic() {
        let registers = GbeRegisters::new();
        assert_eq!(registers.control_status & 0xf, 1);
        assert_eq!(registers.control_status & 0x03ff_ffc0, 0x03ff_ffc0);
        assert_eq!(registers.crt_ddc, 3);
        assert_eq!(registers.flat_panel_ddc, 3);
        assert_eq!(registers.system_clock, 1 << 3);
        assert_eq!(registers.vt[VT_XY], VT_XY_FREEZE);
        assert_eq!(registers.video_capture[2], 6);
        assert_eq!(registers.video_capture[3], 1 << 4);
        assert!(registers.color_map.iter().all(|value| *value == 0));
        assert!(registers.gamma_map.iter().all(|value| *value == 0));
    }

    #[test]
    fn control_status_accepts_all_gpio_data_and_output_enable_bits() {
        let mut registers = GbeRegisters::new();
        registers.write(CONTROL_STATUS, 0).unwrap();
        assert_eq!(registers.control_status & 0x3fff_ffc0, 0);
        assert_eq!(registers.control_status & 0x1f, 0x11);

        registers.write(CONTROL_STATUS, 0x1555_5540).unwrap();
        assert_eq!(registers.control_status & 0x3fff_ffc0, 0x1555_5540);
    }

    #[test]
    fn control_status_resolves_bidirectional_auxiliary_pins() {
        let mut registers = GbeRegisters::new();
        let all_inputs = (0..AUXILIARY_PIN_COUNT)
            .map(auxiliary_output_disable_mask)
            .fold(0, |mask, bit| mask | bit);
        let levels = [
            true, false, true, false, true, false, true, false, true, false,
        ];

        registers
            .write(CONTROL_STATUS, all_inputs | auxiliary_data_mask(1))
            .unwrap();
        let resolved = registers.control_status_with_auxiliary_inputs(levels);
        for (index, expected) in levels.into_iter().enumerate() {
            assert_eq!(resolved & auxiliary_data_mask(index) != 0, expected);
        }
        assert_ne!(registers.control_status & auxiliary_data_mask(1), 0);

        registers
            .write(CONTROL_STATUS, auxiliary_data_mask(1))
            .unwrap();
        let resolved = registers.control_status_with_auxiliary_inputs([false; AUXILIARY_PIN_COUNT]);
        assert_ne!(resolved & auxiliary_data_mask(1), 0);
    }

    #[test]
    fn revision_two_registers_and_ram_extensions_are_holes() {
        let registers = GbeRegisters::new();
        for address in [
            GBE_BASE + 0x18,
            GBE_BASE + 0x1c,
            COLOR_MAP_END,
            GBE_BASE + 0x0006_8000,
        ] {
            assert_eq!(registers.read(address), Err(CrimeBusError::Unsupported));
        }
    }

    #[test]
    fn each_on_chip_ram_honors_its_exact_revision_one_boundary() {
        let mut registers = GbeRegisters::new();
        for (last, first_hole) in [
            (WID_START + 31 * 4, WID_START + 32 * 4),
            (COLOR_MAP_START + 4_607 * 4, COLOR_MAP_END),
            (GAMMA_MAP_START + 255 * 4, GAMMA_MAP_START + 256 * 4),
            (CURSOR_GLYPH_START + 63 * 4, CURSOR_GLYPH_START + 64 * 4),
        ] {
            registers.write(last, 0x1234_5678).unwrap();
            assert!(registers.read(last).is_ok());
            assert_eq!(registers.read(first_hole), Err(CrimeBusError::Unsupported));
        }
    }

    #[test]
    fn hardware_shadow_only_changes_at_explicit_commit() {
        let mut registers = GbeRegisters::new();
        registers.write(FRAME_START + 12, 0x1234_0001).unwrap();
        registers.write(OVERLAY_START + 8, 0x5678_0001).unwrap();
        registers.write(DID_START + 4, 0x0001_1234).unwrap();
        assert_eq!(registers.frame[2], 0);
        assert_eq!(registers.overlay[1], 0);
        assert_eq!(registers.did[0], 0);
        registers.commit_shadow();
        assert_eq!(registers.frame[2], 0x1234_0001);
        assert_eq!(registers.overlay[1], 0x5678_0001);
        assert_eq!(registers.did[0], 0x0001_1234);
    }

    #[test]
    fn fifo_reset_is_rising_edge_triggered_and_vc8_is_write_only() {
        let mut registers = GbeRegisters::new();
        assert_eq!(
            registers.write(FRAME_START, 1 << 15),
            Ok(RegisterWrite::FrameFifoReset)
        );
        assert_eq!(
            registers.write(FRAME_START, 1 << 15),
            Ok(RegisterWrite::None)
        );
        assert_eq!(
            registers.write(VIDEO_CAPTURE_START + 8 * 4, 0x1234_5678),
            Ok(RegisterWrite::Capture)
        );
        assert_eq!(
            registers.read(VIDEO_CAPTURE_START + 8 * 4),
            Err(CrimeBusError::Unsupported)
        );
        assert_eq!(registers.video_capture[8], 0x1234_5678);
    }
}

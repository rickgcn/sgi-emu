//! SGI Graphics Back End device model.

use std::collections::BTreeMap;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;
use se_core::scheduler::SimTime;

use super::crime::protocol::{
    CrimeBusError, CrimeCgiCompletion, CrimeCgiTransaction, CrimeCompletionPayload, CrimeData,
    CrimeLinkDeviceResponse, CrimeLinkOperation, CrimeTransferView,
};

const GBE_BASE: u64 = 0x1600_0000;
const CONTROL_STATUS: u64 = GBE_BASE;
const DEVICE_ID: u64 = GBE_BASE + 0x14;
const CONFIGURATION: u64 = GBE_BASE + 0x18;
const BIST_STATUS: u64 = GBE_BASE + 0x1c;

const TIMING_START: u64 = GBE_BASE + 0x0001_0000;
const TIMING_END: u64 = GBE_BASE + 0x0001_0050;
const VT_XY: u64 = TIMING_START;
const VT_XY_MAX: u64 = TIMING_START + 4;
const OVERLAY_START: u64 = GBE_BASE + 0x0002_0000;
const OVERLAY_END: u64 = GBE_BASE + 0x0002_000c;
const OVERLAY_IN_HARDWARE_CONTROL: u64 = OVERLAY_START + 4;
const OVERLAY_CONTROL: u64 = OVERLAY_START + 8;
const FRAME_START: u64 = GBE_BASE + 0x0003_0000;
const FRAME_END: u64 = GBE_BASE + 0x0003_0010;
const FRAME_IN_HARDWARE_CONTROL: u64 = FRAME_START + 8;
const FRAME_CONTROL: u64 = FRAME_START + 0xc;
const DID_START: u64 = GBE_BASE + 0x0004_0000;
const DID_END: u64 = GBE_BASE + 0x0004_0008;
const DID_IN_HARDWARE_CONTROL: u64 = DID_START;
const DID_CONTROL: u64 = DID_START + 4;
const MODE_START: u64 = GBE_BASE + 0x0004_8000;
const MODE_END: u64 = GBE_BASE + 0x0004_8080;
const COLOR_MAP_START: u64 = GBE_BASE + 0x0005_0000;
const COLOR_MAP_END: u64 = GBE_BASE + 0x0005_6000;
const COLOR_MAP_FIFO: u64 = GBE_BASE + 0x0005_8000;
const GAMMA_MAP_START: u64 = GBE_BASE + 0x0006_0000;
const GAMMA_MAP_END: u64 = GBE_BASE + 0x0006_0400;
const GAMMA_MAP10_START: u64 = GBE_BASE + 0x0006_8000;
const GAMMA_MAP10_END: u64 = GBE_BASE + 0x0006_9000;
const CURSOR_REGISTER_START: u64 = GBE_BASE + 0x0007_0000;
const CURSOR_REGISTER_END: u64 = GBE_BASE + 0x0007_0014;
const CURSOR_GLYPH_START: u64 = GBE_BASE + 0x0007_8000;
const CURSOR_GLYPH_END: u64 = GBE_BASE + 0x0007_8100;
const VIDEO_CAPTURE_START: u64 = GBE_BASE + 0x0008_0000;
const VIDEO_CAPTURE_END: u64 = GBE_BASE + 0x0008_0024;

const CONTROL_STATUS_WRITABLE_MASK: u32 = 0x3eaa_aa80;
const DEVICE_ID_VALUE: u32 = 0x0000_0666;
const PIXEL_REFERENCE_CLOCK_HZ: u64 = 20_000_000;
const VT_XY_FREEZE: u32 = 1 << 31;

/// SGI Graphics Back End connected to the CRIME CGI link.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Gbe {
    id: ComponentId,
    name: String,
    control_status: u32,
    registers: BTreeMap<u64, u32>,
    timebase_hz: u64,
    observed_time: SimTime,
    scan_origin_time: SimTime,
    scan_origin_pixel: u64,
}

crate::component_state!(GbeState, Gbe);

impl Gbe {
    /// Creates a GBE with inactive external sense inputs.
    pub fn new(id: ComponentId, name: impl Into<String>, timebase_hz: u64) -> Self {
        assert!(timebase_hz != 0, "the GBE timebase must be nonzero");
        Self {
            id,
            name: name.into(),
            control_status: 0,
            registers: BTreeMap::new(),
            timebase_hz,
            observed_time: SimTime::ZERO,
            scan_origin_time: SimTime::ZERO,
            scan_origin_pixel: 0,
        }
    }

    /// Updates the simulated time observed by lazy display counters.
    pub fn observe_time(&mut self, now: SimTime) {
        self.observed_time = now;
    }

    fn access(
        &mut self,
        address: u64,
        transfer: CrimeTransferView<'_>,
    ) -> Result<CrimeCompletionPayload, CrimeBusError> {
        if address & 3 != 0 {
            return Err(CrimeBusError::Access);
        }

        match transfer {
            CrimeTransferView::Read { length: 4 } => {
                let value = match address {
                    CONTROL_STATUS => self.control_status,
                    DEVICE_ID => DEVICE_ID_VALUE,
                    CONFIGURATION | BIST_STATUS => 0,
                    COLOR_MAP_FIFO => 0,
                    VT_XY => self.scan_position(),
                    _ if is_backed_register(address) => {
                        self.registers.get(&address).copied().unwrap_or(0)
                    }
                    _ => return Err(CrimeBusError::Unsupported),
                };
                Ok(CrimeCompletionPayload::ReadData(CrimeData::from(
                    value.to_be_bytes(),
                )))
            }
            CrimeTransferView::Write { data, byte_enable }
                if data.len() == 4
                    && byte_enable.len() == 4
                    && byte_enable.iter().all(|enabled| enabled) =>
            {
                let value = u32::from_be_bytes(data.try_into().expect("validated GBE write width"));
                match address {
                    CONTROL_STATUS => {
                        self.control_status = (self.control_status & !CONTROL_STATUS_WRITABLE_MASK)
                            | (value & CONTROL_STATUS_WRITABLE_MASK);
                    }
                    DEVICE_ID | CONFIGURATION | BIST_STATUS | COLOR_MAP_FIFO => {}
                    _ if is_backed_register(address) => {
                        self.registers.insert(address, value);
                        if matches!(address, VT_XY | VT_XY_MAX) {
                            self.reset_scan_origin(value, address);
                        }
                        if let Some(in_hardware) = shadow_destination(address) {
                            // Scan timing is outside this functional model, so
                            // shadow state becomes hardware-visible immediately.
                            self.registers.insert(in_hardware, value);
                        }
                    }
                    _ => return Err(CrimeBusError::Unsupported),
                }
                Ok(CrimeCompletionPayload::WriteComplete)
            }
            CrimeTransferView::Read { .. } | CrimeTransferView::Write { .. } => {
                Err(CrimeBusError::Access)
            }
        }
    }

    fn reset_scan_origin(&mut self, value: u32, address: u64) {
        self.scan_origin_time = self.observed_time;
        self.scan_origin_pixel = if address == VT_XY {
            let maximum = self.registers.get(&VT_XY_MAX).copied().unwrap_or(0);
            let width = u64::from((maximum & 0x0fff) + 1);
            u64::from((value >> 12) & 0x0fff) * width + u64::from(value & 0x0fff)
        } else {
            0
        };
    }

    fn scan_position(&self) -> u32 {
        let stored = self.registers.get(&VT_XY).copied().unwrap_or(0);
        if stored & VT_XY_FREEZE != 0 {
            return stored;
        }
        let maximum = self.registers.get(&VT_XY_MAX).copied().unwrap_or(0);
        let width = u64::from((maximum & 0x0fff) + 1);
        let height = u64::from(((maximum >> 12) & 0x0fff) + 1);
        let frame_pixels = width * height;
        let elapsed = self
            .observed_time
            .get()
            .saturating_sub(self.scan_origin_time.get());
        let advanced = (u128::from(elapsed) * u128::from(PIXEL_REFERENCE_CLOCK_HZ)
            / u128::from(self.timebase_hz)) as u64;
        let pixel = (self.scan_origin_pixel + advanced) % frame_pixels;
        let x = pixel % width;
        let y = pixel / width;
        (y as u32) << 12 | x as u32
    }
}

impl Component for Gbe {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.control_status = 0;
        self.registers.clear();
        self.observed_time = SimTime::ZERO;
        self.scan_origin_time = SimTime::ZERO;
        self.scan_origin_pixel = 0;
    }
}

fn is_backed_register(address: u64) -> bool {
    [
        (GBE_BASE + 4, GBE_BASE + 0x14),
        (TIMING_START, TIMING_END),
        (OVERLAY_START, OVERLAY_END),
        (FRAME_START, FRAME_END),
        (DID_START, DID_END),
        (MODE_START, MODE_END),
        (COLOR_MAP_START, COLOR_MAP_END),
        (GAMMA_MAP_START, GAMMA_MAP_END),
        (GAMMA_MAP10_START, GAMMA_MAP10_END),
        (CURSOR_REGISTER_START, CURSOR_REGISTER_END),
        (CURSOR_GLYPH_START, CURSOR_GLYPH_END),
        (VIDEO_CAPTURE_START, VIDEO_CAPTURE_END),
    ]
    .into_iter()
    .any(|(start, end)| (start..end).contains(&address))
}

const fn shadow_destination(address: u64) -> Option<u64> {
    match address {
        OVERLAY_CONTROL => Some(OVERLAY_IN_HARDWARE_CONTROL),
        FRAME_CONTROL => Some(FRAME_IN_HARDWARE_CONTROL),
        DID_CONTROL => Some(DID_IN_HARDWARE_CONTROL),
        _ => None,
    }
}

impl BusDeviceRole<CrimeCgiTransaction> for Gbe {
    type Response = CrimeLinkDeviceResponse<CrimeCgiCompletion>;

    fn accept(&mut self, transaction: CrimeCgiTransaction) -> Self::Response {
        let result = match &transaction.operation {
            CrimeLinkOperation::Pio(request) => {
                self.access(request.address, request.transfer.view())
            }
            CrimeLinkOperation::Dma(_) | CrimeLinkOperation::InterruptPost(_) => {
                Err(CrimeBusError::Unsupported)
            }
        };
        CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
            id: transaction.id,
            result,
            memory_fault: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use se_core::role::BusDeviceRole;

    use super::*;
    use crate::chipset::crime::protocol::{
        CrimeByteEnable, CrimePioRequest, CrimeTransactionId, CrimeTransfer,
    };

    const GBE: ComponentId = ComponentId::new(1);
    const CRIME: ComponentId = ComponentId::new(2);

    fn transaction(address: u64, transfer: CrimeTransfer) -> CrimeCgiTransaction {
        CrimeCgiTransaction {
            id: CrimeTransactionId::new(7),
            controller: CRIME,
            target: GBE,
            operation: CrimeLinkOperation::Pio(CrimePioRequest { address, transfer }),
        }
    }

    fn result(
        gbe: &mut Gbe,
        address: u64,
        transfer: CrimeTransfer,
    ) -> Result<CrimeCompletionPayload, CrimeBusError> {
        match gbe.accept(transaction(address, transfer)) {
            CrimeLinkDeviceResponse::Complete(completion) => completion.result,
            CrimeLinkDeviceResponse::Deferred => panic!("GBE PIO unexpectedly deferred"),
        }
    }

    #[test]
    fn control_status_supports_prom_probe_and_preserves_read_only_bits() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        assert_eq!(
            result(&mut gbe, CONTROL_STATUS, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(0_u32.to_be_bytes().into()))
        );

        assert_eq!(
            result(
                &mut gbe,
                CONTROL_STATUS,
                CrimeTransfer::write(
                    0x020a_a000_u32.to_be_bytes().into(),
                    CrimeByteEnable::from([true; 4]),
                ),
            ),
            Ok(CrimeCompletionPayload::WriteComplete)
        );
        assert_eq!(
            result(&mut gbe, CONTROL_STATUS, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(
                0x020a_a000_u32.to_be_bytes().into()
            ))
        );

        result(
            &mut gbe,
            CONTROL_STATUS,
            CrimeTransfer::write(
                u32::MAX.to_be_bytes().into(),
                CrimeByteEnable::from([true; 4]),
            ),
        )
        .unwrap();
        assert_eq!(
            result(&mut gbe, CONTROL_STATUS, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(
                CONTROL_STATUS_WRITABLE_MASK.to_be_bytes().into()
            ))
        );
    }

    #[test]
    fn device_id_is_read_only() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        assert_eq!(
            result(&mut gbe, DEVICE_ID, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(
                DEVICE_ID_VALUE.to_be_bytes().into()
            ))
        );
        result(
            &mut gbe,
            DEVICE_ID,
            CrimeTransfer::write(0_u32.to_be_bytes().into(), CrimeByteEnable::from([true; 4])),
        )
        .unwrap();
        assert_eq!(
            result(&mut gbe, DEVICE_ID, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(
                DEVICE_ID_VALUE.to_be_bytes().into()
            ))
        );
    }

    #[test]
    fn invalid_access_shapes_and_unknown_registers_remain_strict() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        assert_eq!(
            result(&mut gbe, CONTROL_STATUS + 1, CrimeTransfer::read(4)),
            Err(CrimeBusError::Access)
        );
        assert_eq!(
            result(&mut gbe, CONTROL_STATUS, CrimeTransfer::read(8)),
            Err(CrimeBusError::Access)
        );
        assert_eq!(
            result(&mut gbe, GBE_BASE + 0x20, CrimeTransfer::read(4)),
            Err(CrimeBusError::Unsupported)
        );
        assert_eq!(
            result(
                &mut gbe,
                CONTROL_STATUS,
                CrimeTransfer::write(
                    0_u32.to_be_bytes().into(),
                    CrimeByteEnable::from([true, false, true, true]),
                ),
            ),
            Err(CrimeBusError::Access)
        );
    }

    #[test]
    fn documented_register_blocks_store_values_and_reset_to_zero() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        for address in [
            TIMING_START + 8,
            OVERLAY_START + 8,
            FRAME_START + 0xc,
            DID_START + 4,
            MODE_START + 0x7c,
            COLOR_MAP_START + 0x5ffc,
            GAMMA_MAP_START + 0x3fc,
            GAMMA_MAP10_START + 0xffc,
            CURSOR_REGISTER_START + 0x10,
            CURSOR_GLYPH_START + 0xfc,
            VIDEO_CAPTURE_START + 0x20,
        ] {
            result(
                &mut gbe,
                address,
                CrimeTransfer::write(
                    0x1234_5678_u32.to_be_bytes().into(),
                    CrimeByteEnable::from([true; 4]),
                ),
            )
            .unwrap();
            assert_eq!(
                result(&mut gbe, address, CrimeTransfer::read(4)),
                Ok(CrimeCompletionPayload::ReadData(
                    0x1234_5678_u32.to_be_bytes().into()
                ))
            );
        }

        gbe.reset();
        assert_eq!(
            result(&mut gbe, FRAME_START + 0xc, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(0_u32.to_be_bytes().into()))
        );
    }

    #[test]
    fn dma_controls_publish_to_the_in_hardware_shadow() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        for (control, in_hardware) in [
            (OVERLAY_CONTROL, OVERLAY_IN_HARDWARE_CONTROL),
            (FRAME_CONTROL, FRAME_IN_HARDWARE_CONTROL),
            (DID_CONTROL, DID_IN_HARDWARE_CONTROL),
        ] {
            result(
                &mut gbe,
                control,
                CrimeTransfer::write(1_u32.to_be_bytes().into(), CrimeByteEnable::from([true; 4])),
            )
            .unwrap();
            assert_eq!(
                result(&mut gbe, in_hardware, CrimeTransfer::read(4)),
                Ok(CrimeCompletionPayload::ReadData(1_u32.to_be_bytes().into()))
            );
        }
    }

    #[test]
    fn scan_position_advances_lazily_and_honors_freeze() {
        let mut gbe = Gbe::new(GBE, "GBE", 1_000_000_000);
        result(
            &mut gbe,
            VT_XY_MAX,
            CrimeTransfer::write(
                0x0000_1009_u32.to_be_bytes().into(),
                CrimeByteEnable::from([true; 4]),
            ),
        )
        .unwrap();
        gbe.observe_time(SimTime::new(50));
        assert_eq!(
            result(&mut gbe, VT_XY, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(1_u32.to_be_bytes().into()))
        );

        result(
            &mut gbe,
            VT_XY,
            CrimeTransfer::write(
                (VT_XY_FREEZE | 0x1002).to_be_bytes().into(),
                CrimeByteEnable::from([true; 4]),
            ),
        )
        .unwrap();
        gbe.observe_time(SimTime::new(50_000));
        assert_eq!(
            result(&mut gbe, VT_XY, CrimeTransfer::read(4)),
            Ok(CrimeCompletionPayload::ReadData(
                (VT_XY_FREEZE | 0x1002).to_be_bytes().into()
            ))
        );
    }
}

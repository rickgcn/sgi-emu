//! LG1 entry graphics board for the SGI Indigo.
//!
//! The board carries a REX1 raster engine, a VC1 display controller, and a
//! Bt479 color palette. The processor reaches every part through the REX
//! register windows: drawing registers occupy the first page, and a
//! configuration window drives the other two devices one byte at a time.

mod bt479;
mod display;
mod rex1;
mod vc1;
mod vram;

use std::sync::Arc;

use se_core::bus::{BusError, DeviceAddr};
use se_core::time::VirtualDuration;
use serde::{Deserialize, Serialize};

use self::bt479::Bt479;
use self::rex1::{ConfigAccess, PeripheralPort, Rex1};
use self::vc1::{SignalState, Vc1};
use self::vram::Vram;
use crate::gio::{GioDevice, GioDeviceSnapshot, GioDisplayState, GioInterrupt};

/// Start of the board's PIO aperture within its GIO slot.
const GIO_PIO_BASE: u64 = 0x003f_0000;
/// Bytes decoded by the board's PIO aperture.
const GIO_PIO_BYTES: u64 = 0x8000;
const GIO_PIO_END: u64 = GIO_PIO_BASE + GIO_PIO_BYTES;
/// REX1 host-data GO port selected by graphics DMA.
const GRAPHICS_DMA_PORT: u64 = GIO_PIO_BASE + rex1::GO_OFFSET + rex1::RWAUX1;

/// Board revision reported through the configuration interface.
///
/// A revision below two selects the Bt479 initialization path, which is the
/// device this board carries. The exact value inside that range is an
/// emulation target rather than a recorded property of a physical board.
const BOARD_REVISION: u8 = 1;

/// Monitor identification reported alongside the board revision.
///
/// The PROM reads this field to choose a sync configuration. Encoding six
/// selects one sync option and every other encoding selects the other.
const MONITOR_CODE: u8 = 0;

/// Selector value that reads the board revision through the clock port.
const CLOCK_SELECTOR_REVISION: u8 = 4;

/// The LG1 graphics board.
#[derive(Clone, Deserialize, Serialize)]
pub struct Lg1 {
    rex: Rex1,
    vc1: Vc1,
    dac: Bt479,
    vram: Vram,
    /// Most recent complete frame, shared with the frontend without copying.
    ///
    /// A snapshot writes the pixels themselves. Sharing is host state: a
    /// restore establishes fresh ownership rather than reviving the handles
    /// the frontend held.
    frame: Option<Arc<Vec<u8>>>,
    /// Set when the displayed picture changed since the last query.
    ///
    /// This is guest-visible progress rather than host delivery state: two
    /// runs that execute the same instructions produce the same flag, so it
    /// belongs in the snapshot.
    display_changed: bool,
}

impl Lg1 {
    /// Creates a board in its power-on state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub fn new() -> Self {
        Self {
            rex: Rex1::new(),
            vc1: Vc1::new(),
            dac: Bt479::new(),
            vram: Vram::new(),
            frame: None,
            display_changed: false,
        }
    }
}

impl GioDevice for Lg1 {
    /// Restores the board reset state, clearing the frame buffer.
    fn reset(&mut self) {
        self.rex.reset();
        self.vc1.reset();
        self.dac.reset();
        self.vram.reset();
        self.frame = None;
        self.display_changed = false;
    }

    /// Reports whether the displayed picture changed, clearing the flag.
    ///
    /// A machine calls this once per advancement to decide whether the
    /// frontend needs a display update.
    fn take_display_update(&mut self) -> bool {
        let changed = self.display_changed;
        self.display_changed = false;
        changed
    }

    /// Returns what the board currently drives onto the monitor.
    ///
    /// The query has no side effects, so a paused machine and a debugger can
    /// both use it without changing guest-visible state.
    fn display_state(&self) -> Option<GioDisplayState> {
        Some(match self.vc1.signal_state() {
            SignalState::NoSignal => GioDisplayState::NoSignal,
            SignalState::Blanked => GioDisplayState::Blank,
            SignalState::Active => match &self.frame {
                Some(pixels) => GioDisplayState::Active {
                    width: vc1::DISPLAY_WIDTH,
                    height: vc1::DISPLAY_HEIGHT,
                    pixels: Arc::clone(pixels),
                },
                None => GioDisplayState::Blank,
            },
        })
    }

    fn interrupt_asserted(&self, interrupt: GioInterrupt) -> bool {
        matches!(interrupt, GioInterrupt::Interrupt2)
            && !matches!(self.vc1.signal_state(), SignalState::NoSignal)
            && self.vc1.in_vertical_blanking()
    }

    /// Advances display timing and composes any frame that completed.
    fn advance_time(&mut self, elapsed: VirtualDuration) {
        if self.vc1.advance_time(elapsed) == 0 {
            return;
        }
        self.compose_frame();
        self.display_changed = true;
    }

    /// Returns the duration until the next display timing boundary.
    fn time_until_event(&self) -> Option<VirtualDuration> {
        self.vc1.time_until_event()
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, or [`BusError::UnimplementedAccess`] for a register
    /// or width the board does not implement.
    fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let Some(offset) = register_offset(address, data.len())? else {
            data.fill(0);
            return Ok(());
        };
        let value = match decode(offset) {
            Some(Register::Drawing(offset)) => self.rex.read_drawing(offset),
            Some(Register::DrawingGo(offset)) => self.rex.read_drawing_go(offset, &mut self.vram),
            Some(Register::Dummy) => self.rex.read_dummy(),
            Some(Register::Config(offset)) => self.rex.read_config(offset).0,
            Some(Register::ConfigGo(offset)) => {
                let (value, access) = self.rex.read_config(offset);
                if let ConfigAccess::Read(port) = access {
                    let received = self.read_peripheral(port);
                    self.rex.complete_config_read(port, received);
                }
                value
            }
            None => return Err(BusError::UnimplementedAccess),
        };
        data.copy_from_slice(&value.to_be_bytes());
        Ok(())
    }

    /// Reads one transaction without disturbing board state.
    ///
    /// Debugger queries use this path so that inspecting the board never
    /// starts a drawing command or consumes a peripheral transfer.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, or [`BusError::UnimplementedAccess`] for a register
    /// or width the board does not implement.
    fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let Some(offset) = register_offset(address, data.len())? else {
            data.fill(0);
            return Ok(());
        };
        let value = match decode(offset) {
            Some(Register::Drawing(offset) | Register::DrawingGo(offset)) => {
                self.rex.read_drawing(offset)
            }
            Some(Register::Dummy) => self.rex.read_dummy(),
            Some(Register::Config(offset) | Register::ConfigGo(offset)) => {
                self.rex.read_config(offset).0
            }
            None => return Err(BusError::UnimplementedAccess),
        };
        data.copy_from_slice(&value.to_be_bytes());
        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, or [`BusError::UnimplementedAccess`] for a register
    /// or width the board does not implement.
    fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let Some(offset) = register_offset(address, data.len())? else {
            return Ok(());
        };
        let value = u32::from_be_bytes(data.try_into().map_err(|_| BusError::InvalidTransaction)?);
        match decode(offset) {
            Some(Register::Drawing(offset)) => self.rex.write_drawing(offset, value),
            Some(Register::DrawingGo(offset)) => {
                self.rex.write_drawing_go(offset, value, &mut self.vram);
            }
            Some(Register::Dummy) => self.rex.write_dummy(value),
            // A SET write only loads the data port. The paired GO write
            // performs the transfer, so a SET and GO pair moves one byte and
            // a GO write on its own still moves that byte.
            Some(Register::Config(offset)) => {
                self.rex.write_config(offset, value);
            }
            Some(Register::ConfigGo(offset)) => {
                let access = self.rex.write_config(offset, value);
                if let ConfigAccess::Write(port, byte) = access {
                    self.write_peripheral(port, byte);
                }
            }
            None => return Err(BusError::UnimplementedAccess),
        }
        Ok(())
    }

    /// Reads a DMA stream through the REX1 RWAUX1 GO port.
    ///
    /// Each big-endian word runs one REX host-data command. PIC1 transfers
    /// complete GIO words even when the descriptor width is not word-aligned.
    fn read_dma(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        if address.get() != GRAPHICS_DMA_PORT {
            return Err(BusError::UnimplementedAccess);
        }
        if !data.len().is_multiple_of(4) {
            return Err(BusError::InvalidTransaction);
        }
        for bytes in data.chunks_exact_mut(4) {
            bytes.copy_from_slice(&self.rex.read_host_data_go(&mut self.vram).to_be_bytes());
        }
        Ok(())
    }

    /// Writes a DMA stream through the REX1 RWAUX1 GO port.
    ///
    /// Each big-endian word runs one REX host-data command. At a rectangle
    /// scan-line end, REX discards unused lanes before the next word begins.
    fn write_dma(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        if address.get() != GRAPHICS_DMA_PORT {
            return Err(BusError::UnimplementedAccess);
        }
        if !data.len().is_multiple_of(4) {
            return Err(BusError::InvalidTransaction);
        }
        for bytes in data.chunks_exact(4) {
            self.rex.write_host_data_go(
                u32::from_be_bytes(bytes.try_into().expect("DMA chunks have a fixed width")),
                &mut self.vram,
            );
        }
        Ok(())
    }

    /// Reports the REX1 DMA endpoint ready.
    ///
    /// REX commands currently complete synchronously and the model has no
    /// input FIFO occupancy or backpressure. A future asynchronous FIFO can
    /// derive this signal from its readiness without changing PIC1.
    fn dma_sync_asserted(&self) -> bool {
        true
    }

    fn snapshot(&self) -> GioDeviceSnapshot {
        GioDeviceSnapshot::Lg1(Box::new(self.clone()))
    }

    fn accepts_snapshot(&self, snapshot: &GioDeviceSnapshot) -> bool {
        matches!(snapshot, GioDeviceSnapshot::Lg1(_))
    }

    fn restore_snapshot(&mut self, snapshot: GioDeviceSnapshot) {
        let GioDeviceSnapshot::Lg1(board) = snapshot;
        *self = *board;
    }
}

impl Lg1 {
    /// Delivers one byte to the peripheral selected by the configuration bus.
    fn write_peripheral(&mut self, port: PeripheralPort, value: u8) {
        let selector = self.rex.config.configsel as u8;
        let before = self.vc1.signal_state();
        match port {
            PeripheralPort::Dac => self.dac.write(bt479::Selector::from_bits(selector), value),
            PeripheralPort::Vc1 => self.vc1.write(vc1::Selector::from_bits(selector), value),
            // The clock generator byte stream is accepted and stored by the
            // register itself. Which device it programs, and how the bytes
            // encode a pixel rate, is not established.
            PeripheralPort::Clock => {}
        }

        // Losing or regaining the video signal changes what the monitor shows
        // without waiting for a frame to complete.
        let after = self.vc1.signal_state();
        if before == after {
            return;
        }
        if matches!(after, SignalState::NoSignal) {
            self.frame = None;
        }
        self.display_changed = true;
    }

    /// Reads the next byte presented by one configuration peripheral.
    fn read_peripheral(&mut self, port: PeripheralPort) -> u8 {
        let selector = self.rex.config.configsel as u8;
        match port {
            PeripheralPort::Dac => self.dac.read(bt479::Selector::from_bits(selector)),
            PeripheralPort::Vc1 => self.vc1.read(vc1::Selector::from_bits(selector)),
            PeripheralPort::Clock => self.read_clock_port(selector),
        }
    }

    /// Returns the byte the clock port presents for one selector.
    ///
    /// Selector four reports the board identity, which is how the PROM learns
    /// the board revision and the attached monitor.
    const fn read_clock_port(&self, selector: u8) -> u8 {
        if selector == CLOCK_SELECTOR_REVISION {
            BOARD_REVISION | (MONITOR_CODE << 3)
        } else {
            self.rex.config.wclock as u8
        }
    }

    /// Builds the frame the display path currently produces.
    fn compose_frame(&mut self) {
        let mut pixels = match self.frame.take() {
            // Reuse the previous allocation when no one else holds it.
            Some(shared) => match Arc::try_unwrap(shared) {
                Ok(pixels) => pixels,
                Err(_) => vec![0; display::FRAME_BYTES],
            },
            None => vec![0; display::FRAME_BYTES],
        };

        match self.vc1.signal_state() {
            SignalState::NoSignal => {
                self.frame = None;
                return;
            }
            SignalState::Blanked => display::compose_blank(&mut pixels),
            SignalState::Active => display::compose(&self.vram, &self.vc1, &self.dac, &mut pixels),
        }
        self.frame = Some(Arc::new(pixels));
    }
}

/// One decoded board register.
enum Register {
    /// A drawing register reached through the SET window.
    Drawing(u64),
    /// A drawing register reached through the GO window.
    DrawingGo(u64),
    /// The pad word between the drawing pages.
    Dummy,
    /// A configuration register reached through the SET window.
    Config(u64),
    /// A configuration register reached through the GO window.
    ConfigGo(u64),
}

/// Decodes one board offset into the register it selects.
const fn decode(offset: u64) -> Option<Register> {
    if offset < rex1::DRAWING_END {
        return Some(Register::Drawing(offset));
    }
    if offset == rex1::DUMMY {
        return Some(Register::Dummy);
    }
    let go = offset.wrapping_sub(rex1::GO_OFFSET);
    if offset >= rex1::GO_OFFSET && go < rex1::DRAWING_END {
        return Some(Register::DrawingGo(go));
    }
    if offset >= rex1::CONFIG_BASE && offset < rex1::CONFIG_END {
        return Some(Register::Config(offset));
    }
    if offset >= rex1::CONFIG_BASE + rex1::GO_OFFSET && go < rex1::CONFIG_END {
        return Some(Register::ConfigGo(go));
    }
    None
}

/// Validates one transaction and returns the register offset it addresses.
///
/// Board registers are whole words on word boundaries; the guest paths
/// examined so far never use a narrower access.
fn register_offset(address: DeviceAddr, length: usize) -> Result<Option<u64>, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }
    let start = address.get();
    let end = start
        .checked_add(length as u64)
        .ok_or(BusError::InvalidTransaction)?;
    if end <= GIO_PIO_BASE || start >= GIO_PIO_END {
        return Ok(None);
    }
    if start < GIO_PIO_BASE || end > GIO_PIO_END {
        return Err(BusError::HardwareFault);
    }
    if length != 4 || !start.is_multiple_of(4) {
        return Err(BusError::UnimplementedAccess);
    }
    Ok(Some(start - GIO_PIO_BASE))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::vram::PlaneGroup;
    use super::{GIO_PIO_BASE, GIO_PIO_END, GRAPHICS_DMA_PORT, Lg1};
    use crate::gio::{GioDevice, GioDisplayState, GioInterrupt};

    /// Offset of the drawing command register in the SET window.
    const COMMAND: u64 = 0x0000;
    /// Offset of the integer horizontal start coordinate.
    const XSTARTI: u64 = 0x000c;
    /// Offset of the full horizontal start coordinate.
    const XSTART: u64 = 0x0014;
    /// Offset of the saved horizontal continuation coordinate.
    const XSAVE: u64 = 0x002c;
    /// Offset of the integer vertical start coordinate.
    const YSTARTI: u64 = 0x001c;
    /// Offset of the integer horizontal end coordinate.
    const XENDI: u64 = 0x0084;
    /// Offset of the integer vertical end coordinate.
    const YENDI: u64 = 0x0088;
    /// Offset of the red color index register.
    const COLORREDI: u64 = 0x0038;
    /// Offset of the register alias window.
    const XSTATE: u64 = 0x0008;
    /// Offset of the auxiliary configuration register.
    const AUX2: u64 = 0x47a8;
    /// Offset of the clock and revision port.
    const WCLOCK: u64 = 0x47e4;
    /// Offset of the palette data port.
    const RWDAC: u64 = 0x47e8;
    /// Offset of the peripheral selector.
    const CONFIGSEL: u64 = 0x47ec;
    /// Offset of the VC1 data port.
    const RWVC1: u64 = 0x47f0;
    /// Distance from a SET register to the matching GO register.
    const GO: u64 = 0x0800;

    /// Writes one board register.
    fn write(board: &mut Lg1, offset: u64, value: u32) {
        board
            .write(DeviceAddr::new(GIO_PIO_BASE + offset), &value.to_be_bytes())
            .unwrap();
    }

    /// Reads one board register.
    fn read(board: &mut Lg1, offset: u64) -> u32 {
        let mut bytes = [0; 4];
        board
            .read(DeviceAddr::new(GIO_PIO_BASE + offset), &mut bytes)
            .unwrap();
        u32::from_be_bytes(bytes)
    }

    /// Writes one byte to a peripheral through the configuration window.
    fn write_peripheral(board: &mut Lg1, port: u64, selector: u32, value: u32) {
        write(board, CONFIGSEL, selector);
        write(board, port, value);
        write(board, port + GO, value);
    }

    /// Sets the VC1 host address through its two selector ports.
    fn set_vc1_address(board: &mut Lg1, address: u16) {
        write_peripheral(board, RWVC1, 5, u32::from(address >> 8));
        write_peripheral(board, RWVC1, 4, u32::from(address & 0xff));
    }

    /// Brings video timing up the way the PROM does.
    fn start_video(board: &mut Lg1) {
        // Upload the frame table and point the timing generator at it.
        set_vc1_address(board, 0x0800);
        for byte in FRAME_TABLE {
            write_peripheral(board, RWVC1, 2, u32::from(byte));
        }
        set_vc1_address(board, 0x0000);
        write_peripheral(board, RWVC1, 0, 0x88);
        write_peripheral(board, RWVC1, 0, 0x00);
        // Release the timing generator and enable the data path.
        write_peripheral(board, RWVC1, 6, 0x1d);
    }

    /// The video frame table the target PROM uploads.
    const FRAME_TABLE: [u8; 0x2a] = [
        0x03, 0x2d, 0x00, 0x00, 0x01, 0x00, 0x15, 0x02, 0x00, 0x3a, 0x26, 0x00, 0x4e, 0x01, 0x00,
        0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00,
        0x64, 0x78, 0x00, 0x64, 0x2f, 0x02, 0x04, 0x01, 0x00, 0x26, 0x03, 0x00,
    ];

    /// Advances the board past one whole frame.
    fn advance_one_frame(board: &mut Lg1) {
        let period = board.time_until_event().unwrap();
        board.advance_time(period);
        let period = board.time_until_event().unwrap();
        board.advance_time(period);
    }

    #[test]
    fn the_prom_probe_reads_back_the_converted_coordinate() {
        let mut board = Lg1::new();

        write(&mut board, XSTARTI, 0x1234_5678);
        write(&mut board, COMMAND, 0);
        read(&mut board, COMMAND + GO);

        assert_eq!(read(&mut board, XSTART), 0x033c_0000);
    }

    #[test]
    fn a_new_board_reports_no_signal() {
        let board = Lg1::new();

        assert_eq!(board.display_state(), Some(GioDisplayState::NoSignal));
        assert!(!board.interrupt_asserted(GioInterrupt::Interrupt2));
        assert_eq!(board.time_until_event(), None);
    }

    #[test]
    fn the_board_identity_is_read_through_the_clock_port() {
        let mut board = Lg1::new();
        write(&mut board, CONFIGSEL, 4);

        // The GO access starts the transfer and the SET access collects it.
        read(&mut board, WCLOCK + GO);

        assert_eq!(read(&mut board, WCLOCK) & 0x07, 1);
    }

    #[test]
    fn the_prom_power_on_patterns_survive_drawing_go_commands() {
        let mut board = Lg1::new();

        for pattern in [0x5555_5555, 0xaaaa_aaaa] {
            for port in [RWVC1, RWDAC, WCLOCK] {
                write(&mut board, port, pattern);
                for _ in 0..3 {
                    write(&mut board, COMMAND + GO, 0);
                }
                assert_eq!(read(&mut board, port), pattern & 0xff);
            }
        }
    }

    #[test]
    fn configuration_reads_return_the_byte_the_previous_access_started() {
        let mut board = Lg1::new();
        // Load two palette entries and read them back through the protocol.
        write_peripheral(&mut board, RWDAC, 0, 0);
        for component in [1, 2, 3, 4, 5, 6] {
            write_peripheral(&mut board, RWDAC, 1, component);
        }
        write_peripheral(&mut board, RWDAC, 3, 0);

        write(&mut board, CONFIGSEL, 1);
        // The first GO read is discarded, then each access returns one byte.
        read(&mut board, RWDAC + GO);

        assert_eq!(read(&mut board, RWDAC + GO) & 0xff, 1);
        assert_eq!(read(&mut board, RWDAC + GO) & 0xff, 2);
        assert_eq!(read(&mut board, RWDAC + GO) & 0xff, 3);
    }

    #[test]
    fn enabling_video_timing_produces_a_frame() {
        let mut board = Lg1::new();

        start_video(&mut board);
        assert_eq!(board.display_state(), Some(GioDisplayState::Blank));

        advance_one_frame(&mut board);

        let Some(GioDisplayState::Active {
            width,
            height,
            pixels,
        }) = board.display_state()
        else {
            panic!("valid timing must produce a frame");
        };
        assert_eq!((width, height), (1024, 768));
        assert_eq!(pixels.len(), 1024 * 768 * 4);
    }

    #[test]
    fn drawn_pixels_reach_the_composed_frame() {
        let mut board = Lg1::new();
        start_video(&mut board);
        // Give palette entry one a distinct color.
        write_peripheral(&mut board, RWDAC, 0, 1);
        for component in [0x11, 0x22, 0x33] {
            write_peripheral(&mut board, RWDAC, 1, component);
        }

        write(&mut board, AUX2, 0x2000_0000);
        // XSTATE bits 7:0 are the foreground color, so the write mask has to
        // be established before the color rather than after it.
        write(&mut board, XSTATE, 0x03ff_0000);
        write(&mut board, COLORREDI, 1);
        write(&mut board, XSTARTI, 3);
        write(&mut board, YSTARTI, 4);
        write(&mut board, COMMAND, 0x3000_0001);
        read(&mut board, COMMAND + GO);
        advance_one_frame(&mut board);

        let Some(GioDisplayState::Active { pixels, .. }) = board.display_state() else {
            panic!("valid timing must produce a frame");
        };
        let offset = (4 * 1024 + 3) * 4;
        assert_eq!(&pixels[offset..offset + 4], [0x11, 0x22, 0x33, 0xff]);
    }

    #[test]
    fn the_prom_clear_screen_reaches_the_whole_visible_frame() {
        let mut board = Lg1::new();
        start_video(&mut board);
        write_peripheral(&mut board, RWDAC, 0, 0);
        for component in [0x40, 0x50, 0x60] {
            write_peripheral(&mut board, RWDAC, 1, component);
        }

        write(&mut board, AUX2, 0x2000_0000);
        write(&mut board, COMMAND, 0x329);
        write(&mut board, XSTATE, 0x03ff_0000);
        write(&mut board, XSTARTI, 0);
        write(&mut board, YSTARTI, 0);
        write(&mut board, XENDI, 1023);
        write(&mut board, YENDI + GO, 767);
        advance_one_frame(&mut board);

        let Some(GioDisplayState::Active { pixels, .. }) = board.display_state() else {
            panic!("valid timing must produce a frame");
        };
        assert!(
            pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [0x40, 0x50, 0x60, 0xff])
        );
    }

    #[test]
    fn retrace_follows_the_scan_position_within_a_frame() {
        let mut board = Lg1::new();
        start_video(&mut board);

        assert!(!board.interrupt_asserted(GioInterrupt::Interrupt2));
        board.advance_time(board.time_until_event().unwrap());
        assert!(board.interrupt_asserted(GioInterrupt::Interrupt2));
        board.advance_time(board.time_until_event().unwrap());
        assert!(!board.interrupt_asserted(GioInterrupt::Interrupt2));
    }

    #[test]
    fn losing_video_timing_withdraws_the_frame() {
        let mut board = Lg1::new();
        start_video(&mut board);
        advance_one_frame(&mut board);
        assert!(matches!(
            board.display_state(),
            Some(GioDisplayState::Active { .. })
        ));

        // Hold the timing generator in reset again.
        write_peripheral(&mut board, RWVC1, 6, 0x02);

        assert_eq!(board.display_state(), Some(GioDisplayState::NoSignal));
        assert!(!board.interrupt_asserted(GioInterrupt::Interrupt2));
    }

    #[test]
    fn blanking_the_data_path_keeps_timing_and_shows_black() {
        let mut board = Lg1::new();
        start_video(&mut board);
        advance_one_frame(&mut board);

        // Clear only the data path enable, leaving the generator running.
        write_peripheral(&mut board, RWVC1, 6, 0x19);

        assert_eq!(board.display_state(), Some(GioDisplayState::Blank));
        assert!(board.time_until_event().is_some());
    }

    #[test]
    fn a_shared_frame_survives_the_next_composition() {
        let mut board = Lg1::new();
        start_video(&mut board);
        advance_one_frame(&mut board);
        let Some(GioDisplayState::Active { pixels, .. }) = board.display_state() else {
            panic!("valid timing must produce a frame");
        };

        advance_one_frame(&mut board);

        assert_eq!(pixels.len(), 1024 * 768 * 4);
    }

    #[test]
    fn debug_reads_leave_drawing_and_peripheral_state_alone() {
        let mut board = Lg1::new();
        write(&mut board, AUX2, 0x2000_0000);
        write(&mut board, COLORREDI, 0x5a);
        write(&mut board, XSTARTI, 2);
        write(&mut board, YSTARTI, 2);
        write(&mut board, COMMAND, 0x3000_0001);
        write(&mut board, XSTATE, 0x03ff_0000);
        let before = board.clone();

        let mut bytes = [0; 4];
        board
            .debug_read(DeviceAddr::new(GIO_PIO_BASE + COMMAND + GO), &mut bytes)
            .unwrap();
        board
            .debug_read(DeviceAddr::new(GIO_PIO_BASE + RWDAC + GO), &mut bytes)
            .unwrap();

        assert_eq!(board.vram.read(super::vram::PlaneGroup::Pixel, 2, 2), 0);
        assert_eq!(read(&mut board, RWDAC), before.rex.read_config(RWDAC).0);
    }

    #[test]
    fn narrow_and_unaligned_accesses_are_rejected_without_side_effects() {
        let mut board = Lg1::new();

        assert_eq!(
            board.write(DeviceAddr::new(GIO_PIO_BASE + COMMAND), &[0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            board.read(DeviceAddr::new(GIO_PIO_BASE + COMMAND + 1), &mut [0; 4]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            board.read(DeviceAddr::new(GIO_PIO_BASE + COMMAND), &mut []),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            board.read(DeviceAddr::new(GIO_PIO_BASE + 0x2000), &mut [0; 4]),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn addresses_outside_the_pio_aperture_behave_as_an_empty_slot() {
        let mut below = [0xff; 4];
        let mut above = [0xff; 4];
        let mut debug = [0xff; 4];
        let mut board = Lg1::new();

        board
            .read(DeviceAddr::new(GIO_PIO_BASE - 4), &mut below)
            .unwrap();
        board
            .read(DeviceAddr::new(GIO_PIO_END), &mut above)
            .unwrap();
        board
            .debug_read(DeviceAddr::new(GIO_PIO_END), &mut debug)
            .unwrap();
        board
            .write(DeviceAddr::new(GIO_PIO_END), &[0x12, 0x34, 0x56, 0x78])
            .unwrap();

        assert_eq!(below, [0; 4]);
        assert_eq!(above, [0; 4]);
        assert_eq!(debug, [0; 4]);
        assert_eq!(read(&mut board, COMMAND), 0);
        assert_eq!(
            board.read(DeviceAddr::new(GIO_PIO_BASE - 1), &mut [0; 2]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            board.write(DeviceAddr::new(GIO_PIO_END - 1), &[0; 2]),
            Err(BusError::HardwareFault)
        );
    }

    #[test]
    fn reset_returns_the_board_to_its_power_on_state() {
        let mut board = Lg1::new();
        start_video(&mut board);
        advance_one_frame(&mut board);

        board.reset();

        assert_eq!(board.display_state(), Some(GioDisplayState::NoSignal));
        assert_eq!(board.time_until_event(), None);
        assert_eq!(read(&mut board, XSTARTI), 0);
    }

    #[test]
    fn board_state_survives_a_snapshot_round_trip() {
        let mut board = Lg1::new();
        start_video(&mut board);
        advance_one_frame(&mut board);

        let encoded = bincode::serde::encode_to_vec(&board, bincode::config::standard()).unwrap();
        let (mut restored, consumed): (Lg1, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();

        assert_eq!(consumed, encoded.len());
        assert_eq!(restored.display_state(), board.display_state());
        assert_eq!(restored.time_until_event(), board.time_until_event());
        assert!(restored.take_display_update());
        assert!(!restored.take_display_update());
    }

    #[test]
    fn graphics_dma_streams_complete_big_endian_host_words() {
        let mut board = Lg1::new();
        write(&mut board, XSTARTI, 8);
        write(&mut board, YSTARTI, 3);
        write(&mut board, XENDI, 1023);
        write(&mut board, AUX2, 0x2000_0000);
        write(&mut board, COMMAND, 0x0020_01a1);
        write(&mut board, XSTATE, 0x13ff_0000);

        assert!(board.dma_sync_asserted());
        assert_eq!(
            board.write_dma(
                DeviceAddr::new(GRAPHICS_DMA_PORT),
                &[1, 2, 3, 4, 5, 6, 7, 8]
            ),
            Ok(())
        );
        for (x, value) in (8..16).zip(1..=8) {
            assert_eq!(board.vram.read(PlaneGroup::Pixel, x, 3), value);
        }
        assert_eq!(board.vram.read(PlaneGroup::Pixel, 16, 3), 0);

        write(&mut board, XSAVE, 8);
        write(&mut board, COMMAND, 0x0020_00ab);
        let mut bytes = [0; 8];
        assert_eq!(
            board.read_dma(DeviceAddr::new(GRAPHICS_DMA_PORT), &mut bytes),
            Ok(())
        );
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);

        assert_eq!(
            board.write_dma(DeviceAddr::new(GRAPHICS_DMA_PORT), &[0; 6]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            board.read_dma(DeviceAddr::new(GRAPHICS_DMA_PORT), &mut [0; 6]),
            Err(BusError::InvalidTransaction)
        );
    }

    #[test]
    fn graphics_dma_discards_unused_lanes_at_rectangle_scanline_ends() {
        let mut board = Lg1::new();
        write(&mut board, XSTARTI, 0x10);
        write(&mut board, YSTARTI, 0);
        write(&mut board, XENDI, 0x36);
        write(&mut board, YENDI, 1);
        write(&mut board, AUX2, 0x2000_0000);
        write(&mut board, COMMAND, 0x3020_00a9);
        write(&mut board, XSTATE, 0x13ff_0000);
        let pixels = (0..39)
            .map(|value| value as u8)
            .chain([0xee])
            .chain((39..78).map(|value| value as u8))
            .chain([0xff])
            .collect::<Vec<_>>();

        assert_eq!(
            board.write_dma(DeviceAddr::new(GRAPHICS_DMA_PORT), &pixels),
            Ok(())
        );
        for (index, value) in pixels[..39]
            .iter()
            .chain(&pixels[40..79])
            .copied()
            .enumerate()
        {
            let x = 0x10 + index as u32 % 39;
            let y = index as u32 / 39;
            assert_eq!(board.vram.read(PlaneGroup::Pixel, x, y), value);
        }
        assert_eq!(board.vram.read(PlaneGroup::Pixel, 0x10, 2), 0);

        write(&mut board, XSTARTI, 0x10);
        write(&mut board, YSTARTI, 0);
        write(&mut board, COMMAND, 0x3000_00ab);
        let mut readback = vec![0; pixels.len()];
        assert_eq!(
            board.read_dma(DeviceAddr::new(GRAPHICS_DMA_PORT), &mut readback),
            Ok(())
        );
        assert_eq!(readback[..39], pixels[..39]);
        assert_eq!(readback[40..79], pixels[40..79]);
    }

    #[test]
    fn graphics_dma_rejects_non_host_data_ports_without_side_effects() {
        let mut board = Lg1::new();
        let before = bincode::serde::encode_to_vec(&board, bincode::config::standard()).unwrap();

        assert_eq!(
            board.write_dma(DeviceAddr::new(GRAPHICS_DMA_PORT - 4), &[1, 2, 3, 4]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            board.read_dma(DeviceAddr::new(GRAPHICS_DMA_PORT + 4), &mut [0; 4]),
            Err(BusError::UnimplementedAccess)
        );
        let after = bincode::serde::encode_to_vec(&board, bincode::config::standard()).unwrap();
        assert_eq!(after, before);
    }
}

//! VC1 video timing, display identifier, XMAP, and cursor controller.
//!
//! The host reaches VC1 through the REX configuration bus. A selector chooses
//! one internal function, and a sixteen-bit host address walks byte by byte
//! through the selected bank. Sixteen-bit registers occupy two consecutive
//! byte addresses with the high byte first, so byte addressing alone produces
//! the documented transfer order.

use se_core::time::VirtualDuration;
use serde::{Deserialize, Serialize};

/// Bytes of external timing SRAM addressable by the host.
const SRAM_BYTES: usize = 0x1_0000;

/// Bytes in the control register bank.
const CONTROL_BYTES: usize = 0x100;

/// Bytes in the XMAP mode table, holding thirty-two even and odd entries.
const XMAP_BYTES: usize = 0x80;

/// Bytes in the test register bank.
const TEST_BYTES: usize = 0x08;

/// Control bank address of the video timing generator entry pointer.
const VID_EP: u16 = 0x00;
/// Control bank address of the line counter.
const VID_LC: u16 = 0x02;
/// Control bank address of the frame counter.
const VID_FC: u16 = 0x10;

/// Control bank address of the display-identifier frame-table pointer.
const DID_EP: u16 = 0x40;

/// Test bank address of the chip revision register.
const CHIP_REVISION: u16 = 0x05;

/// Raw chip revision encoding presented to the guest.
///
/// Software subtracts one from the low three bits, so this encoding reports
/// revision one. The IP12 kernel rejects software revision zero.
const CHIP_REVISION_VALUE: u8 = 0x02;

/// System control bit that holds the timing generator in reset, active low.
const SYS_CTRL_VTG: u8 = 1 << 1;

/// System control bit that enables the VC1 data path, active high.
const SYS_CTRL_VC1: u8 = 1 << 2;

/// System control bit that enables display-identifier generation.
const SYS_CTRL_DID: u8 = 1 << 3;

/// System control bit that displays the hardware cursor, active high.
pub(super) const SYS_CTRL_CURSOR_DISPLAY: u8 = 1 << 5;

/// Control bank address of the cursor bitmap entry pointer.
const CUR_EP: u16 = 0x20;
/// Control bank address of the cursor horizontal position.
const CUR_XL: u16 = 0x22;
/// Control bank address of the cursor vertical position.
const CUR_YL: u16 = 0x24;
/// Control bank address of the cursor palette mode.
const CUR_MODE: u16 = 0x26;
/// Control bank address of the cursor line length.
const CUR_LY: u16 = 0x28;
/// Control bank address of the display-ID horizontal modulus.
const DID_HOR_MOD: u16 = 0x45;

/// Horizontal offset between the cursor register and the visible origin.
const CURSOR_X_OFFSET: i32 = 140;

/// Vertical offset between the cursor register and the visible origin.
const CURSOR_Y_OFFSET: i32 = 39;

/// Bytes of bitmap data describing one cursor.
///
/// The cursor covers thirty-two rows of thirty-two pixels. Two consecutive
/// one-bit planes select foreground and background, so each plane occupies
/// one hundred twenty-eight bytes.
pub(super) const CURSOR_BITMAP_BYTES: usize = 256;

/// System control value presented before software programs the generator.
///
/// The timing generator reset bit is set, so a board that no guest has
/// initialized produces no video signal. The target PROM later writes `0x19`,
/// which clears this bit and starts timing with the data path still disabled,
/// and enables the data path separately when it opens the graphics terminal.
const SYS_CTRL_RESET: u8 = SYS_CTRL_VTG;

/// Mask applied to the entry pointer to obtain a table address.
///
/// Bit fifteen is a flag documented by the SGI initialization source as a
/// VC1A correction rather than part of the address.
const ENTRY_POINTER_ADDRESS: u16 = 0x7fff;

/// Displayed width in pixels.
pub(super) const DISPLAY_WIDTH: u32 = 1024;

/// Displayed height in scan lines.
///
/// The visible window is a board property rather than a value derived from
/// the timing table: identifying visible lines inside a line program requires
/// the run encoding, which remains undecoded. The frame table still supplies
/// the total line count, so the blanking fraction follows the guest's table.
pub(super) const DISPLAY_HEIGHT: u32 = 768;

/// Total lines assumed before the guest uploads a usable frame table.
const DEFAULT_TOTAL_LINES: u32 = DISPLAY_HEIGHT;

/// Provisional duration of one scan line.
///
/// The line period depends on the line program run encoding and the clock
/// generator byte stream, neither of which is decoded. This constant places
/// an 858-line frame near a sixty hertz refresh so guest timing loops make
/// progress; it is not a measured hardware value.
const LINE_PERIOD_ATTOSECONDS: u128 = 19_425_019_425_019;

/// Maximum frame table entries walked before decoding gives up.
const MAX_FRAME_TABLE_ENTRIES: usize = 256;

/// One VC1 function selected by the REX configuration bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Selector {
    /// Timing, cursor, display identifier, and XMAP control registers.
    Control,
    /// The XMAP display mode table.
    XmapMode,
    /// External timing SRAM.
    Sram,
    /// Test registers, including the chip revision.
    Test,
    /// Low byte of the host address.
    AddressLow,
    /// High byte of the host address.
    AddressHigh,
    /// System control.
    SystemControl,
}

impl Selector {
    /// Decodes the three selector bits driven by the configuration bus.
    pub(super) const fn from_bits(bits: u8) -> Self {
        match bits & 0x07 {
            0 => Self::Control,
            1 => Self::XmapMode,
            2 => Self::Sram,
            3 => Self::Test,
            4 => Self::AddressLow,
            5 => Self::AddressHigh,
            _ => Self::SystemControl,
        }
    }
}

/// The video signal state produced by the current control register values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SignalState {
    /// The timing generator is held in reset and produces no video timing.
    NoSignal,
    /// Video timing is valid but the data path is disabled.
    Blanked,
    /// Video timing is valid and pixels reach the display.
    Active,
}

/// The hardware cursor as the display path sees it.
pub(super) struct Cursor {
    /// Signed screen column of the leftmost cursor pixel.
    pub(super) left: i32,
    /// Signed screen row of the topmost cursor pixel.
    pub(super) top: i32,
    /// Base of the sixteen-entry palette submap selected by CUR_MODE.
    pub(super) palette_base: u16,
    /// Foreground and background bit planes, each packed most significant
    /// pixel first and stored as one thirty-two-bit word per row.
    pub(super) bitmap: [u8; CURSOR_BITMAP_BYTES],
}

/// The VC1 controller state.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Vc1 {
    sram: Box<[u8]>,
    control: Box<[u8]>,
    xmap: Box<[u8]>,
    test: Box<[u8]>,
    system_control: u8,
    address: u16,
    /// Attoseconds elapsed inside the current frame.
    frame_elapsed: u128,
    /// Free-running frame counter presented through `VID_FC`.
    frame_counter: u16,
    /// Total lines per frame decoded from frame-table count bytes.
    ///
    /// The intervening line-program addresses remain opaque and do not
    /// determine the fixed visible geometry in this model.
    total_lines: u32,
}

impl Vc1 {
    /// Creates a VC1 with cleared tables and the timing generator in reset.
    pub(super) fn new() -> Self {
        let mut test = vec![0; TEST_BYTES].into_boxed_slice();
        test[usize::from(CHIP_REVISION)] = CHIP_REVISION_VALUE;
        Self {
            sram: vec![0; SRAM_BYTES].into_boxed_slice(),
            control: vec![0; CONTROL_BYTES].into_boxed_slice(),
            xmap: vec![0; XMAP_BYTES].into_boxed_slice(),
            test,
            system_control: SYS_CTRL_RESET,
            address: 0,
            frame_elapsed: 0,
            frame_counter: 0,
            total_lines: DEFAULT_TOTAL_LINES,
        }
    }

    /// Restores the VC1 reset state.
    pub(super) fn reset(&mut self) {
        self.sram.fill(0);
        self.control.fill(0);
        self.xmap.fill(0);
        self.test.fill(0);
        self.test[usize::from(CHIP_REVISION)] = CHIP_REVISION_VALUE;
        self.system_control = SYS_CTRL_RESET;
        self.address = 0;
        self.frame_elapsed = 0;
        self.frame_counter = 0;
        self.total_lines = DEFAULT_TOTAL_LINES;
    }

    /// Returns the video signal state implied by the control registers.
    ///
    /// The timing generator reset bit is active low, so a set bit stops video
    /// timing entirely. With timing present, a disabled data path blanks the
    /// picture without removing the signal.
    pub(super) const fn signal_state(&self) -> SignalState {
        if self.system_control & SYS_CTRL_VTG != 0 {
            SignalState::NoSignal
        } else if self.system_control & SYS_CTRL_VC1 == 0 {
            SignalState::Blanked
        } else {
            SignalState::Active
        }
    }

    /// Returns the sixteen-bit XMAP mode selected by one display identifier.
    pub(super) fn display_mode(&self, identifier: u8) -> u16 {
        let index = usize::from(identifier & 0x1f) * 2;
        u16::from_be_bytes([self.xmap[index], self.xmap[index + 1]])
    }

    /// Fills one displayed scan line with the generated display identifiers.
    ///
    /// Each frame-table word points to a line table. The first line-table
    /// word gives the number of following entries, whose upper eleven bits
    /// are a starting X coordinate and whose lower five bits select an XMAP
    /// mode. Entries are ordered from left to right.
    pub(super) fn fill_display_identifiers(&self, y: u32, identifiers: &mut [u8]) {
        assert_eq!(
            identifiers.len(),
            DISPLAY_WIDTH as usize,
            "display identifier row must match the displayed width"
        );
        identifiers.fill(0);
        if self.system_control & SYS_CTRL_DID == 0 || y >= DISPLAY_HEIGHT {
            return;
        }

        let frame_table = usize::from(self.control_word(DID_EP) & ENTRY_POINTER_ADDRESS);
        let frame_entry = frame_table + y as usize * 2;
        let Some(line_table) = self.sram_word(frame_entry).map(usize::from) else {
            return;
        };
        let Some(entry_count) = self.sram_word(line_table).map(usize::from) else {
            return;
        };

        let mut start = 0;
        let mut identifier = 0;
        let mut has_boundary = false;
        for entry in 0..entry_count.min(DISPLAY_WIDTH as usize) {
            let Some(encoded) = self.sram_word(line_table + 2 + entry * 2) else {
                break;
            };
            let boundary = usize::from(encoded >> 5).min(identifiers.len());
            if has_boundary && boundary <= start {
                continue;
            }
            identifiers[start..boundary].fill(identifier);
            start = boundary;
            identifier = (encoded & 0x1f) as u8;
            has_boundary = true;
        }
        identifiers[start..].fill(identifier);
    }

    /// Returns the hardware cursor when the guest has enabled its display.
    ///
    /// The cursor position registers are offset from the visible origin by
    /// the amounts the SGI headers document, so a cursor parked at the
    /// register origin sits at the top left of the screen. The bitmap lives
    /// in timing SRAM at the address held by the entry pointer.
    pub(super) fn cursor(&self) -> Option<Cursor> {
        if self.system_control & SYS_CTRL_CURSOR_DISPLAY == 0 {
            return None;
        }

        let pointer = usize::from(self.control_word(CUR_EP));
        let mut bitmap = [0; CURSOR_BITMAP_BYTES];
        for (offset, byte) in bitmap.iter_mut().enumerate() {
            *byte = self.sram[(pointer + offset) % SRAM_BYTES];
        }

        Some(Cursor {
            left: i32::from(self.control_word(CUR_XL)) - CURSOR_X_OFFSET,
            top: i32::from(self.control_word(CUR_YL)) - CURSOR_Y_OFFSET,
            palette_base: (self.control_word(CUR_MODE) >> 6) & 0x03f0,
            bitmap,
        })
    }

    /// Returns one sixteen-bit control register, high byte first.
    fn control_word(&self, address: u16) -> u16 {
        let index = usize::from(address) % CONTROL_BYTES;
        u16::from_be_bytes([
            self.control[index],
            self.control[(index + 1) % CONTROL_BYTES],
        ])
    }

    /// Returns one big-endian word from timing SRAM.
    fn sram_word(&self, address: usize) -> Option<u16> {
        Some(u16::from_be_bytes([
            *self.sram.get(address)?,
            *self.sram.get(address.checked_add(1)?)?,
        ]))
    }

    /// Reports whether the current scan position lies in vertical blanking.
    pub(super) fn in_vertical_blanking(&self) -> bool {
        self.current_line() >= DISPLAY_HEIGHT
    }

    /// Advances virtual time and returns the number of completed frames.
    pub(super) fn advance_time(&mut self, elapsed: VirtualDuration) -> u32 {
        if matches!(self.signal_state(), SignalState::NoSignal) {
            return 0;
        }

        self.frame_elapsed += elapsed.as_attoseconds();
        let period = self.frame_period_attoseconds();
        let completed = u32::try_from(self.frame_elapsed / period).unwrap_or(u32::MAX);
        self.frame_elapsed %= period;
        self.frame_counter = self.frame_counter.wrapping_add(completed as u16);
        completed
    }

    /// Returns the duration until the next software-visible timing boundary.
    ///
    /// Both the start of vertical blanking and the end of the frame change
    /// state the guest can observe, so each is scheduled.
    pub(super) fn time_until_event(&self) -> Option<VirtualDuration> {
        if matches!(self.signal_state(), SignalState::NoSignal) {
            return None;
        }

        let blanking_start =
            u128::from(DISPLAY_HEIGHT.min(self.total_lines)) * LINE_PERIOD_ATTOSECONDS;
        let frame_end = self.frame_period_attoseconds();
        let next = if self.frame_elapsed < blanking_start {
            blanking_start
        } else {
            frame_end
        };
        Some(VirtualDuration::from_attoseconds(
            next.saturating_sub(self.frame_elapsed),
        ))
    }

    /// Writes one byte to the selected VC1 function.
    pub(super) fn write(&mut self, selector: Selector, value: u8) {
        match selector {
            Selector::AddressLow => {
                self.address = (self.address & 0xff00) | u16::from(value);
            }
            Selector::AddressHigh => {
                self.address = (self.address & 0x00ff) | (u16::from(value) << 8);
            }
            Selector::SystemControl => self.system_control = value,
            Selector::Control => {
                let index = usize::from(self.address) % CONTROL_BYTES;
                let address = self.address % CONTROL_BYTES as u16;
                self.control[index] = control_byte_value(address, value);
                self.advance_address();
                self.reload_frame_total_lines();
            }
            Selector::XmapMode => {
                let index = usize::from(self.address) % XMAP_BYTES;
                self.xmap[index] = value;
                self.advance_address();
            }
            Selector::Sram => {
                let index = usize::from(self.address) % SRAM_BYTES;
                self.sram[index] = value;
                self.advance_address();
                self.reload_frame_total_lines();
            }
            Selector::Test => {
                let index = usize::from(self.address) % TEST_BYTES;
                self.test[index] = value;
                self.advance_address();
            }
        }
    }

    /// Reads one byte from the selected VC1 function.
    pub(super) fn read(&mut self, selector: Selector) -> u8 {
        let value = self.peek(selector);
        if !matches!(
            selector,
            Selector::AddressLow | Selector::AddressHigh | Selector::SystemControl
        ) {
            self.advance_address();
        }
        value
    }

    /// Returns the byte currently presented by the selected function.
    fn peek(&self, selector: Selector) -> u8 {
        match selector {
            Selector::AddressLow => self.address as u8,
            Selector::AddressHigh => (self.address >> 8) as u8,
            Selector::SystemControl => self.system_control,
            Selector::Control => self.read_control(),
            Selector::XmapMode => self.xmap[usize::from(self.address) % XMAP_BYTES],
            Selector::Sram => self.sram[usize::from(self.address) % SRAM_BYTES],
            Selector::Test => self.test[usize::from(self.address) % TEST_BYTES],
        }
    }

    /// Returns one control bank byte, substituting the live counters.
    ///
    /// The line and frame counters are derived from virtual time rather than
    /// stored, because SGI diagnostics poll `VID_LC` until it enters a range.
    fn read_control(&self) -> u8 {
        const VID_LC_LOW: u16 = VID_LC + 1;
        const VID_FC_LOW: u16 = VID_FC + 1;

        let address = self.address % CONTROL_BYTES as u16;
        let line = self.current_line() as u16;
        match address {
            VID_LC => (line >> 8) as u8,
            VID_LC_LOW => line as u8,
            VID_FC => (self.frame_counter >> 8) as u8,
            VID_FC_LOW => self.frame_counter as u8,
            _ => self.control[usize::from(address)],
        }
    }

    /// Advances the host address by one byte.
    fn advance_address(&mut self) {
        self.address = self.address.wrapping_add(1);
    }

    /// Returns the current line position inside the frame.
    fn current_line(&self) -> u32 {
        u32::try_from(self.frame_elapsed / LINE_PERIOD_ATTOSECONDS)
            .unwrap_or(u32::MAX)
            .min(self.total_lines.saturating_sub(1))
    }

    /// Returns the duration of one complete frame in attoseconds.
    const fn frame_period_attoseconds(&self) -> u128 {
        self.total_lines as u128 * LINE_PERIOD_ATTOSECONDS
    }

    /// Recomputes only the total frame line count from the uploaded table.
    fn reload_frame_total_lines(&mut self) {
        let entry_pointer = u16::from_be_bytes([
            self.control[usize::from(VID_EP)],
            self.control[usize::from(VID_EP) + 1],
        ]);
        let table = entry_pointer & ENTRY_POINTER_ADDRESS;
        if let Some(lines) = decode_frame_total_lines(&self.sram, table) {
            self.total_lines = lines;
            let period = lines as u128 * LINE_PERIOD_ATTOSECONDS;
            self.frame_elapsed %= period;
        }
    }
}

/// Applies the implemented width of a byte-addressed control register.
const fn control_byte_value(address: u16, value: u8) -> u8 {
    match address {
        CUR_XL | CUR_LY => value & 0x07,
        CUR_YL => value & 0x0f,
        DID_HOR_MOD => value & 0x07,
        _ => value,
    }
}

/// Decodes the total line count from a video frame table.
///
/// The table holds a leading byte followed by entries containing one line
/// count and two bytes whose line-program meaning is not decoded here. Only
/// count bytes contribute to the result; the two following bytes are skipped.
/// A zero line count terminates a valid nonempty table.
fn decode_frame_total_lines(sram: &[u8], table: u16) -> Option<u32> {
    let mut offset = usize::from(table).checked_add(1)?;
    let mut lines = 0_u32;
    for _ in 0..MAX_FRAME_TABLE_ENTRIES {
        let count = *sram.get(offset)?;
        if count == 0 {
            return (lines > 0).then_some(lines);
        }
        lines = lines.checked_add(u32::from(count))?;
        offset = offset.checked_add(3)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use se_core::time::VirtualDuration;

    use super::{
        CHIP_REVISION, CUR_LY, CUR_XL, CUR_YL, DID_EP, DID_HOR_MOD, DISPLAY_HEIGHT, DISPLAY_WIDTH,
        LINE_PERIOD_ATTOSECONDS, SYS_CTRL_DID, SYS_CTRL_VC1, SYS_CTRL_VTG, Selector, SignalState,
        VID_EP, Vc1, decode_frame_total_lines,
    };

    /// The video frame table uploaded by the target PROM.
    const PROM_FRAME_TABLE: [u8; 0x2a] = [
        0x03, 0x2d, 0x00, 0x00, 0x01, 0x00, 0x15, 0x02, 0x00, 0x3a, 0x26, 0x00, 0x4e, 0x01, 0x00,
        0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00, 0x64, 0x78, 0x00,
        0x64, 0x78, 0x00, 0x64, 0x2f, 0x02, 0x04, 0x01, 0x00, 0x26, 0x03, 0x00,
    ];

    /// Sets the host address through the two address selectors.
    fn set_address(vc1: &mut Vc1, address: u16) {
        vc1.write(Selector::AddressHigh, (address >> 8) as u8);
        vc1.write(Selector::AddressLow, address as u8);
    }

    /// Uploads bytes into external SRAM at one address.
    fn upload(vc1: &mut Vc1, address: u16, bytes: &[u8]) {
        set_address(vc1, address);
        for &byte in bytes {
            vc1.write(Selector::Sram, byte);
        }
    }

    /// Writes one sixteen-bit control register, high byte first.
    fn write_control_word(vc1: &mut Vc1, address: u16, value: u16) {
        set_address(vc1, address);
        vc1.write(Selector::Control, (value >> 8) as u8);
        vc1.write(Selector::Control, value as u8);
    }

    /// Reads one sixteen-bit control register, high byte first.
    fn read_control_word(vc1: &mut Vc1, address: u16) -> u16 {
        set_address(vc1, address);
        u16::from_be_bytes([vc1.read(Selector::Control), vc1.read(Selector::Control)])
    }

    /// Brings the timing generator out of reset with the data path enabled.
    fn enable_video(vc1: &mut Vc1) {
        vc1.write(Selector::SystemControl, SYS_CTRL_VC1);
    }

    /// Uploads the PROM frame table and points the generator at it.
    fn configure_prom_timing(vc1: &mut Vc1) {
        upload(vc1, 0x0800, &PROM_FRAME_TABLE);
        write_control_word(vc1, VID_EP, 0x8800);
        enable_video(vc1);
    }

    #[test]
    fn selector_bits_match_the_documented_functions() {
        assert_eq!(Selector::from_bits(0), Selector::Control);
        assert_eq!(Selector::from_bits(1), Selector::XmapMode);
        assert_eq!(Selector::from_bits(2), Selector::Sram);
        assert_eq!(Selector::from_bits(3), Selector::Test);
        assert_eq!(Selector::from_bits(4), Selector::AddressLow);
        assert_eq!(Selector::from_bits(5), Selector::AddressHigh);
        assert_eq!(Selector::from_bits(6), Selector::SystemControl);
    }

    #[test]
    fn the_prom_frame_table_yields_858_total_lines() {
        let mut sram = vec![0; 0x1000];
        sram[0x0800..0x0800 + PROM_FRAME_TABLE.len()].copy_from_slice(&PROM_FRAME_TABLE);

        assert_eq!(decode_frame_total_lines(&sram, 0x0800), Some(858));
    }

    #[test]
    fn line_program_address_bytes_do_not_affect_the_total() {
        let first = [0x03, 2, 0x12, 0x34, 3, 0x56, 0x78, 0];
        let second = [0x03, 2, 0xab, 0xcd, 3, 0xef, 0x01, 0];

        assert_eq!(decode_frame_total_lines(&first, 0), Some(5));
        assert_eq!(decode_frame_total_lines(&second, 0), Some(5));
    }

    #[test]
    fn a_table_without_a_terminator_is_rejected() {
        let mut sram = vec![0; 1 + super::MAX_FRAME_TABLE_ENTRIES * 3];
        for entry in 0..super::MAX_FRAME_TABLE_ENTRIES {
            sram[1 + entry * 3] = 1;
        }

        assert_eq!(decode_frame_total_lines(&sram, 0), None);
    }

    #[test]
    fn an_invalid_table_preserves_the_previous_total() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);

        write_control_word(&mut vc1, VID_EP, 0x1000);

        assert_eq!(vc1.total_lines, 858);
    }

    #[test]
    fn the_entry_pointer_flag_bit_is_not_part_of_the_address() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);

        // VID_EP holds 0x8800 while the table sits at 0x0800.
        assert_eq!(vc1.total_lines, 858);
    }

    #[test]
    fn an_uninitialized_generator_produces_no_signal() {
        let vc1 = Vc1::new();

        assert_eq!(vc1.signal_state(), SignalState::NoSignal);
    }

    #[test]
    fn system_control_selects_the_three_signal_states() {
        let mut vc1 = Vc1::new();

        vc1.write(Selector::SystemControl, SYS_CTRL_VTG);
        assert_eq!(vc1.signal_state(), SignalState::NoSignal);

        vc1.write(Selector::SystemControl, 0);
        assert_eq!(vc1.signal_state(), SignalState::Blanked);

        vc1.write(Selector::SystemControl, SYS_CTRL_VC1);
        assert_eq!(vc1.signal_state(), SignalState::Active);
    }

    #[test]
    fn the_line_counter_advances_with_virtual_time() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);

        assert_eq!(read_control_word(&mut vc1, super::VID_LC), 0);
        vc1.advance_time(VirtualDuration::from_attoseconds(
            10 * LINE_PERIOD_ATTOSECONDS,
        ));
        assert_eq!(read_control_word(&mut vc1, super::VID_LC), 10);
    }

    #[test]
    fn the_line_counter_returns_to_the_top_of_the_next_frame() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);

        vc1.advance_time(VirtualDuration::from_attoseconds(
            857 * LINE_PERIOD_ATTOSECONDS,
        ));
        assert_eq!(read_control_word(&mut vc1, super::VID_LC), 857);

        let frames = vc1.advance_time(VirtualDuration::from_attoseconds(LINE_PERIOD_ATTOSECONDS));

        assert_eq!(frames, 1);
        assert_eq!(read_control_word(&mut vc1, super::VID_LC), 0);
        assert_eq!(read_control_word(&mut vc1, super::VID_FC), 1);
    }

    #[test]
    fn counters_are_read_without_side_effects() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);
        vc1.advance_time(VirtualDuration::from_attoseconds(
            5 * LINE_PERIOD_ATTOSECONDS,
        ));

        for _ in 0..4 {
            assert_eq!(read_control_word(&mut vc1, super::VID_LC), 5);
        }
    }

    #[test]
    fn vertical_blanking_starts_after_the_fixed_visible_height() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);

        assert!(!vc1.in_vertical_blanking());
        vc1.advance_time(VirtualDuration::from_attoseconds(
            u128::from(DISPLAY_HEIGHT) * LINE_PERIOD_ATTOSECONDS,
        ));
        assert!(vc1.in_vertical_blanking());
    }

    #[test]
    fn different_totals_change_frame_end_but_not_visible_blanking_start() {
        let mut short = Vc1::new();
        let mut long = Vc1::new();
        upload(
            &mut short,
            0x0800,
            &[0x03, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 35, 0, 0, 0],
        );
        upload(
            &mut long,
            0x0800,
            &[0x03, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 135, 0, 0, 0],
        );
        write_control_word(&mut short, VID_EP, 0x8800);
        write_control_word(&mut long, VID_EP, 0x8800);
        enable_video(&mut short);
        enable_video(&mut long);

        assert_eq!(
            short.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                u128::from(DISPLAY_HEIGHT) * LINE_PERIOD_ATTOSECONDS
            ))
        );
        assert_eq!(long.time_until_event(), short.time_until_event());

        let blanking_start =
            VirtualDuration::from_attoseconds(u128::from(DISPLAY_HEIGHT) * LINE_PERIOD_ATTOSECONDS);
        short.advance_time(blanking_start);
        long.advance_time(blanking_start);

        assert_eq!(
            short.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                32 * LINE_PERIOD_ATTOSECONDS
            ))
        );
        assert_eq!(
            long.time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                132 * LINE_PERIOD_ATTOSECONDS
            ))
        );
    }

    #[test]
    fn a_reset_timing_generator_stops_time_and_events() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);
        vc1.write(Selector::SystemControl, SYS_CTRL_VTG);

        assert_eq!(vc1.time_until_event(), None);
        assert_eq!(
            vc1.advance_time(VirtualDuration::from_attoseconds(
                1_000 * LINE_PERIOD_ATTOSECONDS
            )),
            0
        );
        assert_eq!(read_control_word(&mut vc1, super::VID_LC), 0);
    }

    #[test]
    fn the_host_address_walks_byte_by_byte_through_one_bank() {
        let mut vc1 = Vc1::new();

        upload(&mut vc1, 0x3000, &[1, 2, 3, 4]);

        set_address(&mut vc1, 0x3000);
        assert_eq!(
            [
                vc1.read(Selector::Sram),
                vc1.read(Selector::Sram),
                vc1.read(Selector::Sram),
                vc1.read(Selector::Sram),
            ],
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn banks_keep_independent_contents_at_the_same_address() {
        let mut vc1 = Vc1::new();

        set_address(&mut vc1, 0x0004);
        vc1.write(Selector::Control, 0x11);
        set_address(&mut vc1, 0x0004);
        vc1.write(Selector::XmapMode, 0x22);
        set_address(&mut vc1, 0x0004);
        vc1.write(Selector::Sram, 0x33);

        set_address(&mut vc1, 0x0004);
        assert_eq!(vc1.read(Selector::Control), 0x11);
        set_address(&mut vc1, 0x0004);
        assert_eq!(vc1.read(Selector::XmapMode), 0x22);
        set_address(&mut vc1, 0x0004);
        assert_eq!(vc1.read(Selector::Sram), 0x33);
    }

    #[test]
    fn the_chip_revision_reports_the_encoding_software_decrements() {
        let mut vc1 = Vc1::new();

        set_address(&mut vc1, CHIP_REVISION);

        assert_eq!(vc1.read(Selector::Test) & 0x07, 2);
    }

    #[test]
    fn xmap_entries_are_sixteen_bit_and_high_byte_first() {
        let mut vc1 = Vc1::new();

        set_address(&mut vc1, 0x0004);
        vc1.write(Selector::XmapMode, 0x01);
        vc1.write(Selector::XmapMode, 0x08);

        set_address(&mut vc1, 0x0004);
        assert_eq!(
            u16::from_be_bytes([vc1.read(Selector::XmapMode), vc1.read(Selector::XmapMode)]),
            0x0108
        );
    }

    #[test]
    fn power_on_diagnostic_registers_keep_only_their_implemented_bits() {
        let mut vc1 = Vc1::new();

        for pattern in [0x5555, 0xaaaa] {
            write_control_word(&mut vc1, CUR_XL, pattern);
            write_control_word(&mut vc1, CUR_YL, pattern);
            write_control_word(&mut vc1, CUR_LY, pattern);
            set_address(&mut vc1, DID_HOR_MOD);
            vc1.write(Selector::Control, (pattern >> 8) as u8);

            assert_eq!(read_control_word(&mut vc1, CUR_XL), pattern & 0x07ff);
            assert_eq!(read_control_word(&mut vc1, CUR_YL), pattern & 0x0fff);
            assert_eq!(read_control_word(&mut vc1, CUR_LY), pattern & 0x07ff);
            set_address(&mut vc1, DID_HOR_MOD);
            assert_eq!(vc1.read(Selector::Control), ((pattern >> 8) as u8) & 0x07);
        }
    }

    #[test]
    fn the_prom_initial_modes_are_preserved_verbatim() {
        let mut vc1 = Vc1::new();

        set_address(&mut vc1, 0);
        for index in 0..32_u16 {
            let mode = (index % 4) * 4;
            vc1.write(Selector::XmapMode, 0);
            vc1.write(Selector::XmapMode, mode as u8);
        }

        set_address(&mut vc1, 0);
        for index in 0..32_u16 {
            assert_eq!(
                u16::from_be_bytes([vc1.read(Selector::XmapMode), vc1.read(Selector::XmapMode)]),
                (index % 4) * 4
            );
        }
    }

    #[test]
    fn did_line_entries_select_modes_from_their_starting_columns() {
        let mut vc1 = Vc1::new();
        write_control_word(&mut vc1, DID_EP, 0x4000);
        upload(&mut vc1, 0x4000, &[0x48, 0x00]);
        upload(
            &mut vc1,
            0x4800,
            &[0x00, 0x03, 0x00, 0x01, 0x0c, 0x82, 0x57, 0x83],
        );
        vc1.write(Selector::SystemControl, SYS_CTRL_DID);
        let mut identifiers = [0; DISPLAY_WIDTH as usize];

        vc1.fill_display_identifiers(0, &mut identifiers);

        assert!(identifiers[..100].iter().all(|identifier| *identifier == 1));
        assert!(
            identifiers[100..700]
                .iter()
                .all(|identifier| *identifier == 2)
        );
        assert!(identifiers[700..].iter().all(|identifier| *identifier == 3));
    }

    #[test]
    fn did_pipeline_padding_cannot_replace_the_visible_identifier() {
        let mut vc1 = Vc1::new();
        write_control_word(&mut vc1, DID_EP, 0x4000);
        upload(&mut vc1, 0x4000, &[0x48, 0x00]);
        upload(
            &mut vc1,
            0x4800,
            &[
                0x00, 0x05, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
        );
        vc1.write(Selector::SystemControl, SYS_CTRL_DID);
        let mut identifiers = [0; DISPLAY_WIDTH as usize];

        vc1.fill_display_identifiers(0, &mut identifiers);

        assert!(identifiers.iter().all(|identifier| *identifier == 4));
    }

    #[test]
    fn a_disabled_did_generator_selects_mode_zero() {
        let vc1 = Vc1::new();
        let mut identifiers = [0xff; DISPLAY_WIDTH as usize];

        vc1.fill_display_identifiers(0, &mut identifiers);

        assert!(identifiers.iter().all(|identifier| *identifier == 0));
    }

    #[test]
    fn consecutive_reads_walk_the_uploaded_bytes_in_order() {
        let mut vc1 = Vc1::new();
        upload(&mut vc1, 0x0100, &[0xa5, 0x5a]);

        set_address(&mut vc1, 0x0100);

        assert_eq!(vc1.read(Selector::Sram), 0xa5);
        assert_eq!(vc1.read(Selector::Sram), 0x5a);
    }

    #[test]
    fn reset_restores_tables_counters_and_the_revision() {
        let mut vc1 = Vc1::new();
        configure_prom_timing(&mut vc1);
        vc1.advance_time(VirtualDuration::from_attoseconds(
            900 * LINE_PERIOD_ATTOSECONDS,
        ));

        vc1.reset();

        assert_eq!(vc1.signal_state(), SignalState::NoSignal);
        assert_eq!(vc1.total_lines, super::DEFAULT_TOTAL_LINES);
        assert_eq!(vc1.frame_counter, 0);
        set_address(&mut vc1, CHIP_REVISION);
        assert_eq!(vc1.read(Selector::Test) & 0x07, 2);
    }
}

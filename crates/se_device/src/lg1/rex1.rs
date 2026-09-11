//! REX1 raster engine: drawing registers, contexts, and pixel writes.
//!
//! The chip presents two windows over the same register file. Writes through
//! the SET window update the next context, while writes through the GO window
//! also start the command held in the working state. Reads through the SET
//! window return the current context, which a completed command latches from
//! the next context.

use serde::{Deserialize, Serialize};

use super::vram::{PlaneGroup, Vram};

/// Offset one past the last drawing register.
pub(super) const DRAWING_END: u64 = 0x008c;
/// Offset of the pad word between the SET and GO drawing pages.
pub(super) const DUMMY: u64 = 0x07fc;
/// Distance from a SET register to the matching GO register.
pub(super) const GO_OFFSET: u64 = 0x0800;
/// SET window offset of the first configuration register.
pub(super) const CONFIG_BASE: u64 = 0x4790;
/// Offset one past the last configuration register.
pub(super) const CONFIG_END: u64 = 0x4800;

const COMMAND: u64 = 0x0000;
const AUX1: u64 = 0x0004;
const XSTATE: u64 = 0x0008;
const XSTARTI: u64 = 0x000c;
const XSTARTF: u64 = 0x0010;
const XSTART: u64 = 0x0014;
const XENDF: u64 = 0x0018;
const YSTARTI: u64 = 0x001c;
const YSTARTF: u64 = 0x0020;
const YSTART: u64 = 0x0024;
const YENDF: u64 = 0x0028;
const XSAVE: u64 = 0x002c;
const MINORSLOPE: u64 = 0x0030;
const XYMOVE: u64 = 0x0034;
const COLORREDI: u64 = 0x0038;
const COLORREDF: u64 = 0x003c;
const COLORGREENI: u64 = 0x0040;
const COLORGREENF: u64 = 0x0044;
const COLORBLUEI: u64 = 0x0048;
const COLORBLUEF: u64 = 0x004c;
const SLOPERED: u64 = 0x0050;
const SLOPEGREEN: u64 = 0x0054;
const SLOPEBLUE: u64 = 0x0058;
const COLORBACK: u64 = 0x005c;
const ZPATTERN: u64 = 0x0060;
const LSPATTERN: u64 = 0x0064;
const LSMODE: u64 = 0x0068;
const AWEIGHT: u64 = 0x006c;
pub(super) const RWAUX1: u64 = 0x0070;
const RWAUX2: u64 = 0x0074;
const RWMASK: u64 = 0x0078;
const SMASK1X: u64 = 0x007c;
const SMASK1Y: u64 = 0x0080;
const XENDI: u64 = 0x0084;
const YENDI: u64 = 0x0088;

const SMASK2X: u64 = 0x4790;
const SMASK2Y: u64 = 0x4794;
const SMASK3X: u64 = 0x4798;
const SMASK3Y: u64 = 0x479c;
const SMASK4X: u64 = 0x47a0;
const SMASK4Y: u64 = 0x47a4;
const AUX2: u64 = 0x47a8;
const DIAGVRAM: u64 = 0x47d8;
const DIAGCID: u64 = 0x47dc;
const WCLOCK: u64 = 0x47e4;
const RWDAC: u64 = 0x47e8;
const CONFIGSEL: u64 = 0x47ec;
const RWVC1: u64 = 0x47f0;
const TOGGLECTXT: u64 = 0x47f4;
const CONFIGMODE: u64 = 0x47f8;
const XYWIN: u64 = 0x47fc;

/// Opcode field of the command register.
const OPCODE_MASK: u32 = 0x07;
const OPCODE_NOP: u32 = 0x0;
const OPCODE_DRAW: u32 = 0x1;
const OPCODE_LDPIXEL: u32 = 0x3;

const CMD_BLOCK: u32 = 1 << 3;
const CMD_LENGTH32: u32 = 1 << 4;
const CMD_QUADMODE: u32 = 1 << 5;
const CMD_XYCONTINUE: u32 = 1 << 7;
const CMD_STOPONX: u32 = 1 << 8;
const CMD_STOPONY: u32 = 1 << 9;
const CMD_ENZPATTERN: u32 = 1 << 10;
const CMD_ENLSPATTERN: u32 = 1 << 11;
const CMD_RGBMODE: u32 = 1 << 14;
const CMD_DITHER: u32 = 1 << 15;
const CMD_COLORCOMP: u32 = 1 << 16;
const CMD_SHADE: u32 = 1 << 17;
const CMD_LOGICSRC: u32 = 1 << 19;
const CMD_COLORAUX: u32 = 1 << 21;
const CMD_LSOPAQUE: u32 = 1 << 22;
const CMD_ZOPAQUE: u32 = 1 << 23;
const CMD_SHARED_FLAGS: u32 = CMD_LOGICSRC | CMD_COLORAUX | CMD_LSOPAQUE | CMD_ZOPAQUE;

const XSTATE_COLORAUX: u32 = 1 << 28;
const XSTATE_LOGICSRC: u32 = 1 << 29;
const XSTATE_LSOPAQUE: u32 = 1 << 30;
const XSTATE_ZOPAQUE: u32 = 1 << 31;

const AUX1_COLORCOMPLT: u32 = 1 << 6;
const AUX1_COLORCOMPEQ: u32 = 1 << 7;
const AUX1_COLORCOMPGT: u32 = 1 << 8;
const AUX1_DITHERRANGE: u32 = 1 << 9;

/// Low AUX2 nibble selecting the four screen masks.
const AUX2_SCREEN_MASK_ENABLES: u32 = 0x0f;
/// AUX2 bit offset of the matching inside-mask polarity nibble.
const AUX2_SCREEN_MASK_INSIDE_SHIFT: u32 = 4;

/// Fractional bits held by a full coordinate register.
const COORDINATE_FRACTION_BITS: u32 = 15;
/// Fractional bits held by an end coordinate register.
const END_FRACTION_BITS: u32 = 11;
/// Fractional bits held by a full color register.
const COLOR_FRACTION_BITS: u32 = 11;

/// REX coordinate corresponding to the upper-left stored frame-buffer pixel.
const FRAMEBUFFER_COORDINATE_BIAS: u32 = 0x0800;

/// Mask applied by the twelve-bit drawing address generators.
const FRAMEBUFFER_COORDINATE_MASK: u32 = 0x0fff;

/// Largest valid value in a color DDA before its overflow bit clamps it.
const MAX_COLOR: i64 = (1 << (8 + COLOR_FRACTION_BITS)) - 1;

/// Pixels transferred by one packed host pixel access.
const PACKED_PIXELS: u32 = 4;

/// Bound on pixels touched by one command.
///
/// The frame buffer holds fewer pixels than this, so a well-formed command
/// always finishes first. The bound only stops a malformed guest command from
/// running without end.
const MAX_COMMAND_PIXELS: u32 = 1 << 21;

/// Window-relative four-by-four Bayer thresholds used for RGB dithering.
const DITHER_MATRIX: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// One complete set of drawing registers.
///
/// Coordinate and color fields hold the full-precision view. The narrower
/// integer and fractional views are aliases that convert on access, so the
/// register file stores each value once.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct Registers {
    command: u32,
    aux1: u32,
    /// Raw XSTATE read-back shadow.
    ///
    /// Cross-view read-back is not established, so command and XSTATE retain
    /// their own raw values while drawing uses the shared fields below.
    xstate: u32,
    /// Source logic operation applied to every pixel write.
    ///
    /// Command bits 31:28 and XSTATE bits 27:24 are two views of this one
    /// field, so whichever register the guest writes last supplies it.
    source_rop: u32,
    /// Command-position flags shared with the high XSTATE nibble.
    ///
    /// Both register views replace all four flags when written, and drawing
    /// reads this normalized form regardless of which view the guest used.
    shared_command_flags: u32,
    xstart: u32,
    ystart: u32,
    xendf: u32,
    yendf: u32,
    xsave: u32,
    minorslope: u32,
    xymove: u32,
    color: [u32; 3],
    color_slope: [u32; 3],
    colorback: u32,
    zpattern: u32,
    lspattern: u32,
    lsmode: u32,
    aweight: u32,
    rwaux1: u32,
    rwaux2: u32,
    rwmask: u32,
    smask1x: u32,
    smask1y: u32,
    /// Plane selection, CID match ranges, and screen mask enables.
    aux2: u32,
}

impl Registers {
    /// Updates XSTART and the integer continuation origin copied from it.
    fn write_xstart(&mut self, value: u32) {
        self.xstart = value;
        self.xsave = value >> COORDINATE_FRACTION_BITS;
    }

    /// Reads one drawing register through its addressed view.
    fn read(&self, offset: u64) -> u32 {
        match offset {
            COMMAND => self.command,
            AUX1 => self.aux1,
            XSTATE => self.xstate,
            XSTARTI => self.xstart >> COORDINATE_FRACTION_BITS,
            XSTARTF => self.xstart >> 4,
            XSTART => self.xstart,
            XENDF => self.xendf,
            YSTARTI => self.ystart >> COORDINATE_FRACTION_BITS,
            YSTARTF => self.ystart >> 4,
            YSTART => self.ystart,
            YENDF => self.yendf,
            XSAVE => self.xsave,
            MINORSLOPE => self.minorslope,
            XYMOVE => self.xymove,
            COLORREDI => self.color[0] >> COLOR_FRACTION_BITS,
            COLORREDF => self.color[0],
            COLORGREENI => self.color[1] >> COLOR_FRACTION_BITS,
            COLORGREENF => self.color[1],
            COLORBLUEI => self.color[2] >> COLOR_FRACTION_BITS,
            COLORBLUEF => self.color[2],
            SLOPERED => self.color_slope[0],
            SLOPEGREEN => self.color_slope[1],
            SLOPEBLUE => self.color_slope[2],
            COLORBACK => self.colorback,
            ZPATTERN => self.zpattern,
            LSPATTERN => self.lspattern,
            LSMODE => self.lsmode,
            AWEIGHT => self.aweight,
            RWAUX1 => self.rwaux1,
            RWAUX2 => self.rwaux2,
            RWMASK => self.rwmask,
            SMASK1X => self.smask1x,
            SMASK1Y => self.smask1y,
            XENDI => self.xendf >> END_FRACTION_BITS,
            YENDI => self.yendf >> END_FRACTION_BITS,
            _ => 0,
        }
    }

    /// Writes one drawing register through its addressed view.
    fn write(&mut self, offset: u64, value: u32) {
        match offset {
            COMMAND => {
                self.command = value;
                self.source_rop = (value >> 28) & 0x0f;
                self.shared_command_flags = value & CMD_SHARED_FLAGS;
            }
            AUX1 => self.aux1 = value & 0x3ff,
            // XSTATE is an alias window over the color, background, write
            // mask, logic operation, and command flag state rather than a
            // register of its own, so a write updates each aliased field.
            XSTATE => {
                self.xstate = value;
                self.color[0] = (value & 0xff) << COLOR_FRACTION_BITS;
                self.colorback = (value >> 8) & 0xff;
                self.rwmask = (self.rwmask & 0xff00) | ((value >> 16) & 0xff);
                self.source_rop = (value >> 24) & 0x0f;
                self.shared_command_flags = command_flags_from_xstate(value);
            }
            XSTARTI => self.write_xstart((value & 0xfff) << COORDINATE_FRACTION_BITS),
            XSTARTF => self.write_xstart((value & 0x007f_ffff) << 4),
            XSTART => self.write_xstart(value & 0x07ff_ffff),
            XENDF => self.xendf = value & 0x007f_f800,
            YSTARTI => self.ystart = (value & 0xfff) << COORDINATE_FRACTION_BITS,
            YSTARTF => self.ystart = (value & 0x007f_ffff) << 4,
            YSTART => self.ystart = value & 0x07ff_ffff,
            YENDF => self.yendf = value & 0x007f_f800,
            XSAVE => self.xsave = value & 0xfff,
            MINORSLOPE => self.minorslope = minor_slope(value),
            XYMOVE => self.xymove = value & 0x07ff_07ff,
            COLORREDI => self.color[0] = (value & 0xff) << COLOR_FRACTION_BITS,
            COLORREDF => self.color[0] = value & 0x000f_ffff,
            COLORGREENI => self.color[1] = (value & 0xff) << COLOR_FRACTION_BITS,
            COLORGREENF => self.color[1] = value & 0x000f_ffff,
            COLORBLUEI => self.color[2] = (value & 0xff) << COLOR_FRACTION_BITS,
            COLORBLUEF => self.color[2] = value & 0x000f_ffff,
            SLOPERED => self.color_slope[0] = color_slope(value),
            SLOPEGREEN => self.color_slope[1] = color_slope(value),
            SLOPEBLUE => self.color_slope[2] = color_slope(value),
            COLORBACK => self.colorback = value & 0xff,
            ZPATTERN => self.zpattern = value,
            LSPATTERN => self.lspattern = value,
            LSMODE => self.lsmode = value & 0x000f_ffff,
            AWEIGHT => self.aweight = value,
            RWAUX1 => self.rwaux1 = value,
            RWAUX2 => self.rwaux2 = value,
            RWMASK => self.rwmask = value & 0xffff,
            SMASK1X => self.smask1x = value & 0x03ff_03ff,
            SMASK1Y => self.smask1y = value & 0x03ff_03ff,
            XENDI => self.xendf = (value & 0xfff) << END_FRACTION_BITS,
            YENDI => self.yendf = (value & 0xfff) << END_FRACTION_BITS,
            _ => {}
        }
    }
}

/// Converts the high XSTATE nibble into the corresponding command flags.
const fn command_flags_from_xstate(value: u32) -> u32 {
    let mut flags = 0;
    if value & XSTATE_COLORAUX != 0 {
        flags |= CMD_COLORAUX;
    }
    if value & XSTATE_LOGICSRC != 0 {
        flags |= CMD_LOGICSRC;
    }
    if value & XSTATE_LSOPAQUE != 0 {
        flags |= CMD_LSOPAQUE;
    }
    if value & XSTATE_ZOPAQUE != 0 {
        flags |= CMD_ZOPAQUE;
    }
    flags
}

/// Converts a minor slope write into the value diagnostics read back.
///
/// The encoding is sign and magnitude rather than two's complement, so the
/// value cannot be stored as an ordinary signed integer.
const fn minor_slope(value: u32) -> u32 {
    let mut slope = value & 0x8001_ffff;
    if slope & 0x8000_0000 != 0 {
        slope = (!(slope & 0xfffe_ffff)).wrapping_add(1);
    }
    slope & 0x0001_ffff
}

/// Converts a color slope write into the value diagnostics read back.
const fn color_slope(value: u32) -> u32 {
    let mut slope = value & 0x801f_ffff;
    if slope & 0x8000_0000 != 0 {
        slope = (!(slope & 0xffef_ffff)).wrapping_add(1);
    }
    ((slope & 0x0008_0000) << 1) | (slope & 0x001f_ffff)
}

/// Sign-extends the normalized twenty-one-bit color DDA slope.
const fn signed_color_slope(value: u32) -> i64 {
    let value = (value & 0x001f_ffff) as i64;
    if value & 0x0010_0000 == 0 {
        value
    } else {
        value - 0x0020_0000
    }
}

/// Registers that exist once per board rather than once per context.
#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct ConfigRegisters {
    smask2x: u32,
    smask2y: u32,
    smask3x: u32,
    smask3y: u32,
    smask4x: u32,
    smask4y: u32,
    diagvram: u32,
    diagcid: u32,
    /// Clock generator or revision port, driven through the selector.
    pub(super) wclock: u32,
    /// Bt479 data-port register.
    rwdac: u32,
    /// Selector choosing the addressed configuration peripheral function.
    pub(super) configsel: u32,
    /// VC1 data-port register.
    rwvc1: u32,
    /// System configuration written by the guest.
    ///
    /// Reads return status fields instead of this value.
    configmode: u32,
    xywin: u32,
}

/// A peripheral port reached through the configuration window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PeripheralPort {
    /// The Bt479 data port.
    Dac,
    /// The VC1 data port.
    Vc1,
    /// The clock generator or revision port.
    Clock,
}

/// The action a configuration window access requires from the board.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfigAccess {
    /// The access only touched REX-local state.
    None,
    /// A peripheral transfer must be started with the supplied byte.
    Write(PeripheralPort, u8),
    /// A peripheral read must be started, returning the latched byte.
    Read(PeripheralPort),
}

/// The REX1 raster engine.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Rex1 {
    current: Registers,
    next: Registers,
    /// Configuration window registers, shared by both contexts.
    pub(super) config: ConfigRegisters,
    /// Pad word between the drawing pages, stored without side effects.
    dummy: u32,
}

impl Rex1 {
    /// Creates a raster engine with cleared registers.
    pub(super) fn new() -> Self {
        Self {
            current: Registers::default(),
            next: Registers::default(),
            config: ConfigRegisters::default(),
            dummy: 0,
        }
    }

    /// Restores the reset state of every register.
    pub(super) fn reset(&mut self) {
        self.current = Registers::default();
        self.next = Registers::default();
        self.config = ConfigRegisters::default();
        self.dummy = 0;
    }

    /// Reads one drawing register without starting a command.
    pub(super) fn read_drawing(&self, offset: u64) -> u32 {
        self.current.read(offset)
    }

    /// Writes one drawing register into the next context.
    pub(super) fn write_drawing(&mut self, offset: u64, value: u32) {
        self.next.write(offset, value);
    }

    /// Applies a GO window write and runs the resulting command.
    ///
    /// The written value takes effect before the command runs, so a command
    /// triggered by a final coordinate write uses that coordinate.
    pub(super) fn write_drawing_go(&mut self, offset: u64, value: u32, vram: &mut Vram) {
        self.next.write(offset, value);
        self.execute(vram);
    }

    /// Writes one complete host-data word and runs it.
    pub(super) fn write_host_data_go(&mut self, value: u32, vram: &mut Vram) {
        self.next.rwaux1 = value;
        self.execute(vram);
    }

    /// Reads a GO window register and runs the resulting command.
    ///
    /// The command runs before the access returns, so a pixel read-back
    /// presents the pixels that command latched. Whether the hardware instead
    /// returns the previous contents and latches for the following access is
    /// not established by the sequences examined so far; this is the smaller
    /// model, and a guest that depends on the delay would expose it.
    pub(super) fn read_drawing_go(&mut self, offset: u64, vram: &mut Vram) -> u32 {
        self.execute(vram);
        self.current.read(offset)
    }

    /// Runs one complete host-data word read.
    pub(super) fn read_host_data_go(&mut self, vram: &mut Vram) -> u32 {
        self.execute(vram);
        self.current.rwaux1
    }

    /// Reads the pad word between the drawing pages.
    pub(super) const fn read_dummy(&self) -> u32 {
        self.dummy
    }

    /// Stores the pad word between the drawing pages.
    ///
    /// The X server writes this word after every register access as a drawing
    /// FIFO flush habit. No hardware side effect is established, so the value
    /// is only retained.
    pub(super) const fn write_dummy(&mut self, value: u32) {
        self.dummy = value;
    }

    /// Reads one configuration register.
    ///
    /// Peripheral data ports return their stored byte. A caller using the GO
    /// alias also performs the reported peripheral transfer.
    pub(super) fn read_config(&self, offset: u64) -> (u32, ConfigAccess) {
        match offset {
            SMASK2X => (self.config.smask2x, ConfigAccess::None),
            SMASK2Y => (self.config.smask2y, ConfigAccess::None),
            SMASK3X => (self.config.smask3x, ConfigAccess::None),
            SMASK3Y => (self.config.smask3y, ConfigAccess::None),
            SMASK4X => (self.config.smask4x, ConfigAccess::None),
            SMASK4Y => (self.config.smask4y, ConfigAccess::None),
            AUX2 => (self.current.aux2, ConfigAccess::None),
            DIAGVRAM => (self.config.diagvram, ConfigAccess::None),
            DIAGCID => (self.config.diagcid, ConfigAccess::None),
            WCLOCK => (
                self.config.wclock,
                ConfigAccess::Read(PeripheralPort::Clock),
            ),
            RWDAC => (self.config.rwdac, ConfigAccess::Read(PeripheralPort::Dac)),
            CONFIGSEL => (self.config.configsel, ConfigAccess::None),
            RWVC1 => (self.config.rwvc1, ConfigAccess::Read(PeripheralPort::Vc1)),
            TOGGLECTXT => (0, ConfigAccess::None),
            CONFIGMODE => (self.configmode_status(), ConfigAccess::None),
            XYWIN => (self.config.xywin, ConfigAccess::None),
            _ => (0, ConfigAccess::None),
        }
    }

    /// Writes one configuration register.
    pub(super) fn write_config(&mut self, offset: u64, value: u32) -> ConfigAccess {
        match offset {
            SMASK2X => self.config.smask2x = value & 0x03ff_03ff,
            SMASK2Y => self.config.smask2y = value & 0x03ff_03ff,
            SMASK3X => self.config.smask3x = value & 0x03ff_03ff,
            SMASK3Y => self.config.smask3y = value & 0x03ff_03ff,
            SMASK4X => self.config.smask4x = value & 0x03ff_03ff,
            SMASK4Y => self.config.smask4y = value & 0x03ff_03ff,
            AUX2 => self.next.aux2 = value & 0x7fff_ffff,
            DIAGVRAM => self.config.diagvram = value,
            DIAGCID => self.config.diagcid = value,
            WCLOCK => {
                self.config.wclock = value & 0xff;
                return ConfigAccess::Write(PeripheralPort::Clock, value as u8);
            }
            RWDAC => {
                self.config.rwdac = value & 0xff;
                return ConfigAccess::Write(PeripheralPort::Dac, value as u8);
            }
            CONFIGSEL => self.config.configsel = value & 0x07,
            RWVC1 => {
                self.config.rwvc1 = value & 0xff;
                return ConfigAccess::Write(PeripheralPort::Vc1, value as u8);
            }
            TOGGLECTXT => core::mem::swap(&mut self.current, &mut self.next),
            CONFIGMODE => self.config.configmode = value,
            XYWIN => self.config.xywin = value & 0x0fff_0fff,
            _ => {}
        }
        ConfigAccess::None
    }

    /// Stores the byte received by a GO read in the addressed data port.
    pub(super) const fn complete_config_read(&mut self, port: PeripheralPort, value: u8) {
        match port {
            PeripheralPort::Dac => self.config.rwdac = value as u32,
            PeripheralPort::Vc1 => self.config.rwvc1 = value as u32,
            PeripheralPort::Clock => self.config.wclock = value as u32,
        }
    }

    /// Returns the status fields reported by a configuration mode read.
    ///
    /// Commands complete inside the access that starts them, so the chip is
    /// never busy and both FIFOs are always empty. Guest loops poll these
    /// fields and require them to reach zero.
    const fn configmode_status(&self) -> u32 {
        0
    }

    /// Runs the command held in the working context and latches the result.
    fn execute(&mut self, vram: &mut Vram) {
        let command = self.next.command;
        match command & OPCODE_MASK {
            OPCODE_DRAW => self.draw(vram),
            OPCODE_LDPIXEL => self.read_packed_pixels(vram),
            OPCODE_NOP => {}
            _ => {}
        }
        self.current = self.next.clone();
    }

    /// Runs one drawing command.
    fn draw(&mut self, vram: &mut Vram) {
        let command = self.next.command;
        let group = PlaneGroup::from_aux2(self.next.aux2);
        let host_pixels = self.next.shared_command_flags & CMD_COLORAUX != 0;
        let start_x = if command & CMD_XYCONTINUE == 0 {
            self.next.xsave = self.next.xstart >> COORDINATE_FRACTION_BITS;
            self.next.xstart >> COORDINATE_FRACTION_BITS
        } else {
            self.next.xsave
        };
        let start_y = self.next.ystart >> COORDINATE_FRACTION_BITS;
        let end_x = self.next.xendf >> END_FRACTION_BITS;
        let end_y = self.next.yendf >> END_FRACTION_BITS;

        // The auxiliary color flag takes the source from the host data
        // register instead of the color registers. Color-index commands move
        // four packed pixels per access, while RGB commands move one 24-bit
        // pixel per access.
        if host_pixels {
            if command & CMD_RGBMODE != 0 {
                self.write_host_rgb_pixel(vram, group, start_x, start_y);
            } else {
                self.write_packed_pixels(vram, group, start_x, start_y, end_x);
            }
            return;
        }

        if command & CMD_ENZPATTERN != 0 {
            self.draw_pattern(vram, group, start_x, start_y, end_x, end_y);
            return;
        }

        if command & (CMD_ENLSPATTERN | CMD_QUADMODE | CMD_STOPONX)
            == (CMD_ENLSPATTERN | CMD_QUADMODE | CMD_STOPONX)
        {
            self.draw_line_stipple(vram, group, start_x, start_y, end_x, end_y);
            return;
        }

        // A quad that stops on X draws one horizontal span. Adding BLOCK and
        // STOPONY extends the same address generation across the rectangle;
        // the PROM clears the screen and fills boxes with that combination.
        let fills_span = command & (CMD_BLOCK | CMD_QUADMODE | CMD_STOPONX | CMD_STOPONY)
            == (CMD_QUADMODE | CMD_STOPONX);
        let fills_area = command & CMD_BLOCK != 0
            && command & CMD_QUADMODE != 0
            && command & (CMD_STOPONX | CMD_STOPONY) == (CMD_STOPONX | CMD_STOPONY);
        if fills_span {
            self.draw_rectangle(vram, group, start_x, start_y, end_x, start_y);
            return;
        }
        if fills_area {
            self.draw_rectangle(vram, group, start_x, start_y, end_x, end_y);
            return;
        }

        self.write_pixel(
            vram,
            group,
            start_x,
            start_y,
            self.source_color(0, start_x, start_y),
        );
    }

    /// Draws a rectangle through the selected source and raster operation.
    ///
    /// The programmed start and end coordinates determine traversal direction.
    /// With LOGICSRC enabled, XYMOVE selects a frame-buffer sample for each
    /// destination pixel; otherwise the current color supplies the source.
    fn draw_rectangle(
        &self,
        vram: &mut Vram,
        group: PlaneGroup,
        start_x: u32,
        start_y: u32,
        end_x: u32,
        end_y: u32,
    ) {
        let logic_source = self.next.shared_command_flags & CMD_LOGICSRC != 0;
        let x_offset = signed_coordinate(self.next.xymove >> 16);
        let y_offset = signed_coordinate(self.next.xymove);
        let x_ascending = start_x <= end_x;
        let y_ascending = start_y <= end_y;
        let mut budget = MAX_COMMAND_PIXELS;
        let mut y = start_y;

        loop {
            let mut x = start_x;
            let mut shade_step = 0;
            loop {
                if budget == 0 {
                    return;
                }
                budget -= 1;
                let source = if logic_source {
                    let source_x = i64::from(x) + i64::from(x_offset);
                    let source_y = i64::from(y) + i64::from(y_offset);
                    match (u32::try_from(source_x), u32::try_from(source_y)) {
                        (Ok(source_x), Ok(source_y)) => {
                            self.read_pixel(vram, group, source_x, source_y)
                        }
                        _ => 0,
                    }
                } else {
                    self.source_color(shade_step, x, y)
                };
                self.write_pixel(vram, group, x, y, source);

                if x == end_x {
                    break;
                }
                if x_ascending {
                    x += 1;
                } else {
                    x -= 1;
                }
                shade_step += 1;
            }

            if y == end_y {
                break;
            }
            if y_ascending {
                y += 1;
            } else {
                y -= 1;
            }
        }
    }

    /// Draws one span whose pixels are selected by the Z pattern register.
    ///
    /// The guest supplies glyph fragments in the most significant bits, so
    /// pattern bits are consumed from bit thirty-one downward. When the
    /// command is opaque, cleared bits write the background color instead of
    /// leaving the destination untouched. Completing a row advances Y toward
    /// YEND, which lets text software submit bitmap rows from the baseline
    /// toward the top of a glyph.
    fn draw_pattern(
        &mut self,
        vram: &mut Vram,
        group: PlaneGroup,
        start_x: u32,
        y: u32,
        end_x: u32,
        end_y: u32,
    ) {
        let command = self.next.command;
        let pattern = self.next.zpattern;
        let length = if command & CMD_LENGTH32 == 0 { 16 } else { 32 };
        let opaque = self.next.shared_command_flags & CMD_ZOPAQUE != 0;
        let stop_on_x = command & CMD_STOPONX != 0;
        let background = self.next.colorback as u8;

        let mut x = start_x;
        for bit in 0..length {
            if stop_on_x && x > end_x {
                break;
            }
            if pattern & (0x8000_0000 >> bit) != 0 {
                self.write_pixel(vram, group, x, y, self.source_color(bit, x, y));
            } else if opaque {
                self.write_pixel(vram, group, x, y, background);
            }
            x += 1;
        }

        // STOPONX clips each submitted fragment at XEND. BLOCK distinguishes
        // the final fragment of a scan line, which restores the saved left
        // edge and advances Y; with XYCONTINUE, preceding fragments resume
        // from XEND + 1.
        if command & CMD_BLOCK != 0 && stop_on_x && x > end_x {
            let next_y = if y > end_y {
                y.saturating_sub(1)
            } else {
                y.saturating_add(1)
            };
            self.next.ystart = next_y << COORDINATE_FRACTION_BITS;
            self.next.xsave = self.next.xstart >> COORDINATE_FRACTION_BITS;
        } else {
            self.next.xsave = x;
        }
    }

    /// Draws one span selected by the line stipple pattern.
    ///
    /// Xsgi supplies patterns between seventeen and thirty-two pixels wide.
    /// LSMODE encodes that width minus seventeen in bits 19:16, while the
    /// pattern is consumed from its most significant bit and repeats at the
    /// programmed width. A continued block consumes one GO pattern write per
    /// rectangle row, restoring XSTART and advancing Y for the following row.
    fn draw_line_stipple(
        &mut self,
        vram: &mut Vram,
        group: PlaneGroup,
        start_x: u32,
        y: u32,
        end_x: u32,
        end_y: u32,
    ) {
        let command = self.next.command;
        let pattern = self.next.lspattern;
        let length = ((self.next.lsmode >> 16) & 0x0f) + 17;
        let opaque = self.next.shared_command_flags & CMD_LSOPAQUE != 0;
        let background = self.next.colorback as u8;
        let x_ascending = start_x <= end_x;
        let mut x = start_x;

        for pixel in 0..MAX_COMMAND_PIXELS {
            let bit = 31 - (pixel % length);
            if pattern & (1 << bit) != 0 {
                self.write_pixel(vram, group, x, y, self.source_color(pixel, x, y));
            } else if opaque {
                self.write_pixel(vram, group, x, y, background);
            }

            if x == end_x {
                break;
            }
            x = if x_ascending { x + 1 } else { x - 1 };
        }

        let continues_rows = command & (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE | CMD_STOPONX)
            == (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE | CMD_STOPONX);
        if continues_rows {
            let next_y = if y > end_y {
                y.saturating_sub(1)
            } else {
                y.saturating_add(1)
            };
            self.next.ystart = next_y << COORDINATE_FRACTION_BITS;
            self.next.xsave = self.next.xstart >> COORDINATE_FRACTION_BITS;
        }
    }

    /// Writes packed host pixels into the frame buffer.
    ///
    /// One host word carries four pixels with the leftmost in the most
    /// significant byte. A non-continued QUADMODE/STOPONX command repeats
    /// those four pixels as a tile through XEND. A continued block command
    /// instead consumes one host word at a time; reaching XEND discards any
    /// unused byte lanes, restores XSTART, and advances Y for the next word.
    fn write_packed_pixels(
        &mut self,
        vram: &mut Vram,
        group: PlaneGroup,
        start_x: u32,
        start_y: u32,
        end_x: u32,
    ) {
        let command = self.next.command;
        let tiled_span = command & (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE | CMD_STOPONX)
            == (CMD_QUADMODE | CMD_STOPONX);
        let rectangle = command & (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE)
            == (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE);
        let stop_on_x = self.next.command & CMD_STOPONX != 0;
        let packed = self.next.rwaux1;
        let origin_x = self.next.xstart >> COORDINATE_FRACTION_BITS;
        let end_y = self.next.yendf >> END_FRACTION_BITS;
        let x_ascending = origin_x <= end_x;
        let y_ascending = start_y <= end_y;
        let mut x = start_x;
        let mut y = start_y;
        let mut span_complete = false;

        if tiled_span {
            let mut x = origin_x;
            for pixel in 0..MAX_COMMAND_PIXELS {
                let lane = pixel % PACKED_PIXELS;
                let shift = 8 * (PACKED_PIXELS - 1 - lane);
                self.write_pixel(vram, group, x, y, ((packed >> shift) & 0xff) as u8);
                if x == end_x {
                    break;
                }
                x = if x_ascending { x + 1 } else { x - 1 };
            }
            self.next.xsave = origin_x;
            return;
        }

        for lane in 0..PACKED_PIXELS {
            let past_end = if x_ascending { x > end_x } else { x < end_x };
            if !rectangle && stop_on_x && past_end {
                span_complete = true;
                break;
            }
            let shift = 8 * (PACKED_PIXELS - 1 - lane);
            self.write_pixel(vram, group, x, y, ((packed >> shift) & 0xff) as u8);
            if rectangle && x == end_x {
                x = origin_x;
                if y != end_y {
                    y = if y_ascending { y + 1 } else { y - 1 };
                }
                break;
            } else if !rectangle && stop_on_x && x == end_x {
                span_complete = true;
                break;
            } else {
                x = if x_ascending { x + 1 } else { x - 1 };
            }
        }
        self.next.xsave = if span_complete { origin_x } else { x };
        self.next.ystart = y << COORDINATE_FRACTION_BITS;
    }

    /// Writes one host-supplied RGB pixel and advances the rectangle address.
    ///
    /// In RGB mode, RWAUX1 carries one `0x00RRGGBB` pixel instead of four
    /// color indices. A continued block advances each GO access toward XEND;
    /// reaching the inclusive row end restores XSTART and advances Y toward
    /// YEND. Other commands retain the one-pixel X increment.
    fn write_host_rgb_pixel(&mut self, vram: &mut Vram, group: PlaneGroup, x: u32, y: u32) {
        let rgb = self.next.rwaux1;
        let red = (rgb >> 16) as u8;
        let green = (rgb >> 8) as u8;
        let blue = rgb as u8;
        self.write_pixel(vram, group, x, y, self.rgb_pixel(red, green, blue, x, y));

        let origin_x = self.next.xstart >> COORDINATE_FRACTION_BITS;
        let end_x = self.next.xendf >> END_FRACTION_BITS;
        let end_y = self.next.yendf >> END_FRACTION_BITS;
        let rectangle =
            self.next.command & (CMD_BLOCK | CMD_XYCONTINUE) == (CMD_BLOCK | CMD_XYCONTINUE);
        if rectangle && x == end_x {
            self.next.xsave = origin_x;
            if y != end_y {
                let next_y = if y < end_y { y + 1 } else { y - 1 };
                self.next.ystart = next_y << COORDINATE_FRACTION_BITS;
            }
        } else if rectangle && origin_x > end_x {
            self.next.xsave = x.wrapping_sub(1) & FRAMEBUFFER_COORDINATE_MASK;
        } else {
            self.next.xsave = x.wrapping_add(1) & FRAMEBUFFER_COORDINATE_MASK;
        }
    }

    /// Reads four packed pixels into the host data latch.
    ///
    /// The guest issues this command and then takes the value from the GO
    /// data register, so the pixels must be latched before the access
    /// returns. Continued block reads follow the same rectangular traversal
    /// as host-data writes.
    fn read_packed_pixels(&mut self, vram: &Vram) {
        let group = PlaneGroup::from_aux2(self.next.aux2);
        let command = self.next.command;
        let rectangle = command & (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE)
            == (CMD_BLOCK | CMD_QUADMODE | CMD_XYCONTINUE);
        let origin_x = self.next.xstart >> COORDINATE_FRACTION_BITS;
        let mut x = if command & CMD_XYCONTINUE == 0 {
            self.next.xstart >> COORDINATE_FRACTION_BITS
        } else {
            self.next.xsave
        };
        let end_x = self.next.xendf >> END_FRACTION_BITS;
        let end_y = self.next.yendf >> END_FRACTION_BITS;
        let x_ascending = origin_x <= end_x;
        let mut y = self.next.ystart >> COORDINATE_FRACTION_BITS;
        let y_ascending = y <= end_y;

        let mut latch = self.next.rwaux1;
        for lane in 0..PACKED_PIXELS {
            let shift = 8 * (PACKED_PIXELS - 1 - lane);
            latch =
                (latch & !(0xff << shift)) | u32::from(self.read_pixel(vram, group, x, y)) << shift;
            if rectangle && x == end_x {
                x = origin_x;
                if y != end_y {
                    y = if y_ascending { y + 1 } else { y - 1 };
                }
                break;
            } else {
                x = if rectangle && !x_ascending {
                    x - 1
                } else {
                    x + 1
                };
            }
        }
        self.next.rwaux1 = latch;
        self.next.xsave = x;
        self.next.ystart = y << COORDINATE_FRACTION_BITS;
    }

    /// Returns the color a drawing command writes at one DDA step.
    ///
    /// Color-index drawing uses the red iterator directly. RGB drawing packs
    /// its three eight-bit iterators into the board's red 3, blue 2, green 3
    /// palette index. SHADE advances each iterator by its programmed slope;
    /// the twentieth color bit clamps underflow and overflow at the endpoints.
    fn source_color(&self, shade_step: u32, x: u32, y: u32) -> u8 {
        let red = self.color_component(0, shade_step);
        if self.next.command & CMD_RGBMODE == 0 {
            return red;
        }

        let green = self.color_component(1, shade_step);
        let blue = self.color_component(2, shade_step);
        self.rgb_pixel(red, green, blue, x, y)
    }

    /// Converts full RGB components to the board's R3/B2/G3 pixel format.
    ///
    /// The scaled dither uses the low two bits of window-relative coordinates.
    /// Other command combinations retain direct component truncation.
    fn rgb_pixel(&self, red: u8, green: u8, blue: u8, x: u32, y: u32) -> u8 {
        if self.next.command & CMD_DITHER == 0 || self.next.aux1 & AUX1_DITHERRANGE == 0 {
            return pack_rgb(red, green, blue);
        }

        let threshold = DITHER_MATRIX[(y & 3) as usize][(x & 3) as usize];
        let red = dither_rgb_component(red, 3, threshold);
        let green = dither_rgb_component(green, 3, threshold);
        let blue = dither_rgb_component(blue, 2, threshold);
        (red << 5) | (blue << 3) | green
    }

    /// Returns one clamped eight-bit color-iterator component.
    fn color_component(&self, component: usize, shade_step: u32) -> u8 {
        let mut color = i64::from(self.next.color[component]);
        if self.next.command & CMD_SHADE != 0 {
            color += signed_color_slope(self.next.color_slope[component]) * i64::from(shade_step);
        }
        (color.clamp(0, MAX_COLOR) >> COLOR_FRACTION_BITS) as u8
    }

    /// Reads one logical drawing coordinate through the window origin.
    fn read_pixel(&self, vram: &Vram, group: PlaneGroup, x: u32, y: u32) -> u8 {
        let Some((x, y)) = self.framebuffer_coordinate(x, y) else {
            return 0;
        };
        vram.read(group, x, y)
    }

    /// Maps logical drawing coordinates into stored frame-buffer coordinates.
    fn framebuffer_coordinate(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        let window_x = self.config.xywin >> 16;
        let window_y = self.config.xywin;
        let x = (x + window_x) & FRAMEBUFFER_COORDINATE_MASK;
        let y = (y + window_y) & FRAMEBUFFER_COORDINATE_MASK;
        Some((
            x.checked_sub(FRAMEBUFFER_COORDINATE_BIAS)?,
            y.checked_sub(FRAMEBUFFER_COORDINATE_BIAS)?,
        ))
    }

    /// Applies the logic operation and write mask for one pixel.
    ///
    /// The source is combined with the destination, and only the bits
    /// selected by the write mask replace stored data. Color comparison, when
    /// enabled, can reject the pixel before it is written.
    fn write_pixel(&self, vram: &mut Vram, group: PlaneGroup, x: u32, y: u32, source: u8) {
        let Some((x, y)) = self.framebuffer_coordinate(x, y) else {
            return;
        };
        if !self.screen_masks_admit(x, y) {
            return;
        }
        let destination = vram.read(group, x, y);
        if !self.color_comparison_passes(source, destination) {
            return;
        }
        let operation = self.next.source_rop;
        let result = logic_operation(operation, source, destination);
        vram.write_masked(group, x, y, result, self.write_mask());
    }

    /// Reports whether every enabled screen-mask relation admits a write.
    ///
    /// Screen-mask coordinates are absolute frame-buffer coordinates. AUX2
    /// selects each rectangle and independently chooses whether its inclusive
    /// interior or exterior admits drawing. A pixel is accepted only when it
    /// satisfies every selected relation.
    fn screen_masks_admit(&self, x: u32, y: u32) -> bool {
        let aux2 = self.next.aux2;
        let enabled = aux2 & AUX2_SCREEN_MASK_ENABLES;
        if enabled == 0 {
            return true;
        }

        let masks = [
            (self.next.smask1x, self.next.smask1y),
            (self.config.smask2x, self.config.smask2y),
            (self.config.smask3x, self.config.smask3y),
            (self.config.smask4x, self.config.smask4y),
        ];
        masks
            .iter()
            .enumerate()
            .all(|(index, &(x_bounds, y_bounds))| {
                let enable = 1 << index;
                if enabled & enable == 0 {
                    return true;
                }

                let left = x_bounds & 0x03ff;
                let right = (x_bounds >> 16) & 0x03ff;
                let top = y_bounds & 0x03ff;
                let bottom = (y_bounds >> 16) & 0x03ff;
                let inside = (left..=right).contains(&x) && (top..=bottom).contains(&y);
                let admits_inside = aux2 & (enable << AUX2_SCREEN_MASK_INSIDE_SHIFT) != 0;
                inside == admits_inside
            })
    }

    /// Returns the eight-bit write mask applied to pixel writes.
    ///
    /// The mask lives in the low byte of `RWMASK`; the XSTATE alias window
    /// updates that same byte, so both guest paths converge on one field.
    /// The upper byte holds the read mask, whose effect is not established.
    const fn write_mask(&self) -> u8 {
        (self.next.rwmask & 0xff) as u8
    }

    /// Reports whether color comparison admits one pixel.
    fn color_comparison_passes(&self, source: u8, destination: u8) -> bool {
        if self.next.command & CMD_COLORCOMP == 0 {
            return true;
        }
        let aux1 = self.next.aux1;
        (aux1 & AUX1_COLORCOMPLT != 0 && source < destination)
            || (aux1 & AUX1_COLORCOMPEQ != 0 && source == destination)
            || (aux1 & AUX1_COLORCOMPGT != 0 && source > destination)
    }
}

/// Packs full RGB components into the board's R3/B2/G3 color index.
const fn pack_rgb(red: u8, green: u8, blue: u8) -> u8 {
    (red & 0xe0) | ((blue >> 3) & 0x18) | (green >> 5)
}

/// Scales and dithers one eight-bit RGB component to two or three bits.
const fn dither_rgb_component(value: u8, stored_bits: u32, threshold: u8) -> u8 {
    let scaled = (value >> (4 - stored_bits)) - (value >> 4);
    (scaled >> 4) + ((scaled & 0x0f) > threshold) as u8
}

/// Applies one of the sixteen bitwise raster operations.
const fn logic_operation(operation: u32, source: u8, destination: u8) -> u8 {
    match operation & 0x0f {
        0x0 => 0,
        0x1 => source & destination,
        0x2 => source & !destination,
        0x3 => source,
        0x4 => !source & destination,
        0x5 => destination,
        0x6 => source ^ destination,
        0x7 => source | destination,
        0x8 => !(source | destination),
        0x9 => !(source ^ destination),
        0xa => !destination,
        0xb => source | !destination,
        0xc => !source,
        0xd => !source | destination,
        0xe => !(source & destination),
        _ => 0xff,
    }
}

/// Sign-extends one eleven-bit XYMOVE coordinate.
const fn signed_coordinate(value: u32) -> i32 {
    let value = (value & 0x07ff) as i32;
    if value & 0x0400 == 0 {
        value
    } else {
        value - 0x0800
    }
}

#[cfg(test)]
mod tests {
    use super::super::vram::{PlaneGroup, Vram};
    use super::{
        AUX1, AUX2, COLORBLUEI, COLORGREENF, COLORGREENI, COLORREDF, COLORREDI, COMMAND,
        COORDINATE_FRACTION_BITS, ConfigAccess, LSMODE, LSPATTERN, PeripheralPort, RWAUX1, RWMASK,
        Rex1, SLOPEBLUE, SLOPEGREEN, SLOPERED, SMASK1X, SMASK1Y, SMASK2X, SMASK2Y, XENDI, XSAVE,
        XSTART, XSTARTI, XSTATE, XYMOVE, XYWIN, YENDI, YSTARTI, ZPATTERN, color_slope, minor_slope,
        signed_color_slope,
    };

    /// Selects the pixel plane group through the configuration window.
    fn select_pixel_planes(rex: &mut Rex1) {
        rex.write_config(AUX2, 0x2000_0000);
        rex.write_config(XYWIN, 0x0800_0800);
    }

    /// Draws a solid color-index rectangle with source-copy logic.
    fn draw_solid_rectangle(
        rex: &mut Rex1,
        vram: &mut Vram,
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    ) {
        rex.write_drawing(COMMAND, 0x329);
        rex.write_drawing(XSTATE, 0x03ff_0000);
        rex.write_drawing(COLORREDI, 0x5a);
        rex.write_drawing(XSTARTI, left);
        rex.write_drawing(YSTARTI, top);
        rex.write_drawing(XENDI, right);
        rex.write_drawing_go(YENDI, bottom, vram);
    }

    #[test]
    fn the_probe_sequence_reports_the_converted_coordinate() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();

        rex.write_drawing(XSTARTI, 0x1234_5678);
        rex.write_drawing(COMMAND, 0);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(rex.read_drawing(XSTART), 0x033c_0000);
    }

    #[test]
    fn set_reads_return_the_current_context_until_a_command_latches() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_drawing(XSTARTI, 5);

        assert_eq!(rex.read_drawing(XSTARTI), 0);

        rex.write_drawing(COMMAND, 0);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(rex.read_drawing(XSTARTI), 5);
    }

    #[test]
    fn toggling_the_context_exchanges_both_register_files() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_drawing(XSTARTI, 7);
        rex.write_drawing(COMMAND, 0);
        rex.read_drawing_go(COMMAND, &mut vram);
        rex.write_drawing(XSTARTI, 9);

        rex.write_config(super::TOGGLECTXT, 0);

        assert_eq!(rex.read_drawing(XSTARTI), 9);
    }

    #[test]
    fn a_final_go_coordinate_write_participates_in_the_command() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        for y in 0..=3 {
            for x in 0..=4 {
                vram.write_masked(PlaneGroup::Pixel, x, y, 0x5a, 0xff);
            }
        }
        rex.write_drawing(COMMAND, 0x329);
        rex.write_drawing(XSTATE, 0x03ff_0000);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 3);

        rex.write_drawing_go(YENDI, 2, &mut vram);

        for y in 0..=3 {
            for x in 0..=4 {
                let expected = if x <= 3 && y <= 2 { 0 } else { 0x5a };
                assert_eq!(vram.read(PlaneGroup::Pixel, x, y), expected);
            }
        }
        assert_eq!(rex.read_drawing(YENDI), 2);
    }

    #[test]
    fn the_prom_clear_screen_fills_the_addressed_rectangle() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        // Paint a marker so the clear is observable.
        rex.write_drawing(COMMAND, 0x3000_0001);
        rex.write_drawing(COLORREDI, 0x5a);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 2);
        rex.write_drawing_go(YSTARTI, 2, &mut vram);
        rex.write_drawing(XSTARTI, 1023);
        rex.write_drawing_go(YSTARTI, 767, &mut vram);
        assert_eq!(vram.read(PlaneGroup::Pixel, 2, 2), 0x5a);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1023, 767), 0x5a);

        rex.write_drawing(COMMAND, 0x329);
        rex.write_drawing(XSTATE, 0x03ff_0000);
        rex.write_drawing(COLORREDI, 0);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing_go(YENDI, 767, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 2, 2), 0);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1023, 767), 0);
    }

    #[test]
    fn an_inside_screen_mask_admits_pixels_within_its_rectangle() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_config(XYWIN, 0x0800_0800);
        rex.write_config(AUX2, 0x2000_0011);
        rex.write_drawing(SMASK1X, (4 << 16) | 2);
        rex.write_drawing(SMASK1Y, (5 << 16) | 3);

        draw_solid_rectangle(&mut rex, &mut vram, 0, 0, 6, 6);

        for y in 0..=6 {
            for x in 0..=6 {
                let expected = if (2..=4).contains(&x) && (3..=5).contains(&y) {
                    0x5a
                } else {
                    0
                };
                assert_eq!(vram.read(PlaneGroup::Pixel, x, y), expected);
            }
        }
    }

    #[test]
    fn an_outside_screen_mask_admits_pixels_beyond_its_rectangle() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_config(XYWIN, 0x0800_0800);
        rex.write_config(AUX2, 0x2000_0001);
        rex.write_drawing(SMASK1X, (4 << 16) | 2);
        rex.write_drawing(SMASK1Y, (5 << 16) | 3);

        draw_solid_rectangle(&mut rex, &mut vram, 0, 0, 6, 6);

        for y in 0..=6 {
            for x in 0..=6 {
                let expected = if (2..=4).contains(&x) && (3..=5).contains(&y) {
                    0
                } else {
                    0x5a
                };
                assert_eq!(vram.read(PlaneGroup::Pixel, x, y), expected);
            }
        }
    }

    #[test]
    fn enabled_screen_mask_relations_are_combined() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_config(XYWIN, 0x0800_0800);
        rex.write_config(AUX2, 0x2000_0013);
        rex.write_drawing(SMASK1X, (5 << 16) | 1);
        rex.write_drawing(SMASK1Y, (5 << 16) | 1);
        rex.write_config(SMASK2X, (4 << 16) | 3);
        rex.write_config(SMASK2Y, (5 << 16) | 1);

        draw_solid_rectangle(&mut rex, &mut vram, 0, 0, 6, 6);

        for y in 0..=6 {
            for x in 0..=6 {
                let expected = if (x == 1 || x == 2 || x == 5) && (1..=5).contains(&y) {
                    0x5a
                } else {
                    0
                };
                assert_eq!(vram.read(PlaneGroup::Pixel, x, y), expected);
            }
        }
    }

    #[test]
    fn a_point_command_triggered_by_a_go_read_draws_one_pixel() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x33);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 6);
        rex.write_drawing(COMMAND, 0x3000_0001);

        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 6), 0x33);
        assert_eq!(vram.read(PlaneGroup::Pixel, 5, 6), 0);
    }

    #[test]
    fn the_window_origin_offsets_logical_drawing_coordinates() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_config(XYWIN, 0x0804_0806);
        rex.write_drawing(COLORREDI, 0x5a);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);

        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 6), 0x5a);
        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0);
    }

    #[test]
    fn rgb_mode_packs_red_blue_green_into_the_eight_bit_pixel() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0xe0);
        rex.write_drawing(COLORGREENI, 0xa0);
        rex.write_drawing(COLORBLUEI, 0x80);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 6);

        rex.write_drawing_go(COMMAND, 0x3000_4001, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 6), 0xf5);
    }

    #[test]
    fn a_shaded_rgb_span_advances_all_three_color_iterators() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0);
        rex.write_drawing(COLORGREENI, 0);
        rex.write_drawing(COLORBLUEI, 0);
        rex.write_drawing(SLOPERED, (4096.0_f32 + 32.0).to_bits());
        rex.write_drawing(SLOPEGREEN, (4096.0_f32 + 32.0).to_bits());
        rex.write_drawing(SLOPEBLUE, (4096.0_f32 + 64.0).to_bits());
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 6);

        rex.write_drawing(COMMAND, 0x3206_4121);
        rex.write_drawing_go(XENDI, 6, &mut vram);

        assert_eq!(
            (4..=6)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 6))
                .collect::<Vec<_>>(),
            vec![0x00, 0x29, 0x52]
        );
    }

    #[test]
    fn a_shaded_color_index_span_uses_the_red_iterator() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 10);
        rex.write_drawing(SLOPERED, (4096.0_f32 + 1.0).to_bits());
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 6);

        rex.write_drawing(COMMAND, 0x3002_0121);
        rex.write_drawing_go(XENDI, 6, &mut vram);

        assert_eq!(
            (4..=6)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 6))
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
    }

    #[test]
    fn shaded_color_iterators_clamp_at_both_endpoints() {
        let mut rex = Rex1::new();
        rex.write_drawing(COLORREDF, (1 << 11) | (1 << 10));
        rex.write_drawing(COLORGREENF, (254 << 11) | (1 << 10));
        rex.write_drawing(SLOPERED, (-4096.0_f32 - 2.0).to_bits());
        rex.write_drawing(SLOPEGREEN, (4096.0_f32 + 2.0).to_bits());
        rex.write_drawing(COMMAND, 0x0002_0000);

        assert_eq!(rex.color_component(0, 0), 1);
        assert_eq!(rex.color_component(0, 1), 0);
        assert_eq!(rex.color_component(1, 0), 254);
        assert_eq!(rex.color_component(1, 1), 255);
    }

    #[test]
    fn a_quad_stopping_on_x_draws_a_solid_horizontal_span() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        for y in 2..=4 {
            for x in 3..=9 {
                vram.write_masked(PlaneGroup::Pixel, x, y, 0x5a, 0xff);
            }
        }
        rex.write_drawing(COMMAND, 0x0000_0121);
        rex.write_drawing(XSTATE, 0x030f_0021);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 3);

        rex.write_drawing_go(XENDI, 8, &mut vram);

        for x in 3..=9 {
            let expected = if (4..=8).contains(&x) { 0x51 } else { 0x5a };
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 3), expected);
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 2), 0x5a);
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 4), 0x5a);
        }
    }

    #[test]
    fn a_transparent_line_stipple_selects_pixels_across_a_span() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        for y in 2..=4 {
            for x in 1..=21 {
                vram.write_masked(PlaneGroup::Pixel, x, y, 0x5a, 0xff);
            }
        }
        rex.write_drawing(COMMAND, 0x0000_0921);
        rex.write_drawing(XSTATE, 0x03ff_00f0);
        rex.write_drawing(LSMODE, 0);
        rex.write_drawing(LSPATTERN, 0xa000_0000);
        rex.write_drawing(XSTARTI, 2);
        rex.write_drawing(YSTARTI, 3);

        rex.write_drawing_go(XENDI, 20, &mut vram);

        for x in 1..=21 {
            let expected = if matches!(x, 2 | 4 | 19) { 0xf0 } else { 0x5a };
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 3), expected);
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 2), 0x5a);
            assert_eq!(vram.read(PlaneGroup::Pixel, x, 4), 0x5a);
        }
    }

    #[test]
    fn an_opaque_stippled_rectangle_consumes_one_pattern_per_row() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        for y in 2..=5 {
            for x in 3..=8 {
                vram.write_masked(PlaneGroup::Pixel, x, y, 0x5a, 0xff);
            }
        }
        rex.write_drawing(XSTATE, 0x03ff_11f0);
        rex.write_drawing(LSMODE, 0);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 7);
        rex.write_drawing(YENDI, 4);
        rex.write_drawing_go(COMMAND, 0, &mut vram);
        rex.write_drawing(COMMAND, 0x3040_09a9);

        rex.write_drawing_go(LSPATTERN, 0xa000_0000, &mut vram);

        assert_eq!(
            (4..=7)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 3))
                .collect::<Vec<_>>(),
            vec![0xf0, 0x11, 0xf0, 0x11]
        );
        assert!((4..=7).all(|x| vram.read(PlaneGroup::Pixel, x, 4) == 0x5a));

        rex.write_drawing_go(LSPATTERN, 0x5000_0000, &mut vram);

        assert_eq!(
            (4..=7)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 4))
                .collect::<Vec<_>>(),
            vec![0x11, 0xf0, 0x11, 0xf0]
        );
        for y in 3..=4 {
            assert_eq!(vram.read(PlaneGroup::Pixel, 3, y), 0x5a);
            assert_eq!(vram.read(PlaneGroup::Pixel, 8, y), 0x5a);
        }
        for y in [2, 5] {
            assert!((3..=8).all(|x| vram.read(PlaneGroup::Pixel, x, y) == 0x5a));
        }
    }

    #[test]
    fn the_write_mask_protects_unselected_bits() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0xff);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 1);
        rex.write_drawing(YSTARTI, 1);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        rex.write_drawing(COLORREDI, 0x00);
        rex.write_drawing(RWMASK, 0x0f);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 1), 0xf0);
    }

    #[test]
    fn every_logic_operation_matches_the_lg1_truth_table() {
        const SOURCE: u8 = 0xcc;
        const DESTINATION: u8 = 0xf0;
        const EXPECTED: [u8; 16] = [
            0x00, 0xc0, 0x0c, 0xcc, 0x30, 0xf0, 0x3c, 0xfc, 0x03, 0xc3, 0x0f, 0xcf, 0x33, 0xf3,
            0x3f, 0xff,
        ];

        for (operation, expected) in EXPECTED.into_iter().enumerate() {
            let mut rex = Rex1::new();
            let mut vram = Vram::new();
            select_pixel_planes(&mut rex);
            rex.write_drawing(RWMASK, 0xff);
            rex.write_drawing(XSTARTI, 0);
            rex.write_drawing(YSTARTI, 0);
            rex.write_drawing(COLORREDI, u32::from(DESTINATION));
            rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

            rex.write_drawing(COLORREDI, u32::from(SOURCE));
            rex.write_drawing_go(COMMAND, ((operation as u32) << 28) | 0x0001, &mut vram);

            assert_eq!(
                vram.read(PlaneGroup::Pixel, 0, 0),
                expected,
                "logic operation {operation:#x}"
            );
        }
    }

    #[test]
    fn xstate_last_write_supplies_the_logic_operation() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(COLORREDI, 0xf0);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        // XSTATE follows the command and therefore replaces its source logic
        // operation with exclusive or while also supplying source color cc.
        rex.write_drawing(COMMAND, 0x0000_0001);
        rex.write_drawing(XSTATE, 0x06ff_00cc);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x3c);
    }

    #[test]
    fn command_last_write_replaces_the_xstate_logic_operation() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(COLORREDI, 0xf0);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        rex.write_drawing(XSTATE, 0x06ff_00cc);
        rex.write_drawing(COMMAND, 0x3000_0001);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0xcc);
    }

    #[test]
    fn a_narrow_glyph_draws_sixteen_pattern_bits() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x0f);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x3000_05a9);

        rex.write_drawing_go(ZPATTERN, 0xa000_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 0), 0);
        assert_eq!(vram.read(PlaneGroup::Pixel, 2, 0), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 3, 0), 0);
    }

    #[test]
    fn prom_style_glyph_rows_advance_from_the_baseline_toward_yend() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x0f);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 12);
        rex.write_drawing(XENDI, 10);
        rex.write_drawing(YENDI, 0);
        rex.write_drawing(COMMAND, 0x3000_05b9);

        // These are the bottom, middle, and top rows of an asymmetric F-like
        // glyph, in the order used by the firmware text path.
        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);
        rex.write_drawing_go(ZPATTERN, 0xfc00_0000, &mut vram);
        rex.write_drawing_go(ZPATTERN, 0xfe00_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 12), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 5, 12), 0);
        assert!((4..=9).all(|x| vram.read(PlaneGroup::Pixel, x, 11) == 0x0f));
        assert!((4..=10).all(|x| vram.read(PlaneGroup::Pixel, x, 10) == 0x0f));
        assert!((4..=10).all(|x| vram.read(PlaneGroup::Pixel, x, 13) == 0));
        assert_eq!(rex.read_drawing(YSTARTI), 9);
    }

    #[test]
    fn writing_xstart_restarts_a_continuing_pattern_at_the_new_origin() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x0f);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSAVE, 4);
        rex.write_drawing(XSTARTI, 13);
        rex.write_drawing(YSTARTI, 12);
        rex.write_drawing(XENDI, 19);
        rex.write_drawing(YENDI, 0);
        rex.write_drawing(COMMAND, 0x3000_05b9);

        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 12), 0);
        assert_eq!(vram.read(PlaneGroup::Pixel, 13, 12), 0x0f);
    }

    #[test]
    fn screen_to_screen_copy_preserves_an_overlapping_console_scroll() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        for (y, value) in [(5, 0x11), (6, 0x22), (7, 0x33)] {
            for x in 2..=4 {
                vram.write_masked(PlaneGroup::Pixel, x, y, value, 0xff);
            }
        }
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 2);
        rex.write_drawing(YSTARTI, 4);
        rex.write_drawing(XENDI, 4);
        rex.write_drawing(YENDI, 6);
        rex.write_drawing(COMMAND, 0x3008_0329);

        // The text port's forward scroll keeps a positive Y offset after it
        // converts the destination rectangle into REX coordinates. Sampling
        // destination Y plus that offset while following the programmed
        // start-to-end order consumes each source row before replacement.
        rex.write_drawing_go(XYMOVE, 1, &mut vram);

        for (y, value) in [(4, 0x11), (5, 0x22), (6, 0x33)] {
            assert!((2..=4).all(|x| vram.read(PlaneGroup::Pixel, x, y) == value));
        }
    }

    #[test]
    fn logic_source_pixels_use_the_common_rop_and_write_mask() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        vram.write_masked(PlaneGroup::Pixel, 5, 5, 0xff, 0xff);
        vram.write_masked(PlaneGroup::Pixel, 4, 4, 0x30, 0xff);
        rex.write_drawing(RWMASK, 0x0f);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 4);
        rex.write_drawing(XENDI, 4);
        rex.write_drawing(YENDI, 4);
        rex.write_drawing(COMMAND, 0x6008_0329);

        rex.write_drawing_go(XYMOVE, 0x0001_0001, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 4), 0x3f);
    }

    #[test]
    fn an_opaque_glyph_writes_the_background_for_cleared_bits() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x0f);
        rex.write_drawing(super::COLORBACK, 0x07);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x3080_05a9);

        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 0), 0x07);
    }

    #[test]
    fn xstate_zopaque_alias_writes_the_background() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x0000_05a9);
        rex.write_drawing(XSTATE, 0x83ff_070f);

        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 0), 0x07);
    }

    #[test]
    fn context_toggle_preserves_shared_xstate_flags() {
        let mut rex = Rex1::new();
        let mut setup_vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x0000_05a9);
        rex.write_drawing(XSTATE, 0x83ff_070f);
        rex.read_drawing_go(COMMAND, &mut setup_vram);

        // Replace the next context with a transparent version, then exchange
        // contexts. The captured context must restore the aliased opaque bit.
        rex.write_drawing(XSTATE, 0x03ff_070f);
        rex.write_config(super::TOGGLECTXT, 0);
        rex.write_drawing(XSAVE, 0);

        let mut vram = Vram::new();
        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x0f);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 0), 0x07);
    }

    #[test]
    fn a_wide_glyph_advances_only_after_the_block_fragment() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(COLORREDI, 0x21);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 4);
        rex.write_drawing(YSTARTI, 12);
        rex.write_drawing(YENDI, 0);

        // The firmware omits BLOCK from every complete sixteen-pixel fragment
        // so reaching the fragment's XEND continues on the same scan line.
        rex.write_drawing(COMMAND, 0x3000_05a1);
        rex.write_drawing(XENDI, 19);
        rex.write_drawing_go(ZPATTERN, 0xffff_0000, &mut vram);
        assert!((4..=19).all(|x| vram.read(PlaneGroup::Pixel, x, 12) == 0x21));
        assert_eq!(rex.read_drawing(YSTARTI), 12);

        // BLOCK marks the final fragment, which completes the scan line and
        // restores the saved left edge before advancing toward YEND.
        rex.write_drawing(COMMAND, 0x3000_05a9);
        rex.write_drawing(XENDI, 27);
        rex.write_drawing_go(ZPATTERN, 0xff00_0000, &mut vram);
        assert!((20..=27).all(|x| vram.read(PlaneGroup::Pixel, x, 12) == 0x21));
        assert_eq!(rex.read_drawing(YSTARTI), 11);

        rex.write_drawing(COMMAND, 0x3000_05a1);
        rex.write_drawing(XENDI, 19);
        rex.write_drawing_go(ZPATTERN, 0x8000_0000, &mut vram);
        assert_eq!(vram.read(PlaneGroup::Pixel, 4, 11), 0x21);
    }

    #[test]
    fn packed_pixel_writes_cover_four_pixels_per_host_word() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 1023);
        // Command bit 21 and XSTATE bit 28 are the same auxiliary color flag,
        // which takes the source from the host data register. XSTATE follows
        // the command so it supplies the shared logic operation, matching the
        // order the PROM uses when it clears the screen.
        rex.write_drawing(COMMAND, 0x0020_01a1);
        rex.write_drawing(XSTATE, 0x13ff_0000);

        rex.write_drawing_go(RWAUX1, 0x0102_0304, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 8, 3), 1);
        assert_eq!(vram.read(PlaneGroup::Pixel, 9, 3), 2);
        assert_eq!(vram.read(PlaneGroup::Pixel, 10, 3), 3);
        assert_eq!(vram.read(PlaneGroup::Pixel, 11, 3), 4);
    }

    #[test]
    fn rgb_host_pixel_writes_traverse_a_descending_rectangle() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 4);
        rex.write_drawing(XENDI, 9);
        rex.write_drawing(YENDI, 3);
        rex.write_drawing(COMMAND, 0x3024_c089);

        rex.write_drawing_go(RWAUX1, 0x00ff_0000, &mut vram);
        rex.write_drawing_go(RWAUX1, 0x0000_ff00, &mut vram);
        rex.write_drawing_go(RWAUX1, 0x0000_00ff, &mut vram);
        rex.write_drawing_go(RWAUX1, 0x00ff_ffff, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 8, 4), 0xe0);
        assert_eq!(vram.read(PlaneGroup::Pixel, 9, 4), 0x07);
        assert_eq!(vram.read(PlaneGroup::Pixel, 8, 3), 0x18);
        assert_eq!(vram.read(PlaneGroup::Pixel, 9, 3), 0xff);
        assert_eq!(rex.next.xsave, 8);
        assert_eq!(rex.next.ystart >> COORDINATE_FRACTION_BITS, 3);
    }

    #[test]
    fn rgb_host_pixels_use_window_relative_scaled_bayer_dither() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(AUX1, 0x200);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(XENDI, 3);
        rex.write_drawing(YENDI, 0);
        rex.write_drawing(COMMAND, 0x3024_c089);

        for _ in 0..4 {
            rex.write_drawing_go(RWAUX1, 0x0080_8080, &mut vram);
        }

        assert_eq!(
            (0..=3)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 0))
                .collect::<Vec<_>>(),
            vec![0x94, 0x6b, 0x94, 0x6b]
        );
    }

    #[test]
    fn one_packed_tile_word_repeats_across_each_programmed_scan_line() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 13);
        rex.write_drawing(COMMAND, 0x0020_0121);
        rex.write_drawing(XSTATE, 0x13ff_0000);

        rex.write_drawing_go(RWAUX1, 0x0102_0304, &mut vram);

        assert_eq!(
            (8..=13)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 3))
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 1, 2]
        );

        rex.write_drawing(YSTARTI, 4);
        rex.write_drawing_go(RWAUX1, 0x090a_0b0c, &mut vram);

        assert_eq!(
            (8..=11)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 4))
                .collect::<Vec<_>>(),
            vec![9, 10, 11, 12]
        );
    }

    #[test]
    fn xstate_coloraux_alias_selects_packed_host_pixels() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x0000_0121);
        rex.write_drawing(XSTATE, 0x13ff_0000);

        rex.write_drawing_go(RWAUX1, 0x0102_0304, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Pixel, 8, 3), 1);
        assert_eq!(vram.read(PlaneGroup::Pixel, 9, 3), 2);
        assert_eq!(vram.read(PlaneGroup::Pixel, 10, 3), 3);
        assert_eq!(vram.read(PlaneGroup::Pixel, 11, 3), 4);
    }

    #[test]
    fn packed_pixel_reads_latch_four_pixels_for_the_host() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x0020_01a1);
        rex.write_drawing(XSTATE, 0x13ff_0000);
        rex.write_drawing_go(RWAUX1, 0x0102_0304, &mut vram);

        // The read-back command sets XYCONTINUE, so it resumes from the saved
        // column rather than the start coordinate. Rewinding that column is
        // what lets the guest read the span it just wrote.
        rex.write_drawing(XSAVE, 8);
        rex.write_drawing(COMMAND, 0x0020_00ab);

        assert_eq!(rex.read_drawing_go(RWAUX1, &mut vram), 0x0102_0304);
    }

    #[test]
    fn rewriting_xstart_redirects_a_packed_host_pixel_stream() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 8);
        rex.write_drawing(YSTARTI, 3);
        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0x0020_0121);
        rex.write_drawing(XSTATE, 0x13ff_0000);
        rex.write_drawing_go(RWAUX1, 0x0102_0304, &mut vram);
        // Writing XSTART also updates XSAVE, so the second access begins at
        // the newly programmed column rather than continuing from twelve.
        rex.write_drawing(XSTARTI, 20);
        rex.write_drawing_go(RWAUX1, 0x0506_0708, &mut vram);

        assert_eq!(
            (8..=11)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 3))
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            (20..=23)
                .map(|x| vram.read(PlaneGroup::Pixel, x, 3))
                .collect::<Vec<_>>(),
            vec![5, 6, 7, 8]
        );
    }

    #[test]
    fn color_comparison_rejects_pixels_outside_the_selected_relation() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 0);
        rex.write_drawing(YSTARTI, 0);
        rex.write_drawing(COLORREDI, 0x40);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        // Greater-than comparison with a smaller source leaves the pixel.
        rex.write_drawing(super::AUX1, super::AUX1_COLORCOMPGT);
        rex.write_drawing(COLORREDI, 0x20);
        rex.write_drawing_go(COMMAND, 0x3001_0001, &mut vram);
        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x40);

        // A larger source satisfies the same comparison.
        rex.write_drawing(COLORREDI, 0x60);
        rex.write_drawing_go(COMMAND, 0x3001_0001, &mut vram);
        assert_eq!(vram.read(PlaneGroup::Pixel, 0, 0), 0x60);
    }

    #[test]
    fn drawing_reaches_the_plane_group_selected_by_aux2() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        rex.write_config(XYWIN, 0x0800_0800);
        rex.write_drawing(COLORREDI, 0x03);
        rex.write_drawing(RWMASK, 0xff);
        rex.write_drawing(XSTARTI, 1);
        rex.write_drawing(YSTARTI, 1);

        rex.write_config(AUX2, 0x4000_0000);
        rex.write_drawing_go(COMMAND, 0x3000_0001, &mut vram);

        assert_eq!(vram.read(PlaneGroup::Overlay, 1, 1), 0x03);
        assert_eq!(vram.read(PlaneGroup::Pixel, 1, 1), 0);
    }

    #[test]
    fn a_go_nop_latches_aux2_for_set_readback() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();

        rex.write_config(AUX2, 0x2000_0000);
        assert_eq!(rex.read_config(AUX2).0, 0);

        rex.write_drawing_go(COMMAND, 0, &mut vram);

        assert_eq!(rex.read_config(AUX2).0, 0x2000_0000);
    }

    #[test]
    fn peripheral_ports_report_the_transfer_the_board_must_perform() {
        let mut rex = Rex1::new();

        assert_eq!(
            rex.write_config(super::RWDAC, 0xa5),
            ConfigAccess::Write(PeripheralPort::Dac, 0xa5)
        );
        assert_eq!(
            rex.write_config(super::RWVC1, 0x5a),
            ConfigAccess::Write(PeripheralPort::Vc1, 0x5a)
        );
        assert_eq!(
            rex.read_config(super::RWDAC).1,
            ConfigAccess::Read(PeripheralPort::Dac)
        );
        assert_eq!(
            rex.read_config(super::RWVC1).1,
            ConfigAccess::Read(PeripheralPort::Vc1)
        );
        assert_eq!(rex.read_config(AUX2).1, ConfigAccess::None);
    }

    #[test]
    fn configuration_mode_reads_report_an_idle_chip() {
        let mut rex = Rex1::new();

        rex.write_config(super::CONFIGMODE, 0x807f_e000);

        assert_eq!(rex.read_config(super::CONFIGMODE).0, 0);
    }

    #[test]
    fn the_configuration_selector_keeps_three_bits() {
        let mut rex = Rex1::new();

        rex.write_config(super::CONFIGSEL, 0xffff_fff9);

        assert_eq!(rex.read_config(super::CONFIGSEL).0, 1);
    }

    #[test]
    fn the_pad_word_stores_without_side_effects() {
        let mut rex = Rex1::new();
        let before = rex.clone();

        rex.write_dummy(0);

        assert_eq!(rex.read_dummy(), 0);
        assert_eq!(rex.current, before.current);
        assert_eq!(rex.next, before.next);
    }

    #[test]
    fn slope_registers_use_sign_and_magnitude_encoding() {
        // A positive write passes through the diagnostic read-back formula.
        assert_eq!(minor_slope(0x0000_1234), 0x0000_1234);
        assert_eq!(minor_slope(0x0001_ffff), 0x0001_ffff);

        // The sign bit selects a negation rather than a two's complement
        // value, so the read-back differs from ordinary signed storage.
        assert_eq!(minor_slope(0x8000_1234), 0x0001_edcc);

        // The color formula duplicates bit nineteen into bit twenty.
        assert_eq!(color_slope(0x0010_0000), 0x0010_0000);
        assert_eq!(color_slope(0x0008_0000), 0x0018_0000);
        assert_eq!(color_slope(0x000f_ffff), 0x001f_ffff);

        let positive = color_slope((4096.0_f32 + 1.0).to_bits());
        let negative = color_slope((-4096.0_f32 - 1.0).to_bits());
        assert_eq!(signed_color_slope(positive), 1 << 11);
        assert_eq!(signed_color_slope(negative), -(1 << 11));
    }

    #[test]
    fn integer_and_full_coordinate_views_share_one_register() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();

        rex.write_drawing(XSTARTI, 100);
        rex.write_drawing(COMMAND, 0);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(rex.read_drawing(XSTARTI), 100);
        assert_eq!(rex.read_drawing(XSTART), 100 << 15);
    }

    #[test]
    fn end_coordinate_views_convert_between_integer_and_fraction() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();

        rex.write_drawing(XENDI, 1023);
        rex.write_drawing(COMMAND, 0);
        rex.read_drawing_go(COMMAND, &mut vram);

        assert_eq!(rex.read_drawing(XENDI), 1023);
        assert_eq!(rex.read_drawing(super::XENDF), 1023 << 11);
    }

    #[test]
    fn reset_clears_both_contexts_and_configuration() {
        let mut rex = Rex1::new();
        let mut vram = Vram::new();
        select_pixel_planes(&mut rex);
        rex.write_drawing(XSTARTI, 5);
        rex.write_drawing_go(COMMAND, 0, &mut vram);

        rex.reset();

        assert_eq!(rex.read_drawing(XSTARTI), 0);
        assert_eq!(rex.read_config(AUX2).0, 0);
    }
}

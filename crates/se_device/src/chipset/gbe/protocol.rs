//! Owned protocols emitted and accepted by the Graphics Back End.

use se_core::component::ComponentId;
use se_core::scheduler::{SimDuration, SimTime};
use se_core::tracing::OwnedTraceEvent;

use crate::bus::two_wire::TwoWireDrive;
use crate::chipset::crime::protocol::{CrimeCgiCompletion, CrimeCgiTransaction};

/// Board wiring used by GBE's controller roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GbeWiring {
    /// CRIME component accepting DMA and interrupt transactions.
    pub crime: ComponentId,

    /// CRT DDC open-drain bus.
    pub crt_ddc: ComponentId,

    /// Flat-panel DDC open-drain bus.
    pub flat_panel_ddc: ComponentId,

    /// Board-level input levels for the ten bidirectional auxiliary pins.
    pub auxiliary_inputs: [bool; 10],
}

/// Externally supplied pixel-clock source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GbeExternalClock {
    /// Single-ended TTL pixel clock.
    Ttl,

    /// Differential pixel clock.
    Differential,
}

/// Host-neutral external input accepted by GBE.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GbeExternalInput {
    /// Active-low monitor sense input level.
    SenseN(bool),

    /// Frame-lock input level.
    FrameLock(bool),

    /// Configures one exact external pixel-clock frequency.
    PixelClock {
        /// Selected input.
        source: GbeExternalClock,

        /// Frequency numerator in hertz.
        numerator_hz: u64,

        /// Frequency denominator.
        denominator: u64,
    },

    /// Disconnects one external pixel-clock input.
    DisconnectPixelClock(GbeExternalClock),

    /// Replaces the board-level input levels for all auxiliary pins.
    Auxiliary([bool; 10]),
}

/// Software-observable digital GBE output pins.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GbeOutputPins {
    /// CRT horizontal synchronization level.
    pub crt_hsync: bool,

    /// CRT vertical synchronization level.
    pub crt_vsync: bool,

    /// CRT blanking level.
    pub crt_blank: bool,

    /// Flat-panel horizontal drive level.
    pub flat_panel_hdrive: bool,

    /// Flat-panel vertical drive level.
    pub flat_panel_vdrive: bool,

    /// Flat-panel data-enable level.
    pub flat_panel_data_enable: bool,

    /// Field-to-reference-frame output level.
    pub f2rf: bool,

    /// Two-bit auxiliary WID output.
    pub aux: u8,

    /// General-purpose output drives in register order, or `None` when high impedance.
    pub gpio: [Option<bool>; 10],
}

/// Field phase associated with one published display frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GbeFrameField {
    /// Non-interlaced output.
    Progressive,

    /// First interlaced field.
    First,

    /// Second interlaced field.
    Second,
}

/// Selects how raster content for published display frames is produced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GbeRasterMode {
    /// Fetches and converts pixels through the modeled DMA pipeline every frame.
    CycleAccurate,

    /// Runs scanout timing only; frame content is composed on demand from
    /// memory contents when a consumer actually observes it.
    #[default]
    OnDemand,
}

/// Timing-side announcement of one completed display frame.
///
/// Announced frames carry no pixel content; content is composed on demand from
/// the register and memory state associated with the announcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GbeFrameMeta {
    /// Monotonic frame sequence.
    pub sequence: u64,

    /// Simulation time at completion.
    pub completed_at: SimTime,

    /// Visible width in pixels.
    pub width: u32,

    /// Visible height in pixels.
    pub height: u32,

    /// Captured field phase.
    pub field: GbeFrameField,
}

/// Completed host-neutral RGBA8888 display frame.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GbeFrame {
    /// Monotonic frame sequence.
    pub sequence: u64,

    /// Simulation time at completion.
    pub completed_at: SimTime,

    /// Visible width in pixels.
    pub width: u32,

    /// Visible height in pixels.
    pub height: u32,

    /// Bytes between adjacent rows.
    pub stride: u32,

    /// Captured field phase.
    pub field: GbeFrameField,

    /// Row-major RGBA8888 bytes.
    pub rgba: Vec<u8>,
}

/// Scheduled internal GBE transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum GbeEvent {
    /// Reaches the next scanline boundary.
    Scanline {
        /// Timing epoch that armed the event.
        epoch: u64,
    },

    /// Drains one color-map FIFO entry in an enabled write window.
    ColorMapDrain {
        /// Timing epoch that armed the event.
        epoch: u64,
    },
}

/// Action emitted while polling GBE.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[allow(clippy::large_enum_variant)]
pub enum GbeAction {
    /// Schedules one internal transition.
    Schedule {
        /// Delay from the current machine time.
        delay: SimDuration,

        /// Scheduled event.
        event: GbeEvent,
    },

    /// Starts one GBE-controller CGI transaction.
    StartCgi(CrimeCgiTransaction),

    /// Changes GBE's drive state on one DDC bus.
    SetDdc {
        /// Destination two-wire bus.
        bus: ComponentId,

        /// New open-drain output state.
        drive: TwoWireDrive,
    },

    /// Completes a deferred CRIME-controller PIO transaction.
    CompleteCgiDevice(CrimeCgiCompletion),

    /// Publishes one completed display frame.
    PublishFrame(GbeFrame),

    /// Announces one completed display frame whose content is composed on demand.
    AnnounceFrame(GbeFrameMeta),

    /// Emits structured diagnostic information.
    Trace(Box<OwnedTraceEvent>),
}

/// Result of polling GBE.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[allow(clippy::large_enum_variant)]
pub enum GbePoll {
    /// One action is ready.
    Action(GbeAction),

    /// No action is ready.
    Idle,
}

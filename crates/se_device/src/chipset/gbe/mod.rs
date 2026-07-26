//! SGI Graphics Back End Revision 1.1 digital functional model.

pub mod protocol;

pub(crate) mod display;
mod registers;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::scheduler::{RationalClockProjection, SimDuration, SimTime};
use se_core::tracing::{OwnedTraceEvent, OwnedTraceField, OwnedTraceValue, TraceLevel};

use crate::bus::two_wire::{TwoWireDrive, TwoWireLineDelivery};
use crate::chipset::crime::protocol::{
    CrimeBusError, CrimeByteEnable, CrimeCgiCompletion, CrimeCgiTransaction,
    CrimeCompletionPayload, CrimeData, CrimeDmaRequest, CrimeInterruptPost,
    CrimeLinkDeviceResponse, CrimeLinkOperation, CrimePioRequest, CrimeTransactionId,
    CrimeTransfer, CrimeTransferView,
};

use self::display::{
    PlaneDepth, color_from_normal, color_from_overlay, cursor_color, decode_raw_pixels_into,
    did_block_for_line, did_for_pixel, filter_fullscreen_rgba, visible_dimensions,
};
use self::protocol::{
    GbeAction, GbeEvent, GbeExternalClock, GbeExternalInput, GbeFrame, GbeFrameField,
    GbeOutputPins, GbePoll, GbeWiring,
};
use self::registers::{
    AUXILIARY_PIN_COUNT, COLOR_MAP_FIFO, DOT_CLOCK_RUN, GbeRegisters, GbeRegistersState,
    RegisterWrite, VT_F2RF_LOCK, VT_FLAGS, VT_HBLANK, VT_HCMAP, VT_HPIXEN, VT_HSYNC, VT_INTR01,
    VT_INTR23, VT_VBLANK, VT_VCMAP, VT_VPIXEN, VT_VSYNC, VT_XY, VT_XY_FREEZE, VT_XY_MAX,
    auxiliary_data_mask, auxiliary_output_disable_mask,
};

const PIXEL_REFERENCE_CLOCK_HZ: u64 = 20_000_000;
const DMA_ALIGNMENT: u64 = 32;
const DMA_PAGE_SIZE: u64 = 8 * 1_024;
const PIXEL_BURST_SIZE: usize = 512;
const MAX_DMA_READS: usize = 8;
const MAX_DMA_WRITES: usize = 1;
const COLOR_MAP_FIFO_CAPACITY: usize = 64;

/// Immutable GBE register value captured for a bounded synchronous read batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GbeSynchronousReadProjection {
    /// Physical address of the projected register.
    pub physical_address: u64,

    /// Big-endian register value.
    pub value: u32,
}

impl GbeSynchronousReadProjection {
    /// Completes the projected aligned word read.
    pub fn read(self, physical_address: u64, length: u16) -> Option<CrimeData> {
        (physical_address == self.physical_address && length == 4)
            .then(|| self.value.to_be_bytes().into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct ExternalClockState {
    numerator_hz: u64,
    denominator: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct ColorMapWrite {
    index: u16,
    value: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct DeferredColorMapWrite {
    controller: ComponentId,
    id: CrimeTransactionId,
    write: ColorMapWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum Plane {
    Normal,
    Overlay,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum DmaDestination {
    TilePointers(Plane),
    DidFrame,
    DidBlock(u8),
    Line {
        frame: u64,
        y: u16,
        plane: Plane,
        segment: u16,
    },
    Capture,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct DmaJob {
    address: u64,
    transfer: CrimeTransfer,
    destination: DmaDestination,
    destination_offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PendingDma {
    destination: DmaDestination,
    destination_offset: usize,
    read_order: Option<u64>,
    write: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct CompletedRead {
    pending: PendingDma,
    completion: CrimeCgiCompletion,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct LineFetch {
    normal: PlaneLineFetch,
    overlay: PlaneLineFetch,
}

impl LineFetch {
    fn new() -> Self {
        Self {
            normal: PlaneLineFetch::default(),
            overlay: PlaneLineFetch::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PlaneLineFetch {
    segments: Vec<LineSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct LineSegment {
    data: Vec<u8>,
    source_pixel: usize,
    output_pixel: usize,
    pixels: usize,
    remaining: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct WorkingFrame {
    sequence: u64,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

/// SGI Graphics Back End connected bidirectionally to the CRIME CGI link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gbe {
    id: ComponentId,
    name: String,
    wiring: GbeWiring,
    registers: GbeRegisters,
    timebase_hz: u64,
    observed_time: SimTime,
    scan_origin_time: SimTime,
    scan_origin_pixel: u64,
    pixel_clock_remainder: u64,
    timing_epoch: u64,
    scheduled_scanline_cycles: Option<u64>,
    ttl_clock: Option<ExternalClockState>,
    differential_clock: Option<ExternalClockState>,
    sense_n: bool,
    frame_lock: bool,
    frame_lock_pending: bool,
    frame_lock_last_rising: Option<SimTime>,
    frame_lock_period_ticks: Option<u64>,
    f2rf_level: bool,
    ddc_clock_high: [bool; 2],
    ddc_data_high: [bool; 2],
    auxiliary_inputs: [bool; AUXILIARY_PIN_COUNT],
    color_map_fifo: VecDeque<ColorMapWrite>,
    deferred_color_map: VecDeque<DeferredColorMapWrite>,
    color_map_drain_scheduled: bool,
    actions: VecDeque<GbeAction>,
    next_transaction: u128,
    next_read_order: u64,
    next_read_commit: u64,
    pending_dma: VecDeque<DmaJob>,
    outstanding_dma: BTreeMap<CrimeTransactionId, PendingDma>,
    outstanding_interrupt_posts: BTreeSet<CrimeTransactionId>,
    completed_reads: BTreeMap<u64, CompletedRead>,
    reads_in_flight: usize,
    writes_in_flight: usize,
    normal_tile_pointers: Vec<u8>,
    overlay_tile_pointers: Vec<u8>,
    did_frame_table: Vec<u8>,
    did_frame_chunks_remaining: usize,
    did_line_blocks: BTreeMap<u8, Vec<u8>>,
    line_fetches: BTreeMap<(u64, u16), LineFetch>,
    working_frame: Option<WorkingFrame>,
    next_frame_sequence: u64,
    interrupt_posted: [bool; 4],
    capture_writes_remaining: usize,
}

/// Serializable dynamic state of the Graphics Back End.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GbeState {
    id: ComponentId,
    wiring: GbeWiring,
    registers: GbeRegistersState,
    timebase_hz: u64,
    observed_time: SimTime,
    scan_origin_time: SimTime,
    scan_origin_pixel: u64,
    pixel_clock_remainder: u64,
    timing_epoch: u64,
    scheduled_scanline_cycles: Option<u64>,
    ttl_clock: Option<ExternalClockState>,
    differential_clock: Option<ExternalClockState>,
    sense_n: bool,
    frame_lock: bool,
    frame_lock_pending: bool,
    frame_lock_last_rising: Option<SimTime>,
    frame_lock_period_ticks: Option<u64>,
    f2rf_level: bool,
    ddc_clock_high: [bool; 2],
    ddc_data_high: [bool; 2],
    auxiliary_inputs: [bool; AUXILIARY_PIN_COUNT],
    color_map_fifo: VecDeque<ColorMapWrite>,
    deferred_color_map: VecDeque<DeferredColorMapWrite>,
    color_map_drain_scheduled: bool,
    actions: VecDeque<GbeAction>,
    next_transaction: u128,
    next_read_order: u64,
    next_read_commit: u64,
    pending_dma: VecDeque<DmaJob>,
    outstanding_dma: BTreeMap<CrimeTransactionId, PendingDma>,
    outstanding_interrupt_posts: BTreeSet<CrimeTransactionId>,
    completed_reads: BTreeMap<u64, CompletedRead>,
    reads_in_flight: usize,
    writes_in_flight: usize,
    normal_tile_pointers: Vec<u8>,
    overlay_tile_pointers: Vec<u8>,
    did_frame_table: Vec<u8>,
    did_frame_chunks_remaining: usize,
    did_line_blocks: BTreeMap<u8, Vec<u8>>,
    line_fetches: BTreeMap<(u64, u16), LineFetch>,
    working_frame: Option<WorkingFrame>,
    next_frame_sequence: u64,
    interrupt_posted: [bool; 4],
    capture_writes_remaining: usize,
}

impl GbeState {
    fn invalid(component: ComponentId, invariant: &'static str) -> ComponentStateError {
        ComponentStateError::InvalidState {
            component,
            invariant,
        }
    }

    fn pixel_frequency_numerator(&self) -> Option<u64> {
        match (self.registers.control_status >> 28) & 3 {
            0 => self.ttl_clock.map(|clock| clock.numerator_hz),
            1 => self.differential_clock.map(|clock| clock.numerator_hz),
            2 => None,
            _ if self.registers.dot_clock & DOT_CLOCK_RUN != 0 => {
                let multiplier = u64::from((self.registers.dot_clock & 0xff) + 1);
                Some(PIXEL_REFERENCE_CLOCK_HZ * multiplier)
            }
            _ => None,
        }
    }

    fn validate_frame(frame: &GbeFrame, observed_time: SimTime) -> bool {
        let Some(stride) = frame.width.checked_mul(4) else {
            return false;
        };
        let Some(bytes) = usize::try_from(stride)
            .ok()
            .and_then(|stride| stride.checked_mul(frame.height as usize))
        else {
            return false;
        };
        frame.width <= 4_096
            && frame.height <= 4_096
            && frame.stride == stride
            && frame.rgba.len() == bytes
            && frame.completed_at <= observed_time
    }

    fn destination_bounds(
        &self,
        destination: &DmaDestination,
        offset: usize,
        length: usize,
    ) -> bool {
        let within = |target_length: usize| {
            offset <= target_length
                && offset
                    .checked_add(length)
                    .is_some_and(|end| end <= target_length)
        };
        match destination {
            DmaDestination::TilePointers(Plane::Normal) => within(self.normal_tile_pointers.len()),
            DmaDestination::TilePointers(Plane::Overlay) => {
                within(self.overlay_tile_pointers.len())
            }
            DmaDestination::DidFrame => within(self.did_frame_table.len()),
            DmaDestination::DidBlock(block) => self
                .did_line_blocks
                .get(block)
                .is_none_or(|target| within(target.len())),
            DmaDestination::Line {
                frame,
                y,
                plane,
                segment,
            } => self
                .line_fetches
                .get(&(*frame, *y))
                .and_then(|fetch| match plane {
                    Plane::Normal => fetch.normal.segments.get(usize::from(*segment)),
                    Plane::Overlay => fetch.overlay.segments.get(usize::from(*segment)),
                })
                .is_none_or(|target| within(target.data.len())),
            DmaDestination::Capture => true,
        }
    }

    fn validate_dma_job(&self, job: &DmaJob) -> bool {
        if !job.address.is_multiple_of(DMA_ALIGNMENT) {
            return false;
        }
        let (length, write) = match job.transfer.view() {
            CrimeTransferView::Read { length } => (usize::from(length), false),
            CrimeTransferView::Write { data, byte_enable } => {
                if data.len() != byte_enable.len() || byte_enable.iter().any(|enabled| !enabled) {
                    return false;
                }
                (data.len(), true)
            }
        };
        if length == 0
            || length > PIXEL_BURST_SIZE
            || !length.is_multiple_of(DMA_ALIGNMENT as usize)
            || job
                .address
                .checked_add(length as u64 - 1)
                .is_none_or(|end| job.address / DMA_PAGE_SIZE != end / DMA_PAGE_SIZE)
            || write != matches!(job.destination, DmaDestination::Capture)
            || !matches!(job.destination, DmaDestination::Line { .. })
                && !write
                && length != DMA_ALIGNMENT as usize
        {
            return false;
        }
        self.destination_bounds(&job.destination, job.destination_offset, length)
    }

    fn validate(&self, component: ComponentId) -> Result<(), ComponentStateError> {
        if let Err(invariant) = GbeRegisters::from_state(self.registers.clone()).validate_state() {
            return Err(Self::invalid(component, invariant));
        }
        if self.timebase_hz == 0
            || self.scan_origin_time > self.observed_time
            || self
                .frame_lock_last_rising
                .is_some_and(|time| time > self.observed_time)
            || self.frame_lock_period_ticks == Some(0)
        {
            return Err(Self::invalid(
                component,
                "GBE timebase, timing anchors, and frame-lock period must be valid",
            ));
        }
        if [self.ttl_clock, self.differential_clock]
            .into_iter()
            .flatten()
            .any(|clock| clock.numerator_hz == 0 || clock.denominator == 0)
        {
            return Err(Self::invalid(
                component,
                "GBE external clock numerator and denominator must be nonzero",
            ));
        }
        let pixel_frequency = self.pixel_frequency_numerator();
        if match pixel_frequency {
            Some(frequency) => self.pixel_clock_remainder >= frequency,
            None => self.pixel_clock_remainder != 0,
        } {
            return Err(Self::invalid(
                component,
                "GBE pixel-clock remainder must be normalized",
            ));
        }
        let scan_width = u64::from((self.registers.vt[VT_XY_MAX] & 0x0fff) + 1);
        let scan_height = u64::from(((self.registers.vt[VT_XY_MAX] >> 12) & 0x0fff) + 1);
        let frame_pixels = scan_width * scan_height;
        if self.scan_origin_pixel >= frame_pixels
            || self.scheduled_scanline_cycles.is_some_and(|cycles| {
                pixel_frequency.is_none() || cycles == 0 || cycles > scan_width
            })
            || self.color_map_drain_scheduled
                && (pixel_frequency.is_none() || self.color_map_fifo.is_empty())
        {
            return Err(Self::invalid(
                component,
                "GBE scan origin and scheduled timing work must be in range",
            ));
        }
        if (self.registers.control_status & (1 << 4) != 0) != self.sense_n {
            return Err(Self::invalid(
                component,
                "GBE monitor sense register must match the external input",
            ));
        }
        let color_write_valid = |write: &ColorMapWrite| {
            usize::from(write.index) < self.registers.color_map.len()
                && write.value & !0xffff_ff00 == 0
        };
        if self.color_map_fifo.len() > COLOR_MAP_FIFO_CAPACITY
            || self
                .color_map_fifo
                .iter()
                .any(|write| !color_write_valid(write))
            || self.deferred_color_map.iter().any(|deferred| {
                deferred.controller != self.wiring.crime || !color_write_valid(&deferred.write)
            })
        {
            return Err(Self::invalid(
                component,
                "GBE color-map queues must fit capacity and contain valid writes",
            ));
        }
        if self
            .pending_dma
            .iter()
            .any(|job| !self.validate_dma_job(job))
        {
            return Err(Self::invalid(
                component,
                "GBE queued DMA transfers must have valid type, shape, and destination bounds",
            ));
        }
        if self.outstanding_dma.values().any(|pending| {
            pending.write == pending.read_order.is_some()
                || !self.destination_bounds(&pending.destination, pending.destination_offset, 0)
        }) {
            return Err(Self::invalid(
                component,
                "GBE outstanding DMA type and read order must agree",
            ));
        }
        if self.completed_reads.iter().any(|(order, completed)| {
            completed.pending.write
                || completed.pending.read_order != Some(*order)
                || !self.destination_bounds(
                    &completed.pending.destination,
                    completed.pending.destination_offset,
                    match &completed.completion.result {
                        Ok(CrimeCompletionPayload::ReadData(data)) => data.len(),
                        _ => 0,
                    },
                )
                || matches!(
                    &completed.completion.result,
                    Ok(CrimeCompletionPayload::ReadData(data))
                        if data.is_empty()
                            || data.len() > PIXEL_BURST_SIZE
                            || !data.len().is_multiple_of(DMA_ALIGNMENT as usize)
                )
        }) {
            return Err(Self::invalid(
                component,
                "GBE completed reads must retain their order and destination bounds",
            ));
        }
        let outstanding_reads = self
            .outstanding_dma
            .values()
            .filter(|pending| !pending.write)
            .count();
        let outstanding_writes = self
            .outstanding_dma
            .values()
            .filter(|pending| pending.write)
            .count();
        if self.reads_in_flight != outstanding_reads
            || self.writes_in_flight != outstanding_writes
            || self.reads_in_flight > MAX_DMA_READS
            || self.writes_in_flight > MAX_DMA_WRITES
        {
            return Err(Self::invalid(
                component,
                "GBE in-flight DMA counters must match the outstanding transaction table",
            ));
        }
        let mut read_orders = BTreeSet::new();
        for pending in self
            .outstanding_dma
            .values()
            .filter(|pending| !pending.write)
        {
            if !read_orders.insert(pending.read_order.expect("read order was validated")) {
                return Err(Self::invalid(
                    component,
                    "GBE outstanding read orders must be unique",
                ));
            }
        }
        for order in self.completed_reads.keys() {
            if !read_orders.insert(*order) {
                return Err(Self::invalid(
                    component,
                    "GBE outstanding and completed read orders must be unique",
                ));
            }
        }
        let read_span = self.next_read_order.wrapping_sub(self.next_read_commit);
        if u64::try_from(read_orders.len()) != Ok(read_span)
            || read_orders
                .iter()
                .any(|order| order.wrapping_sub(self.next_read_commit) >= read_span)
        {
            return Err(Self::invalid(
                component,
                "GBE read commit cursor must cover every uncommitted read exactly once",
            ));
        }
        let mut transaction_ids: BTreeSet<_> = self.outstanding_dma.keys().copied().collect();
        if transaction_ids.len() != self.outstanding_dma.len()
            || self
                .outstanding_interrupt_posts
                .iter()
                .any(|id| !transaction_ids.insert(*id))
            || self
                .completed_reads
                .values()
                .any(|completed| !transaction_ids.insert(completed.completion.id))
            || transaction_ids
                .iter()
                .any(|id| id.get() == self.next_transaction)
        {
            return Err(Self::invalid(
                component,
                "GBE active transaction identifiers must be unique",
            ));
        }
        let capture_jobs = self
            .pending_dma
            .iter()
            .filter(|job| matches!(job.destination, DmaDestination::Capture))
            .count()
            + self
                .outstanding_dma
                .values()
                .filter(|pending| matches!(pending.destination, DmaDestination::Capture))
                .count();
        if self.capture_writes_remaining != capture_jobs {
            return Err(Self::invalid(
                component,
                "GBE capture write count must match pending and outstanding DMA work",
            ));
        }
        let did_frame_jobs = self
            .pending_dma
            .iter()
            .filter(|job| matches!(job.destination, DmaDestination::DidFrame))
            .count()
            + self
                .outstanding_dma
                .values()
                .filter(|pending| matches!(pending.destination, DmaDestination::DidFrame))
                .count()
            + self
                .completed_reads
                .values()
                .filter(|completed| {
                    matches!(completed.pending.destination, DmaDestination::DidFrame)
                })
                .count();
        if self.did_frame_chunks_remaining != did_frame_jobs
            || !matches!(self.did_frame_table.len(), 0 | 256)
            || self
                .did_line_blocks
                .values()
                .any(|block| block.len() != PIXEL_BURST_SIZE)
            || !self
                .normal_tile_pointers
                .len()
                .is_multiple_of(DMA_ALIGNMENT as usize)
            || !self
                .overlay_tile_pointers
                .len()
                .is_multiple_of(DMA_ALIGNMENT as usize)
        {
            return Err(Self::invalid(
                component,
                "GBE DID and tile-pointer buffers must match outstanding DMA chunks",
            ));
        }
        if let Some(frame) = &self.working_frame {
            let Some(bytes) = frame
                .width
                .checked_mul(frame.height)
                .and_then(|pixels| pixels.checked_mul(4))
            else {
                return Err(Self::invalid(
                    component,
                    "GBE working frame dimensions must not overflow",
                ));
            };
            if frame.width == 0
                || frame.height == 0
                || frame.width > 4_096
                || frame.height > 4_096
                || frame.rgba.len() != bytes
                || self.next_frame_sequence.wrapping_sub(frame.sequence) != 1
            {
                return Err(Self::invalid(
                    component,
                    "GBE working frame dimensions and RGBA length must agree",
                ));
            }
        } else if !self.line_fetches.is_empty() {
            return Err(Self::invalid(
                component,
                "GBE line fetches require a working frame",
            ));
        }
        for ((sequence, y), fetch) in &self.line_fetches {
            let Some(frame) = self.working_frame.as_ref() else {
                unreachable!("line fetch ownership was validated");
            };
            if *sequence != frame.sequence || usize::from(*y) >= frame.height {
                return Err(Self::invalid(
                    component,
                    "GBE line fetch keys must identify the working frame",
                ));
            }
            for (plane, segments) in [
                (Plane::Normal, &fetch.normal.segments),
                (Plane::Overlay, &fetch.overlay.segments),
            ] {
                if segments.len() > usize::from(u16::MAX) {
                    return Err(Self::invalid(
                        component,
                        "GBE line segment index must fit its DMA destination",
                    ));
                }
                let depth = match plane {
                    Plane::Normal => PlaneDepth::from_frame_register(self.registers.frame[0]),
                    Plane::Overlay => PlaneDepth::Eight,
                };
                for (index, segment) in segments.iter().enumerate() {
                    let destination = DmaDestination::Line {
                        frame: *sequence,
                        y: *y,
                        plane,
                        segment: index as u16,
                    };
                    let remaining = self
                        .pending_dma
                        .iter()
                        .filter(|job| job.destination == destination)
                        .count()
                        + self
                            .outstanding_dma
                            .values()
                            .filter(|pending| pending.destination == destination)
                            .count()
                        + self
                            .completed_reads
                            .values()
                            .filter(|completed| completed.pending.destination == destination)
                            .count();
                    if segment.data.len() != PIXEL_BURST_SIZE
                        || segment.pixels == 0
                        || segment
                            .source_pixel
                            .checked_add(segment.pixels)
                            .is_none_or(|end| end > depth.tile_width())
                        || segment
                            .output_pixel
                            .checked_add(segment.pixels)
                            .is_none_or(|end| end > frame.width)
                        || usize::from(segment.remaining) != remaining
                    {
                        return Err(Self::invalid(
                            component,
                            "GBE line segments and remaining DMA chunks must be in bounds",
                        ));
                    }
                }
            }
        }
        if self.actions.iter().any(|action| match action {
            GbeAction::Schedule { .. } | GbeAction::CompleteCgiDevice(_) | GbeAction::Trace(_) => {
                false
            }
            GbeAction::SetDdc { bus, drive } => {
                !matches!(
                    *bus,
                    bus if bus == self.wiring.crt_ddc || bus == self.wiring.flat_panel_ddc
                ) || drive.source != component
                    || drive.time > self.observed_time
            }
            GbeAction::PublishFrame(frame) => !Self::validate_frame(frame, self.observed_time),
            GbeAction::StartCgi(transaction) => {
                if transaction.controller != component || transaction.target != self.wiring.crime {
                    return true;
                }
                match &transaction.operation {
                    CrimeLinkOperation::Dma(request) => self
                        .outstanding_dma
                        .get(&transaction.id)
                        .is_none_or(|pending| {
                            pending.write
                                != matches!(
                                    request.transfer.view(),
                                    CrimeTransferView::Write { .. }
                                )
                        }),
                    CrimeLinkOperation::InterruptPost(post) => {
                        !self.outstanding_interrupt_posts.contains(&transaction.id)
                            || !(16..20).contains(&post.interrupt_bit)
                    }
                    CrimeLinkOperation::Pio(_) => true,
                }
            }
        }) {
            return Err(Self::invalid(
                component,
                "GBE actions must use configured endpoints and valid frame or transaction state",
            ));
        }
        Ok(())
    }
}

impl Gbe {
    /// Creates a reset GBE with no connected external clocks or monitor endpoint.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        timebase_hz: u64,
        wiring: GbeWiring,
    ) -> Self {
        assert!(timebase_hz != 0, "the GBE timebase must be nonzero");
        Self {
            id,
            name: name.into(),
            wiring,
            registers: GbeRegisters::new(),
            timebase_hz,
            observed_time: SimTime::ZERO,
            scan_origin_time: SimTime::ZERO,
            scan_origin_pixel: 0,
            pixel_clock_remainder: 0,
            timing_epoch: 0,
            scheduled_scanline_cycles: None,
            ttl_clock: None,
            differential_clock: None,
            sense_n: true,
            frame_lock: false,
            frame_lock_pending: false,
            frame_lock_last_rising: None,
            frame_lock_period_ticks: None,
            f2rf_level: false,
            ddc_clock_high: [true; 2],
            ddc_data_high: [true; 2],
            auxiliary_inputs: wiring.auxiliary_inputs,
            color_map_fifo: VecDeque::new(),
            deferred_color_map: VecDeque::new(),
            color_map_drain_scheduled: false,
            actions: VecDeque::new(),
            next_transaction: 0,
            next_read_order: 0,
            next_read_commit: 0,
            pending_dma: VecDeque::new(),
            outstanding_dma: BTreeMap::new(),
            outstanding_interrupt_posts: BTreeSet::new(),
            completed_reads: BTreeMap::new(),
            reads_in_flight: 0,
            writes_in_flight: 0,
            normal_tile_pointers: Vec::new(),
            overlay_tile_pointers: Vec::new(),
            did_frame_table: Vec::new(),
            did_frame_chunks_remaining: 0,
            did_line_blocks: BTreeMap::new(),
            line_fetches: BTreeMap::new(),
            working_frame: None,
            next_frame_sequence: 0,
            interrupt_posted: [false; 4],
            capture_writes_remaining: 0,
        }
    }

    /// Captures the GBE's dynamic hardware state.
    pub fn save_state(&self) -> GbeState {
        GbeState {
            id: self.id,
            wiring: self.wiring,
            registers: self.registers.save_state(),
            timebase_hz: self.timebase_hz,
            observed_time: self.observed_time,
            scan_origin_time: self.scan_origin_time,
            scan_origin_pixel: self.scan_origin_pixel,
            pixel_clock_remainder: self.pixel_clock_remainder,
            timing_epoch: self.timing_epoch,
            scheduled_scanline_cycles: self.scheduled_scanline_cycles,
            ttl_clock: self.ttl_clock,
            differential_clock: self.differential_clock,
            sense_n: self.sense_n,
            frame_lock: self.frame_lock,
            frame_lock_pending: self.frame_lock_pending,
            frame_lock_last_rising: self.frame_lock_last_rising,
            frame_lock_period_ticks: self.frame_lock_period_ticks,
            f2rf_level: self.f2rf_level,
            ddc_clock_high: self.ddc_clock_high,
            ddc_data_high: self.ddc_data_high,
            auxiliary_inputs: self.auxiliary_inputs,
            color_map_fifo: self.color_map_fifo.clone(),
            deferred_color_map: self.deferred_color_map.clone(),
            color_map_drain_scheduled: self.color_map_drain_scheduled,
            actions: self.actions.clone(),
            next_transaction: self.next_transaction,
            next_read_order: self.next_read_order,
            next_read_commit: self.next_read_commit,
            pending_dma: self.pending_dma.clone(),
            outstanding_dma: self.outstanding_dma.clone(),
            outstanding_interrupt_posts: self.outstanding_interrupt_posts.clone(),
            completed_reads: self.completed_reads.clone(),
            reads_in_flight: self.reads_in_flight,
            writes_in_flight: self.writes_in_flight,
            normal_tile_pointers: self.normal_tile_pointers.clone(),
            overlay_tile_pointers: self.overlay_tile_pointers.clone(),
            did_frame_table: self.did_frame_table.clone(),
            did_frame_chunks_remaining: self.did_frame_chunks_remaining,
            did_line_blocks: self.did_line_blocks.clone(),
            line_fetches: self.line_fetches.clone(),
            working_frame: self.working_frame.clone(),
            next_frame_sequence: self.next_frame_sequence,
            interrupt_posted: self.interrupt_posted,
            capture_writes_remaining: self.capture_writes_remaining,
        }
    }

    /// Restores dynamic state after validating wiring, timing, DMA, and frame invariants.
    pub fn restore_state(&mut self, state: GbeState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        if self.wiring != state.wiring {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "wiring",
            });
        }
        if self.timebase_hz != state.timebase_hz {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "timebase_hz",
            });
        }
        state.validate(self.id)?;

        self.registers = GbeRegisters::from_state(state.registers);
        self.observed_time = state.observed_time;
        self.scan_origin_time = state.scan_origin_time;
        self.scan_origin_pixel = state.scan_origin_pixel;
        self.pixel_clock_remainder = state.pixel_clock_remainder;
        self.timing_epoch = state.timing_epoch;
        self.scheduled_scanline_cycles = state.scheduled_scanline_cycles;
        self.ttl_clock = state.ttl_clock;
        self.differential_clock = state.differential_clock;
        self.sense_n = state.sense_n;
        self.frame_lock = state.frame_lock;
        self.frame_lock_pending = state.frame_lock_pending;
        self.frame_lock_last_rising = state.frame_lock_last_rising;
        self.frame_lock_period_ticks = state.frame_lock_period_ticks;
        self.f2rf_level = state.f2rf_level;
        self.ddc_clock_high = state.ddc_clock_high;
        self.ddc_data_high = state.ddc_data_high;
        self.auxiliary_inputs = state.auxiliary_inputs;
        self.color_map_fifo = state.color_map_fifo;
        self.deferred_color_map = state.deferred_color_map;
        self.color_map_drain_scheduled = state.color_map_drain_scheduled;
        self.actions = state.actions;
        self.next_transaction = state.next_transaction;
        self.next_read_order = state.next_read_order;
        self.next_read_commit = state.next_read_commit;
        self.pending_dma = state.pending_dma;
        self.outstanding_dma = state.outstanding_dma;
        self.outstanding_interrupt_posts = state.outstanding_interrupt_posts;
        self.completed_reads = state.completed_reads;
        self.reads_in_flight = state.reads_in_flight;
        self.writes_in_flight = state.writes_in_flight;
        self.normal_tile_pointers = state.normal_tile_pointers;
        self.overlay_tile_pointers = state.overlay_tile_pointers;
        self.did_frame_table = state.did_frame_table;
        self.did_frame_chunks_remaining = state.did_frame_chunks_remaining;
        self.did_line_blocks = state.did_line_blocks;
        self.line_fetches = state.line_fetches;
        self.working_frame = state.working_frame;
        self.next_frame_sequence = state.next_frame_sequence;
        self.interrupt_posted = state.interrupt_posted;
        self.capture_writes_remaining = state.capture_writes_remaining;
        Ok(())
    }

    /// Updates the simulated time observed by lazy display counters.
    pub fn observe_time(&mut self, now: SimTime) {
        assert!(now >= self.observed_time, "GBE time cannot move backwards");
        self.observed_time = now;
    }

    /// Captures the active framebuffer register for a bounded read batch.
    pub fn synchronous_frame_active_projection(&self) -> GbeSynchronousReadProjection {
        GbeSynchronousReadProjection {
            physical_address: registers::FRAME_START + 8,
            value: self.registers.frame[2],
        }
    }

    /// Commits the time observed by proven active-frame register reads.
    pub fn commit_synchronous_frame_active_reads(
        &mut self,
        reads: u64,
        last_delivery_time: SimTime,
    ) {
        if reads != 0 {
            self.observe_time(self.observed_time.max(last_delivery_time));
        }
    }

    /// Applies one host-neutral external input transition.
    pub fn apply_external_input(&mut self, input: GbeExternalInput) {
        match input {
            GbeExternalInput::SenseN(value) => {
                self.sense_n = value;
                if value {
                    self.registers.control_status |= 1 << 4;
                } else {
                    self.registers.control_status &= !(1 << 4);
                }
            }
            GbeExternalInput::FrameLock(value) => {
                if value && !self.frame_lock {
                    if let Some(previous) = self.frame_lock_last_rising {
                        let period = self.observed_time.get().saturating_sub(previous.get());
                        if period != 0 {
                            self.frame_lock_period_ticks = Some(period);
                        }
                    }
                    self.frame_lock_last_rising = Some(self.observed_time);
                    self.frame_lock_pending = true;
                }
                self.frame_lock = value;
            }
            GbeExternalInput::PixelClock {
                source,
                numerator_hz,
                denominator,
            } => {
                assert!(numerator_hz != 0, "an external clock must be nonzero");
                assert!(
                    denominator != 0,
                    "an external clock denominator must be nonzero"
                );
                let state = Some(ExternalClockState {
                    numerator_hz,
                    denominator,
                });
                match source {
                    GbeExternalClock::Ttl => self.ttl_clock = state,
                    GbeExternalClock::Differential => self.differential_clock = state,
                }
                self.restart_timing(false);
            }
            GbeExternalInput::DisconnectPixelClock(source) => {
                match source {
                    GbeExternalClock::Ttl => self.ttl_clock = None,
                    GbeExternalClock::Differential => self.differential_clock = None,
                }
                self.restart_timing(false);
            }
            GbeExternalInput::Auxiliary(levels) => self.auxiliary_inputs = levels,
        }
    }

    /// Observes one aggregate DDC line transition delivered by a two-wire bus.
    pub fn observe_ddc(&mut self, delivery: TwoWireLineDelivery) {
        let index = if delivery.bus == self.wiring.crt_ddc {
            0
        } else if delivery.bus == self.wiring.flat_panel_ddc {
            1
        } else {
            return;
        };
        self.ddc_clock_high[index] = !delivery.clock_low;
        self.ddc_data_high[index] = !delivery.data_low;
        self.registers.set_ddc_levels(
            index == 1,
            self.ddc_clock_high[index],
            self.ddc_data_high[index],
        );
    }

    /// Handles one scheduled GBE timing transition.
    pub fn handle_event(&mut self, event: GbeEvent) {
        let epoch = match event {
            GbeEvent::Scanline { epoch } | GbeEvent::ColorMapDrain { epoch } => epoch,
        };
        if epoch != self.timing_epoch {
            return;
        }
        match event {
            GbeEvent::Scanline { .. } => self.handle_scanline(),
            GbeEvent::ColorMapDrain { .. } => {
                self.color_map_drain_scheduled = false;
                if self.color_map_window_active() {
                    self.drain_one_color_map_entry();
                }
                self.schedule_color_map_drain();
            }
        }
    }

    /// Polls one pending GBE action.
    pub fn poll(&mut self) -> GbePoll {
        self.actions
            .pop_front()
            .map(GbePoll::Action)
            .unwrap_or(GbePoll::Idle)
    }

    /// Returns current digital output pin levels.
    pub fn output_pins(&self) -> GbeOutputPins {
        let position = self.scan_position();
        let x = (position & 0x0fff) as u16;
        let y = ((position >> 12) & 0x0fff) as u16;
        let flags = self.registers.vt[VT_FLAGS];
        let hsync = interval_contains(self.registers.vt[VT_HSYNC], x);
        let vsync = interval_contains(self.registers.vt[VT_VSYNC], y);
        let hblank = interval_contains(self.registers.vt[VT_HBLANK], x);
        let vblank = interval_contains(self.registers.vt[VT_VBLANK], y);
        let hdrive = interval_contains(self.registers.vt[registers::FP_HDRIVE], x);
        let vdrive = interval_contains(self.registers.vt[registers::FP_VDRIVE], y);
        let data_enable = interval_contains(self.registers.vt[registers::FP_DATA_ENABLE], x)
            && interval_contains(self.registers.vt[VT_HPIXEN], x)
            && interval_contains(self.registers.vt[VT_VPIXEN], y);
        let crt_hsync = if flags & (1 << 3) != 0 {
            flags & (1 << 2) != 0
        } else {
            hsync ^ (flags & (1 << 2) != 0)
        };
        let crt_vsync = if flags & (1 << 1) != 0 {
            flags & 1 != 0
        } else {
            vsync ^ (flags & 1 != 0)
        };
        let aux = interval_offset(self.registers.vt[VT_VPIXEN], y)
            .zip(interval_offset(self.registers.vt[VT_HPIXEN], x))
            .and_then(|(active_y, active_x)| {
                let did_y = active_y
                    .wrapping_add(upper_endpoint(self.registers.vt[registers::DID_START_XY]));
                let did_x = active_x
                    .wrapping_add(lower_endpoint(self.registers.vt[registers::DID_START_XY]));
                let block = did_block_for_line(&self.did_frame_table, did_y)?;
                let line = self.did_line_blocks.get(&block)?;
                let did = did_for_pixel(&self.did_frame_table, line, did_y, did_x);
                Some(((self.registers.wid[usize::from(did)] >> 11) & 3) as u8)
            })
            .unwrap_or(((self.registers.wid[0] >> 11) & 3) as u8);
        GbeOutputPins {
            crt_hsync,
            crt_vsync,
            crt_blank: hblank || vblank,
            flat_panel_hdrive: if flags & (1 << 3) != 0 {
                flags & (1 << 2) != 0
            } else {
                hdrive ^ (flags & (1 << 2) != 0)
            },
            flat_panel_vdrive: if flags & (1 << 1) != 0 {
                flags & 1 != 0
            } else {
                vdrive ^ (flags & 1 != 0)
            },
            flat_panel_data_enable: data_enable,
            f2rf: self.f2rf_level || flags & (1 << 6) != 0,
            aux,
            gpio: std::array::from_fn(|index| {
                (self.registers.control_status & auxiliary_output_disable_mask(index) == 0)
                    .then_some(self.registers.control_status & auxiliary_data_mask(index) != 0)
            }),
        }
    }

    fn access_read(&self, address: u64) -> Result<CrimeCompletionPayload, CrimeBusError> {
        let value = if address == registers::CONTROL_STATUS {
            self.registers
                .control_status_with_auxiliary_inputs(self.auxiliary_inputs)
        } else if address == registers::VT_START {
            self.scan_position()
        } else if address == COLOR_MAP_FIFO {
            self.color_map_fifo.len().min(COLOR_MAP_FIFO_CAPACITY - 1) as u32
        } else {
            self.registers.read(address)?
        };
        Ok(CrimeCompletionPayload::ReadData(value.to_be_bytes().into()))
    }

    fn access_write(
        &mut self,
        controller: ComponentId,
        id: CrimeTransactionId,
        address: u64,
        value: u32,
    ) -> Result<bool, CrimeBusError> {
        let effect = self.registers.write(address, value)?;
        match effect {
            RegisterWrite::None => {}
            RegisterWrite::DotClock => self.restart_timing(false),
            RegisterWrite::Ddc {
                flat_panel,
                clock_released,
                data_released,
            } => {
                let bus = if flat_panel {
                    self.wiring.flat_panel_ddc
                } else {
                    self.wiring.crt_ddc
                };
                self.actions.push_back(GbeAction::SetDdc {
                    bus,
                    drive: TwoWireDrive {
                        source: self.id,
                        time: self.observed_time,
                        clock_low: !clock_released,
                        data_low: !data_released,
                    },
                });
            }
            RegisterWrite::Timing { counter_written } => self.restart_timing(counter_written),
            RegisterWrite::Shadow => {}
            RegisterWrite::FrameFifoReset => self.cancel_plane_dma(Plane::Normal),
            RegisterWrite::OverlayFifoReset => self.cancel_plane_dma(Plane::Overlay),
            RegisterWrite::ColorMap { index, value } => {
                let write = ColorMapWrite { index, value };
                if self.color_map_fifo.len() == COLOR_MAP_FIFO_CAPACITY {
                    self.deferred_color_map.push_back(DeferredColorMapWrite {
                        controller,
                        id,
                        write,
                    });
                    return Ok(false);
                }
                self.color_map_fifo.push_back(write);
                self.schedule_color_map_drain();
            }
            RegisterWrite::Capture => {}
        }
        Ok(true)
    }

    fn scan_position(&self) -> u32 {
        let stored = self.registers.vt[VT_XY];
        if stored & VT_XY_FREEZE != 0 {
            return stored;
        }
        let Some(projection) = self.pixel_projection() else {
            return stored;
        };
        let elapsed = self
            .observed_time
            .get()
            .saturating_sub(self.scan_origin_time.get());
        let cycles = projection
            .cycles_until_elapsed_at_least(elapsed.saturating_add(1))
            .unwrap_or(1)
            .saturating_sub(1);
        let width = u64::from((self.registers.vt[VT_XY_MAX] & 0x0fff) + 1);
        let height = u64::from(((self.registers.vt[VT_XY_MAX] >> 12) & 0x0fff) + 1);
        let frame_pixels = width.saturating_mul(height).max(1);
        let pixel = self.scan_origin_pixel.wrapping_add(cycles) % frame_pixels;
        (((pixel / width) as u32) << 12) | (pixel % width) as u32
    }

    fn pixel_projection(&self) -> Option<RationalClockProjection> {
        let selection = (self.registers.control_status >> 28) & 3;
        let frequency = match selection {
            0 => self.ttl_clock?,
            1 => self.differential_clock?,
            2 => return None,
            _ => {
                if self.registers.dot_clock & DOT_CLOCK_RUN == 0 {
                    return None;
                }
                let m = u64::from((self.registers.dot_clock & 0xff) + 1);
                let n = u64::from(((self.registers.dot_clock >> 8) & 0x3f) + 1);
                let p = (self.registers.dot_clock >> 14) & 3;
                ExternalClockState {
                    numerator_hz: PIXEL_REFERENCE_CLOCK_HZ.saturating_mul(m),
                    denominator: n.saturating_mul(1_u64 << p),
                }
            }
        };
        Some(RationalClockProjection::new(
            self.timebase_hz,
            frequency.numerator_hz,
            frequency.denominator,
            self.pixel_clock_remainder % frequency.numerator_hz,
        ))
    }

    fn restart_timing(&mut self, counter_written: bool) {
        self.timing_epoch = self.timing_epoch.wrapping_add(1);
        self.scheduled_scanline_cycles = None;
        self.color_map_drain_scheduled = false;
        if counter_written {
            let value = self.registers.vt[VT_XY];
            let width = u64::from((self.registers.vt[VT_XY_MAX] & 0x0fff) + 1);
            self.scan_origin_pixel = u64::from((value >> 12) & 0x0fff)
                .saturating_mul(width)
                .saturating_add(u64::from(value & 0x0fff));
            self.scan_origin_time = self.observed_time;
            self.pixel_clock_remainder = 0;
        } else {
            let value = self.scan_position();
            let width = u64::from((self.registers.vt[VT_XY_MAX] & 0x0fff) + 1);
            self.scan_origin_pixel =
                u64::from((value >> 12) & 0x0fff) * width + u64::from(value & 0x0fff);
            self.scan_origin_time = self.observed_time;
            self.pixel_clock_remainder = 0;
        }
        if self.registers.vt[VT_XY] & VT_XY_FREEZE == 0 && self.pixel_projection().is_some() {
            self.begin_scanline((self.scan_origin_pixel / self.scan_width()) as u16);
            self.schedule_next_scanline();
            self.schedule_color_map_drain();
        }
    }

    fn schedule_next_scanline(&mut self) {
        if self.scheduled_scanline_cycles.is_some() {
            return;
        }
        let Some(projection) = self.pixel_projection() else {
            return;
        };
        let x = self.scan_origin_pixel % self.scan_width();
        let cycles = self.scan_width().saturating_sub(x).max(1);
        let Some(delay) = projection.elapsed(cycles) else {
            return;
        };
        self.scheduled_scanline_cycles = Some(cycles);
        self.actions.push_back(GbeAction::Schedule {
            delay,
            event: GbeEvent::Scanline {
                epoch: self.timing_epoch,
            },
        });
    }

    fn handle_scanline(&mut self) {
        let Some(cycles) = self.scheduled_scanline_cycles.take() else {
            return;
        };
        let Some(mut projection) = self.pixel_projection() else {
            return;
        };
        let _ = projection.advance(cycles);
        self.pixel_clock_remainder = projection.remainder();
        let frame_pixels = self.scan_width().saturating_mul(self.scan_height()).max(1);
        self.scan_origin_pixel = self.scan_origin_pixel.wrapping_add(cycles) % frame_pixels;
        if self.frame_lock_pending {
            self.scan_origin_pixel = u64::from(upper_endpoint(self.registers.vt[VT_F2RF_LOCK]))
                .saturating_mul(self.scan_width())
                % frame_pixels;
            self.frame_lock_pending = false;
        }
        self.scan_origin_time = self.observed_time;
        let y = (self.scan_origin_pixel / self.scan_width()) as u16;
        if y == lower_endpoint(self.registers.vt[VT_F2RF_LOCK]) {
            self.f2rf_level = !self.f2rf_level;
        }
        let previous_y = if y == 0 {
            (self.scan_height() - 1) as u16
        } else {
            y - 1
        };
        self.finish_scanline(previous_y);
        let frame_height = self.working_frame.as_ref().map(|frame| frame.height);
        if frame_height.is_some_and(|height| {
            self.active_frame_y(previous_y, height).is_some()
                && self.active_frame_y(y, height).is_none()
        }) {
            self.finish_frame();
        }
        self.schedule_color_map_drain();
        let vblank_on = upper_endpoint(self.registers.vt[VT_VBLANK]);
        if y == vblank_on {
            self.registers.commit_shadow();
            self.prefetch_frame_metadata();
        }
        self.post_timing_interrupts(y);
        self.begin_scanline(y);
        self.schedule_next_scanline();
    }

    fn scan_width(&self) -> u64 {
        u64::from((self.registers.vt[VT_XY_MAX] & 0x0fff) + 1)
    }

    fn scan_height(&self) -> u64 {
        u64::from(((self.registers.vt[VT_XY_MAX] >> 12) & 0x0fff) + 1)
    }

    fn display_dimensions(&self) -> (usize, usize) {
        let (framebuffer_width, framebuffer_height) = visible_dimensions(&self.registers);
        let framebuffer_pixels = framebuffer_width.saturating_mul(framebuffer_height);
        let timing_height = usize::from(upper_endpoint(self.registers.vt[VT_VBLANK]));
        let height = if timing_height == 0 {
            framebuffer_height
        } else {
            timing_height.min(4_096)
        };
        let width = if height != 0 && framebuffer_pixels.is_multiple_of(height) {
            framebuffer_pixels / height
        } else {
            framebuffer_width
        };
        (width.min(4_096), height)
    }

    fn active_frame_y(&self, scan_y: u16, frame_height: usize) -> Option<u16> {
        if frame_height == 0 {
            return None;
        }
        let total = usize::try_from(self.scan_height()).unwrap_or(0);
        let span = interval_length(self.registers.vt[VT_VPIXEN], total);
        let offset = interval_offset_with_modulus(self.registers.vt[VT_VPIXEN], scan_y, total)?;
        let pipeline_lines = span.saturating_sub(frame_height);
        let frame_y = offset.checked_sub(pipeline_lines)?;
        (frame_y < frame_height).then_some(frame_y as u16)
    }

    fn begin_scanline(&mut self, y: u16) {
        let (width, height) = self.display_dimensions();
        let Some(active_y) = self.active_frame_y(y, height) else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        if self.working_frame.is_none() {
            let sequence = self.next_frame_sequence;
            self.next_frame_sequence = self.next_frame_sequence.wrapping_add(1);
            self.working_frame = Some(WorkingFrame {
                sequence,
                width,
                height,
                rgba: vec![0; width.saturating_mul(height).saturating_mul(4)],
            });
        }
        let sequence = self
            .working_frame
            .as_ref()
            .map(|frame| frame.sequence)
            .unwrap_or(0);
        self.line_fetches
            .insert((sequence, active_y), LineFetch::new());
        self.queue_plane_line(sequence, active_y, Plane::Normal, width);
        self.queue_plane_line(sequence, active_y, Plane::Overlay, width);
        let did_y =
            active_y.wrapping_add(upper_endpoint(self.registers.vt[registers::DID_START_XY]));
        if self.registers.did[0] & (1 << 16) != 0
            && let Some(block) = did_block_for_line(&self.did_frame_table, did_y)
            && !self.did_line_blocks.contains_key(&block)
        {
            let base = u64::from(self.registers.did[0] & 0xffff) << 16;
            self.did_line_blocks.insert(block, vec![0; 512]);
            self.queue_dma_read(
                base + u64::from(block) * 512,
                512,
                DmaDestination::DidBlock(block),
            );
        }
        self.pump_dma();
    }

    fn finish_scanline(&mut self, y: u16) {
        let Some(frame) = self.working_frame.as_ref() else {
            return;
        };
        let Some(active_y) = self.active_frame_y(y, frame.height) else {
            return;
        };
        let sequence = frame.sequence;
        let width = frame.width;
        let fetch = self
            .line_fetches
            .remove(&(sequence, active_y))
            .unwrap_or_else(LineFetch::new);
        if fetch
            .normal
            .segments
            .iter()
            .chain(&fetch.overlay.segments)
            .any(|segment| segment.remaining != 0)
        {
            self.trace_dma_error("display-fifo-underflow", u64::from(active_y), 0);
        }
        let depth = PlaneDepth::from_frame_register(self.registers.frame[0]);
        let normal = decode_plane_line(&fetch.normal, depth, width);
        let overlay = decode_plane_line(&fetch.overlay, PlaneDepth::Eight, width);
        let did_y =
            active_y.wrapping_add(upper_endpoint(self.registers.vt[registers::DID_START_XY]));
        let did_x_start = lower_endpoint(self.registers.vt[registers::DID_START_XY]);
        let cursor_start = self.registers.vt[registers::CURSOR_START_XY];
        let cursor_y = triggered_counter(
            y,
            upper_endpoint(self.registers.vt[VT_VBLANK]),
            upper_endpoint(cursor_start),
            self.scan_height(),
        );
        let active_x_start = upper_endpoint(self.registers.vt[VT_HPIXEN]);
        let scan_width = self.scan_width();
        let cursor_x_start = triggered_counter(
            active_x_start,
            lower_endpoint(cursor_start),
            0x0fe0,
            scan_width,
        );
        let did_block = did_block_for_line(&self.did_frame_table, did_y)
            .and_then(|block| self.did_line_blocks.get(&block));
        let registers = &self.registers;
        let did_frame_table = &self.did_frame_table;
        let Some(frame) = self.working_frame.as_mut() else {
            return;
        };
        let offset = usize::from(active_y) * width * 4;
        let row = &mut frame.rgba[offset..offset + width * 4];
        for x in 0..width {
            let did = did_block
                .map(|table| {
                    did_for_pixel(
                        did_frame_table,
                        table,
                        did_y,
                        (x.min(u16::MAX as usize) as u16).wrapping_add(did_x_start),
                    )
                })
                .unwrap_or(0);
            let (mut rgb, _) =
                color_from_normal(registers, normal.get(x).copied().unwrap_or(0), depth, did);
            if let Some(overlay) = overlay
                .get(x)
                .and_then(|value| color_from_overlay(registers, *value as u8))
            {
                rgb = overlay;
            }
            let cursor_x = (u64::from(cursor_x_start) + x as u64) as u16 & 0x0fff;
            if let Some(cursor) =
                cursor_color(registers, usize::from(cursor_x), usize::from(cursor_y))
            {
                rgb = cursor;
            }
            let offset = x * 4;
            row[offset..offset + 3].copy_from_slice(&rgb);
            row[offset + 3] = 0xff;
        }
    }

    fn finish_frame(&mut self) {
        let Some(frame) = self.working_frame.take() else {
            return;
        };
        let published = GbeFrame {
            sequence: frame.sequence,
            completed_at: self.observed_time,
            width: frame.width as u32,
            height: frame.height as u32,
            stride: frame.width.saturating_mul(4) as u32,
            field: GbeFrameField::Progressive,
            rgba: frame.rgba,
        };
        self.start_video_capture(&published);
        self.actions.push_back(GbeAction::PublishFrame(published));
    }

    fn queue_plane_line(&mut self, sequence: u64, y: u16, plane: Plane, output_width: usize) {
        let enabled = match plane {
            Plane::Normal => self.registers.frame[2] & 1 != 0,
            Plane::Overlay => self.registers.overlay[1] & 1 != 0,
        };
        if !enabled {
            return;
        }
        let pointers = match plane {
            Plane::Normal => self.normal_tile_pointers.clone(),
            Plane::Overlay => self.overlay_tile_pointers.clone(),
        };
        let depth = match plane {
            Plane::Normal => PlaneDepth::from_frame_register(self.registers.frame[0]),
            Plane::Overlay => PlaneDepth::Eight,
        };
        let (plane_width, plane_height, tiles) = match plane {
            Plane::Normal => {
                let (width, height) = visible_dimensions(&self.registers);
                (width, height, tile_columns(self.registers.frame[0]))
            }
            Plane::Overlay => (
                plane_width(self.registers.overlay[0], PlaneDepth::Eight),
                usize::try_from(self.registers.frame[1] >> 16).unwrap_or(0),
                tile_columns(self.registers.overlay[0]),
            ),
        };
        if plane_width == 0 || plane_height == 0 || tiles == 0 {
            return;
        }
        let mut output_pixel = 0;
        while output_pixel < output_width {
            let linear_pixel = usize::from(y)
                .saturating_mul(output_width)
                .saturating_add(output_pixel);
            let source_y = linear_pixel / plane_width;
            if source_y >= plane_height {
                break;
            }
            let source_x = linear_pixel % plane_width;
            let tile_x = source_x / depth.tile_width();
            let source_pixel = source_x % depth.tile_width();
            let pixels = (output_width - output_pixel)
                .min(plane_width - source_x)
                .min(depth.tile_width() - source_pixel);
            let pointer_offset = ((source_y / 128) * tiles + tile_x) * 2;
            let page = pointers
                .get(pointer_offset..pointer_offset + 2)
                .map(|pointer| u16::from_be_bytes([pointer[0], pointer[1]]))
                .unwrap_or(0);
            let segment = {
                let fetch = self
                    .line_fetches
                    .get_mut(&(sequence, y))
                    .expect("a queued plane line must have fetch state");
                let segments = match plane {
                    Plane::Normal => &mut fetch.normal.segments,
                    Plane::Overlay => &mut fetch.overlay.segments,
                };
                let index = segments.len();
                segments.push(LineSegment {
                    data: vec![0; PIXEL_BURST_SIZE],
                    source_pixel,
                    output_pixel,
                    pixels,
                    remaining: 0,
                });
                index
            };
            if page != 0 {
                let row_offset = (source_y % 128) as u64 * 512;
                let destination = DmaDestination::Line {
                    frame: sequence,
                    y,
                    plane,
                    segment: segment as u16,
                };
                let chunks =
                    self.queue_dma_read(u64::from(page) << 16 | row_offset, 512, destination);
                let fetch = self
                    .line_fetches
                    .get_mut(&(sequence, y))
                    .expect("a queued plane line must retain fetch state");
                let segment = match plane {
                    Plane::Normal => &mut fetch.normal.segments[segment],
                    Plane::Overlay => &mut fetch.overlay.segments[segment],
                };
                segment.remaining = chunks.min(u8::MAX as usize) as u8;
            }
            output_pixel += pixels;
        }
    }

    fn prefetch_frame_metadata(&mut self) {
        self.normal_tile_pointers.clear();
        self.overlay_tile_pointers.clear();
        self.did_frame_table.clear();
        self.did_frame_chunks_remaining = 0;
        self.did_line_blocks.clear();
        let (_, height) = visible_dimensions(&self.registers);
        let tile_rows = height.div_ceil(128);
        let normal_tiles = tile_columns(self.registers.frame[0]);
        let overlay_tiles = tile_columns(self.registers.overlay[0]);
        if self.registers.frame[2] & 1 != 0 {
            let bytes = align_up(normal_tiles.saturating_mul(tile_rows).saturating_mul(2), 32);
            self.normal_tile_pointers.resize(bytes, 0);
            self.queue_dma_read(
                u64::from(self.registers.frame[2] & !0x1f),
                bytes,
                DmaDestination::TilePointers(Plane::Normal),
            );
        }
        if self.registers.overlay[1] & 1 != 0 {
            let bytes = align_up(
                overlay_tiles.saturating_mul(tile_rows).saturating_mul(2),
                32,
            );
            self.overlay_tile_pointers.resize(bytes, 0);
            self.queue_dma_read(
                u64::from(self.registers.overlay[1] & !0x1f),
                bytes,
                DmaDestination::TilePointers(Plane::Overlay),
            );
        }
        if self.registers.did[0] & (1 << 16) != 0 {
            self.did_frame_table.resize(256, 0);
            self.did_frame_chunks_remaining = self.queue_dma_read(
                u64::from(self.registers.did[0] & 0xffff) << 16,
                256,
                DmaDestination::DidFrame,
            );
        }
        self.pump_dma();
    }

    fn prefetch_did_line_blocks(&mut self) {
        let blocks = self
            .did_frame_table
            .chunks_exact(4)
            .map(|entry| {
                let value =
                    u32::from_be_bytes(entry.try_into().expect("four-byte DID frame entry"));
                ((value >> 6) & 0x7f) as u8
            })
            .collect::<BTreeSet<_>>();
        let base = u64::from(self.registers.did[0] & 0xffff) << 16;
        for block in blocks {
            self.did_line_blocks.insert(block, vec![0; 512]);
            self.queue_dma_read(
                base + u64::from(block) * 512,
                512,
                DmaDestination::DidBlock(block),
            );
        }
    }

    fn queue_dma_read(
        &mut self,
        address: u64,
        length: usize,
        destination: DmaDestination,
    ) -> usize {
        if length == 0
            || !address.is_multiple_of(DMA_ALIGNMENT)
            || !length.is_multiple_of(DMA_ALIGNMENT as usize)
        {
            self.trace_dma_error("invalid-read-shape", address, length);
            return 0;
        }
        let mut queued = 0;
        let mut offset = 0;
        let maximum_burst = if matches!(destination, DmaDestination::Line { .. }) {
            PIXEL_BURST_SIZE
        } else {
            DMA_ALIGNMENT as usize
        };
        while offset < length {
            let current = address + offset as u64;
            let page_remaining = (DMA_PAGE_SIZE - current % DMA_PAGE_SIZE) as usize;
            let chunk = (length - offset).min(maximum_burst).min(page_remaining);
            if !chunk.is_multiple_of(DMA_ALIGNMENT as usize) {
                self.trace_dma_error("unaligned-read-split", current, chunk);
                break;
            }
            self.pending_dma.push_back(DmaJob {
                address: current,
                transfer: CrimeTransfer::read(chunk as u16),
                destination: destination.clone(),
                destination_offset: offset,
            });
            queued += 1;
            offset += chunk;
        }
        queued
    }

    fn queue_dma_write(
        &mut self,
        address: u64,
        data: CrimeData,
        destination_offset: usize,
    ) -> bool {
        if !address.is_multiple_of(DMA_ALIGNMENT)
            || !data.len().is_multiple_of(DMA_ALIGNMENT as usize)
            || data.len() > PIXEL_BURST_SIZE
            || address / DMA_PAGE_SIZE != (address + data.len() as u64 - 1) / DMA_PAGE_SIZE
        {
            self.trace_dma_error("invalid-write-shape", address, data.len());
            return false;
        }
        let enables = CrimeByteEnable::enabled(data.len());
        self.pending_dma.push_back(DmaJob {
            address,
            transfer: CrimeTransfer::write(data, enables),
            destination: DmaDestination::Capture,
            destination_offset,
        });
        true
    }

    fn pump_dma(&mut self) {
        let mut retained = VecDeque::new();
        while let Some(job) = self.pending_dma.pop_front() {
            let write = matches!(job.transfer.view(), CrimeTransferView::Write { .. });
            if (write && self.writes_in_flight >= MAX_DMA_WRITES)
                || (!write && self.reads_in_flight >= MAX_DMA_READS)
            {
                retained.push_back(job);
                continue;
            }
            let id = CrimeTransactionId::new(self.next_transaction);
            self.next_transaction = self.next_transaction.wrapping_add(1);
            let read_order = if write {
                None
            } else {
                let order = self.next_read_order;
                self.next_read_order = self.next_read_order.wrapping_add(1);
                Some(order)
            };
            if write {
                self.writes_in_flight += 1;
            } else {
                self.reads_in_flight += 1;
            }
            self.outstanding_dma.insert(
                id,
                PendingDma {
                    destination: job.destination,
                    destination_offset: job.destination_offset,
                    read_order,
                    write,
                },
            );
            self.actions
                .push_back(GbeAction::StartCgi(CrimeCgiTransaction {
                    id,
                    controller: self.id,
                    target: self.wiring.crime,
                    operation: CrimeLinkOperation::Dma(CrimeDmaRequest {
                        address: job.address,
                        transfer: job.transfer,
                    }),
                }));
        }
        self.pending_dma = retained;
    }

    fn accept_dma_completion(&mut self, completion: CrimeCgiCompletion) {
        if self.outstanding_interrupt_posts.remove(&completion.id) {
            return;
        }
        let Some(pending) = self.outstanding_dma.remove(&completion.id) else {
            self.trace_dma_error("unexpected-completion", completion.id.get() as u64, 0);
            return;
        };
        if pending.write {
            self.writes_in_flight = self.writes_in_flight.saturating_sub(1);
            self.apply_dma_completion(pending, completion);
        } else {
            self.reads_in_flight = self.reads_in_flight.saturating_sub(1);
            let order = pending
                .read_order
                .expect("a read DMA operation must carry commit order");
            self.completed_reads.insert(
                order,
                CompletedRead {
                    pending,
                    completion,
                },
            );
            while let Some(completed) = self.completed_reads.remove(&self.next_read_commit) {
                self.next_read_commit = self.next_read_commit.wrapping_add(1);
                self.apply_dma_completion(completed.pending, completed.completion);
            }
        }
        self.pump_dma();
    }

    fn apply_dma_completion(&mut self, pending: PendingDma, completion: CrimeCgiCompletion) {
        let successful = completion.memory_fault.is_none() && completion.result.is_ok();
        if pending.write {
            if !successful {
                self.registers.video_capture[3] |= 1 << 2;
                self.capture_writes_remaining = 0;
                self.pending_dma
                    .retain(|job| !matches!(job.destination, DmaDestination::Capture));
            } else {
                self.capture_writes_remaining = self.capture_writes_remaining.saturating_sub(1);
                if self.capture_writes_remaining == 0 {
                    self.registers.video_capture[3] |= 1 << 3;
                    if self.registers.video_capture[2] & (1 << 2) != 0 {
                        self.registers.video_capture[3] ^= 1 << 0;
                    }
                }
            }
            return;
        }
        let data = match completion.result {
            Ok(CrimeCompletionPayload::ReadData(data)) if completion.memory_fault.is_none() => data,
            _ => {
                self.trace_dma_error("read-fault", completion.id.get() as u64, 0);
                CrimeData::default()
            }
        };
        match pending.destination {
            DmaDestination::TilePointers(plane) => {
                let target = match plane {
                    Plane::Normal => &mut self.normal_tile_pointers,
                    Plane::Overlay => &mut self.overlay_tile_pointers,
                };
                copy_into(target, pending.destination_offset, &data);
            }
            DmaDestination::DidFrame => {
                copy_into(&mut self.did_frame_table, pending.destination_offset, &data);
                self.did_frame_chunks_remaining = self.did_frame_chunks_remaining.saturating_sub(1);
                if self.did_frame_chunks_remaining == 0 {
                    self.prefetch_did_line_blocks();
                }
            }
            DmaDestination::DidBlock(block) => {
                if let Some(target) = self.did_line_blocks.get_mut(&block) {
                    copy_into(target, pending.destination_offset, &data);
                }
            }
            DmaDestination::Line {
                frame,
                y,
                plane,
                segment,
            } => {
                if let Some(fetch) = self.line_fetches.get_mut(&(frame, y)) {
                    let segments = match plane {
                        Plane::Normal => &mut fetch.normal.segments,
                        Plane::Overlay => &mut fetch.overlay.segments,
                    };
                    if let Some(segment) = segments.get_mut(usize::from(segment)) {
                        copy_into(&mut segment.data, pending.destination_offset, &data);
                        segment.remaining = segment.remaining.saturating_sub(1);
                    }
                }
            }
            DmaDestination::Capture => {}
        }
    }

    fn start_video_capture(&mut self, frame: &GbeFrame) {
        if self.registers.video_capture[2] & 1 == 0 || self.capture_writes_remaining != 0 {
            return;
        }
        self.registers.video_capture[3] &= !((1 << 2) | (1 << 3));
        self.registers.video_capture[3] = (self.registers.video_capture[3] & !(1 << 4))
            | ((self.registers.video_capture[3] & 1) << 4);
        let horizontal = self.registers.video_capture[0];
        let vertical = self.registers.video_capture[1];
        let capture_origin = self.registers.vt[registers::VIDEO_CAPTURE_START_XY];
        let origin_x = capture_origin & 0x0fff;
        let origin_y = (capture_origin >> 12) & 0x0fff;
        let x0 = usize::try_from((horizontal & 0x0fff).wrapping_sub(origin_x) & 0x0fff)
            .unwrap_or(0)
            .min(frame.width as usize);
        let x1 = usize::try_from(((horizontal >> 12) & 0x0fff).wrapping_sub(origin_x) & 0x0fff)
            .unwrap_or(0)
            .min(frame.width.saturating_sub(1) as usize);
        let y0 = usize::try_from((vertical & 0x0fff).wrapping_sub(origin_y) & 0x0fff)
            .unwrap_or(0)
            .min(frame.height as usize);
        let y1 = usize::try_from(((vertical >> 12) & 0x0fff).wrapping_sub(origin_y) & 0x0fff)
            .unwrap_or(0)
            .min(frame.height.saturating_sub(1) as usize);
        if x1 < x0 || y1 < y0 {
            return;
        }
        let crop_width = x1 - x0 + 1;
        let crop_height = y1 - y0 + 1;
        let mut crop = vec![0; crop_width * crop_height * 4];
        for y in 0..crop_height {
            let source = ((y0 + y) * frame.width as usize + x0) * 4;
            let target = y * crop_width * 4;
            crop[target..target + crop_width * 4]
                .copy_from_slice(&frame.rgba[source..source + crop_width * 4]);
        }
        let fullscreen = self.registers.video_capture[2] & (1 << 3) != 0;
        let pal = self.frame_rate_hz().is_some_and(|rate| rate < 55);
        let (width, height, rgba) = if fullscreen {
            filter_fullscreen_rgba(
                &crop,
                crop_width,
                crop_height,
                pal,
                self.registers.video_capture[2] & (1 << 1) != 0,
                self.registers.video_capture[3] & 1 != 0,
            )
        } else {
            (crop_width, crop_height, crop)
        };
        let diagnostic = self.registers.video_capture[2] & (1 << 4) != 0;
        let mut encoded = Vec::with_capacity(width * height * if diagnostic { 2 } else { 4 });
        for pixel in rgba.chunks_exact(4) {
            if diagnostic {
                let packed = (u16::from(pixel[0] >> 3) << 10)
                    | (u16::from(pixel[1] >> 3) << 5)
                    | u16::from(pixel[2] >> 3);
                encoded.extend_from_slice(&packed.to_be_bytes());
            } else {
                encoded.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xff]);
            }
        }
        let pages = self.capture_pages();
        if encoded.len() > pages.len().saturating_mul(65_536) {
            self.registers.video_capture[3] |= 1 << 2;
            return;
        }
        let mut offset = 0;
        while offset < encoded.len() && offset / 65_536 < pages.len() {
            let page_index = offset / 65_536;
            let page_offset = offset % 65_536;
            let chunk = (encoded.len() - offset)
                .min(PIXEL_BURST_SIZE)
                .min(65_536 - page_offset);
            let mut data = vec![0; PIXEL_BURST_SIZE];
            data[..chunk].copy_from_slice(&encoded[offset..offset + chunk]);
            let address = (u64::from(pages[page_index]) << 16) | page_offset as u64;
            if self.queue_dma_write(address, data.into(), offset) {
                self.capture_writes_remaining += 1;
            } else {
                self.registers.video_capture[3] |= 1 << 2;
                break;
            }
            offset += chunk;
        }
        self.pump_dma();
    }

    fn capture_pages(&self) -> Vec<u16> {
        self.registers.video_capture[4..=8]
            .iter()
            .flat_map(|value| [(value >> 16) as u16, *value as u16])
            .collect()
    }

    fn frame_rate_hz(&self) -> Option<u64> {
        if let Some(period) = self.frame_lock_period_ticks {
            return Some(self.timebase_hz / period);
        }
        let projection = self.pixel_projection()?;
        let pixels = self.scan_width().saturating_mul(self.scan_height());
        let denominator = projection.frequency_denominator().saturating_mul(pixels);
        (denominator != 0).then(|| projection.frequency_numerator_hz() / denominator)
    }

    fn drain_one_color_map_entry(&mut self) {
        let Some(write) = self.color_map_fifo.pop_front() else {
            return;
        };
        self.registers.color_map[usize::from(write.index)] = write.value;
        if let Some(deferred) = self.deferred_color_map.pop_front() {
            self.color_map_fifo.push_back(deferred.write);
            self.actions
                .push_back(GbeAction::CompleteCgiDevice(CrimeCgiCompletion {
                    id: deferred.id,
                    result: Ok(CrimeCompletionPayload::WriteComplete),
                    memory_fault: None,
                }));
        }
    }

    fn color_map_window_active(&self) -> bool {
        let position = self.scan_position();
        let x = (position & 0x0fff) as u16;
        let y = ((position >> 12) & 0x0fff) as u16;
        interval_contains(self.registers.vt[VT_HCMAP], x)
            || interval_contains(self.registers.vt[VT_VCMAP], y)
    }

    fn schedule_color_map_drain(&mut self) {
        if self.color_map_drain_scheduled || self.color_map_fifo.is_empty() {
            return;
        }
        let Some(projection) = self.pixel_projection() else {
            return;
        };
        if self.registers.vt[VT_XY] & VT_XY_FREEZE != 0 {
            return;
        }
        let position = self.scan_position();
        let x = u64::from(position & 0x0fff);
        let y = u64::from((position >> 12) & 0x0fff);
        let width = self.scan_width();
        let height = self.scan_height();
        let mut distance = u64::MAX;
        if self.color_map_window_active() {
            distance = 1;
        } else {
            let horizontal = self.registers.vt[VT_HCMAP];
            if lower_endpoint(horizontal) != upper_endpoint(horizontal) {
                let on = u64::from(upper_endpoint(horizontal));
                distance = (on + width - x) % width;
                if distance == 0 {
                    distance = width;
                }
            }
            let vertical = self.registers.vt[VT_VCMAP];
            if lower_endpoint(vertical) != upper_endpoint(vertical) {
                let on = u64::from(upper_endpoint(vertical));
                let lines = (on + height - y) % height;
                let vertical_distance = if lines == 0 {
                    width.saturating_sub(x)
                } else {
                    lines.saturating_mul(width).saturating_sub(x)
                };
                distance = distance.min(vertical_distance.max(1));
            }
        }
        if distance == u64::MAX {
            return;
        }
        let elapsed = self
            .observed_time
            .get()
            .saturating_sub(self.scan_origin_time.get());
        let completed = projection
            .cycles_until_elapsed_at_least(elapsed.saturating_add(1))
            .unwrap_or(1)
            .saturating_sub(1);
        let Some(target) = projection.elapsed(completed.saturating_add(distance)) else {
            return;
        };
        let delay = SimDuration::new(target.get().saturating_sub(elapsed).max(1));
        self.color_map_drain_scheduled = true;
        self.actions.push_back(GbeAction::Schedule {
            delay,
            event: GbeEvent::ColorMapDrain {
                epoch: self.timing_epoch,
            },
        });
    }

    fn post_timing_interrupts(&mut self, y: u16) {
        let targets = [
            upper_endpoint(self.registers.vt[VT_INTR01]),
            lower_endpoint(self.registers.vt[VT_INTR01]),
            upper_endpoint(self.registers.vt[VT_INTR23]),
            lower_endpoint(self.registers.vt[VT_INTR23]),
        ];
        for (index, target) in targets.into_iter().enumerate() {
            if y == target && !self.interrupt_posted[index] {
                self.interrupt_posted[index] = true;
                let id = CrimeTransactionId::new(self.next_transaction);
                self.next_transaction = self.next_transaction.wrapping_add(1);
                self.outstanding_interrupt_posts.insert(id);
                self.actions
                    .push_back(GbeAction::StartCgi(CrimeCgiTransaction {
                        id,
                        controller: self.id,
                        target: self.wiring.crime,
                        operation: CrimeLinkOperation::InterruptPost(CrimeInterruptPost {
                            interrupt_bit: 16 + index as u8,
                            asserted: true,
                        }),
                    }));
            } else if y != target {
                self.interrupt_posted[index] = false;
            }
        }
    }

    fn cancel_plane_dma(&mut self, plane: Plane) {
        self.pending_dma.retain(|job| {
            !matches!(
                job.destination,
                DmaDestination::Line {
                    plane: job_plane,
                    ..
                } if job_plane == plane
            )
        });
        self.line_fetches.clear();
    }

    fn trace_dma_error(&mut self, event: &str, address: u64, length: usize) {
        self.actions
            .push_back(GbeAction::Trace(Box::new(OwnedTraceEvent {
                level: TraceLevel::Warn,
                target: "gbe.dma".into(),
                event: event.to_owned().into(),
                fields: vec![
                    OwnedTraceField {
                        key: "address".into(),
                        value: OwnedTraceValue::Hex64(address),
                    },
                    OwnedTraceField {
                        key: "length".into(),
                        value: OwnedTraceValue::U64(length as u64),
                    },
                ]
                .into(),
            })));
    }

    fn reset_state(&mut self) {
        self.registers = GbeRegisters::new();
        self.observed_time = SimTime::ZERO;
        self.scan_origin_time = SimTime::ZERO;
        self.scan_origin_pixel = 0;
        self.pixel_clock_remainder = 0;
        self.timing_epoch = self.timing_epoch.wrapping_add(1);
        self.scheduled_scanline_cycles = None;
        self.ttl_clock = None;
        self.differential_clock = None;
        self.sense_n = true;
        self.frame_lock = false;
        self.frame_lock_pending = false;
        self.frame_lock_last_rising = None;
        self.frame_lock_period_ticks = None;
        self.f2rf_level = false;
        self.ddc_clock_high = [true; 2];
        self.ddc_data_high = [true; 2];
        self.auxiliary_inputs = self.wiring.auxiliary_inputs;
        self.color_map_fifo.clear();
        self.deferred_color_map.clear();
        self.color_map_drain_scheduled = false;
        self.actions.clear();
        self.next_transaction = 0;
        self.next_read_order = 0;
        self.next_read_commit = 0;
        self.pending_dma.clear();
        self.outstanding_dma.clear();
        self.outstanding_interrupt_posts.clear();
        self.completed_reads.clear();
        self.reads_in_flight = 0;
        self.writes_in_flight = 0;
        self.normal_tile_pointers.clear();
        self.overlay_tile_pointers.clear();
        self.did_frame_table.clear();
        self.did_frame_chunks_remaining = 0;
        self.did_line_blocks.clear();
        self.line_fetches.clear();
        self.working_frame = None;
        self.next_frame_sequence = 0;
        self.interrupt_posted = [false; 4];
        self.capture_writes_remaining = 0;
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
        self.reset_state();
    }
}

impl BusDeviceRole<CrimeCgiTransaction> for Gbe {
    type Response = CrimeLinkDeviceResponse<CrimeCgiCompletion>;

    fn accept(&mut self, transaction: CrimeCgiTransaction) -> Self::Response {
        let result = match &transaction.operation {
            CrimeLinkOperation::Pio(CrimePioRequest { address, transfer }) => {
                if address & 3 != 0 {
                    Err(CrimeBusError::Access)
                } else {
                    match transfer.view() {
                        CrimeTransferView::Read { length: 4 } => self.access_read(*address),
                        CrimeTransferView::Write { data, byte_enable }
                            if data.len() == 4
                                && byte_enable.len() == 4
                                && byte_enable.iter().all(|enabled| enabled) =>
                        {
                            let value = u32::from_be_bytes(
                                data.try_into().expect("validated GBE write width"),
                            );
                            match self.access_write(
                                transaction.controller,
                                transaction.id,
                                *address,
                                value,
                            ) {
                                Ok(true) => Ok(CrimeCompletionPayload::WriteComplete),
                                Ok(false) => return CrimeLinkDeviceResponse::Deferred,
                                Err(error) => Err(error),
                            }
                        }
                        CrimeTransferView::Read { .. } | CrimeTransferView::Write { .. } => {
                            Err(CrimeBusError::Access)
                        }
                    }
                }
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

impl BusControllerRole<CrimeCgiCompletion> for Gbe {
    fn complete(&mut self, completion: CrimeCgiCompletion) {
        self.accept_dma_completion(completion);
    }
}

fn lower_endpoint(value: u32) -> u16 {
    (value & 0x0fff) as u16
}

fn upper_endpoint(value: u32) -> u16 {
    ((value >> 12) & 0x0fff) as u16
}

fn triggered_counter(position: u16, trigger: u16, preset: u16, modulus: u64) -> u16 {
    let modulus = u32::try_from(modulus).unwrap_or(4_096).clamp(1, 4_096);
    let position = u32::from(position) % modulus;
    let trigger = u32::from(trigger) % modulus;
    let distance = (position + modulus - trigger) % modulus;
    (u32::from(preset) + distance) as u16 & 0x0fff
}

fn interval_contains(value: u32, position: u16) -> bool {
    let off = lower_endpoint(value);
    let on = upper_endpoint(value);
    if on == off {
        false
    } else if on < off {
        position >= on && position < off
    } else {
        position >= on || position < off
    }
}

fn interval_offset(value: u32, position: u16) -> Option<u16> {
    interval_contains(value, position)
        .then(|| position.wrapping_sub(upper_endpoint(value)) & 0x0fff)
}

fn interval_length(value: u32, modulus: usize) -> usize {
    if modulus == 0 {
        return 0;
    }
    let on = usize::from(upper_endpoint(value)) % modulus;
    let off = usize::from(lower_endpoint(value)) % modulus;
    if on == off {
        0
    } else if on < off {
        off - on
    } else {
        modulus - on + off
    }
}

fn interval_offset_with_modulus(value: u32, position: u16, modulus: usize) -> Option<usize> {
    if modulus == 0 {
        return None;
    }
    let on = usize::from(upper_endpoint(value)) % modulus;
    let off = usize::from(lower_endpoint(value)) % modulus;
    let position = usize::from(position) % modulus;
    let active = if on == off {
        false
    } else if on < off {
        (on..off).contains(&position)
    } else {
        position >= on || position < off
    };
    active.then(|| (position + modulus - on) % modulus)
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn tile_columns(size_register: u32) -> usize {
    usize::try_from((size_register >> 5) & 0xff).unwrap_or(0)
        + usize::from(size_register & 0x1f != 0)
}

fn plane_width(size_register: u32, depth: PlaneDepth) -> usize {
    usize::try_from((size_register >> 5) & 0xff)
        .unwrap_or(0)
        .saturating_mul(depth.tile_width())
        .saturating_add(
            usize::try_from(size_register & 0x1f)
                .unwrap_or(0)
                .saturating_mul(32)
                / depth.bytes_per_pixel(),
        )
}

fn decode_plane_line(fetch: &PlaneLineFetch, depth: PlaneDepth, width: usize) -> Vec<u32> {
    let mut output = vec![0; width];
    for segment in &fetch.segments {
        let available = segment
            .data
            .len()
            .checked_div(depth.bytes_per_pixel())
            .unwrap_or(0)
            .saturating_sub(segment.source_pixel);
        let pixels = segment
            .pixels
            .min(available)
            .min(output.len().saturating_sub(segment.output_pixel));
        if pixels == 0 {
            continue;
        }
        let decoded = decode_raw_pixels_into(
            &segment.data,
            depth,
            segment.source_pixel,
            &mut output[segment.output_pixel..segment.output_pixel + pixels],
        );
        debug_assert_eq!(decoded, pixels);
    }
    output
}

fn copy_into(target: &mut [u8], offset: usize, source: &[u8]) {
    let Some(destination) = target.get_mut(offset..offset.saturating_add(source.len())) else {
        return;
    };
    destination.copy_from_slice(source);
}

#[cfg(test)]
mod tests {
    use se_core::role::{BusControllerRole, BusDeviceRole};

    use super::*;
    use crate::chipset::crime::protocol::{CrimeByteEnable, CrimeMemoryFault, CrimePioRequest};

    const GBE: ComponentId = ComponentId::new(1);
    const CRIME: ComponentId = ComponentId::new(2);

    fn gbe() -> Gbe {
        Gbe::new(
            GBE,
            "GBE",
            1_000_000_000,
            GbeWiring {
                crime: CRIME,
                crt_ddc: ComponentId::new(3),
                flat_panel_ddc: ComponentId::new(4),
                auxiliary_inputs: [true; AUXILIARY_PIN_COUNT],
            },
        )
    }

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

    fn read_word(gbe: &mut Gbe, address: u64) -> u32 {
        let CrimeCompletionPayload::ReadData(data) =
            result(gbe, address, CrimeTransfer::read(4)).unwrap()
        else {
            panic!("GBE read returned the wrong payload");
        };
        u32::from_be_bytes(data.as_ref().try_into().unwrap())
    }

    #[test]
    fn revision_two_holes_are_unsupported() {
        let mut gbe = gbe();
        for address in [
            registers::GBE_BASE + 0x18,
            registers::GBE_BASE + 0x1c,
            registers::GBE_BASE + 0x0006_8000,
            registers::COLOR_MAP_END,
        ] {
            assert_eq!(
                result(&mut gbe, address, CrimeTransfer::read(4)),
                Err(CrimeBusError::Unsupported)
            );
        }
    }

    #[test]
    fn accesses_are_aligned_big_endian_full_word_transfers() {
        let mut gbe = gbe();
        assert_eq!(
            result(
                &mut gbe,
                registers::CONTROL_STATUS + 1,
                CrimeTransfer::read(4),
            ),
            Err(CrimeBusError::Access)
        );
        assert_eq!(
            result(&mut gbe, registers::CONTROL_STATUS, CrimeTransfer::read(8),),
            Err(CrimeBusError::Access)
        );
        let value = 0x300a_a000_u32;
        result(
            &mut gbe,
            registers::CONTROL_STATUS,
            CrimeTransfer::write(value.to_be_bytes().into(), CrimeByteEnable::from([true; 4])),
        )
        .unwrap();
        let read = result(&mut gbe, registers::CONTROL_STATUS, CrimeTransfer::read(4)).unwrap();
        assert!(matches!(read, CrimeCompletionPayload::ReadData(_)));
    }

    #[test]
    fn auxiliary_inputs_resolve_control_status_and_high_impedance_outputs() {
        let mut gbe = gbe();
        let control = 0x020a_a000_u32;
        result(
            &mut gbe,
            registers::CONTROL_STATUS,
            CrimeTransfer::write(
                control.to_be_bytes().into(),
                CrimeByteEnable::from([true; 4]),
            ),
        )
        .unwrap();

        let input_data_mask = [3, 4, 5, 6, 9]
            .into_iter()
            .map(auxiliary_data_mask)
            .fold(0, |mask, bit| mask | bit);
        assert_eq!(
            read_word(&mut gbe, registers::CONTROL_STATUS) & input_data_mask,
            input_data_mask
        );
        assert_eq!(gbe.registers.control_status & input_data_mask, 0);
        let outputs = gbe.output_pins();
        assert_eq!(outputs.gpio[0], Some(false));
        for index in [3, 4, 5, 6, 9] {
            assert_eq!(outputs.gpio[index], None);
        }

        gbe.apply_external_input(GbeExternalInput::Auxiliary([false; AUXILIARY_PIN_COUNT]));
        assert_eq!(
            read_word(&mut gbe, registers::CONTROL_STATUS) & input_data_mask,
            0
        );
        assert_eq!(gbe.registers.control_status, control | 0x11);

        let all_inputs = (0..AUXILIARY_PIN_COUNT)
            .map(auxiliary_output_disable_mask)
            .fold(0, |mask, bit| mask | bit);
        result(
            &mut gbe,
            registers::CONTROL_STATUS,
            CrimeTransfer::write(
                all_inputs.to_be_bytes().into(),
                CrimeByteEnable::from([true; 4]),
            ),
        )
        .unwrap();
        let levels = [
            true, false, true, false, true, false, true, false, true, false,
        ];
        gbe.apply_external_input(GbeExternalInput::Auxiliary(levels));
        assert_eq!(gbe.output_pins().gpio, [None; AUXILIARY_PIN_COUNT]);
        let resolved = read_word(&mut gbe, registers::CONTROL_STATUS);
        for (index, expected) in levels.into_iter().enumerate() {
            assert_eq!(resolved & auxiliary_data_mask(index) != 0, expected);
        }
    }

    #[test]
    fn reset_restores_board_auxiliary_input_levels() {
        let mut gbe = gbe();
        gbe.apply_external_input(GbeExternalInput::Auxiliary([false; AUXILIARY_PIN_COUNT]));
        gbe.reset();
        assert_eq!(gbe.auxiliary_inputs, [true; AUXILIARY_PIN_COUNT]);
    }

    #[test]
    fn pll_uses_exact_revision_one_run_bit_and_frequency() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN | 100 | (4 << 8) | (2 << 14);
        let projection = gbe.pixel_projection().unwrap();
        assert_eq!(projection.frequency_numerator_hz(), 2_020_000_000);
        assert_eq!(projection.frequency_denominator(), 20);
    }

    #[test]
    fn dma_reads_commit_in_issue_order() {
        let mut gbe = gbe();
        gbe.normal_tile_pointers.resize(64, 0);
        gbe.queue_dma_read(0x1fe0, 64, DmaDestination::TilePointers(Plane::Normal));
        gbe.pump_dma();
        let mut transactions = Vec::new();
        while let GbePoll::Action(action) = gbe.poll() {
            if let GbeAction::StartCgi(transaction) = action {
                transactions.push(transaction);
            }
        }
        assert_eq!(transactions.len(), 2);
        for transaction in transactions.into_iter().rev() {
            gbe.complete(CrimeCgiCompletion {
                id: transaction.id,
                result: Ok(CrimeCompletionPayload::ReadData(
                    vec![transaction.id.get() as u8; 32].into(),
                )),
                memory_fault: None,
            });
        }
        assert_eq!(&gbe.normal_tile_pointers[0..32], &[0; 32]);
        assert_eq!(&gbe.normal_tile_pointers[32..64], &[1; 32]);
    }

    #[test]
    fn pixel_bursts_split_at_eight_kibibyte_boundaries_and_stop_at_eight_reads() {
        let mut gbe = gbe();
        assert_eq!(
            gbe.queue_dma_read(
                0x1f00,
                512,
                DmaDestination::Line {
                    frame: 0,
                    y: 0,
                    plane: Plane::Normal,
                    segment: 0,
                },
            ),
            2
        );
        for address in (0x4000..0x6000).step_by(512) {
            gbe.queue_dma_read(address, 512, DmaDestination::TilePointers(Plane::Normal));
        }
        gbe.pump_dma();
        assert_eq!(gbe.reads_in_flight, 8);
        assert!(gbe.outstanding_dma.values().all(|pending| !pending.write));
    }

    #[test]
    fn full_color_map_fifo_defers_the_sixty_fifth_write() {
        let mut gbe = gbe();
        for index in 0..64 {
            assert!(
                gbe.access_write(
                    CRIME,
                    CrimeTransactionId::new(index),
                    registers::COLOR_MAP_START + index as u64 * 4,
                    index as u32,
                )
                .unwrap()
            );
        }
        assert_eq!(gbe.color_map_fifo.len(), 64);
        assert_eq!(
            gbe.access_read(COLOR_MAP_FIFO),
            Ok(CrimeCompletionPayload::ReadData(
                63_u32.to_be_bytes().into()
            ))
        );
        assert!(
            !gbe.access_write(
                CRIME,
                CrimeTransactionId::new(64),
                registers::COLOR_MAP_START,
                0x00ab_cdef,
            )
            .unwrap()
        );
        gbe.drain_one_color_map_entry();
        assert_eq!(gbe.color_map_fifo.len(), 64);
        assert!(matches!(
            gbe.poll(),
            GbePoll::Action(GbeAction::CompleteCgiDevice(CrimeCgiCompletion {
                id,
                ..
            })) if id == CrimeTransactionId::new(64)
        ));
    }

    #[test]
    fn color_map_fifo_drains_one_entry_on_each_enabled_pixel_clock() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN;
        gbe.registers.vt[VT_XY_MAX] = (1 << 12) | 3;
        gbe.registers.vt[VT_HCMAP] = 2;
        gbe.registers.vt[VT_XY] = 0;
        gbe.restart_timing(true);
        gbe.access_write(
            CRIME,
            CrimeTransactionId::new(1),
            registers::COLOR_MAP_START,
            0x1122_3300,
        )
        .unwrap();

        let mut scheduled = None;
        while let GbePoll::Action(action) = gbe.poll() {
            if let GbeAction::Schedule {
                delay,
                event: event @ GbeEvent::ColorMapDrain { .. },
            } = action
            {
                scheduled = Some((delay, event));
            }
        }
        assert_eq!(gbe.registers.color_map[0], 0);
        let (delay, event) = scheduled.expect("an enabled color-map window must drain the FIFO");
        gbe.observe_time(SimTime::new(delay.get()));
        gbe.handle_event(event);
        assert_eq!(gbe.registers.color_map[0], 0x1122_3300);
        assert!(gbe.color_map_fifo.is_empty());
    }

    #[test]
    fn linux_linear_tile_mapping_becomes_a_six_hundred_forty_by_four_eighty_raster() {
        let mut gbe = gbe();
        gbe.registers.frame[0] = 1 << 5;
        gbe.registers.frame[1] = 600 << 16;
        gbe.registers.frame[2] = 0x2_0001;
        gbe.registers.vt[VT_XY_MAX] = (525 << 12) | 799;
        gbe.registers.vt[VT_VBLANK] = (480 << 12) | 525;
        gbe.registers.vt[VT_VPIXEN] = (525 << 12) | 480;
        gbe.normal_tile_pointers = vec![0, 1];

        assert_eq!(gbe.display_dimensions(), (640, 480));
        assert_eq!(gbe.active_frame_y(525, 480), None);
        assert_eq!(gbe.active_frame_y(0, 480), Some(0));
        assert_eq!(gbe.active_frame_y(479, 480), Some(479));
        assert_eq!(gbe.active_frame_y(480, 480), None);

        gbe.line_fetches.insert((0, 0), LineFetch::new());
        gbe.queue_plane_line(0, 0, Plane::Normal, 640);
        let fetch = gbe.line_fetches.get(&(0, 0)).unwrap();
        assert_eq!(fetch.normal.segments.len(), 2);
        assert_eq!(
            fetch
                .normal
                .segments
                .iter()
                .map(|segment| (segment.source_pixel, segment.output_pixel, segment.pixels))
                .collect::<Vec<_>>(),
            vec![(0, 0, 512), (0, 512, 128)]
        );
        assert_eq!(
            gbe.pending_dma
                .iter()
                .map(|job| job.address)
                .collect::<Vec<_>>(),
            vec![0x1_0000, 0x1_0200]
        );
    }

    #[test]
    fn linux_linear_mapping_publishes_a_complete_black_vga_frame_when_dma_is_off() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN;
        gbe.registers.frame[0] = 1 << 5;
        gbe.registers.frame[1] = 600 << 16;
        gbe.registers.vt[VT_XY_MAX] = (525 << 12) | 799;
        gbe.registers.vt[VT_VBLANK] = (480 << 12) | 525;
        gbe.registers.vt[VT_VPIXEN] = (525 << 12) | 480;
        gbe.registers.vt[VT_XY] = 0;
        gbe.restart_timing(true);

        let mut now = SimTime::ZERO;
        let mut published = None;
        for _ in 0..=480 {
            let mut scanline = None;
            while let GbePoll::Action(action) = gbe.poll() {
                match action {
                    GbeAction::Schedule {
                        delay,
                        event: event @ GbeEvent::Scanline { .. },
                    } => scanline = Some((delay, event)),
                    GbeAction::PublishFrame(frame) => published = Some(frame),
                    _ => {}
                }
            }
            if published.is_some() {
                break;
            }
            let (delay, event) = scanline.expect("running timing must schedule each scanline");
            now = SimTime::new(now.get() + delay.get());
            gbe.observe_time(now);
            gbe.handle_event(event);
        }

        let frame = published.expect("the active VGA raster must publish a frame");
        assert_eq!((frame.width, frame.height, frame.stride), (640, 480, 2_560));
        assert_eq!(frame.rgba.len(), 640 * 480 * 4);
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 0xff])
        );
    }

    #[test]
    fn palette_changes_between_scanline_deadlines_produce_deterministic_tearing() {
        let mut gbe = gbe();
        gbe.registers.vt[VT_XY_MAX] = (2 << 12) | 1;
        gbe.registers.vt[VT_VPIXEN] = 2;
        gbe.registers.frame[0] = 1;
        gbe.registers.frame[1] = 2 << 16;
        gbe.registers.wid[0] = (1 << 10) | 3;
        gbe.working_frame = Some(WorkingFrame {
            sequence: 0,
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        });
        gbe.line_fetches.insert((0, 0), LineFetch::new());
        gbe.line_fetches.insert((0, 1), LineFetch::new());

        gbe.registers.color_map[0] = 0xff00_0000;
        gbe.finish_scanline(0);
        gbe.registers.color_map[0] = 0x0000_ff00;
        gbe.finish_scanline(1);

        let frame = gbe.working_frame.unwrap();
        assert_eq!(&frame.rgba[..8], &[0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff]);
        assert_eq!(&frame.rgba[8..], &[0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff]);
    }

    #[test]
    fn prom_cursor_timing_projects_the_active_origin_to_counter_zero() {
        let mut gbe = gbe();
        gbe.registers.vt[VT_XY_MAX] = (106 << 12) | 1_679;
        gbe.registers.vt[VT_VBLANK] = 64 << 12;
        gbe.registers.vt[VT_HPIXEN] = (1_658 << 12) | 1_659;
        gbe.registers.vt[VT_VPIXEN] = (105 << 12) | 106;
        gbe.registers.vt[registers::CURSOR_START_XY] = 0x00fd_765a;
        gbe.registers.cursor[1] = 1;
        gbe.registers.cursor[2] = 0xff00_0000;
        gbe.registers.cursor_glyph[0] = 1 << 30;
        gbe.working_frame = Some(WorkingFrame {
            sequence: 0,
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        });
        gbe.line_fetches.insert((0, 0), LineFetch::new());

        gbe.finish_scanline(105);

        assert_eq!(triggered_counter(1_658, 0x65a, 0x0fe0, 1_680), 0);
        assert_eq!(triggered_counter(0, 0x65a, 0x0fe0, 1_680), 22);
        assert_eq!(triggered_counter(105, 64, 0x0fd7, 107), 0);
        assert_eq!(triggered_counter(0, 64, 0x0fd7, 107), 2);
        assert_eq!(gbe.working_frame.unwrap().rgba, [0xff, 0, 0, 0xff]);
    }

    #[test]
    fn cursor_counter_advances_linearly_across_the_scan_wrap() {
        let scan_width = 1_680;
        let active_x_start = 1_658;
        let trigger = 0x65a;
        let start = triggered_counter(active_x_start, trigger, 0x0fe0, scan_width);

        for offset in 0..128 {
            let scan_x = (u64::from(active_x_start) + offset) % scan_width;
            let projected = (u64::from(start) + offset) as u16 & 0x0fff;
            assert_eq!(
                projected,
                triggered_counter(scan_x as u16, trigger, 0x0fe0, scan_width)
            );
        }
    }

    #[test]
    fn running_timing_publishes_black_frames_when_display_dma_is_disabled() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN;
        gbe.registers.vt[VT_XY_MAX] = (3 << 12) | 3;
        gbe.registers.vt[VT_VPIXEN] = 2;
        gbe.registers.frame[0] = 1;
        gbe.registers.frame[1] = 2 << 16;
        gbe.registers.vt[VT_XY] = 0;
        gbe.restart_timing(true);

        let mut now = SimTime::ZERO;
        let mut published = None;
        for _ in 0..5 {
            let mut scheduled = None;
            while let GbePoll::Action(action) = gbe.poll() {
                match action {
                    GbeAction::Schedule { delay, event } => scheduled = Some((delay, event)),
                    GbeAction::PublishFrame(frame) => published = Some(frame),
                    _ => {}
                }
            }
            if published.is_some() {
                break;
            }
            let (delay, event) = scheduled.expect("running timing must schedule a scanline");
            now = SimTime::new(now.get() + delay.get());
            gbe.observe_time(now);
            gbe.handle_event(event);
        }
        let frame = published.expect("the vertical active region must publish one frame");
        assert_eq!((frame.width, frame.height, frame.stride), (32, 2, 128));
        assert_eq!(frame.rgba.len(), 32 * 2 * 4);
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 0xff])
        );
    }

    #[test]
    fn frame_lock_period_selects_pal_and_presets_y_at_the_next_scanline() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN;
        gbe.registers.vt[VT_XY_MAX] = (9 << 12) | 3;
        gbe.registers.vt[VT_F2RF_LOCK] = 7 << 12;
        gbe.registers.vt[VT_XY] = 0;
        gbe.restart_timing(true);
        gbe.apply_external_input(GbeExternalInput::FrameLock(true));
        let (delay, event) = loop {
            let GbePoll::Action(action) = gbe.poll() else {
                panic!("running timing must schedule a scanline");
            };
            if let GbeAction::Schedule { delay, event } = action {
                break (delay, event);
            }
        };
        gbe.observe_time(SimTime::new(delay.get()));
        gbe.handle_event(event);
        assert_eq!((gbe.scan_position() >> 12) & 0x0fff, 7);

        gbe.apply_external_input(GbeExternalInput::FrameLock(false));
        gbe.observe_time(SimTime::new(20_000_000));
        gbe.apply_external_input(GbeExternalInput::FrameLock(true));
        assert_eq!(gbe.frame_rate_hz(), Some(50));
    }

    #[test]
    fn four_vertical_interrupts_map_to_crime_bits_sixteen_through_nineteen() {
        let mut gbe = gbe();
        gbe.registers.vt[VT_INTR01] = (5 << 12) | 6;
        gbe.registers.vt[VT_INTR23] = (7 << 12) | 8;
        for (y, expected) in [(5, 16), (6, 17), (7, 18), (8, 19)] {
            gbe.post_timing_interrupts(y);
            assert!(matches!(
                gbe.poll(),
                GbePoll::Action(GbeAction::StartCgi(CrimeCgiTransaction {
                    operation: CrimeLinkOperation::InterruptPost(CrimeInterruptPost {
                        interrupt_bit,
                        asserted: true,
                    }),
                    ..
                })) if interrupt_bit == expected
            ));
        }
    }

    #[test]
    fn one_line_timing_does_not_repeat_interrupt_rising_edges() {
        let mut gbe = gbe();
        gbe.registers.control_status = 3 << 28;
        gbe.registers.dot_clock = DOT_CLOCK_RUN;
        gbe.registers.vt[VT_XY_MAX] = 0;
        gbe.registers.vt[VT_XY] = 0;
        gbe.restart_timing(true);

        let mut now = SimTime::ZERO;
        let mut interrupt_posts = 0;
        for _ in 0..3 {
            let mut scheduled = None;
            while let GbePoll::Action(action) = gbe.poll() {
                match action {
                    GbeAction::Schedule { delay, event } => scheduled = Some((delay, event)),
                    GbeAction::StartCgi(CrimeCgiTransaction {
                        operation: CrimeLinkOperation::InterruptPost(_),
                        ..
                    }) => interrupt_posts += 1,
                    _ => {}
                }
            }
            let (delay, event) = scheduled.expect("running timing must schedule a scanline");
            now = SimTime::new(now.get() + delay.get());
            gbe.observe_time(now);
            gbe.handle_event(event);
        }
        while let GbePoll::Action(action) = gbe.poll() {
            if matches!(
                action,
                GbeAction::StartCgi(CrimeCgiTransaction {
                    operation: CrimeLinkOperation::InterruptPost(_),
                    ..
                })
            ) {
                interrupt_posts += 1;
            }
        }

        assert_eq!(interrupt_posts, 4);
    }

    #[test]
    fn crt_and_flat_panel_ddc_lines_are_independent() {
        let mut gbe = gbe();
        assert!(
            gbe.access_write(CRIME, CrimeTransactionId::new(1), registers::CRT_DDC, 0,)
                .unwrap()
        );
        assert!(matches!(
            gbe.poll(),
            GbePoll::Action(GbeAction::SetDdc { bus, drive })
                if bus == ComponentId::new(3) && drive.clock_low && drive.data_low
        ));

        gbe.observe_ddc(TwoWireLineDelivery {
            bus: ComponentId::new(3),
            source: GBE,
            time: SimTime::ZERO,
            source_clock_low: true,
            source_data_low: true,
            clock_low: true,
            data_low: true,
        });
        assert_eq!(gbe.registers.crt_ddc, 0);
        assert_eq!(gbe.registers.flat_panel_ddc, 3);

        gbe.observe_ddc(TwoWireLineDelivery {
            bus: ComponentId::new(4),
            source: GBE,
            time: SimTime::ZERO,
            source_clock_low: false,
            source_data_low: true,
            clock_low: false,
            data_low: true,
        });
        assert_eq!(gbe.registers.crt_ddc, 0);
        assert_eq!(gbe.registers.flat_panel_ddc, 2);
    }

    #[test]
    fn serialized_state_preserves_partial_frame_fifo_and_inflight_dma() {
        let mut reference = gbe();
        reference.apply_external_input(GbeExternalInput::Auxiliary([
            true, false, true, false, true, false, true, false, true, false,
        ]));
        reference.color_map_fifo.push_back(ColorMapWrite {
            index: 7,
            value: 0x1122_3300,
        });
        reference.working_frame = Some(WorkingFrame {
            sequence: 9,
            width: 2,
            height: 2,
            rgba: vec![1; 16],
        });
        reference.next_frame_sequence = 10;
        reference.normal_tile_pointers.resize(32, 0);
        reference.queue_dma_read(0x2_0000, 32, DmaDestination::TilePointers(Plane::Normal));
        reference.pump_dma();

        let encoded = postcard::to_stdvec(&reference.save_state()).unwrap();
        let state: GbeState = postcard::from_bytes(&encoded).unwrap();
        let mut restored = gbe();
        restored.restore_state(state).unwrap();

        assert_eq!(restored.auxiliary_inputs, reference.auxiliary_inputs);

        assert_eq!(
            postcard::to_stdvec(&restored.save_state()).unwrap(),
            encoded
        );
    }

    #[test]
    fn serialized_state_preserves_device_local_trace_actions() {
        let mut reference = gbe();
        BusControllerRole::<CrimeCgiCompletion>::complete(
            &mut reference,
            CrimeCgiCompletion {
                id: CrimeTransactionId::new(17),
                result: Ok(CrimeCompletionPayload::WriteComplete),
                memory_fault: None,
            },
        );

        let encoded = postcard::to_stdvec(&reference.save_state()).unwrap();
        let state: GbeState = postcard::from_bytes(&encoded).unwrap();
        let mut restored = gbe();
        restored.restore_state(state).unwrap();

        let expected = reference.poll();
        let actual = restored.poll();
        assert_eq!(actual, expected);
        assert!(matches!(
            actual,
            GbePoll::Action(GbeAction::Trace(event))
                if event.target == "gbe.dma" && event.event == "unexpected-completion"
        ));
    }

    #[test]
    fn state_restore_preserves_name_and_rejects_wiring_and_timebase_changes_atomically() {
        let mut source = gbe();
        source.apply_external_input(GbeExternalInput::Auxiliary([false; AUXILIARY_PIN_COUNT]));
        let mut renamed = Gbe::new(GBE, "replacement name", source.timebase_hz, source.wiring);
        renamed.restore_state(source.save_state()).unwrap();
        assert_eq!(renamed.name(), "replacement name");
        assert_eq!(renamed.auxiliary_inputs, [false; AUXILIARY_PIN_COUNT]);

        let mismatched = [
            Gbe::new(
                GBE,
                "source",
                source.timebase_hz,
                GbeWiring {
                    crime: ComponentId::new(99),
                    ..source.wiring
                },
            )
            .save_state(),
            Gbe::new(GBE, "source", source.timebase_hz / 2, source.wiring).save_state(),
        ];
        for state in mismatched {
            let mut target = gbe();
            let before = target.clone();
            assert!(matches!(
                target.restore_state(state),
                Err(ComponentStateError::ConfigurationMismatch { .. })
            ));
            assert_eq!(target, before);
        }
    }

    #[test]
    fn state_restore_rejects_malformed_clock_dma_counters_and_frame_atomically() {
        let mut dma = gbe();
        dma.normal_tile_pointers.resize(32, 0);
        dma.queue_dma_read(0x2_0000, 32, DmaDestination::TilePointers(Plane::Normal));
        dma.pump_dma();

        let mut zero_clock = gbe().save_state();
        zero_clock.ttl_clock = Some(ExternalClockState {
            numerator_hz: 0,
            denominator: 1,
        });
        let mut missing_order = dma.save_state();
        missing_order
            .outstanding_dma
            .values_mut()
            .next()
            .unwrap()
            .read_order = None;
        let mut wrong_count = dma.save_state();
        wrong_count.reads_in_flight += 1;
        let mut bad_frame = gbe().save_state();
        bad_frame.working_frame = Some(WorkingFrame {
            sequence: 0,
            width: 2,
            height: 2,
            rgba: vec![0; 15],
        });
        bad_frame.next_frame_sequence = 1;

        for state in [zero_clock, missing_order, wrong_count, bad_frame] {
            let mut target = gbe();
            let before = target.clone();
            assert!(matches!(
                target.restore_state(state),
                Err(ComponentStateError::InvalidState { .. })
            ));
            assert_eq!(target, before);
        }
    }

    #[test]
    fn one_to_one_capture_emits_big_endian_rgba_in_a_five_hundred_twelve_byte_write() {
        let mut gbe = gbe();
        gbe.registers.video_capture[0] = 1 << 12;
        gbe.registers.video_capture[1] = 1 << 12;
        gbe.registers.video_capture[2] = 1 | (1 << 2);
        gbe.registers.video_capture[4] = 1 << 16;
        let frame = GbeFrame {
            sequence: 0,
            completed_at: SimTime::ZERO,
            width: 2,
            height: 2,
            stride: 8,
            field: GbeFrameField::Progressive,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        };
        gbe.start_video_capture(&frame);
        let transaction = loop {
            let GbePoll::Action(action) = gbe.poll() else {
                panic!("capture must issue one CGI write");
            };
            if let GbeAction::StartCgi(transaction) = action {
                break transaction;
            }
        };
        let transaction_id = transaction.id;
        let CrimeLinkOperation::Dma(request) = transaction.operation else {
            panic!("capture must use DMA");
        };
        assert_eq!(request.address, 0x1_0000);
        let CrimeTransferView::Write { data, byte_enable } = request.transfer.view() else {
            panic!("capture DMA must be a write");
        };
        assert_eq!(data.len(), 512);
        assert!(byte_enable.iter().all(|enabled| enabled));
        assert_eq!(
            &data[..16],
            &[
                1, 2, 3, 0xff, 5, 6, 7, 0xff, 9, 10, 11, 0xff, 13, 14, 15, 0xff
            ]
        );
        assert!(data[16..].iter().all(|value| *value == 0));
        assert_eq!(gbe.registers.video_capture[3] & (1 << 4), 0);
        gbe.complete(CrimeCgiCompletion {
            id: transaction_id,
            result: Ok(CrimeCompletionPayload::WriteComplete),
            memory_fault: None,
        });
        assert_ne!(gbe.registers.video_capture[3] & (1 << 3), 0);
        assert_ne!(gbe.registers.video_capture[3] & 1, 0);
        assert_eq!(gbe.registers.video_capture[3] & (1 << 4), 0);
    }

    #[test]
    fn capture_page_exhaustion_marks_the_field_corrupt_without_partial_dma() {
        let mut gbe = gbe();
        gbe.registers.video_capture[0] = 511 << 12;
        gbe.registers.video_capture[1] = 320 << 12;
        gbe.registers.video_capture[2] = 1;
        gbe.registers.video_capture[4..=8].fill(0x0001_0001);
        let frame = GbeFrame {
            sequence: 0,
            completed_at: SimTime::ZERO,
            width: 512,
            height: 321,
            stride: 2_048,
            field: GbeFrameField::Progressive,
            rgba: vec![0x7f; 512 * 321 * 4],
        };

        gbe.start_video_capture(&frame);

        assert_ne!(gbe.registers.video_capture[3] & (1 << 2), 0);
        assert_eq!(gbe.registers.video_capture[3] & (1 << 3), 0);
        assert_eq!(gbe.capture_writes_remaining, 0);
        assert!(gbe.pending_dma.is_empty());
        assert!(matches!(gbe.poll(), GbePoll::Idle));
    }

    #[test]
    fn capture_stream_crosses_a_descriptor_page_only_on_five_hundred_twelve_byte_boundaries() {
        let mut gbe = gbe();
        gbe.registers.video_capture[0] = 299 << 12;
        gbe.registers.video_capture[1] = 55 << 12;
        gbe.registers.video_capture[2] = 1;
        gbe.registers.video_capture[4] = (1 << 16) | 2;
        let frame = GbeFrame {
            sequence: 0,
            completed_at: SimTime::ZERO,
            width: 300,
            height: 56,
            stride: 1_200,
            field: GbeFrameField::Progressive,
            rgba: vec![0x40; 300 * 56 * 4],
        };

        gbe.start_video_capture(&frame);

        let first = loop {
            let GbePoll::Action(action) = gbe.poll() else {
                panic!("capture must start its first write");
            };
            if let GbeAction::StartCgi(transaction) = action {
                break transaction;
            }
        };
        let CrimeLinkOperation::Dma(first) = first.operation else {
            panic!("capture must use DMA");
        };
        let mut addresses = vec![first.address];
        addresses.extend(gbe.pending_dma.iter().map(|job| job.address));
        assert!(
            addresses
                .windows(2)
                .any(|pair| pair == [0x1_fe00, 0x2_0000])
        );
        assert!(
            gbe.pending_dma
                .iter()
                .all(|job| job.transfer.length() == 512)
        );
    }

    #[test]
    fn capture_write_fault_aborts_the_field_without_end_of_field() {
        let mut gbe = gbe();
        gbe.registers.video_capture[0] = 1 << 12;
        gbe.registers.video_capture[1] = 1 << 12;
        gbe.registers.video_capture[2] = 1;
        gbe.registers.video_capture[4] = 1 << 16;
        let frame = GbeFrame {
            sequence: 0,
            completed_at: SimTime::ZERO,
            width: 2,
            height: 2,
            stride: 8,
            field: GbeFrameField::Progressive,
            rgba: vec![0; 16],
        };
        gbe.start_video_capture(&frame);
        let id = loop {
            let GbePoll::Action(action) = gbe.poll() else {
                panic!("capture must issue one write");
            };
            if let GbeAction::StartCgi(transaction) = action {
                break transaction.id;
            }
        };

        gbe.complete(CrimeCgiCompletion {
            id,
            result: Ok(CrimeCompletionPayload::WriteComplete),
            memory_fault: Some(CrimeMemoryFault::Address),
        });

        assert_ne!(gbe.registers.video_capture[3] & (1 << 2), 0);
        assert_eq!(gbe.registers.video_capture[3] & (1 << 3), 0);
        assert_eq!(gbe.capture_writes_remaining, 0);
        assert!(
            gbe.pending_dma
                .iter()
                .all(|job| !matches!(job.destination, DmaDestination::Capture))
        );
    }
}

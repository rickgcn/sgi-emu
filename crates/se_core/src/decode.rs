//! Assembly-time compiled physical address decoder.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::address::{AddressSpaceConfig, DeviceAddr, PhysAddr, PhysRange};
use crate::bus::{Bus, BusFault, BusInitiator, DirectAccess, DirectSpan, MmioAccess};
use crate::device::{Device, DeviceCtx, DeviceError, DeviceId};
use crate::event::{ScheduledEvent, SchedulerShared};

const BANK_SHIFT: u32 = 32;
const BANK_SIZE: u128 = 1_u128 << BANK_SHIFT;
const SLOT_SHIFT: u32 = 16;
const SLOT_SIZE: u128 = 1_u128 << SLOT_SHIFT;
const SLOTS_PER_BANK: usize = 1 << (BANK_SHIFT - SLOT_SHIFT);
const SPARSE_PAGE_SLOTS: usize = 256;
const SPARSE_PAGE_COUNT: usize = SLOTS_PER_BANK / SPARSE_PAGE_SLOTS;
const DENSE_SLOT_THRESHOLD: usize = 4_096;

/// Errors produced while assigning stable runtime device identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceRegistryError {
    /// No additional identity can be represented while retaining a `u32` count.
    TooManyDevices,
}

impl fmt::Display for DeviceRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyDevices => formatter.write_str("device registry exhausted u32 identities"),
        }
    }
}

impl Error for DeviceRegistryError {}

/// Builder that assigns `DeviceId` values independently of address mappings.
#[derive(Default)]
pub struct DeviceRegistryBuilder {
    devices: Vec<Box<dyn Device>>,
}

impl DeviceRegistryBuilder {
    /// Creates an empty device registry builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a device and returns its registration-order identity.
    pub fn register(&mut self, device: Box<dyn Device>) -> Result<DeviceId, DeviceRegistryError> {
        if self.devices.len() >= u32::MAX as usize {
            return Err(DeviceRegistryError::TooManyDevices);
        }
        let id =
            u32::try_from(self.devices.len()).map_err(|_| DeviceRegistryError::TooManyDevices)?;
        self.devices.push(device);
        Ok(DeviceId::from_raw(id))
    }

    /// Returns the number of registered devices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Returns whether no devices are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Mapping {
    device: DeviceId,
    physical: PhysRange,
    device_base: DeviceAddr,
}

/// Errors produced while declaring or validating a physical address map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressMapError {
    /// Implemented physical address bits must be in `1..=63`.
    InvalidPhysicalAddressBits(u8),
    /// A physical range exceeds the machine profile's address space.
    PhysicalRangeOutOfSpace {
        /// Rejected physical range.
        range: PhysRange,
        /// Configured number of physical address bits.
        physical_address_bits: u8,
    },
    /// Translating the complete mapping would overflow device-local space.
    DeviceRangeOverflow {
        /// Mapping's device-local base.
        device_base: DeviceAddr,
        /// Mapping length in bytes.
        len: u64,
    },
    /// Two physical mappings overlap at byte granularity.
    PhysicalOverlap {
        /// Earlier mapping in physical-address order.
        first: PhysRange,
        /// Overlapping later mapping.
        second: PhysRange,
    },
}

impl fmt::Display for AddressMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPhysicalAddressBits(bits) => {
                write!(formatter, "physical address width {bits} is outside 1..=63")
            }
            Self::PhysicalRangeOutOfSpace {
                range,
                physical_address_bits,
            } => write!(
                formatter,
                "physical range {:#x}..{:#x} exceeds {physical_address_bits}-bit space",
                range.start().get(),
                range.end_exclusive()
            ),
            Self::DeviceRangeOverflow { device_base, len } => write!(
                formatter,
                "device range at {:#x} with length {len:#x} overflows u64",
                device_base.get()
            ),
            Self::PhysicalOverlap { first, second } => write!(
                formatter,
                "physical ranges {:#x}..{:#x} and {:#x}..{:#x} overlap",
                first.start().get(),
                first.end_exclusive(),
                second.start().get(),
                second.end_exclusive()
            ),
        }
    }
}

impl Error for AddressMapError {}

/// Declarative physical mappings for one explicitly configured address space.
pub struct AddressMap {
    config: AddressSpaceConfig,
    mappings: Vec<Mapping>,
}

impl AddressMap {
    /// Creates an empty map for an explicit physical address geometry.
    pub fn new(config: AddressSpaceConfig) -> Result<Self, AddressMapError> {
        if !(1..=63).contains(&config.physical_address_bits) {
            return Err(AddressMapError::InvalidPhysicalAddressBits(
                config.physical_address_bits,
            ));
        }
        Ok(Self {
            config,
            mappings: Vec::new(),
        })
    }

    /// Declares an exact physical range and its device-local base.
    pub fn map_region(
        &mut self,
        device: DeviceId,
        physical: PhysRange,
        device_base: DeviceAddr,
    ) -> Result<(), AddressMapError> {
        if !self.config.contains_range(physical) {
            return Err(AddressMapError::PhysicalRangeOutOfSpace {
                range: physical,
                physical_address_bits: self.config.physical_address_bits,
            });
        }
        device_base.checked_add(physical.len() - 1).ok_or(
            AddressMapError::DeviceRangeOverflow {
                device_base,
                len: physical.len(),
            },
        )?;
        self.mappings.push(Mapping {
            device,
            physical,
            device_base,
        });
        Ok(())
    }

    /// Returns the configured physical address geometry.
    #[must_use]
    pub const fn config(&self) -> AddressSpaceConfig {
        self.config
    }

    /// Returns the number of declared physical mappings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    /// Returns whether no physical mappings are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }
}

/// Errors produced while validating and compiling a complete decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecoderBuildError {
    /// The address map itself is invalid.
    AddressMap(AddressMapError),
    /// A mapping references an identity not present in the registry.
    UnknownDevice(DeviceId),
    /// The internal route table exhausted its `u32` identity space.
    TooManyRoutes,
    /// The edge-table collection exhausted its `u32` identity space.
    TooManyEdgeTables,
}

impl fmt::Display for DecoderBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressMap(error) => write!(formatter, "invalid address map: {error}"),
            Self::UnknownDevice(device) => {
                write!(
                    formatter,
                    "mapping references unknown device {}",
                    device.get()
                )
            }
            Self::TooManyRoutes => formatter.write_str("decoder exhausted u32 route identities"),
            Self::TooManyEdgeTables => {
                formatter.write_str("decoder exhausted u32 edge-table identities")
            }
        }
    }
}

impl Error for DecoderBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AddressMap(error) => Some(error),
            _ => None,
        }
    }
}

impl From<AddressMapError> for DecoderBuildError {
    fn from(error: AddressMapError) -> Self {
        Self::AddressMap(error)
    }
}

#[derive(Clone, Copy)]
struct Route {
    device: DeviceId,
    physical_start: u64,
    physical_end: u64,
    device_base: DeviceAddr,
}

#[derive(Clone, Copy)]
struct EdgeRange {
    start: u64,
    end: u64,
    route: u32,
}

#[derive(Clone, Copy, Default)]
enum SlotEntry {
    #[default]
    Unmapped,
    Direct(u32),
    Edge(u32),
}

struct SparseSlots {
    pages: Box<[Option<Box<[SlotEntry]>>]>,
}

impl SparseSlots {
    fn compile(entries: &BTreeMap<u16, SlotEntry>) -> Self {
        let mut pages: Vec<Option<Box<[SlotEntry]>>> = std::iter::repeat_with(|| None)
            .take(SPARSE_PAGE_COUNT)
            .collect();
        for (&slot, &entry) in entries {
            let page_index = usize::from(slot) / SPARSE_PAGE_SLOTS;
            let slot_index = usize::from(slot) % SPARSE_PAGE_SLOTS;
            let page = pages[page_index].get_or_insert_with(|| {
                vec![SlotEntry::Unmapped; SPARSE_PAGE_SLOTS].into_boxed_slice()
            });
            page[slot_index] = entry;
        }
        Self {
            pages: pages.into_boxed_slice(),
        }
    }

    #[inline]
    fn get(&self, slot: u16) -> SlotEntry {
        let page_index = usize::from(slot) / SPARSE_PAGE_SLOTS;
        let slot_index = usize::from(slot) % SPARSE_PAGE_SLOTS;
        self.pages[page_index]
            .as_ref()
            .map_or(SlotEntry::Unmapped, |page| page[slot_index])
    }
}

enum Bank {
    Unmapped,
    Uniform(u32),
    Dense(Box<[SlotEntry]>),
    Sparse(SparseSlots),
}

impl Bank {
    #[inline]
    fn route(&self, addr: PhysAddr, edge_tables: &[Box<[EdgeRange]>]) -> Option<u32> {
        match self {
            Self::Unmapped => None,
            Self::Uniform(route) => Some(*route),
            Self::Dense(slots) => {
                let slot = ((addr.get() >> SLOT_SHIFT) & 0xffff) as usize;
                resolve_slot(slots[slot], addr, edge_tables)
            }
            Self::Sparse(slots) => {
                let slot = ((addr.get() >> SLOT_SHIFT) & 0xffff) as u16;
                resolve_slot(slots.get(slot), addr, edge_tables)
            }
        }
    }

    fn add_stats(&self, stats: &mut DecoderLayoutStats) {
        match self {
            Self::Unmapped => stats.unmapped_banks += 1,
            Self::Uniform(_) => stats.uniform_banks += 1,
            Self::Dense(slots) => {
                stats.dense_banks += 1;
                add_slot_stats(slots.iter().copied(), stats);
            }
            Self::Sparse(slots) => {
                stats.sparse_banks += 1;
                for page in slots.pages.iter().flatten() {
                    add_slot_stats(page.iter().copied(), stats);
                }
            }
        }
    }
}

#[inline]
fn resolve_slot(entry: SlotEntry, addr: PhysAddr, edge_tables: &[Box<[EdgeRange]>]) -> Option<u32> {
    match entry {
        SlotEntry::Unmapped => None,
        SlotEntry::Direct(route) => Some(route),
        SlotEntry::Edge(table) => resolve_edge(table, addr, edge_tables),
    }
}

#[cold]
#[inline(never)]
fn resolve_edge(table: u32, addr: PhysAddr, edge_tables: &[Box<[EdgeRange]>]) -> Option<u32> {
    let edges = &edge_tables[table as usize];
    let insertion = edges.partition_point(|edge| edge.start <= addr.get());
    insertion.checked_sub(1).and_then(|index| {
        let edge = edges[index];
        (addr.get() < edge.end).then_some(edge.route)
    })
}

fn add_slot_stats(entries: impl Iterator<Item = SlotEntry>, stats: &mut DecoderLayoutStats) {
    for entry in entries {
        match entry {
            SlotEntry::Unmapped => {}
            SlotEntry::Direct(_) => stats.direct_slots += 1,
            SlotEntry::Edge(_) => stats.edge_slots += 1,
        }
    }
}

enum RadixEntry {
    Empty,
    Node(Box<RadixNode>),
    Bank(Box<Bank>),
}

struct RadixNode {
    entries: Box<[RadixEntry]>,
}

impl RadixNode {
    fn new() -> Self {
        Self {
            entries: std::iter::repeat_with(|| RadixEntry::Empty)
                .take(256)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        }
    }

    fn insert(&mut self, key: u32, depth: u8, bank: Bank) {
        let shift = u32::from(depth - 1) * 8;
        let index = ((key >> shift) & 0xff) as usize;
        if depth == 1 {
            self.entries[index] = RadixEntry::Bank(Box::new(bank));
            return;
        }
        if matches!(self.entries[index], RadixEntry::Empty) {
            self.entries[index] = RadixEntry::Node(Box::new(Self::new()));
        }
        match &mut self.entries[index] {
            RadixEntry::Node(node) => node.insert(key, depth - 1, bank),
            RadixEntry::Empty | RadixEntry::Bank(_) => {
                unreachable!("radix depth is fixed for every bank key")
            }
        }
    }

    #[inline]
    fn get(&self, key: u32, depth: u8) -> Option<&Bank> {
        let shift = u32::from(depth - 1) * 8;
        let index = ((key >> shift) & 0xff) as usize;
        match &self.entries[index] {
            RadixEntry::Empty => None,
            RadixEntry::Node(node) if depth > 1 => node.get(key, depth - 1),
            RadixEntry::Bank(bank) if depth == 1 => Some(bank),
            RadixEntry::Node(_) | RadixEntry::Bank(_) => None,
        }
    }

    fn add_stats(&self, stats: &mut DecoderLayoutStats) {
        for entry in &self.entries {
            match entry {
                RadixEntry::Empty => {}
                RadixEntry::Node(node) => node.add_stats(stats),
                RadixEntry::Bank(bank) => bank.add_stats(stats),
            }
        }
    }
}

struct BankDirectory {
    low: Bank,
    high_depth: u8,
    high: Option<RadixNode>,
}

impl BankDirectory {
    fn new(physical_address_bits: u8) -> Self {
        let high_depth = physical_address_bits.saturating_sub(32).div_ceil(8);
        Self {
            low: Bank::Unmapped,
            high_depth,
            high: (high_depth != 0).then(RadixNode::new),
        }
    }

    fn insert(&mut self, key: u32, bank: Bank) {
        if key == 0 {
            self.low = bank;
        } else {
            self.high
                .as_mut()
                .expect("validated high bank must have a radix root")
                .insert(key, self.high_depth, bank);
        }
    }

    #[inline]
    fn get(&self, key: u32) -> Option<&Bank> {
        if key == 0 {
            Some(&self.low)
        } else {
            self.get_high(key)
        }
    }

    #[inline(never)]
    fn get_high(&self, key: u32) -> Option<&Bank> {
        self.high
            .as_ref()
            .and_then(|root| root.get(key, self.high_depth))
    }

    fn stats(&self) -> DecoderLayoutStats {
        let mut stats = DecoderLayoutStats::default();
        self.low.add_stats(&mut stats);
        if let Some(high) = &self.high {
            high.add_stats(&mut stats);
        }
        stats
    }
}

/// Counts of compiled decoder structures, useful for validation and profiling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecoderLayoutStats {
    /// Explicitly represented unmapped banks, including bank zero when empty.
    pub unmapped_banks: usize,
    /// Completely linear 4 GiB banks.
    pub uniform_banks: usize,
    /// Banks represented by 65,536 direct slot entries.
    pub dense_banks: usize,
    /// Banks represented by fixed two-level sparse slot radix tables.
    pub sparse_banks: usize,
    /// Slots that directly identify one route.
    pub direct_slots: usize,
    /// Slots that require an exact edge lookup.
    pub edge_slots: usize,
    /// Exact edge tables referenced by edge slots.
    pub edge_tables: usize,
}

#[derive(Clone, Copy)]
struct ResolvedRoute {
    device: DeviceId,
    addr: DeviceAddr,
    remaining: u64,
}

/// Runtime access errors for explicitly taking a device out of the decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceSlotError {
    /// The registry contains no such identity.
    UnknownDevice(DeviceId),
    /// The device is already out for an event callback or external operation.
    DeviceTaken(DeviceId),
    /// A put operation targeted a slot that still contains its device.
    DevicePresent(DeviceId),
}

impl fmt::Display for DeviceSlotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDevice(device) => write!(formatter, "unknown device {}", device.get()),
            Self::DeviceTaken(device) => {
                write!(formatter, "device {} is already taken", device.get())
            }
            Self::DevicePresent(device) => {
                write!(
                    formatter,
                    "device {} slot is already occupied",
                    device.get()
                )
            }
        }
    }
}

impl Error for DeviceSlotError {}

/// Errors produced while dispatching a scheduled event through the decoder.
#[derive(Debug)]
pub enum DeviceDispatchError {
    /// The destination cannot be taken from the device registry.
    Slot(DeviceSlotError),
    /// The destination device rejected its event callback.
    Device(DeviceError),
}

impl fmt::Display for DeviceDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slot(error) => write!(formatter, "cannot dispatch event: {error}"),
            Self::Device(error) => write!(formatter, "device event failed: {error}"),
        }
    }
}

impl Error for DeviceDispatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Slot(error) => Some(error),
            Self::Device(error) => Some(error),
        }
    }
}

/// Compiled exact physical decoder and its registered devices.
pub struct Decoder {
    config: AddressSpaceConfig,
    devices: Vec<Option<Box<dyn Device>>>,
    routes: Box<[Route]>,
    edge_tables: Vec<Box<[EdgeRange]>>,
    banks: BankDirectory,
}

impl Decoder {
    /// Validates the complete graph and compiles its adaptive routing index.
    pub fn build(
        devices: DeviceRegistryBuilder,
        address_map: AddressMap,
    ) -> Result<Self, DecoderBuildError> {
        let device_count = devices.devices.len();
        let mut mappings = address_map.mappings;
        for mapping in &mappings {
            if mapping.device.get() as usize >= device_count {
                return Err(DecoderBuildError::UnknownDevice(mapping.device));
            }
        }
        mappings.sort_by_key(|mapping| mapping.physical.start().get());
        for pair in mappings.windows(2) {
            if pair[1].physical.start().get() < pair[0].physical.end_exclusive() {
                return Err(AddressMapError::PhysicalOverlap {
                    first: pair[0].physical,
                    second: pair[1].physical,
                }
                .into());
            }
        }
        if mappings.len() > u32::MAX as usize {
            return Err(DecoderBuildError::TooManyRoutes);
        }

        let routes: Vec<Route> = mappings
            .iter()
            .map(|mapping| Route {
                device: mapping.device,
                physical_start: mapping.physical.start().get(),
                physical_end: mapping.physical.end_exclusive(),
                device_base: mapping.device_base,
            })
            .collect();
        let mut bank_routes: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (route_id, route) in routes.iter().enumerate() {
            let route_id = u32::try_from(route_id).map_err(|_| DecoderBuildError::TooManyRoutes)?;
            let first = (route.physical_start >> BANK_SHIFT) as u32;
            let last = ((route.physical_end - 1) >> BANK_SHIFT) as u32;
            let mut bank = first;
            loop {
                bank_routes.entry(bank).or_default().push(route_id);
                if bank == last {
                    break;
                }
                bank += 1;
            }
        }

        let mut edge_tables = Vec::new();
        let mut banks = BankDirectory::new(address_map.config.physical_address_bits);
        for (bank_key, route_ids) in bank_routes {
            let bank = compile_bank(bank_key, &route_ids, &routes, &mut edge_tables)?;
            banks.insert(bank_key, bank);
        }

        Ok(Self {
            config: address_map.config,
            devices: devices.devices.into_iter().map(Some).collect(),
            routes: routes.into_boxed_slice(),
            edge_tables,
            banks,
        })
    }

    /// Returns a physical bus port permanently bound to one initiator identity.
    pub fn port(&mut self, initiator: BusInitiator) -> BusPort<'_> {
        BusPort {
            decoder: self,
            initiator,
        }
    }

    /// Returns the physical address geometry compiled into this decoder.
    #[must_use]
    pub const fn config(&self) -> AddressSpaceConfig {
        self.config
    }

    /// Returns the number of registered device slots.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Returns immutable access to a device that is currently present.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Option<&(dyn Device + '_)> {
        match self.devices.get(id.get() as usize) {
            Some(Some(device)) => Some(device.as_ref()),
            Some(None) | None => None,
        }
    }

    /// Returns mutable access to a device that is currently present.
    pub fn device_mut(&mut self, id: DeviceId) -> Option<&mut (dyn Device + '_)> {
        match self.devices.get_mut(id.get() as usize) {
            Some(Some(device)) => Some(device.as_mut()),
            Some(None) | None => None,
        }
    }

    /// Temporarily removes a device so its callback can borrow the decoder bus.
    pub fn take_device(&mut self, id: DeviceId) -> Result<Box<dyn Device>, DeviceSlotError> {
        let slot = self
            .devices
            .get_mut(id.get() as usize)
            .ok_or(DeviceSlotError::UnknownDevice(id))?;
        slot.take().ok_or(DeviceSlotError::DeviceTaken(id))
    }

    /// Returns a temporarily removed device to its exact registry slot.
    pub fn put_device(
        &mut self,
        id: DeviceId,
        device: Box<dyn Device>,
    ) -> Result<(), (DeviceSlotError, Box<dyn Device>)> {
        let Some(slot) = self.devices.get_mut(id.get() as usize) else {
            return Err((DeviceSlotError::UnknownDevice(id), device));
        };
        if slot.is_some() {
            return Err((DeviceSlotError::DevicePresent(id), device));
        }
        *slot = Some(device);
        Ok(())
    }

    /// Dispatches one event while keeping every window of its target faulted.
    pub fn dispatch_event(
        &mut self,
        event: ScheduledEvent,
        scheduler: &SchedulerShared,
    ) -> Result<(), DeviceDispatchError> {
        let id = event.device;
        let mut device = self.take_device(id).map_err(DeviceDispatchError::Slot)?;
        let handle = scheduler.handle(id);
        let result = {
            let mut port = self.port(BusInitiator::Device(id));
            let mut context = DeviceCtx {
                now: scheduler.now(),
                bus: &mut port,
                sched: &handle,
            };
            device.on_event(event.tag, event.payload, &mut context)
        };
        self.devices[id.get() as usize] = Some(device);
        result.map_err(DeviceDispatchError::Device)
    }

    /// Returns statistics describing the selected compiled representations.
    #[must_use]
    pub fn layout_stats(&self) -> DecoderLayoutStats {
        let mut stats = self.banks.stats();
        stats.edge_tables = self.edge_tables.len();
        stats
    }

    #[inline(always)]
    fn resolve(&self, addr: PhysAddr, width: u64) -> Result<ResolvedRoute, BusFault> {
        if width == 0 || !self.config.contains(addr) {
            return Err(BusFault::Unmapped);
        }
        let end = addr.get().checked_add(width).ok_or(BusFault::Unmapped)?;
        if let Some(limit) = self.config.upper_bound_exclusive()
            && end > limit
        {
            return Err(BusFault::Unmapped);
        }
        let bank_key = (addr.get() >> BANK_SHIFT) as u32;
        let bank = self.banks.get(bank_key).ok_or(BusFault::Unmapped)?;
        let route_id = bank
            .route(addr, &self.edge_tables)
            .ok_or(BusFault::Unmapped)?;
        let route = self.routes[route_id as usize];
        if addr.get() < route.physical_start || end > route.physical_end {
            return Err(BusFault::Unmapped);
        }
        let offset = addr.get() - route.physical_start;
        let local = route
            .device_base
            .checked_add(offset)
            .ok_or(BusFault::Unmapped)?;
        Ok(ResolvedRoute {
            device: route.device,
            addr: local,
            remaining: route.physical_end - addr.get(),
        })
    }

    #[inline(always)]
    fn device_for_route(
        &mut self,
        route: ResolvedRoute,
    ) -> Result<&mut (dyn Device + '_), BusFault> {
        match &mut self.devices[route.device.get() as usize] {
            Some(device) => Ok(device.as_mut()),
            None => Err(BusFault::Fault),
        }
    }

    #[inline(always)]
    fn read8_for(&mut self, initiator: BusInitiator, addr: PhysAddr) -> Result<u8, BusFault> {
        let route = self.resolve(addr, 1)?;
        self.device_for_route(route)?.read8(MmioAccess {
            initiator,
            addr: route.addr,
        })
    }

    #[inline(always)]
    fn read16_for(&mut self, initiator: BusInitiator, addr: PhysAddr) -> Result<u16, BusFault> {
        let route = self.resolve(addr, 2)?;
        self.device_for_route(route)?.read16(MmioAccess {
            initiator,
            addr: route.addr,
        })
    }

    #[inline(always)]
    fn read32_for(&mut self, initiator: BusInitiator, addr: PhysAddr) -> Result<u32, BusFault> {
        let route = self.resolve(addr, 4)?;
        self.device_for_route(route)?.read32(MmioAccess {
            initiator,
            addr: route.addr,
        })
    }

    #[inline(always)]
    fn read64_for(&mut self, initiator: BusInitiator, addr: PhysAddr) -> Result<u64, BusFault> {
        let route = self.resolve(addr, 8)?;
        self.device_for_route(route)?.read64(MmioAccess {
            initiator,
            addr: route.addr,
        })
    }

    #[inline(always)]
    fn write8_for(
        &mut self,
        initiator: BusInitiator,
        addr: PhysAddr,
        value: u8,
    ) -> Result<(), BusFault> {
        let route = self.resolve(addr, 1)?;
        self.device_for_route(route)?.write8(
            MmioAccess {
                initiator,
                addr: route.addr,
            },
            value,
        )
    }

    #[inline(always)]
    fn write16_for(
        &mut self,
        initiator: BusInitiator,
        addr: PhysAddr,
        value: u16,
    ) -> Result<(), BusFault> {
        let route = self.resolve(addr, 2)?;
        self.device_for_route(route)?.write16(
            MmioAccess {
                initiator,
                addr: route.addr,
            },
            value,
        )
    }

    #[inline(always)]
    fn write32_for(
        &mut self,
        initiator: BusInitiator,
        addr: PhysAddr,
        value: u32,
    ) -> Result<(), BusFault> {
        let route = self.resolve(addr, 4)?;
        self.device_for_route(route)?.write32(
            MmioAccess {
                initiator,
                addr: route.addr,
            },
            value,
        )
    }

    #[inline(always)]
    fn write64_for(
        &mut self,
        initiator: BusInitiator,
        addr: PhysAddr,
        value: u64,
    ) -> Result<(), BusFault> {
        let route = self.resolve(addr, 8)?;
        self.device_for_route(route)?.write64(
            MmioAccess {
                initiator,
                addr: route.addr,
            },
            value,
        )
    }

    fn read_block_for(
        &mut self,
        initiator: BusInitiator,
        mut addr: PhysAddr,
        mut output: &mut [u8],
    ) -> Result<(), BusFault> {
        if output.is_empty() {
            return Ok(());
        }
        while !output.is_empty() {
            let route = self.resolve(addr, 1)?;
            let output_len = u64::try_from(output.len()).map_err(|_| BusFault::Unmapped)?;
            let chunk_len = route.remaining.min(output_len);
            let chunk_len = usize::try_from(chunk_len).map_err(|_| BusFault::Unmapped)?;
            let (chunk, rest) = output.split_at_mut(chunk_len);
            self.device_for_route(route)?.read_block(
                MmioAccess {
                    initiator,
                    addr: route.addr,
                },
                chunk,
            )?;
            output = rest;
            if !output.is_empty() {
                addr = addr
                    .checked_add(u64::try_from(chunk_len).map_err(|_| BusFault::Unmapped)?)
                    .ok_or(BusFault::Unmapped)?;
            }
        }
        Ok(())
    }

    fn write_block_for(
        &mut self,
        initiator: BusInitiator,
        mut addr: PhysAddr,
        mut input: &[u8],
    ) -> Result<(), BusFault> {
        if input.is_empty() {
            return Ok(());
        }
        while !input.is_empty() {
            let route = self.resolve(addr, 1)?;
            let input_len = u64::try_from(input.len()).map_err(|_| BusFault::Unmapped)?;
            let chunk_len = route.remaining.min(input_len);
            let chunk_len = usize::try_from(chunk_len).map_err(|_| BusFault::Unmapped)?;
            let (chunk, rest) = input.split_at(chunk_len);
            self.device_for_route(route)?.write_block(
                MmioAccess {
                    initiator,
                    addr: route.addr,
                },
                chunk,
            )?;
            input = rest;
            if !input.is_empty() {
                addr = addr
                    .checked_add(u64::try_from(chunk_len).map_err(|_| BusFault::Unmapped)?)
                    .ok_or(BusFault::Unmapped)?;
            }
        }
        Ok(())
    }

    fn direct_span_for(
        &mut self,
        initiator: BusInitiator,
        addr: PhysAddr,
        requested: usize,
        access: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault> {
        if requested == 0 {
            return Ok(None);
        }
        let route = self.resolve(addr, 1)?;
        let route_limit = usize::try_from(route.remaining).unwrap_or(usize::MAX);
        let limit = requested.min(route_limit);
        let span = self.device_for_route(route)?.direct_span(
            MmioAccess {
                initiator,
                addr: route.addr,
            },
            limit,
            access,
        )?;
        Ok(span.and_then(|span| span.truncate(limit)))
    }
}

fn compile_bank(
    bank_key: u32,
    route_ids: &[u32],
    routes: &[Route],
    edge_tables: &mut Vec<Box<[EdgeRange]>>,
) -> Result<Bank, DecoderBuildError> {
    let bank_start = u128::from(bank_key) * BANK_SIZE;
    let bank_end = bank_start + BANK_SIZE;
    if route_ids.len() == 1 {
        let route = routes[route_ids[0] as usize];
        if u128::from(route.physical_start) <= bank_start
            && u128::from(route.physical_end) >= bank_end
        {
            return Ok(Bank::Uniform(route_ids[0]));
        }
    }

    let mut slot_routes: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
    for &route_id in route_ids {
        let route = routes[route_id as usize];
        let start = u128::from(route.physical_start).max(bank_start);
        let end = u128::from(route.physical_end).min(bank_end);
        let first_slot = ((start - bank_start) >> SLOT_SHIFT) as u16;
        let last_slot = (((end - 1) - bank_start) >> SLOT_SHIFT) as u16;
        let mut slot = first_slot;
        loop {
            slot_routes.entry(slot).or_default().push(route_id);
            if slot == last_slot {
                break;
            }
            slot += 1;
        }
    }

    let mut entries = BTreeMap::new();
    for (slot, mut ids) in slot_routes {
        ids.sort_by_key(|route_id| routes[*route_id as usize].physical_start);
        let slot_start = bank_start + u128::from(slot) * SLOT_SIZE;
        let slot_end = slot_start + SLOT_SIZE;
        let entry = if ids.len() == 1 {
            let route = routes[ids[0] as usize];
            if u128::from(route.physical_start) <= slot_start
                && u128::from(route.physical_end) >= slot_end
            {
                SlotEntry::Direct(ids[0])
            } else {
                compile_edge(&ids, routes, edge_tables)?
            }
        } else {
            compile_edge(&ids, routes, edge_tables)?
        };
        entries.insert(slot, entry);
    }

    if entries.len() >= DENSE_SLOT_THRESHOLD {
        let mut dense = vec![SlotEntry::Unmapped; SLOTS_PER_BANK];
        for (slot, entry) in entries {
            dense[usize::from(slot)] = entry;
        }
        Ok(Bank::Dense(dense.into_boxed_slice()))
    } else {
        Ok(Bank::Sparse(SparseSlots::compile(&entries)))
    }
}

fn compile_edge(
    route_ids: &[u32],
    routes: &[Route],
    edge_tables: &mut Vec<Box<[EdgeRange]>>,
) -> Result<SlotEntry, DecoderBuildError> {
    let table_id =
        u32::try_from(edge_tables.len()).map_err(|_| DecoderBuildError::TooManyEdgeTables)?;
    let table = route_ids
        .iter()
        .map(|route_id| {
            let route = routes[*route_id as usize];
            EdgeRange {
                start: route.physical_start,
                end: route.physical_end,
                route: *route_id,
            }
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    edge_tables.push(table);
    Ok(SlotEntry::Edge(table_id))
}

/// A mutable physical bus view bound to one CPU or DMA initiator.
pub struct BusPort<'a> {
    decoder: &'a mut Decoder,
    initiator: BusInitiator,
}

impl BusPort<'_> {
    /// Returns the identity attached to every transaction from this port.
    #[must_use]
    pub const fn initiator(&self) -> BusInitiator {
        self.initiator
    }
}

impl Bus for BusPort<'_> {
    #[inline]
    fn read8(&mut self, addr: PhysAddr) -> Result<u8, BusFault> {
        self.decoder.read8_for(self.initiator, addr)
    }

    #[inline]
    fn read16(&mut self, addr: PhysAddr) -> Result<u16, BusFault> {
        self.decoder.read16_for(self.initiator, addr)
    }

    #[inline]
    fn read32(&mut self, addr: PhysAddr) -> Result<u32, BusFault> {
        self.decoder.read32_for(self.initiator, addr)
    }

    #[inline]
    fn read64(&mut self, addr: PhysAddr) -> Result<u64, BusFault> {
        self.decoder.read64_for(self.initiator, addr)
    }

    #[inline]
    fn write8(&mut self, addr: PhysAddr, value: u8) -> Result<(), BusFault> {
        self.decoder.write8_for(self.initiator, addr, value)
    }

    #[inline]
    fn write16(&mut self, addr: PhysAddr, value: u16) -> Result<(), BusFault> {
        self.decoder.write16_for(self.initiator, addr, value)
    }

    #[inline]
    fn write32(&mut self, addr: PhysAddr, value: u32) -> Result<(), BusFault> {
        self.decoder.write32_for(self.initiator, addr, value)
    }

    #[inline]
    fn write64(&mut self, addr: PhysAddr, value: u64) -> Result<(), BusFault> {
        self.decoder.write64_for(self.initiator, addr, value)
    }

    #[inline]
    fn read_block(&mut self, addr: PhysAddr, output: &mut [u8]) -> Result<(), BusFault> {
        self.decoder.read_block_for(self.initiator, addr, output)
    }

    #[inline]
    fn write_block(&mut self, addr: PhysAddr, input: &[u8]) -> Result<(), BusFault> {
        self.decoder.write_block_for(self.initiator, addr, input)
    }

    #[inline]
    fn direct_span(
        &mut self,
        addr: PhysAddr,
        requested: usize,
        access: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault> {
        self.decoder
            .direct_span_for(self.initiator, addr, requested, access)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use crate::address::{AddressSpaceConfig, DeviceAddr, PhysAddr, PhysRange};
    use crate::bus::{
        Bus, BusFault, BusInitiator, CpuId, DirectAccess, DirectSpan, MmioAccess, MmioDevice,
    };
    use crate::device::{Device, DeviceCtx, DeviceError, DeviceId};
    use crate::event::{ScheduledEvent, SchedulerShared};
    use crate::inspect::{InspectCommand, InspectError, Introspect};
    use crate::save::{Saveable, StateError, StateReader, StateWriter};

    use super::{
        AddressMap, AddressMapError, Decoder, DecoderBuildError, DeviceRegistryBuilder,
        DeviceSlotError,
    };

    enum TestStorage {
        Dense(Vec<u8>),
        AddressOnly,
    }

    struct TestDevice {
        storage: TestStorage,
        initiators: Vec<BusInitiator>,
        fixed_calls: u32,
        block_calls: u32,
        reject_blocks: bool,
        event_bus_fault: Option<BusFault>,
    }

    impl TestDevice {
        fn memory(size: usize) -> Self {
            Self {
                storage: TestStorage::Dense(vec![0; size]),
                initiators: Vec::new(),
                fixed_calls: 0,
                block_calls: 0,
                reject_blocks: false,
                event_bus_fault: None,
            }
        }

        fn address_only() -> Self {
            Self {
                storage: TestStorage::AddressOnly,
                initiators: Vec::new(),
                fixed_calls: 0,
                block_calls: 0,
                reject_blocks: false,
                event_bus_fault: None,
            }
        }

        fn record_fixed(&mut self, access: MmioAccess) {
            self.initiators.push(access.initiator);
            self.fixed_calls += 1;
        }

        fn read_bytes<const N: usize>(&mut self, access: MmioAccess) -> Result<[u8; N], BusFault> {
            self.record_fixed(access);
            match &self.storage {
                TestStorage::Dense(bytes) => {
                    let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                    bytes
                        .get(start..start.checked_add(N).ok_or(BusFault::Fault)?)
                        .ok_or(BusFault::Fault)?
                        .try_into()
                        .map_err(|_| BusFault::Fault)
                }
                TestStorage::AddressOnly => {
                    let encoded = access.addr.get().to_be_bytes();
                    Ok(encoded[encoded.len() - N..]
                        .try_into()
                        .expect("fixed-width suffix has exact length"))
                }
            }
        }

        fn write_bytes(&mut self, access: MmioAccess, input: &[u8]) -> Result<(), BusFault> {
            self.record_fixed(access);
            match &mut self.storage {
                TestStorage::Dense(bytes) => {
                    let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                    let end = start.checked_add(input.len()).ok_or(BusFault::Fault)?;
                    bytes
                        .get_mut(start..end)
                        .ok_or(BusFault::Fault)?
                        .copy_from_slice(input);
                    Ok(())
                }
                TestStorage::AddressOnly => Ok(()),
            }
        }

        fn bytes(&self) -> &[u8] {
            match &self.storage {
                TestStorage::Dense(bytes) => bytes,
                TestStorage::AddressOnly => &[],
            }
        }
    }

    impl MmioDevice for TestDevice {
        fn read8(&mut self, access: MmioAccess) -> Result<u8, BusFault> {
            Ok(self.read_bytes::<1>(access)?[0])
        }

        fn read16(&mut self, access: MmioAccess) -> Result<u16, BusFault> {
            Ok(u16::from_be_bytes(self.read_bytes(access)?))
        }

        fn read32(&mut self, access: MmioAccess) -> Result<u32, BusFault> {
            Ok(u32::from_be_bytes(self.read_bytes(access)?))
        }

        fn read64(&mut self, access: MmioAccess) -> Result<u64, BusFault> {
            Ok(u64::from_be_bytes(self.read_bytes(access)?))
        }

        fn write8(&mut self, access: MmioAccess, value: u8) -> Result<(), BusFault> {
            self.write_bytes(access, &[value])
        }

        fn write16(&mut self, access: MmioAccess, value: u16) -> Result<(), BusFault> {
            self.write_bytes(access, &value.to_be_bytes())
        }

        fn write32(&mut self, access: MmioAccess, value: u32) -> Result<(), BusFault> {
            self.write_bytes(access, &value.to_be_bytes())
        }

        fn write64(&mut self, access: MmioAccess, value: u64) -> Result<(), BusFault> {
            self.write_bytes(access, &value.to_be_bytes())
        }

        fn read_block(&mut self, access: MmioAccess, output: &mut [u8]) -> Result<(), BusFault> {
            self.initiators.push(access.initiator);
            self.block_calls += 1;
            if self.reject_blocks {
                return Err(BusFault::Fault);
            }
            match &self.storage {
                TestStorage::Dense(bytes) => {
                    let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                    let end = start.checked_add(output.len()).ok_or(BusFault::Fault)?;
                    output.copy_from_slice(bytes.get(start..end).ok_or(BusFault::Fault)?);
                }
                TestStorage::AddressOnly => {
                    for (offset, byte) in output.iter_mut().enumerate() {
                        *byte = access.addr.get().wrapping_add(offset as u64) as u8;
                    }
                }
            }
            Ok(())
        }

        fn write_block(&mut self, access: MmioAccess, input: &[u8]) -> Result<(), BusFault> {
            self.initiators.push(access.initiator);
            self.block_calls += 1;
            if self.reject_blocks {
                return Err(BusFault::Fault);
            }
            match &mut self.storage {
                TestStorage::Dense(bytes) => {
                    let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                    let end = start.checked_add(input.len()).ok_or(BusFault::Fault)?;
                    bytes
                        .get_mut(start..end)
                        .ok_or(BusFault::Fault)?
                        .copy_from_slice(input);
                }
                TestStorage::AddressOnly => {}
            }
            Ok(())
        }

        fn direct_span(
            &mut self,
            access: MmioAccess,
            requested: usize,
            _kind: DirectAccess,
        ) -> Result<Option<DirectSpan<'_>>, BusFault> {
            self.initiators.push(access.initiator);
            match &mut self.storage {
                TestStorage::Dense(bytes) => {
                    let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                    let remaining = bytes.get_mut(start..).ok_or(BusFault::Fault)?;
                    let available = remaining.len().min(requested);
                    Ok(DirectSpan::from_slice(&mut remaining[..available]))
                }
                TestStorage::AddressOnly => Ok(None),
            }
        }
    }

    impl Saveable for TestDevice {
        fn snapshot_version(&self) -> u32 {
            1
        }

        fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError> {
            writer.serialize(&())
        }

        fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError> {
            if version != 1 {
                return Err(StateError::UnsupportedVersion(version));
            }
            reader.deserialize::<()>()
        }
    }

    impl Introspect for TestDevice {
        fn commands(&self) -> &[InspectCommand] {
            &[]
        }

        fn execute(
            &mut self,
            command: &str,
            _arguments: &[&str],
            _output: &mut dyn Write,
        ) -> Result<(), InspectError> {
            Err(InspectError::UnknownCommand(command.to_owned()))
        }
    }

    impl Device for TestDevice {
        fn reset(&mut self, _soft: bool) {}

        fn on_event(
            &mut self,
            _tag: u32,
            payload: u64,
            context: &mut DeviceCtx<'_>,
        ) -> Result<(), DeviceError> {
            self.event_bus_fault = context.bus.read8(PhysAddr::new(payload)).err();
            Ok(())
        }
    }

    fn config(bits: u8) -> AddressSpaceConfig {
        AddressSpaceConfig {
            physical_address_bits: bits,
        }
    }

    fn range(start: u64, len: u64) -> PhysRange {
        PhysRange::from_start_len(PhysAddr::new(start), len).unwrap()
    }

    fn test_device(decoder: &Decoder, id: DeviceId) -> &TestDevice {
        decoder
            .device(id)
            .unwrap()
            .as_any()
            .downcast_ref::<TestDevice>()
            .unwrap()
    }

    fn test_device_mut(decoder: &mut Decoder, id: DeviceId) -> &mut TestDevice {
        decoder
            .device_mut(id)
            .unwrap()
            .as_any_mut()
            .downcast_mut::<TestDevice>()
            .unwrap()
    }

    fn single_device_decoder(
        bits: u8,
        device: TestDevice,
        physical: PhysRange,
        device_base: DeviceAddr,
    ) -> (Decoder, DeviceId) {
        let mut registry = DeviceRegistryBuilder::new();
        let id = registry.register(Box::new(device)).unwrap();
        let mut map = AddressMap::new(config(bits)).unwrap();
        map.map_region(id, physical, device_base).unwrap();
        (Decoder::build(registry, map).unwrap(), id)
    }

    #[test]
    fn decoder_routes_32_40_and_48_bit_addresses() {
        for (bits, start) in [(32, 0x1000), (40, 0x01_0000_1000), (48, 0x0100_0000_1000)] {
            let (mut decoder, _) = single_device_decoder(
                bits,
                TestDevice::address_only(),
                range(start, 0x1000),
                DeviceAddr::new(0x2000),
            );
            let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
            assert_eq!(port.read32(PhysAddr::new(start + 8)), Ok(0x2008));
            port.write64(PhysAddr::new(start + 16), 0x1234).unwrap();
        }
    }

    #[test]
    fn four_gibibyte_mapping_compiles_as_a_uniform_high_bank() {
        let start = 1_u64 << 32;
        let (mut decoder, _) = single_device_decoder(
            40,
            TestDevice::address_only(),
            range(start, 1_u64 << 32),
            DeviceAddr::new(0),
        );
        let stats = decoder.layout_stats();
        assert_eq!(stats.uniform_banks, 1);
        assert_eq!(stats.dense_banks, 0);
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(3)));
        assert_eq!(port.read64(PhysAddr::new(start + 0x1234)), Ok(0x1234));
    }

    #[test]
    fn mapping_may_end_exactly_at_physical_space_limit() {
        let limit = 1_u64 << 40;
        let (mut decoder, _) = single_device_decoder(
            40,
            TestDevice::memory(0x100),
            range(limit - 0x100, 0x100),
            DeviceAddr::new(0),
        );
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        port.write8(PhysAddr::new(limit - 1), 0x5a).unwrap();
        assert_eq!(port.read8(PhysAddr::new(limit - 1)), Ok(0x5a));
        assert_eq!(port.read8(PhysAddr::new(limit)), Err(BusFault::Unmapped));
    }

    #[test]
    fn device_local_range_checks_the_last_reachable_byte() {
        let (mut decoder, _) = single_device_decoder(
            32,
            TestDevice::address_only(),
            range(0x1000, 1),
            DeviceAddr::new(u64::MAX),
        );
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        assert_eq!(port.read8(PhysAddr::new(0x1000)), Ok(0xff));

        let mut overflow = AddressMap::new(config(32)).unwrap();
        assert!(matches!(
            overflow.map_region(
                DeviceId::from_raw(0),
                range(0x2000, 2),
                DeviceAddr::new(u64::MAX)
            ),
            Err(AddressMapError::DeviceRangeOverflow { .. })
        ));
    }

    #[test]
    fn multiple_windows_alias_one_device_and_allow_windowless_devices() {
        let mut registry = DeviceRegistryBuilder::new();
        let memory = registry
            .register(Box::new(TestDevice::memory(0x40)))
            .unwrap();
        let windowless = registry
            .register(Box::new(TestDevice::address_only()))
            .unwrap();
        let mut map = AddressMap::new(config(32)).unwrap();
        map.map_region(memory, range(0x1000, 0x20), DeviceAddr::new(0))
            .unwrap();
        map.map_region(memory, range(0x3000, 0x20), DeviceAddr::new(0))
            .unwrap();
        let mut decoder = Decoder::build(registry, map).unwrap();
        assert!(decoder.device(windowless).is_some());
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        port.write32(PhysAddr::new(0x1004), 0x1122_3344).unwrap();
        assert_eq!(port.read32(PhysAddr::new(0x3004)), Ok(0x1122_3344));
        assert_eq!(port.read8(PhysAddr::new(0x0fff)), Err(BusFault::Unmapped));
        assert_eq!(port.read8(PhysAddr::new(0x1020)), Err(BusFault::Unmapped));
        assert_eq!(port.read8(PhysAddr::new(0x2fff)), Err(BusFault::Unmapped));
        assert_eq!(port.read8(PhysAddr::new(0x3020)), Err(BusFault::Unmapped));
    }

    #[test]
    fn one_slot_can_hold_multiple_exact_non_overlapping_windows() {
        let mut registry = DeviceRegistryBuilder::new();
        let first = registry
            .register(Box::new(TestDevice::memory(0x10)))
            .unwrap();
        let second = registry
            .register(Box::new(TestDevice::memory(0x10)))
            .unwrap();
        let mut map = AddressMap::new(config(32)).unwrap();
        map.map_region(first, range(0x100, 0x10), DeviceAddr::new(0))
            .unwrap();
        map.map_region(second, range(0x120, 0x10), DeviceAddr::new(0))
            .unwrap();
        let mut decoder = Decoder::build(registry, map).unwrap();
        assert_eq!(decoder.layout_stats().edge_slots, 1);
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        port.write8(PhysAddr::new(0x100), 1).unwrap();
        port.write8(PhysAddr::new(0x12f), 2).unwrap();
        for unmapped in [0xff, 0x110, 0x11f, 0x130] {
            assert_eq!(port.read8(PhysAddr::new(unmapped)), Err(BusFault::Unmapped));
        }
    }

    #[test]
    fn fixed_width_transaction_cannot_cross_a_route_boundary() {
        let (mut decoder, id) = single_device_decoder(
            32,
            TestDevice::memory(8),
            range(0x1000, 4),
            DeviceAddr::new(0),
        );
        {
            let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
            assert_eq!(port.read32(PhysAddr::new(0x1002)), Err(BusFault::Unmapped));
        }
        assert_eq!(test_device(&decoder, id).fixed_calls, 0);
    }

    #[test]
    fn block_access_segments_at_routes_and_preserves_partial_completion() {
        let mut registry = DeviceRegistryBuilder::new();
        let first = registry.register(Box::new(TestDevice::memory(4))).unwrap();
        let second = registry.register(Box::new(TestDevice::memory(4))).unwrap();
        let mut map = AddressMap::new(config(32)).unwrap();
        map.map_region(first, range(0x100, 4), DeviceAddr::new(0))
            .unwrap();
        map.map_region(second, range(0x104, 4), DeviceAddr::new(0))
            .unwrap();
        let mut decoder = Decoder::build(registry, map).unwrap();
        test_device_mut(&mut decoder, second).reject_blocks = true;

        let mut output = [0xaa; 8];
        {
            let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
            assert_eq!(
                port.write_block(PhysAddr::new(0x100), &[1, 2, 3, 4, 5, 6, 7, 8]),
                Err(BusFault::Fault)
            );
            assert_eq!(
                port.read_block(PhysAddr::new(0x100), &mut output),
                Err(BusFault::Fault)
            );
        }
        assert_eq!(test_device(&decoder, first).bytes(), &[1, 2, 3, 4]);
        assert_eq!(&output[..4], &[1, 2, 3, 4]);
        assert_eq!(&output[4..], &[0xaa; 4]);
        assert_eq!(test_device(&decoder, first).block_calls, 2);
        assert_eq!(test_device(&decoder, second).block_calls, 2);
    }

    #[test]
    fn block_hot_path_calls_device_once_within_one_route() {
        let (mut decoder, id) = single_device_decoder(
            32,
            TestDevice::memory(128),
            range(0x1000, 128),
            DeviceAddr::new(0),
        );
        {
            let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
            port.write_block(PhysAddr::new(0x1020), &[7; 64]).unwrap();
        }
        assert_eq!(test_device(&decoder, id).block_calls, 1);
    }

    #[test]
    fn direct_span_is_clamped_to_physical_route_boundary() {
        let (mut decoder, _) = single_device_decoder(
            32,
            TestDevice::memory(16),
            range(0x1000, 4),
            DeviceAddr::new(0),
        );
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        let span = port
            .direct_span(PhysAddr::new(0x1002), 10, DirectAccess::Write)
            .unwrap()
            .unwrap();
        assert_eq!(span.len(), 2);
    }

    #[test]
    fn registration_order_controls_ids_not_mapping_order() {
        let mut registry = DeviceRegistryBuilder::new();
        let first = registry.register(Box::new(TestDevice::memory(1))).unwrap();
        let second = registry.register(Box::new(TestDevice::memory(1))).unwrap();
        assert_eq!(first.get(), 0);
        assert_eq!(second.get(), 1);
        let mut map = AddressMap::new(config(32)).unwrap();
        map.map_region(second, range(0x1000, 1), DeviceAddr::new(0))
            .unwrap();
        map.map_region(first, range(0x2000, 1), DeviceAddr::new(0))
            .unwrap();
        let decoder = Decoder::build(registry, map).unwrap();
        assert!(decoder.device(first).is_some());
        assert!(decoder.device(second).is_some());
    }

    #[test]
    fn ports_preserve_distinct_cpu_initiators_on_one_route() {
        let (mut decoder, id) = single_device_decoder(
            32,
            TestDevice::memory(4),
            range(0x1000, 4),
            DeviceAddr::new(0),
        );
        decoder
            .port(BusInitiator::Cpu(CpuId::from_raw(2)))
            .read8(PhysAddr::new(0x1000))
            .unwrap();
        decoder
            .port(BusInitiator::Cpu(CpuId::from_raw(9)))
            .read8(PhysAddr::new(0x1000))
            .unwrap();
        assert_eq!(
            test_device(&decoder, id).initiators,
            vec![
                BusInitiator::Cpu(CpuId::from_raw(2)),
                BusInitiator::Cpu(CpuId::from_raw(9))
            ]
        );
    }

    #[test]
    fn build_rejects_overlap_unknown_device_and_local_overflow() {
        assert!(matches!(
            AddressMap::new(config(64)),
            Err(AddressMapError::InvalidPhysicalAddressBits(64))
        ));

        let mut registry = DeviceRegistryBuilder::new();
        let id = registry.register(Box::new(TestDevice::memory(8))).unwrap();
        let mut overlap = AddressMap::new(config(32)).unwrap();
        overlap
            .map_region(id, range(0x1000, 8), DeviceAddr::new(0))
            .unwrap();
        overlap
            .map_region(id, range(0x1007, 8), DeviceAddr::new(8))
            .unwrap();
        assert!(matches!(
            Decoder::build(registry, overlap),
            Err(DecoderBuildError::AddressMap(
                AddressMapError::PhysicalOverlap { .. }
            ))
        ));

        let registry = DeviceRegistryBuilder::new();
        let mut unknown = AddressMap::new(config(32)).unwrap();
        unknown
            .map_region(DeviceId::from_raw(0), range(0x1000, 1), DeviceAddr::new(0))
            .unwrap();
        assert!(matches!(
            Decoder::build(registry, unknown),
            Err(DecoderBuildError::UnknownDevice(_))
        ));

        let mut overflow = AddressMap::new(config(32)).unwrap();
        assert!(matches!(
            overflow.map_region(
                DeviceId::from_raw(0),
                range(0, 2),
                DeviceAddr::new(u64::MAX)
            ),
            Err(AddressMapError::DeviceRangeOverflow { .. })
        ));
    }

    #[test]
    fn compiler_selects_dense_sparse_and_edge_representations() {
        let (dense, _) = single_device_decoder(
            32,
            TestDevice::address_only(),
            range(0, 0x2000_0000),
            DeviceAddr::new(0),
        );
        assert_eq!(dense.layout_stats().dense_banks, 1);
        assert_eq!(dense.layout_stats().direct_slots, 8_192);

        let (sparse, _) = single_device_decoder(
            48,
            TestDevice::address_only(),
            range(0x0100_0000_1234, 0x100),
            DeviceAddr::new(0),
        );
        let stats = sparse.layout_stats();
        assert_eq!(stats.sparse_banks, 1);
        assert_eq!(stats.edge_slots, 1);
        assert_eq!(stats.edge_tables, 1);
    }

    #[test]
    fn taken_device_faults_all_windows_until_it_is_returned() {
        let (mut decoder, id) = single_device_decoder(
            32,
            TestDevice::memory(4),
            range(0x1000, 4),
            DeviceAddr::new(0),
        );
        let device = decoder.take_device(id).unwrap();
        assert!(matches!(
            decoder.take_device(id),
            Err(DeviceSlotError::DeviceTaken(_))
        ));
        assert_eq!(
            decoder
                .port(BusInitiator::Cpu(CpuId::from_raw(0)))
                .read8(PhysAddr::new(0x1000)),
            Err(BusFault::Fault)
        );
        decoder.put_device(id, device).ok().unwrap();
        assert_eq!(
            decoder
                .port(BusInitiator::Cpu(CpuId::from_raw(0)))
                .read8(PhysAddr::new(0x1000)),
            Ok(0)
        );
    }

    #[test]
    fn event_callback_cannot_access_its_own_taken_endpoint() {
        let (mut decoder, id) = single_device_decoder(
            32,
            TestDevice::memory(4),
            range(0x1000, 4),
            DeviceAddr::new(0),
        );
        let scheduler = SchedulerShared::new();
        decoder
            .dispatch_event(
                ScheduledEvent {
                    vtime: 0,
                    device: id,
                    tag: 1,
                    payload: 0x1000,
                },
                &scheduler,
            )
            .unwrap();
        assert_eq!(
            test_device(&decoder, id).event_bus_fault,
            Some(BusFault::Fault)
        );
    }

    #[test]
    fn empty_block_does_not_decode_its_address() {
        let (mut decoder, _) =
            single_device_decoder(32, TestDevice::memory(1), range(0, 1), DeviceAddr::new(0));
        let mut port = decoder.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        assert_eq!(port.read_block(PhysAddr::new(u64::MAX), &mut []), Ok(()));
        assert_eq!(port.write_block(PhysAddr::new(u64::MAX), &[]), Ok(()));
    }
}

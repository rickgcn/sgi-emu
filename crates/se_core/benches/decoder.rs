use std::fmt::Write;
use std::hint::black_box;
use std::time::{Duration, Instant};

use se_core::address::{AddressSpaceConfig, DeviceAddr, PhysAddr, PhysRange};
use se_core::bus::{
    Bus, BusFault, BusInitiator, CpuId, DirectAccess, DirectSpan, MmioAccess, MmioDevice,
};
use se_core::decode::{AddressMap, Decoder, DeviceRegistryBuilder};
use se_core::device::{Device, DeviceCtx, DeviceError};
use se_core::inspect::{InspectCommand, InspectError, Introspect};
use se_core::save::{Saveable, StateError, StateReader, StateWriter};

const SCALAR_ITERATIONS: u64 = 2_000_000;
const BLOCK_ITERATIONS: u64 = 500_000;
const SAMPLES: usize = 7;

enum Storage {
    AddressOnly,
    Bytes(Vec<u8>),
}

struct BenchDevice {
    storage: Storage,
}

impl BenchDevice {
    fn address_only() -> Self {
        Self {
            storage: Storage::AddressOnly,
        }
    }

    fn memory(size: usize) -> Self {
        Self {
            storage: Storage::Bytes(vec![0x5a; size]),
        }
    }

    fn read<const N: usize>(&self, addr: DeviceAddr) -> Result<[u8; N], BusFault> {
        match &self.storage {
            Storage::AddressOnly => {
                let encoded = addr.get().to_be_bytes();
                Ok(encoded[encoded.len() - N..]
                    .try_into()
                    .expect("fixed-width suffix has exact length"))
            }
            Storage::Bytes(bytes) => {
                let start = usize::try_from(addr.get()).map_err(|_| BusFault::Fault)?;
                bytes
                    .get(start..start.checked_add(N).ok_or(BusFault::Fault)?)
                    .ok_or(BusFault::Fault)?
                    .try_into()
                    .map_err(|_| BusFault::Fault)
            }
        }
    }

    fn write(&mut self, addr: DeviceAddr, input: &[u8]) -> Result<(), BusFault> {
        match &mut self.storage {
            Storage::AddressOnly => Ok(()),
            Storage::Bytes(bytes) => {
                let start = usize::try_from(addr.get()).map_err(|_| BusFault::Fault)?;
                let end = start.checked_add(input.len()).ok_or(BusFault::Fault)?;
                bytes
                    .get_mut(start..end)
                    .ok_or(BusFault::Fault)?
                    .copy_from_slice(input);
                Ok(())
            }
        }
    }
}

impl MmioDevice for BenchDevice {
    #[inline]
    fn read8(&mut self, access: MmioAccess) -> Result<u8, BusFault> {
        Ok(self.read::<1>(access.addr)?[0])
    }

    #[inline]
    fn read16(&mut self, access: MmioAccess) -> Result<u16, BusFault> {
        Ok(u16::from_be_bytes(self.read(access.addr)?))
    }

    #[inline]
    fn read32(&mut self, access: MmioAccess) -> Result<u32, BusFault> {
        Ok(u32::from_be_bytes(self.read(access.addr)?))
    }

    #[inline]
    fn read64(&mut self, access: MmioAccess) -> Result<u64, BusFault> {
        Ok(u64::from_be_bytes(self.read(access.addr)?))
    }

    #[inline]
    fn write8(&mut self, access: MmioAccess, value: u8) -> Result<(), BusFault> {
        self.write(access.addr, &[value])
    }

    #[inline]
    fn write16(&mut self, access: MmioAccess, value: u16) -> Result<(), BusFault> {
        self.write(access.addr, &value.to_be_bytes())
    }

    #[inline]
    fn write32(&mut self, access: MmioAccess, value: u32) -> Result<(), BusFault> {
        self.write(access.addr, &value.to_be_bytes())
    }

    #[inline]
    fn write64(&mut self, access: MmioAccess, value: u64) -> Result<(), BusFault> {
        self.write(access.addr, &value.to_be_bytes())
    }

    #[inline]
    fn read_block(&mut self, access: MmioAccess, output: &mut [u8]) -> Result<(), BusFault> {
        match &self.storage {
            Storage::AddressOnly => {
                for (offset, byte) in output.iter_mut().enumerate() {
                    *byte = access.addr.get().wrapping_add(offset as u64) as u8;
                }
                Ok(())
            }
            Storage::Bytes(bytes) => {
                let start = usize::try_from(access.addr.get()).map_err(|_| BusFault::Fault)?;
                let end = start.checked_add(output.len()).ok_or(BusFault::Fault)?;
                output.copy_from_slice(bytes.get(start..end).ok_or(BusFault::Fault)?);
                Ok(())
            }
        }
    }

    #[inline]
    fn write_block(&mut self, access: MmioAccess, input: &[u8]) -> Result<(), BusFault> {
        self.write(access.addr, input)
    }

    fn direct_span(
        &mut self,
        _access: MmioAccess,
        _requested: usize,
        _kind: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault> {
        Ok(None)
    }
}

impl Saveable for BenchDevice {
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

impl Introspect for BenchDevice {
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

impl Device for BenchDevice {
    fn reset(&mut self, _soft: bool) {}

    fn on_event(
        &mut self,
        _tag: u32,
        _payload: u64,
        _context: &mut DeviceCtx<'_>,
    ) -> Result<(), DeviceError> {
        Ok(())
    }
}

struct FlatControl {
    slots: Box<[u32]>,
    devices: Vec<Box<dyn MmioDevice>>,
}

impl FlatControl {
    fn dense_low_bank() -> Self {
        let mut slots = vec![u32::MAX; 65_536];
        slots[..8_192].fill(0);
        Self {
            slots: slots.into_boxed_slice(),
            devices: vec![dynamic_control_endpoint()],
        }
    }

    #[inline]
    fn read32(&mut self, addr: PhysAddr) -> Result<u32, BusFault> {
        let route = self.slots[(addr.get() >> 16) as usize];
        if route == u32::MAX {
            return Err(BusFault::Unmapped);
        }
        self.devices[route as usize].read32(MmioAccess {
            initiator: BusInitiator::Cpu(CpuId::from_raw(0)),
            addr: DeviceAddr::new(addr.get()),
        })
    }
}

struct AlternateEndpoint;

impl MmioDevice for AlternateEndpoint {
    fn read8(&mut self, access: MmioAccess) -> Result<u8, BusFault> {
        Ok(access.addr.get() as u8)
    }

    fn read16(&mut self, access: MmioAccess) -> Result<u16, BusFault> {
        Ok(access.addr.get() as u16)
    }

    fn read32(&mut self, access: MmioAccess) -> Result<u32, BusFault> {
        Ok(access.addr.get() as u32)
    }

    fn read64(&mut self, access: MmioAccess) -> Result<u64, BusFault> {
        Ok(access.addr.get())
    }

    fn write8(&mut self, _access: MmioAccess, _value: u8) -> Result<(), BusFault> {
        Ok(())
    }

    fn write16(&mut self, _access: MmioAccess, _value: u16) -> Result<(), BusFault> {
        Ok(())
    }

    fn write32(&mut self, _access: MmioAccess, _value: u32) -> Result<(), BusFault> {
        Ok(())
    }

    fn write64(&mut self, _access: MmioAccess, _value: u64) -> Result<(), BusFault> {
        Ok(())
    }

    fn direct_span(
        &mut self,
        _access: MmioAccess,
        _requested: usize,
        _kind: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault> {
        Ok(None)
    }
}

#[inline(never)]
fn dynamic_control_endpoint() -> Box<dyn MmioDevice> {
    if black_box(false) {
        Box::new(AlternateEndpoint)
    } else {
        Box::new(BenchDevice::address_only())
    }
}

fn build_decoder(bits: u8, start: u64, len: u64, device: BenchDevice) -> Decoder {
    let mut devices = DeviceRegistryBuilder::new();
    let id = devices.register(Box::new(device)).unwrap();
    let mut map = AddressMap::new(AddressSpaceConfig {
        physical_address_bits: bits,
    })
    .unwrap();
    map.map_region(
        id,
        PhysRange::from_start_len(PhysAddr::new(start), len).unwrap(),
        DeviceAddr::new(0),
    )
    .unwrap();
    Decoder::build(devices, map).unwrap()
}

fn build_unmapped_decoder() -> Decoder {
    let mut devices = DeviceRegistryBuilder::new();
    devices
        .register(Box::new(BenchDevice::address_only()))
        .unwrap();
    Decoder::build(
        devices,
        AddressMap::new(AddressSpaceConfig {
            physical_address_bits: 48,
        })
        .unwrap(),
    )
    .unwrap()
}

fn measure(mut operation: impl FnMut(u64) -> u64, iterations: u64) -> Duration {
    for index in 0..100_000 {
        black_box(operation(black_box(index)));
    }
    let start = Instant::now();
    let mut digest = 0_u64;
    for index in 0..iterations {
        digest ^= black_box(operation(black_box(index)));
    }
    black_box(digest);
    start.elapsed()
}

fn median_ns(mut operation: impl FnMut(u64) -> u64, iterations: u64) -> (f64, Duration) {
    let mut samples = (0..SAMPLES)
        .map(|_| measure(&mut operation, iterations))
        .collect::<Vec<_>>();
    samples.sort_unstable();
    let median = samples[SAMPLES / 2];
    (median.as_secs_f64() * 1e9 / iterations as f64, median)
}

fn report(name: &str, ns_per_operation: f64, elapsed: Duration) {
    println!("{name:<30} {ns_per_operation:>9.3} ns/op  median {elapsed:?}");
}

fn main() {
    println!("se_core decoder release benchmark");
    println!("samples={SAMPLES} scalar_iterations={SCALAR_ITERATIONS}");

    let mut control = FlatControl::dense_low_bank();
    let (control_ns, elapsed) = median_ns(
        |index| {
            let addr = PhysAddr::new(0x0100_0000 + ((index & 0xff) << 2));
            u64::from(control.read32(addr).unwrap())
        },
        SCALAR_ITERATIONS,
    );
    report("iris_flat_control", control_ns, elapsed);

    let mut dense = build_decoder(32, 0, 0x2000_0000, BenchDevice::address_only());
    let (dense_ns, elapsed) = {
        let mut port = dense.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                let addr = PhysAddr::new(0x0100_0000 + ((index & 0xff) << 2));
                u64::from(port.read32(addr).unwrap())
            },
            SCALAR_ITERATIONS,
        )
    };
    report("decoder_dense_direct", dense_ns, elapsed);

    let high_start = 1_u64 << 40;
    let mut uniform = build_decoder(48, high_start, 1_u64 << 32, BenchDevice::address_only());
    let (uniform_ns, elapsed) = {
        let mut port = uniform.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                let addr = PhysAddr::new(high_start + ((index & 0xff) << 2));
                u64::from(port.read32(addr).unwrap())
            },
            SCALAR_ITERATIONS,
        )
    };
    report("decoder_uniform_high", uniform_ns, elapsed);

    let mut edge = build_decoder(48, 0x1_0000_1234, 0x100, BenchDevice::address_only());
    let (edge_ns, elapsed) = {
        let mut port = edge.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                let addr = PhysAddr::new(0x1_0000_1234 + ((index & 0x1f) << 2));
                u64::from(port.read32(addr).unwrap())
            },
            SCALAR_ITERATIONS,
        )
    };
    report("decoder_edge", edge_ns, elapsed);

    let mut unmapped = build_unmapped_decoder();
    let (unmapped_ns, elapsed) = {
        let mut port = unmapped.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                u64::from(
                    port.read32(PhysAddr::new(0x2_0000_0000 + (index & 0xff)))
                        .is_err(),
                )
            },
            SCALAR_ITERATIONS,
        )
    };
    report("decoder_unmapped", unmapped_ns, elapsed);

    let mut block_read = build_decoder(32, 0, 4096, BenchDevice::memory(4096));
    let mut read_buffer = [0_u8; 64];
    let (block_read_ns, elapsed) = {
        let mut port = block_read.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                let addr = PhysAddr::new((index & 0x3f) * 64);
                port.read_block(addr, &mut read_buffer).unwrap();
                u64::from(read_buffer[(index as usize) & 0x3f])
            },
            BLOCK_ITERATIONS,
        )
    };
    report("decoder_read_block_64", block_read_ns, elapsed);

    let mut block_write = build_decoder(32, 0, 4096, BenchDevice::memory(4096));
    let write_buffer = [0xa5_u8; 64];
    let (block_write_ns, elapsed) = {
        let mut port = block_write.port(BusInitiator::Cpu(CpuId::from_raw(0)));
        median_ns(
            |index| {
                let addr = PhysAddr::new((index & 0x3f) * 64);
                port.write_block(addr, &write_buffer).unwrap();
                index
            },
            BLOCK_ITERATIONS,
        )
    };
    report("decoder_write_block_64", block_write_ns, elapsed);

    println!(
        "dense/control ratio              {:>9.3}x",
        dense_ns / control_ns
    );
}

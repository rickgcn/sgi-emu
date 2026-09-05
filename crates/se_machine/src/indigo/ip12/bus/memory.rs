use se_core::bus::{BusFault, PhysAddr};
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use serde::{Deserialize, Serialize};

use super::address::local_memory_transaction_is_contained;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct LocalMemory {
    modules: [Option<Ram>; 4],
}

impl LocalMemory {
    pub(super) const fn new(modules: [Option<Ram>; 4]) -> Self {
        Self { modules }
    }

    pub(super) fn read(
        &self,
        pic1: &Pic1,
        address: PhysAddr,
        data: &mut [u8],
    ) -> Result<(), BusFault> {
        if !local_memory_transaction_is_contained(address, data.len())? {
            return Err(BusFault::Unmapped);
        }
        let Some((index, offset)) = pic1.decode_memory(address, data.len())? else {
            return Err(BusFault::Unmapped);
        };
        let Some(ram) = &self.modules[index] else {
            data.fill(0);
            return Ok(());
        };
        match ram.read(offset, data) {
            Ok(()) => Ok(()),
            Err(BusFault::Unmapped) => {
                data.fill(0);
                Ok(())
            }
            Err(fault) => Err(fault),
        }
    }

    pub(super) fn write(
        &mut self,
        pic1: &mut Pic1,
        address: PhysAddr,
        data: &[u8],
    ) -> Result<(), BusFault> {
        if !local_memory_transaction_is_contained(address, data.len())? {
            pic1.report_address_error();
            return Ok(());
        }
        let Some((index, offset)) = pic1.decode_memory(address, data.len())? else {
            pic1.report_address_error();
            return Ok(());
        };
        let Some(ram) = &mut self.modules[index] else {
            return Ok(());
        };
        match ram.write(offset, data) {
            Ok(()) | Err(BusFault::Unmapped) => Ok(()),
            Err(fault) => Err(fault),
        }
    }

    pub(super) fn read_dma(&self, pic1: &mut Pic1, address: u32, data: &mut [u8]) -> bool {
        let address = PhysAddr::new(u64::from(address));
        let Ok(Some((index, offset))) = pic1.decode_memory(address, data.len()) else {
            pic1.report_address_error();
            return false;
        };
        let Some(ram) = &self.modules[index] else {
            pic1.report_address_error();
            return false;
        };
        if ram.read(offset, data).is_err() {
            pic1.report_address_error();
            return false;
        }
        true
    }

    pub(super) fn write_dma(&mut self, pic1: &mut Pic1, address: u32, data: &[u8]) -> bool {
        let address = PhysAddr::new(u64::from(address));
        let Ok(Some((index, offset))) = pic1.decode_memory(address, data.len()) else {
            pic1.report_address_error();
            return false;
        };
        let Some(ram) = &mut self.modules[index] else {
            pic1.report_address_error();
            return false;
        };
        if ram.write(offset, data).is_err() {
            pic1.report_address_error();
            return false;
        }
        true
    }

    #[cfg(test)]
    pub(super) fn module(&self, index: usize) -> Option<&Ram> {
        self.modules[index].as_ref()
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_device::ram::Ram;

    use super::super::address::{LOCAL_MEMORY_END, PIC1_BASE};
    use super::super::test_support::{bus, bus_with_memory, configure_memory, read_word};

    #[test]
    fn routes_real_memory_through_the_pic1_configuration() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_023f, 0x023f_023f);

        assert_eq!(
            bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 0), Ok(0x0123_4567));
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024 - 4),
                &0x89ab_cdef_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x89ab_cdef));
    }

    #[test]
    fn installed_storage_boundary_is_a_probe_hole_not_an_alias() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_023f, 0x023f_023f);
        bus.write(
            PhysAddr::new(8 * 1024 * 1024 - 4),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024), Ok(0));
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024),
                &0x89ab_cdef_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x0123_4567));

        let mut crossing = [0xff; 4];
        assert_eq!(
            bus.read(PhysAddr::new(8 * 1024 * 1024 - 2), &mut crossing),
            Ok(())
        );
        assert_eq!(crossing, [0; 4]);
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024 - 2),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x0123_4567));
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn uninstalled_descriptor_is_a_probe_hole() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_0f10, 0x023f_023f);
        let address = 16_u64 << 22;

        assert_eq!(read_word(&mut bus, address), Ok(0));
        assert_eq!(
            bus.write(PhysAddr::new(address), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, address), Ok(0));
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn unmatched_local_reads_fault_and_writes_latch_an_error() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x023f_023f, 0x023f_023f);

        assert_eq!(
            bus.read(PhysAddr::new(0), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.debug_read(PhysAddr::new(0), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());

        bus.write(PhysAddr::new(PIC1_BASE + 0x1_0210), &[0])
            .unwrap();
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn separate_ram_modules_do_not_alias() {
        let mut bus = bus_with_memory([
            Some(Ram::new(16 * 1024 * 1024)),
            Some(Ram::new(32 * 1024 * 1024)),
            None,
            None,
        ]);
        configure_memory(&mut bus, 0x0300_0704, 0x023f_023f);
        let second_base = 4_u64 << 22;

        bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes())
            .unwrap();
        bus.write(PhysAddr::new(second_base), &0x89ab_cdef_u32.to_be_bytes())
            .unwrap();

        assert_eq!(read_word(&mut bus, 0), Ok(0x0123_4567));
        assert_eq!(read_word(&mut bus, second_base), Ok(0x89ab_cdef));
        assert_eq!(
            bus.write(
                PhysAddr::new(second_base + 32 * 1024 * 1024 - 4),
                &0xfedc_ba98_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(
            read_word(&mut bus, second_base + 32 * 1024 * 1024 - 4),
            Ok(0xfedc_ba98)
        );
    }

    #[test]
    fn local_memory_window_crossing_writes_latch_an_error() {
        let mut bus = bus();

        assert_eq!(
            bus.write(PhysAddr::new(LOCAL_MEMORY_END - 2), &[1, 2, 3, 4]),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());
        assert_eq!(
            bus.read(PhysAddr::new(LOCAL_MEMORY_END - 2), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
    }
}

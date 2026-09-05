use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
use serde::{Deserialize, Serialize};

use super::R3000Config;

const WORD_BYTES: usize = 4;
const PAGE_FRAME_MASK: u32 = 0xffff_f000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CacheEntry {
    page_frame: u32,
    data: [u8; WORD_BYTES],
    valid: bool,
}

impl CacheEntry {
    const INVALID: Self = Self {
        page_frame: 0,
        data: [0; WORD_BYTES],
        valid: false,
    };
}

#[derive(Clone, Deserialize, Serialize)]
struct Cache {
    entries: Box<[CacheEntry]>,
    refill_bytes: usize,
}

impl Cache {
    fn new(cache_bytes: usize, refill_bytes: usize) -> Self {
        Self {
            entries: vec![CacheEntry::INVALID; cache_bytes / WORD_BYTES].into_boxed_slice(),
            refill_bytes,
        }
    }

    fn read(
        &mut self,
        address: PhysAddr,
        data: &mut [u8],
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        let access = self.access(address, data.len());
        let entry = self.entries[access.index];
        if entry.valid && entry.page_frame == access.page_frame {
            data.copy_from_slice(&entry.data[access.offset..access.offset + data.len()]);
            return Ok(());
        }

        let refill_base = access.word_address & !((self.refill_bytes as u32) - 1);
        let mut refill = vec![0; self.refill_bytes];
        for (word_index, word) in refill.chunks_exact_mut(WORD_BYTES).enumerate() {
            let word_address = refill_base + (word_index * WORD_BYTES) as u32;
            bus.read(PhysAddr::new(u64::from(word_address)), word)?;
        }

        for (word_index, word) in refill.chunks_exact(WORD_BYTES).enumerate() {
            let word_address = refill_base + (word_index * WORD_BYTES) as u32;
            let index = self.index(word_address);
            self.entries[index] = CacheEntry {
                page_frame: page_frame(word_address),
                data: word.try_into().expect("a cache word has four bytes"),
                valid: true,
            };
        }

        let entry = self.entries[access.index];
        data.copy_from_slice(&entry.data[access.offset..access.offset + data.len()]);
        Ok(())
    }

    fn write(
        &mut self,
        address: PhysAddr,
        data: &[u8],
        partial_store_enabled: bool,
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        let access = self.access(address, data.len());

        if data.len() == WORD_BYTES {
            bus.write(address, data)?;
            self.entries[access.index] = CacheEntry {
                page_frame: access.page_frame,
                data: data.try_into().expect("a full cache store has four bytes"),
                valid: true,
            };
            return Ok(());
        }

        let entry = self.entries[access.index];
        let hit = entry.valid && entry.page_frame == access.page_frame;
        if partial_store_enabled && hit {
            let mut merged = entry.data;
            merged[access.offset..access.offset + data.len()].copy_from_slice(data);
            bus.write(PhysAddr::new(u64::from(access.word_address)), &merged)?;
            self.entries[access.index].data = merged;
        } else {
            bus.write(address, data)?;
            if !partial_store_enabled {
                self.entries[access.index].valid = false;
            }
        }

        Ok(())
    }

    fn read_isolated(&self, address: PhysAddr, data: &mut [u8]) -> bool {
        let access = self.access(address, data.len());
        let entry = self.entries[access.index];
        data.copy_from_slice(&entry.data[access.offset..access.offset + data.len()]);
        !(entry.valid && entry.page_frame == access.page_frame)
    }

    fn write_isolated(&mut self, address: PhysAddr, data: &[u8]) {
        let access = self.access(address, data.len());
        if data.len() == WORD_BYTES {
            self.entries[access.index] = CacheEntry {
                page_frame: access.page_frame,
                data: data.try_into().expect("a full cache store has four bytes"),
                valid: true,
            };
        } else {
            self.entries[access.index].valid = false;
        }
    }

    fn access(&self, address: PhysAddr, length: usize) -> CacheAccess {
        assert!((1..=WORD_BYTES).contains(&length));
        assert!(address.get() <= u64::from(u32::MAX));

        let address = address.get() as u32;
        let word_address = address & !(WORD_BYTES as u32 - 1);
        let offset = (address & (WORD_BYTES as u32 - 1)) as usize;
        assert!(offset + length <= WORD_BYTES);

        CacheAccess {
            word_address,
            offset,
            index: self.index(word_address),
            page_frame: page_frame(word_address),
        }
    }

    fn index(&self, word_address: u32) -> usize {
        ((word_address >> 2) as usize) & (self.entries.len() - 1)
    }
}

#[derive(Clone, Copy)]
struct CacheAccess {
    word_address: u32,
    offset: usize,
    index: usize,
    page_frame: u32,
}

const fn page_frame(word_address: u32) -> u32 {
    word_address & PAGE_FRAME_MASK
}

#[derive(Clone, Copy)]
pub(super) enum CacheBank {
    Instruction,
    Data,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Caches {
    instruction: Cache,
    data: Cache,
    partial_store_enabled: bool,
}

impl Caches {
    pub(super) fn new(config: R3000Config) -> Self {
        Self {
            instruction: Cache::new(
                config.instruction_cache_bytes(),
                config.instruction_refill_bytes(),
            ),
            data: Cache::new(config.data_cache_bytes(), config.data_refill_bytes()),
            partial_store_enabled: config.partial_store_enabled(),
        }
    }

    pub(super) fn read(
        &mut self,
        bank: CacheBank,
        address: PhysAddr,
        data: &mut [u8],
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        self.cache_mut(bank).read(address, data, bus)
    }

    pub(super) fn write(
        &mut self,
        bank: CacheBank,
        address: PhysAddr,
        data: &[u8],
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        let partial_store_enabled = self.partial_store_enabled;
        self.cache_mut(bank)
            .write(address, data, partial_store_enabled, bus)
    }

    pub(super) fn read_isolated(
        &self,
        bank: CacheBank,
        address: PhysAddr,
        data: &mut [u8],
    ) -> bool {
        self.cache(bank).read_isolated(address, data)
    }

    pub(super) fn write_isolated(&mut self, bank: CacheBank, address: PhysAddr, data: &[u8]) {
        self.cache_mut(bank).write_isolated(address, data);
    }

    pub(super) fn debug_entries(
        &self,
        bank: CacheBank,
    ) -> (usize, Vec<(u32, [u8; WORD_BYTES], bool)>) {
        let cache = self.cache(bank);
        let entries = cache
            .entries
            .iter()
            .map(|entry| (entry.page_frame, entry.data, entry.valid))
            .collect();
        (cache.refill_bytes, entries)
    }

    fn cache(&self, bank: CacheBank) -> &Cache {
        match bank {
            CacheBank::Instruction => &self.instruction,
            CacheBank::Data => &self.data,
        }
    }

    fn cache_mut(&mut self, bank: CacheBank) -> &mut Cache {
        match bank {
            CacheBank::Instruction => &mut self.instruction,
            CacheBank::Data => &mut self.data,
        }
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_float::backend::Backend;

    use super::{CacheBank, Caches};
    use crate::mips1::r3000::R3000Config;

    const CONFIG: R3000Config =
        R3000Config::new(1, 4 * 1024, 4 * 1024, 16, 4, true, Backend::SoftFloat);

    struct TestBus {
        memory: Vec<u8>,
        reads: Vec<(PhysAddr, usize)>,
        writes: Vec<(PhysAddr, Vec<u8>)>,
        read_fault_address: Option<PhysAddr>,
        write_fault: Option<BusFault>,
    }

    impl TestBus {
        fn new() -> Self {
            Self {
                memory: (0..=u8::MAX).cycle().take(0x4000).collect(),
                reads: Vec::new(),
                writes: Vec::new(),
                read_fault_address: None,
                write_fault: None,
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.reads.push((address, data.len()));
            if self.read_fault_address == Some(address) {
                return Err(BusFault::Unmapped);
            }

            let start = address.get() as usize;
            let source = self
                .memory
                .get(start..start + data.len())
                .ok_or(BusFault::Unmapped)?;
            data.copy_from_slice(source);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            if let Some(fault) = self.write_fault {
                return Err(fault);
            }

            self.writes.push((address, data.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn new_caches_are_invalid_and_zero_filled() {
        let caches = Caches::new(CONFIG);
        let mut data = [0xff; 4];

        let miss = caches.read_isolated(CacheBank::Instruction, PhysAddr::new(0x1234), &mut data);

        assert!(miss);
        assert_eq!(data, [0; 4]);
    }

    #[test]
    fn configuration_accepts_supported_geometries_and_rejects_others() {
        for cache_bytes in [
            4 * 1024,
            8 * 1024,
            16 * 1024,
            32 * 1024,
            64 * 1024,
            128 * 1024,
            256 * 1024,
        ] {
            let config =
                R3000Config::new(1, cache_bytes, cache_bytes, 4, 4, true, Backend::SoftFloat);
            let _ = Caches::new(config);
        }
        for refill_bytes in [4, 16, 32, 64, 128] {
            let config = R3000Config::new(
                1,
                4 * 1024,
                4 * 1024,
                refill_bytes,
                refill_bytes,
                true,
                Backend::SoftFloat,
            );
            let _ = Caches::new(config);
        }

        for cache_bytes in [0, 3 * 1024, 6 * 1024, 512 * 1024] {
            let cache_bytes = std::hint::black_box(cache_bytes);
            assert!(
                std::panic::catch_unwind(|| {
                    R3000Config::new(1, cache_bytes, 4 * 1024, 4, 4, true, Backend::SoftFloat)
                })
                .is_err()
            );
        }
        for refill_bytes in [0, 8, 256] {
            let refill_bytes = std::hint::black_box(refill_bytes);
            assert!(
                std::panic::catch_unwind(|| {
                    R3000Config::new(
                        1,
                        4 * 1024,
                        4 * 1024,
                        refill_bytes,
                        4,
                        true,
                        Backend::SoftFloat,
                    )
                })
                .is_err()
            );
        }
    }

    #[test]
    fn index_wrap_uses_page_frame_tag_for_multiple_cache_sizes() {
        for cache_bytes in [4 * 1024, 8 * 1024] {
            let config = R3000Config::new(1, cache_bytes, 4 * 1024, 4, 4, true, Backend::SoftFloat);
            let mut caches = Caches::new(config);
            let first = PhysAddr::new(0x234);
            let alias = PhysAddr::new(0x234 + cache_bytes as u64);
            caches.write_isolated(CacheBank::Instruction, first, &[1, 2, 3, 4]);

            let mut data = [0; 4];
            assert!(caches.read_isolated(CacheBank::Instruction, alias, &mut data));
            assert_eq!(data, [1, 2, 3, 4]);

            caches.write_isolated(CacheBank::Instruction, alias, &[5, 6, 7, 8]);
            assert!(caches.read_isolated(CacheBank::Instruction, first, &mut data));
            assert_eq!(data, [5, 6, 7, 8]);
            assert!(!caches.read_isolated(CacheBank::Instruction, alias, &mut data));
        }
    }

    #[test]
    fn read_miss_refills_the_configured_block_and_then_hits() {
        let mut caches = Caches::new(CONFIG);
        let mut bus = TestBus::new();
        let mut first = [0; 4];

        caches
            .read(
                CacheBank::Instruction,
                PhysAddr::new(0x24),
                &mut first,
                &mut bus,
            )
            .expect("refill should succeed");
        let reads_after_refill = bus.reads.len();
        let mut second = [0; 4];
        caches
            .read(
                CacheBank::Instruction,
                PhysAddr::new(0x24),
                &mut second,
                &mut bus,
            )
            .expect("resident word should be readable");

        assert_eq!(first, [0x24, 0x25, 0x26, 0x27]);
        assert_eq!(second, first);
        assert_eq!(bus.reads.len(), reads_after_refill);
        assert_eq!(
            &bus.reads,
            &[
                (PhysAddr::new(0x20), 4),
                (PhysAddr::new(0x24), 4),
                (PhysAddr::new(0x28), 4),
                (PhysAddr::new(0x2c), 4),
            ]
        );
    }

    #[test]
    fn every_supported_refill_size_commits_words_in_address_order() {
        for refill_bytes in [4, 16, 32, 64, 128] {
            let config = R3000Config::new(
                1,
                4 * 1024,
                4 * 1024,
                refill_bytes,
                4,
                true,
                Backend::SoftFloat,
            );
            let mut caches = Caches::new(config);
            let mut bus = TestBus::new();
            let address = PhysAddr::new(0x1ac);
            let mut data = [0; 4];

            caches
                .read(CacheBank::Instruction, address, &mut data, &mut bus)
                .expect("refill should succeed");

            let refill_base = 0x1ac_u64 & !((refill_bytes as u64) - 1);
            let expected_reads: Vec<_> = (0..refill_bytes / 4)
                .map(|word| (PhysAddr::new(refill_base + (word * 4) as u64), 4))
                .collect();
            assert_eq!(bus.reads, expected_reads);
            assert_eq!(data, [0xac, 0xad, 0xae, 0xaf]);

            for word in 0..refill_bytes / 4 {
                let word_address = PhysAddr::new(refill_base + (word * 4) as u64);
                let mut resident = [0; 4];
                assert!(
                    !caches.read_isolated(CacheBank::Instruction, word_address, &mut resident,)
                );
                let first_byte = word_address.get() as u8;
                assert_eq!(
                    resident,
                    [
                        first_byte,
                        first_byte.wrapping_add(1),
                        first_byte.wrapping_add(2),
                        first_byte.wrapping_add(3),
                    ]
                );
            }
        }
    }

    #[test]
    fn cache_hits_return_every_supported_access_width_without_bus_reads() {
        let mut caches = Caches::new(CONFIG);
        caches.write_isolated(
            CacheBank::Instruction,
            PhysAddr::new(0x300),
            &[10, 11, 12, 13],
        );
        let mut bus = TestBus::new();
        bus.read_fault_address = Some(PhysAddr::new(0x300));

        for (offset, length, expected) in [
            (3, 1, &[13][..]),
            (2, 2, &[12, 13][..]),
            (1, 3, &[11, 12, 13][..]),
            (0, 4, &[10, 11, 12, 13][..]),
        ] {
            let mut data = [0; 4];
            caches
                .read(
                    CacheBank::Instruction,
                    PhysAddr::new(0x300 + offset),
                    &mut data[..length],
                    &mut bus,
                )
                .expect("resident access should hit");
            assert_eq!(&data[..length], expected);
        }

        assert!(bus.reads.is_empty());
    }

    #[test]
    fn failed_refill_preserves_cache_and_output() {
        let mut caches = Caches::new(CONFIG);
        caches.write_isolated(CacheBank::Instruction, PhysAddr::new(0x20), &[1, 2, 3, 4]);
        let mut bus = TestBus::new();
        bus.read_fault_address = Some(PhysAddr::new(0x1028));
        let mut output = [0xaa; 4];

        assert_eq!(
            caches.read(
                CacheBank::Instruction,
                PhysAddr::new(0x1024),
                &mut output,
                &mut bus,
            ),
            Err(BusFault::Unmapped)
        );

        let mut resident = [0; 4];
        assert!(!caches.read_isolated(CacheBank::Instruction, PhysAddr::new(0x20), &mut resident,));
        assert_eq!(resident, [1, 2, 3, 4]);
        assert_eq!(output, [0xaa; 4]);
    }

    #[test]
    fn normal_stores_follow_full_and_partial_write_policies() {
        let mut caches = Caches::new(CONFIG);
        let mut bus = TestBus::new();

        caches
            .write(
                CacheBank::Data,
                PhysAddr::new(0x40),
                &[1, 2, 3, 4],
                &mut bus,
            )
            .expect("full store should succeed");
        caches
            .write(
                CacheBank::Data,
                PhysAddr::new(0x40),
                &[4, 3, 2, 1],
                &mut bus,
            )
            .expect("resident full store should succeed");
        caches
            .write(CacheBank::Data, PhysAddr::new(0x41), &[9, 8], &mut bus)
            .expect("resident partial store should merge");
        caches
            .write(CacheBank::Data, PhysAddr::new(0x1041), &[7, 6], &mut bus)
            .expect("nonresident partial store should bypass the cache");

        assert_eq!(
            bus.writes,
            vec![
                (PhysAddr::new(0x40), vec![1, 2, 3, 4]),
                (PhysAddr::new(0x40), vec![4, 3, 2, 1]),
                (PhysAddr::new(0x40), vec![4, 9, 8, 1]),
                (PhysAddr::new(0x1041), vec![7, 6]),
            ]
        );
        let mut resident = [0; 4];
        assert!(!caches.read_isolated(CacheBank::Data, PhysAddr::new(0x40), &mut resident,));
        assert_eq!(resident, [4, 9, 8, 1]);
    }

    #[test]
    fn disabled_partial_stores_invalidate_after_a_successful_bus_write() {
        const NO_PARTIAL: R3000Config =
            R3000Config::new(1, 4 * 1024, 4 * 1024, 4, 4, false, Backend::SoftFloat);
        let mut caches = Caches::new(NO_PARTIAL);
        let mut bus = TestBus::new();
        caches.write_isolated(CacheBank::Data, PhysAddr::new(0x40), &[1, 2, 3, 4]);

        caches
            .write(CacheBank::Data, PhysAddr::new(0x1042), &[9], &mut bus)
            .expect("partial store should succeed");

        let mut data = [0; 4];
        assert!(caches.read_isolated(CacheBank::Data, PhysAddr::new(0x40), &mut data));
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(bus.writes, vec![(PhysAddr::new(0x1042), vec![9])]);
    }

    #[test]
    fn isolated_partial_store_invalidates_without_changing_data() {
        let mut caches = Caches::new(CONFIG);
        caches.write_isolated(CacheBank::Instruction, PhysAddr::new(0x80), &[1, 2, 3, 4]);

        caches.write_isolated(CacheBank::Instruction, PhysAddr::new(0x81), &[9, 8]);

        let mut data = [0; 4];
        assert!(caches.read_isolated(CacheBank::Instruction, PhysAddr::new(0x80), &mut data,));
        assert_eq!(data, [1, 2, 3, 4]);
    }

    #[test]
    fn failed_normal_store_does_not_mutate_the_cache() {
        let mut caches = Caches::new(CONFIG);
        caches.write_isolated(CacheBank::Data, PhysAddr::new(0xc0), &[1, 2, 3, 4]);
        let mut bus = TestBus::new();
        bus.write_fault = Some(BusFault::Unmapped);

        assert_eq!(
            caches.write(CacheBank::Data, PhysAddr::new(0xc1), &[9], &mut bus,),
            Err(BusFault::Unmapped)
        );

        let mut data = [0; 4];
        assert!(!caches.read_isolated(CacheBank::Data, PhysAddr::new(0xc0), &mut data));
        assert_eq!(data, [1, 2, 3, 4]);
    }

    #[test]
    fn instruction_and_data_caches_keep_independent_geometry_and_state() {
        let config = R3000Config::new(1, 4 * 1024, 8 * 1024, 4, 4, true, Backend::SoftFloat);
        let mut caches = Caches::new(config);
        caches.write_isolated(CacheBank::Instruction, PhysAddr::new(0), &[1, 2, 3, 4]);
        caches.write_isolated(CacheBank::Data, PhysAddr::new(0), &[5, 6, 7, 8]);
        caches.write_isolated(
            CacheBank::Instruction,
            PhysAddr::new(0x1000),
            &[9, 10, 11, 12],
        );
        caches.write_isolated(CacheBank::Data, PhysAddr::new(0x1000), &[13, 14, 15, 16]);

        let mut instruction = [0; 4];
        let mut data = [0; 4];
        assert!(caches.read_isolated(CacheBank::Instruction, PhysAddr::new(0), &mut instruction,));
        assert!(!caches.read_isolated(CacheBank::Data, PhysAddr::new(0), &mut data,));
        assert_eq!(instruction, [9, 10, 11, 12]);
        assert_eq!(data, [5, 6, 7, 8]);
    }
}

use se_core::bus::PhysAddr;

const KSEG1_START: u32 = 0xa000_0000;
const KSEG1_END: u32 = 0xbfff_ffff;
const PHYSICAL_ADDRESS_MASK: u32 = 0x1fff_ffff;

pub(super) fn translate_instruction_address(virtual_address: u32) -> Option<PhysAddr> {
    (KSEG1_START..=KSEG1_END)
        .contains(&virtual_address)
        .then_some(PhysAddr::new(u64::from(
            virtual_address & PHYSICAL_ADDRESS_MASK,
        )))
}

#[cfg(test)]
mod tests {
    use se_core::bus::PhysAddr;

    use super::translate_instruction_address;

    #[test]
    fn translates_kseg1_boundaries_and_reset_vector() {
        assert_eq!(
            translate_instruction_address(0xa000_0000),
            Some(PhysAddr::new(0))
        );
        assert_eq!(
            translate_instruction_address(0xbfc0_0000),
            Some(PhysAddr::new(0x1fc0_0000))
        );
        assert_eq!(
            translate_instruction_address(0xbfff_ffff),
            Some(PhysAddr::new(0x1fff_ffff))
        );
    }

    #[test]
    fn rejects_addresses_outside_kseg1() {
        assert_eq!(translate_instruction_address(0x9fff_ffff), None);
        assert_eq!(translate_instruction_address(0xc000_0000), None);
    }
}

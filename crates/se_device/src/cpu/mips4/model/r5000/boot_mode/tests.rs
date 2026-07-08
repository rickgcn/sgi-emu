use super::*;

fn boot_mode(bits: u64) -> R5000BootMode {
    R5000BootMode::from_low_bits(bits).unwrap()
}

fn field_bits(shift: usize, value: u8) -> u64 {
    (value as u64) << shift
}

#[test]
fn bitstream_uses_documented_bit_numbering() {
    let mode = boot_mode(
        (1u64 << 8)
            | (1u64 << 11)
            | (1u64 << 12)
            | (1u64 << 15)
            | (1u64 << 18)
            | (1u64 << 20)
            | (1u64 << 33)
            | (1u64 << 37)
            | (1u64 << 38),
    );

    assert_eq!(mode.low_bits(), mode.words_le()[0]);
    assert_eq!(mode.bit(0), Ok(false));
    assert_eq!(mode.bit(8), Ok(true));
    assert_eq!(mode.bit(255), Ok(false));
    assert_eq!(
        mode.bit(256),
        Err(R5000BootModeError::BitIndexOutOfRange { index: 256 })
    );
}

#[test]
fn constructors_reject_reserved_bits() {
    assert_eq!(
        R5000BootMode::from_low_bits(1),
        Err(R5000BootModeError::ReservedBitSet { bit: 0 })
    );
    assert_eq!(
        R5000BootMode::from_low_bits(1u64 << 19),
        Err(R5000BootModeError::ReservedBitSet { bit: 19 })
    );
    assert_eq!(
        R5000BootMode::from_low_bits(1u64 << 21),
        Err(R5000BootModeError::ReservedBitSet { bit: 21 })
    );
    assert_eq!(
        R5000BootMode::from_low_bits(1u64 << 34),
        Err(R5000BootModeError::ReservedBitSet { bit: 34 })
    );
    assert_eq!(
        R5000BootMode::from_low_bits(1u64 << 39),
        Err(R5000BootModeError::ReservedBitSet { bit: 39 })
    );
    assert_eq!(
        R5000BootMode::from_words_le([0, 1, 0, 0]),
        Err(R5000BootModeError::ReservedBitSet { bit: 64 })
    );
}

#[test]
fn constructors_accept_legacy_revision_workaround_bits() {
    let mode = boot_mode((1u64 << 20) | (1u64 << 33) | (1u64 << 37));

    assert!(mode.revision_2_41_or_lower_workaround_bit20());
    assert!(mode.revision_2_41_or_lower_workaround_bit33());
    assert!(mode.revision_2_x_or_lower_workaround_bit37());
}

#[test]
fn transmit_data_patterns_accept_documented_values() {
    let patterns = [
        R5000TransmitDataPattern::Dddd,
        R5000TransmitDataPattern::DdxDdx,
        R5000TransmitDataPattern::DdxxDdxx,
        R5000TransmitDataPattern::Dxdxdxdx,
        R5000TransmitDataPattern::DdxxxDdxxx,
        R5000TransmitDataPattern::DdxxxxDdxxxx,
        R5000TransmitDataPattern::DxxDxxDxxDxx,
        R5000TransmitDataPattern::DdxxxxxxDdxxxxxx,
        R5000TransmitDataPattern::DxxxDxxxDxxxDxxx,
    ];

    for (bits, pattern) in patterns.into_iter().enumerate() {
        let mode = boot_mode(field_bits(FIELD_TRANSMIT_DATA_PATTERN_SHIFT, bits as u8));

        assert_eq!(mode.transmit_data_pattern_bits(), bits as u8);
        assert_eq!(mode.transmit_data_pattern(), pattern);
        assert_eq!(pattern.bits(), bits as u8);
    }

    assert_eq!(
        R5000BootMode::from_low_bits(field_bits(FIELD_TRANSMIT_DATA_PATTERN_SHIFT, 9)),
        Err(R5000BootModeError::ReservedFieldValue {
            field: R5000BootModeField::TransmitDataPattern,
            value: 9,
        })
    );
}

#[test]
fn clock_multiplier_handles_integer_ratios_and_r5000a_override() {
    let expected = [
        R5000ClockMultiplier::Times2,
        R5000ClockMultiplier::Times3,
        R5000ClockMultiplier::Times4,
        R5000ClockMultiplier::Times5,
        R5000ClockMultiplier::Times6,
        R5000ClockMultiplier::Times7,
        R5000ClockMultiplier::Times8,
    ];

    for (bits, multiplier) in expected.into_iter().enumerate() {
        let mode = boot_mode(field_bits(FIELD_SYS_CLOCK_RATIO_SHIFT, bits as u8));

        assert_eq!(mode.sys_clock_ratio_bits(), bits as u8);
        assert_eq!(mode.sys_clock_ratio_multiplier(), Some(multiplier));
        assert_eq!(mode.effective_clock_multiplier(), multiplier);
    }

    assert_eq!(
        R5000BootMode::from_low_bits(field_bits(FIELD_SYS_CLOCK_RATIO_SHIFT, 7)),
        Err(R5000BootModeError::ReservedFieldValue {
            field: R5000BootModeField::SysClockRatio,
            value: 7,
        })
    );

    let override_mode = boot_mode(
        field_bits(FIELD_SYS_CLOCK_RATIO_SHIFT, 7) | (1u64 << BIT_R5000A_TWO_POINT_FIVE_MULTIPLIER),
    );

    assert_eq!(override_mode.sys_clock_ratio_multiplier(), None);
    assert!(override_mode.two_point_five_clock_multiplier_enabled());
    assert_eq!(
        override_mode.effective_clock_multiplier(),
        R5000ClockMultiplier::TimesTwoAndOneHalf
    );
    assert_eq!(override_mode.effective_clock_multiplier().numerator(), 5);
    assert_eq!(override_mode.effective_clock_multiplier().denominator(), 2);
}

#[test]
fn endian_bit_is_ored_with_big_endian_pin() {
    let little_mode = boot_mode(0);
    let big_mode = boot_mode(1u64 << BIT_ENDIAN_MODE);

    assert_eq!(little_mode.endianness(false), Mips4Endianness::Little);
    assert_eq!(little_mode.endianness(true), Mips4Endianness::Big);
    assert_eq!(big_mode.endianness(false), Mips4Endianness::Big);
    assert_eq!(big_mode.endianness(true), Mips4Endianness::Big);
}

#[test]
fn non_block_write_modes_reject_reserved_value() {
    let cases = [
        (0, R5000NonBlockWriteMode::Vr4x00Compatible),
        (2, R5000NonBlockWriteMode::PipelinedWrites),
        (3, R5000NonBlockWriteMode::WriteReissue),
    ];

    for (bits, mode) in cases {
        let boot_mode = boot_mode(field_bits(FIELD_NON_BLOCK_WRITE_SHIFT, bits));

        assert_eq!(boot_mode.non_block_write_bits(), bits);
        assert_eq!(boot_mode.non_block_write_mode(), mode);
        assert_eq!(mode.bits(), bits);
    }

    assert_eq!(
        R5000BootMode::from_low_bits(field_bits(FIELD_NON_BLOCK_WRITE_SHIFT, 1)),
        Err(R5000BootModeError::ReservedFieldValue {
            field: R5000BootModeField::NonBlockWrite,
            value: 1,
        })
    );
}

#[test]
fn simple_boolean_and_protocol_fields_decode() {
    let mode = boot_mode(
        (1u64 << BIT_TIMER_INTERRUPT_DISABLE)
            | (1u64 << BIT_SECONDARY_CACHE_ENABLE)
            | (1u64 << BIT_SECONDARY_CACHE_SRAM_PROTOCOL)
            | (1u64 << BIT_COUNT_UPDATE_RATE),
    );

    assert!(!mode.timer_interrupt_enabled());
    assert!(mode.secondary_cache_enabled());
    assert_eq!(
        mode.secondary_cache_sram_protocol(),
        R5000SecondaryCacheSramProtocol::Burst
    );
    assert_eq!(mode.count_update_rate(), R5000CountUpdateRate::PClock);

    let default_mode = boot_mode(0);

    assert!(default_mode.timer_interrupt_enabled());
    assert!(!default_mode.secondary_cache_enabled());
    assert_eq!(
        default_mode.secondary_cache_sram_protocol(),
        R5000SecondaryCacheSramProtocol::Pipelined
    );
    assert_eq!(
        default_mode.count_update_rate(),
        R5000CountUpdateRate::HalfPClock
    );
}

#[test]
fn driver_slew_rate_uses_documented_encodings() {
    let cases = [
        (0b10, R5000DriverSlewRate::Percent100),
        (0b11, R5000DriverSlewRate::Percent83),
        (0b00, R5000DriverSlewRate::Percent67),
        (0b01, R5000DriverSlewRate::Percent50),
    ];

    for (bits, rate) in cases {
        let mode = boot_mode(field_bits(FIELD_DRIVER_SLEW_RATE_SHIFT, bits));

        assert_eq!(mode.driver_slew_rate_bits(), bits);
        assert_eq!(mode.driver_slew_rate(), rate);
        assert_eq!(rate.bits(), bits);
    }
}

#[test]
fn secondary_cache_size_decodes_documented_values() {
    let cases = [
        (0, R5000SecondaryCacheSize::Size512Kib, 512 * 1024),
        (1, R5000SecondaryCacheSize::Size1Mib, 1024 * 1024),
        (2, R5000SecondaryCacheSize::Size2Mib, 2 * 1024 * 1024),
    ];

    for (bits, size, size_bytes) in cases {
        let mode = boot_mode(field_bits(FIELD_SECONDARY_CACHE_SIZE_SHIFT, bits));

        assert_eq!(mode.secondary_cache_size_bits(), bits);
        assert_eq!(mode.secondary_cache_size(), size);
        assert_eq!(size.bits(), bits);
        assert_eq!(size.size_bytes(), size_bytes);
    }

    assert_eq!(
        R5000BootMode::from_low_bits(field_bits(FIELD_SECONDARY_CACHE_SIZE_SHIFT, 3)),
        Err(R5000BootModeError::ReservedFieldValue {
            field: R5000BootModeField::SecondaryCacheSize,
            value: 3,
        })
    );
}

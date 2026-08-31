use super::{Ip12Error, PROM_BYTES};

pub(super) fn normalize_u56_prom(mut bytes: Vec<u8>) -> Result<Vec<u8>, Ip12Error> {
    if bytes.len() != PROM_BYTES {
        return Err(Ip12Error::InvalidPromSize {
            expected: PROM_BYTES,
            actual: bytes.len(),
        });
    }

    for halfword in bytes.chunks_exact_mut(2) {
        halfword.swap(0, 1);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{PROM_BYTES, normalize_u56_prom};

    #[test]
    fn rejects_lengths_other_than_the_u56_image_size() {
        assert!(normalize_u56_prom(vec![0; PROM_BYTES - 1]).is_err());
        assert!(normalize_u56_prom(vec![0; PROM_BYTES + 1]).is_err());
    }

    #[test]
    fn swaps_each_halfword_and_is_an_involution() {
        let mut raw = vec![0; PROM_BYTES];
        raw[..6].copy_from_slice(&[0xf0, 0x0b, 0x80, 0x00, 0x12, 0x34]);

        let canonical = normalize_u56_prom(raw.clone()).unwrap();
        assert_eq!(&canonical[..6], &[0x0b, 0xf0, 0x00, 0x80, 0x34, 0x12]);
        assert_eq!(normalize_u56_prom(canonical).unwrap(), raw);
    }
}

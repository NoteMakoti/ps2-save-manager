//! Hamming-code ECC used in the spare area of PS2 memory card pages.
//!
//! Each 128 byte chunk of a page gets 3 bytes of ECC; a 512 byte page
//! therefore has 12 ECC bytes followed by 4 padding bytes in its 16 byte
//! spare area.

const fn parity(b: u8) -> u8 {
    (b.count_ones() & 1) as u8
}

const fn make_cp_masks() -> [u8; 256] {
    let cpmasks: [u8; 7] = [0x55, 0x33, 0x0F, 0x00, 0xAA, 0xCC, 0xF0];
    let mut table = [0u8; 256];
    let mut b = 0;
    while b < 256 {
        let mut mask = 0u8;
        let mut i = 0;
        while i < 7 {
            mask |= parity(b as u8 & cpmasks[i]) << i;
            i += 1;
        }
        table[b] = mask;
        b += 1;
    }
    table
}

static CP_MASKS: [u8; 256] = make_cp_masks();

/// ECC for a 128-byte chunk.
pub fn ecc_chunk(chunk: &[u8]) -> [u8; 3] {
    debug_assert_eq!(chunk.len(), 128);
    let mut cp: u8 = 0x77;
    let mut lp0: u8 = 0x7F;
    let mut lp1: u8 = 0x7F;
    for (i, &b) in chunk.iter().enumerate() {
        cp ^= CP_MASKS[b as usize];
        if parity(b) != 0 {
            lp0 ^= !(i as u8);
            lp1 ^= i as u8;
        }
    }
    [cp, lp0 & 0x7F, lp1]
}

/// Fill `spare` with ECC for `page` (any multiple of 128 bytes).
pub fn ecc_page(page: &[u8], spare: &mut [u8]) {
    spare.fill(0);
    for (n, chunk) in page.chunks(128).enumerate() {
        let e = ecc_chunk(chunk);
        if n * 3 + 3 <= spare.len() {
            spare[n * 3..n * 3 + 3].copy_from_slice(&e);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EccStatus {
    Ok,
    Corrected,
    Failed,
}

/// Check (and correct single bit errors in) a 128-byte chunk.
pub fn check_chunk(chunk: &mut [u8], ecc: &[u8]) -> EccStatus {
    let computed = ecc_chunk(chunk);
    if computed == [ecc[0], ecc[1], ecc[2]] {
        return EccStatus::Ok;
    }
    let cp_diff = (computed[0] ^ ecc[0]) & 0x77;
    let lp0_diff = (computed[1] ^ ecc[1]) & 0x7F;
    let lp1_diff = (computed[2] ^ ecc[2]) & 0x7F;
    let lp_comp = lp0_diff ^ lp1_diff;
    let cp_comp = (cp_diff >> 4) ^ (cp_diff & 0x07);
    if lp_comp == 0x7F && cp_comp == 0x07 {
        // single bit error in the data
        chunk[lp1_diff as usize] ^= 1 << (cp_diff >> 4);
        return EccStatus::Corrected;
    }
    if (cp_diff == 0 && lp0_diff == 0 && lp1_diff == 0)
        || lp_comp.count_ones() + cp_comp.count_ones() == 1
    {
        // single bit error in the ECC itself
        return EccStatus::Corrected;
    }
    EccStatus::Failed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_chunk() {
        // Known value: ECC of an all-zero chunk.
        assert_eq!(ecc_chunk(&[0u8; 128]), [0x77, 0x7F, 0x7F]);
    }
    #[test]
    fn corrects_single_bit() {
        let mut data: Vec<u8> = (0..128u32).map(|i| (i * 7 + 3) as u8).collect();
        let e = ecc_chunk(&data);
        data[37] ^= 0x10;
        assert_eq!(check_chunk(&mut data, &e), EccStatus::Corrected);
        assert_eq!(ecc_chunk(&data), e);
    }
}

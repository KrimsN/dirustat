//! Bounds-checked little-endian field readers. No indexing, no panics: every read returns `None` when short.

/// Returns `len` bytes at `offset`, or `None` when out of range or on overflow.
pub(crate) fn slice_at(bytes: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    bytes.get(offset..offset.checked_add(len)?)
}

pub(crate) fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(slice_at(bytes, offset, 2)?.try_into().ok()?))
}

pub(crate) fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(slice_at(bytes, offset, 4)?.try_into().ok()?))
}

pub(crate) fn le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(slice_at(bytes, offset, 8)?.try_into().ok()?))
}

/// Little-endian unsigned integer of 1..=8 bytes (longer input is truncated to its first 8 bytes).
pub(crate) fn le_uint(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    for (dst, src) in buf.iter_mut().zip(bytes) {
        *dst = *src;
    }
    u64::from_le_bytes(buf)
}

/// Little-endian two's complement integer of 1..=8 bytes, sign-extended.
pub(crate) fn le_int(bytes: &[u8]) -> i64 {
    let negative = bytes.last().is_some_and(|b| b & 0x80 != 0);
    let mut buf = if negative { [0xFFu8; 8] } else { [0u8; 8] };
    for (dst, src) in buf.iter_mut().zip(bytes) {
        *dst = *src;
    }
    i64::from_le_bytes(buf)
}

/// Decodes `units` UTF-16LE code units starting at `offset`.
pub(crate) fn utf16_units(bytes: &[u8], offset: usize, units: usize) -> Option<Vec<u16>> {
    let raw = slice_at(bytes, offset, units.checked_mul(2)?)?;
    Some(
        raw.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)]))
            .collect(),
    )
}

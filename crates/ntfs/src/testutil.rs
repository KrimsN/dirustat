//! Builders for synthetic MFT records, shared by the unit tests. They produce the real on-disk layout,
//! including the update sequence array, so parsing is tested against bytes shaped like the real thing.

use crate::runs::Run;

pub(crate) const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;

fn align8(n: usize) -> usize {
    n.div_ceil(8) * 8
}

pub(crate) fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn put_utf16(out: &mut Vec<u8>, units: &[u16]) {
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
}

/// Builds a resident attribute.
pub(crate) fn resident(ty: u32, name: &[u16], flags: u16, value: &[u8]) -> Vec<u8> {
    let name_off = 24usize;
    let value_off = align8(name_off + name.len() * 2);
    let total = align8(value_off + value.len());
    let mut a = vec![0u8; total];
    a[0..4].copy_from_slice(&ty.to_le_bytes());
    a[4..8].copy_from_slice(&u32::try_from(total).unwrap().to_le_bytes());
    a[8] = 0;
    a[9] = u8::try_from(name.len()).unwrap();
    a[10..12].copy_from_slice(&u16::try_from(name_off).unwrap().to_le_bytes());
    a[12..14].copy_from_slice(&flags.to_le_bytes());
    a[16..20].copy_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
    a[20..22].copy_from_slice(&u16::try_from(value_off).unwrap().to_le_bytes());
    let mut nm = Vec::new();
    put_utf16(&mut nm, name);
    a[name_off..name_off + nm.len()].copy_from_slice(&nm);
    a[value_off..value_off + value.len()].copy_from_slice(value);
    a
}

pub(crate) struct NonResident<'a> {
    pub ty: u32,
    pub name: &'a [u16],
    pub flags: u16,
    pub start_vcn: u64,
    pub allocated: u64,
    pub size: u64,
    pub initialized: u64,
    pub compression_unit: u16,
    pub compressed_size: Option<u64>,
    pub runs: &'a [u8],
}

impl Default for NonResident<'_> {
    fn default() -> Self {
        Self {
            ty: 0x80,
            name: &[],
            flags: 0,
            start_vcn: 0,
            allocated: 0,
            size: 0,
            initialized: 0,
            compression_unit: 0,
            compressed_size: None,
            runs: &[0],
        }
    }
}

pub(crate) fn nonresident(n: &NonResident<'_>) -> Vec<u8> {
    let header = if n.compressed_size.is_some() { 72 } else { 64 };
    let name_off = header;
    let runs_off = align8(name_off + n.name.len() * 2);
    let total = align8(runs_off + n.runs.len());
    let mut a = vec![0u8; total];
    a[0..4].copy_from_slice(&n.ty.to_le_bytes());
    a[4..8].copy_from_slice(&u32::try_from(total).unwrap().to_le_bytes());
    a[8] = 1;
    a[9] = u8::try_from(n.name.len()).unwrap();
    a[10..12].copy_from_slice(&u16::try_from(name_off).unwrap().to_le_bytes());
    a[12..14].copy_from_slice(&n.flags.to_le_bytes());
    a[16..24].copy_from_slice(&n.start_vcn.to_le_bytes());
    a[32..34].copy_from_slice(&u16::try_from(runs_off).unwrap().to_le_bytes());
    a[34..36].copy_from_slice(&n.compression_unit.to_le_bytes());
    a[40..48].copy_from_slice(&n.allocated.to_le_bytes());
    a[48..56].copy_from_slice(&n.size.to_le_bytes());
    a[56..64].copy_from_slice(&n.initialized.to_le_bytes());
    if let Some(c) = n.compressed_size {
        a[64..72].copy_from_slice(&c.to_le_bytes());
    }
    let mut nm = Vec::new();
    put_utf16(&mut nm, n.name);
    a[name_off..name_off + nm.len()].copy_from_slice(&nm);
    a[runs_off..runs_off + n.runs.len()].copy_from_slice(n.runs);
    a
}

pub(crate) fn standard_info(modified: u64, attributes: u32) -> Vec<u8> {
    let mut v = vec![0u8; 72];
    v[0..8].copy_from_slice(&(modified - 1).to_le_bytes());
    v[8..16].copy_from_slice(&modified.to_le_bytes());
    v[16..24].copy_from_slice(&(modified + 1).to_le_bytes());
    v[24..32].copy_from_slice(&(modified + 2).to_le_bytes());
    v[32..36].copy_from_slice(&attributes.to_le_bytes());
    resident(0x10, &[], 0, &v)
}

pub(crate) fn file_name(parent: u64, parent_seq: u16, namespace: u8, name: &[u16], real: u64, alloc: u64) -> Vec<u8> {
    let mut v = vec![0u8; 66];
    let pref = parent | (u64::from(parent_seq) << 48);
    v[0..8].copy_from_slice(&pref.to_le_bytes());
    v[40..48].copy_from_slice(&alloc.to_le_bytes());
    v[48..56].copy_from_slice(&real.to_le_bytes());
    v[56..60].copy_from_slice(&0x20u32.to_le_bytes());
    v[64] = u8::try_from(name.len()).unwrap();
    v[65] = namespace;
    put_utf16(&mut v, name);
    resident(0x30, &[], 0, &v)
}

/// One `$ATTRIBUTE_LIST` entry.
pub(crate) fn list_entry(ty: u32, named: bool, start_vcn: u64, segment: u64) -> Vec<u8> {
    let mut e = vec![0u8; 32];
    e[0..4].copy_from_slice(&ty.to_le_bytes());
    e[4..6].copy_from_slice(&32u16.to_le_bytes());
    e[6] = u8::from(named);
    e[8..16].copy_from_slice(&start_vcn.to_le_bytes());
    e[16..24].copy_from_slice(&segment.to_le_bytes());
    e
}

fn min_unsigned(v: u64) -> usize {
    (1..=8).find(|&n| n == 8 || v >> (8 * n) == 0).unwrap_or(8)
}

fn min_signed(v: i64) -> usize {
    (1..=8)
        .find(|&n| {
            if n == 8 {
                return true;
            }
            let shift = 64 - 8 * n;
            (v << shift) >> shift == v
        })
        .unwrap_or(8)
}

/// Encodes runs (relative offsets, minimal field widths) plus the `0` terminator.
pub(crate) fn encode_runs(runs: &[Run]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = 0i64;
    for run in runs {
        let ln = min_unsigned(run.len);
        match run.lcn {
            Some(lcn) => {
                let lcn = i64::try_from(lcn).unwrap();
                let delta = lcn - prev;
                prev = lcn;
                let on = min_signed(delta);
                out.push(u8::try_from(ln | (on << 4)).unwrap());
                out.extend_from_slice(&run.len.to_le_bytes()[..ln]);
                out.extend_from_slice(&delta.to_le_bytes()[..on]);
            }
            None => {
                out.push(u8::try_from(ln).unwrap());
                out.extend_from_slice(&run.len.to_le_bytes()[..ln]);
            }
        }
    }
    out.push(0);
    out
}

/// Assembles a full record (with fixups applied to the on-disk form).
pub(crate) struct RecordBuilder {
    pub record_size: usize,
    pub flags: u16,
    pub sequence: u16,
    pub links: u16,
    pub base: u64,
    pub number: Option<u32>,
    pub attrs: Vec<Vec<u8>>,
}

impl RecordBuilder {
    pub fn file() -> Self {
        Self { record_size: 1024, flags: 1, sequence: 7, links: 1, base: 0, number: Some(42), attrs: Vec::new() }
    }

    pub fn directory() -> Self {
        Self { flags: 3, ..Self::file() }
    }

    pub fn attr(mut self, a: Vec<u8>) -> Self {
        self.attrs.push(a);
        self
    }

    /// The record as stored on disk, fixups installed. `usn` is the update sequence number.
    pub fn build(&self) -> Vec<u8> {
        let mut r = vec![0u8; self.record_size];
        let usa_count = self.record_size / 512 + 1;
        let usa_off = 48usize;
        let first = align8(usa_off + usa_count * 2);
        r[0..4].copy_from_slice(b"FILE");
        r[4..6].copy_from_slice(&u16::try_from(usa_off).unwrap().to_le_bytes());
        r[6..8].copy_from_slice(&u16::try_from(usa_count).unwrap().to_le_bytes());
        r[16..18].copy_from_slice(&self.sequence.to_le_bytes());
        r[18..20].copy_from_slice(&self.links.to_le_bytes());
        r[20..22].copy_from_slice(&u16::try_from(first).unwrap().to_le_bytes());
        r[22..24].copy_from_slice(&self.flags.to_le_bytes());
        r[28..32].copy_from_slice(&u32::try_from(self.record_size).unwrap().to_le_bytes());
        r[32..40].copy_from_slice(&self.base.to_le_bytes());
        if let Some(n) = self.number {
            r[44..48].copy_from_slice(&n.to_le_bytes());
        }
        let mut pos = first;
        for a in &self.attrs {
            r[pos..pos + a.len()].copy_from_slice(a);
            pos += a.len();
        }
        r[pos..pos + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        pos += 8;
        r[24..28].copy_from_slice(&u32::try_from(pos).unwrap().to_le_bytes());
        install_fixups(&mut r, 512, 0xBEEF);
        r
    }
}

/// Inverse of `apply_fixups`: stores each block trailer in the array and writes the sequence number.
pub(crate) fn install_fixups(r: &mut [u8], stride: usize, usn: u16) {
    let usa_off = usize::from(u16::from_le_bytes([r[4], r[5]]));
    r[usa_off..usa_off + 2].copy_from_slice(&usn.to_le_bytes());
    for k in 1..=(r.len() / stride) {
        let t = k * stride - 2;
        let orig = [r[t], r[t + 1]];
        r[usa_off + 2 * k..usa_off + 2 * k + 2].copy_from_slice(&orig);
        r[t..t + 2].copy_from_slice(&usn.to_le_bytes());
    }
}

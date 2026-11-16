//! Construtores de registros FILE sintéticos para testes.

#![allow(dead_code)]

fn align8(n: usize) -> usize {
    n.div_ceil(8) * 8
}

pub struct Rec {
    size: usize,
    flags: u16,
    base: u64,
    attrs: Vec<Vec<u8>>,
}

impl Rec {
    pub fn new(size: usize, flags: u16) -> Self {
        Self {
            size,
            flags,
            base: 0,
            attrs: Vec::new(),
        }
    }
    pub fn base(mut self, rec: u64) -> Self {
        self.base = rec;
        self
    }
    pub fn attr(mut self, a: Vec<u8>) -> Self {
        self.attrs.push(a);
        self
    }

    /// Serializa com cabeçalho, USA (usn = 0xBEEF) e fixups aplicados (como no disco).
    pub fn build(self) -> Vec<u8> {
        let mut b = vec![0u8; self.size];
        let usa_count = self.size / 512 + 1;
        b[0..4].copy_from_slice(b"FILE");
        b[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        b[6..8].copy_from_slice(&(usa_count as u16).to_le_bytes());
        let first_attr = align8(0x30 + usa_count * 2);
        b[0x14..0x16].copy_from_slice(&(first_attr as u16).to_le_bytes());
        b[0x16..0x18].copy_from_slice(&self.flags.to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&(self.size as u32).to_le_bytes());
        b[0x20..0x28].copy_from_slice(&self.base.to_le_bytes());
        let mut off = first_attr;
        for a in &self.attrs {
            b[off..off + a.len()].copy_from_slice(a);
            off += a.len();
        }
        b[off..off + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        off += 8;
        b[0x18..0x1C].copy_from_slice(&(off as u32).to_le_bytes());
        // USA
        let usn = 0xBEEFu16.to_le_bytes();
        b[0x30..0x32].copy_from_slice(&usn);
        for i in 1..usa_count {
            let end = i * 512;
            let (x, y) = (b[end - 2], b[end - 1]);
            b[0x30 + i * 2] = x;
            b[0x30 + i * 2 + 1] = y;
            b[end - 2..end].copy_from_slice(&usn);
        }
        b
    }
}

fn header(ty: u32, len: usize, non_res: bool, name: &str, flags: u16) -> Vec<u8> {
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&ty.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = u8::from(non_res);
    a[9] = name.encode_utf16().count() as u8;
    a[12..14].copy_from_slice(&flags.to_le_bytes());
    a
}

fn write_name(a: &mut [u8], name: &str, off: usize) {
    a[10..12].copy_from_slice(&(off as u16).to_le_bytes());
    for (i, u) in name.encode_utf16().enumerate() {
        a[off + i * 2..off + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
}

pub fn resident_attr(ty: u32, name: &str, value: &[u8]) -> Vec<u8> {
    let name_bytes = name.encode_utf16().count() * 2;
    let val_off = align8(24 + name_bytes);
    let len = align8(val_off + value.len());
    let mut a = header(ty, len, false, name, 0);
    a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
    a[20..22].copy_from_slice(&(val_off as u16).to_le_bytes());
    write_name(&mut a, name, 24);
    a[val_off..val_off + value.len()].copy_from_slice(value);
    a
}

pub fn data_resident_attr(name: &str, value: &[u8]) -> Vec<u8> {
    resident_attr(0x80, name, value)
}

pub fn std_info_attr(mtime: i64, dos_attrs: u32) -> Vec<u8> {
    let mut v = vec![0u8; 72];
    v[8..16].copy_from_slice(&mtime.to_le_bytes());
    v[32..36].copy_from_slice(&dos_attrs.to_le_bytes());
    resident_attr(0x10, "", &v)
}

pub fn file_name_attr(parent: u64, name: &str, namespace: u8) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut v = vec![0u8; 66 + units.len() * 2];
    v[0..8].copy_from_slice(&(parent | (1 << 48)).to_le_bytes());
    v[64] = units.len() as u8;
    v[65] = namespace;
    for (i, u) in units.iter().enumerate() {
        v[66 + i * 2..68 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    resident_attr(0x30, "", &v)
}

pub fn reparse_attr() -> Vec<u8> {
    resident_attr(0xC0, "", &[0u8; 8])
}

/// `compressed`: valor do campo "compressed size" (só presente com unidade de compressão ≠ 0).
pub fn data_nonresident_attr(
    name: &str,
    start_vcn: u64,
    real: u64,
    allocated: u64,
    compressed: Option<u64>,
    flags: u16,
    runlist: &[u8],
) -> Vec<u8> {
    let name_bytes = name.encode_utf16().count() * 2;
    let hdr = if compressed.is_some() { 72 } else { 64 };
    let rl_off = align8(hdr + name_bytes);
    let len = align8(rl_off + runlist.len());
    let mut a = header(0x80, len, true, name, flags);
    a[16..24].copy_from_slice(&start_vcn.to_le_bytes());
    a[32..34].copy_from_slice(&(rl_off as u16).to_le_bytes());
    a[34..36].copy_from_slice(&(if compressed.is_some() { 4u16 } else { 0 }).to_le_bytes());
    a[40..48].copy_from_slice(&allocated.to_le_bytes());
    a[48..56].copy_from_slice(&real.to_le_bytes());
    a[56..64].copy_from_slice(&real.to_le_bytes());
    if let Some(c) = compressed {
        a[64..72].copy_from_slice(&c.to_le_bytes());
    }
    write_name(&mut a, name, hdr);
    a[rl_off..rl_off + runlist.len()].copy_from_slice(runlist);
    a
}

/// `$ATTRIBUTE_LIST` residente com entradas (tipo, vcn inicial, registro).
pub fn attribute_list_attr(entries: &[(u32, u64, u64)]) -> Vec<u8> {
    let mut v = Vec::new();
    for &(ty, vcn, rec) in entries {
        let mut e = vec![0u8; 32];
        e[0..4].copy_from_slice(&ty.to_le_bytes());
        e[4..6].copy_from_slice(&32u16.to_le_bytes());
        e[8..16].copy_from_slice(&vcn.to_le_bytes());
        e[16..24].copy_from_slice(&(rec | (1 << 48)).to_le_bytes());
        v.extend_from_slice(&e);
    }
    resident_attr(0x20, "", &v)
}

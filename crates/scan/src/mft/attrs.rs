//! Navegação e decodificação dos atributos de um registro FILE já com *fixups* aplicados.

pub const T_STANDARD_INFO: u32 = 0x10;
pub const T_ATTRIBUTE_LIST: u32 = 0x20;
pub const T_FILE_NAME: u32 = 0x30;
pub const T_DATA: u32 = 0x80;
pub const T_REPARSE_POINT: u32 = 0xC0;
const T_END: u32 = 0xFFFF_FFFF;

pub const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
pub const ATTR_FLAG_SPARSE: u16 = 0x8000;

pub fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
pub fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
pub fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Um atributo (cabeçalho + corpo) dentro do registro.
#[derive(Clone, Copy, Debug)]
pub struct Attr<'a> {
    pub ty: u32,
    pub non_resident: bool,
    pub name_len: u8,
    pub flags: u16,
    pub raw: &'a [u8],
}

/// Cabeçalho da parte não residente.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NonResident {
    pub start_vcn: u64,
    pub compression_unit: u16,
    pub allocated: u64,
    pub real_size: u64,
    pub initialized: u64,
    /// Só existe quando `compression_unit != 0`.
    pub compressed: Option<u64>,
}

impl<'a> Attr<'a> {
    /// Valor de um atributo residente.
    pub fn resident_value(&self) -> Option<&'a [u8]> {
        if self.non_resident {
            return None;
        }
        let len = u32_at(self.raw, 16)? as usize;
        let off = u16_at(self.raw, 20)? as usize;
        self.raw.get(off..off.checked_add(len)?)
    }

    pub fn non_resident(&self) -> Option<NonResident> {
        if !self.non_resident {
            return None;
        }
        let unit = u16_at(self.raw, 34)?;
        Some(NonResident {
            start_vcn: u64_at(self.raw, 16)?,
            compression_unit: unit,
            allocated: u64_at(self.raw, 40)?,
            real_size: u64_at(self.raw, 48)?,
            initialized: u64_at(self.raw, 56)?,
            compressed: if unit != 0 {
                u64_at(self.raw, 64)
            } else {
                None
            },
        })
    }

    /// Bytes da run list (atributo não residente).
    pub fn runlist(&self) -> Option<&'a [u8]> {
        if !self.non_resident {
            return None;
        }
        let off = u16_at(self.raw, 32)? as usize;
        self.raw.get(off..)
    }

    /// Nome do stream (vazio para o stream padrão), em UTF-16.
    pub fn name_utf16(&self) -> Vec<u16> {
        let off = u16_at(self.raw, 10).unwrap_or(0) as usize;
        let n = self.name_len as usize * 2;
        self.raw
            .get(off..off + n)
            .map(|b| {
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub struct Attrs<'a> {
    rec: &'a [u8],
    off: usize,
    end: usize,
}

/// Percorre os atributos de um registro (já validado por [`super::record::apply_fixups`]).
pub fn attrs(rec: &[u8]) -> Attrs<'_> {
    let first = u16_at(rec, 0x14).unwrap_or(0) as usize;
    let used = (u32_at(rec, 0x18).unwrap_or(0) as usize).min(rec.len());
    Attrs {
        rec,
        off: first,
        end: used,
    }
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Attr<'a>;
    fn next(&mut self) -> Option<Attr<'a>> {
        if self.off + 16 > self.end {
            return None;
        }
        let ty = u32_at(self.rec, self.off)?;
        if ty == T_END {
            return None;
        }
        let len = u32_at(self.rec, self.off + 4)? as usize;
        if len < 16 || !len.is_multiple_of(8) || self.off + len > self.end {
            return None;
        }
        let raw = &self.rec[self.off..self.off + len];
        self.off += len;
        Some(Attr {
            ty,
            non_resident: raw[8] != 0,
            name_len: raw[9],
            flags: u16_at(raw, 12)?,
            raw,
        })
    }
}

/// `$STANDARD_INFORMATION`: (data de modificação, atributos de arquivo do DOS).
pub fn parse_standard_info(v: &[u8]) -> Option<(i64, u32)> {
    Some((u64_at(v, 8)? as i64, u32_at(v, 32)?))
}

pub struct FileName<'a> {
    /// Número do registro do pai (48 bits baixos da referência).
    pub parent: u64,
    pub mtime: i64,
    pub attrs: u32,
    /// 0 POSIX, 1 Win32, 2 DOS, 3 Win32+DOS.
    pub namespace: u8,
    name_bytes: &'a [u8],
}

impl FileName<'_> {
    pub fn name_units(&self) -> impl Iterator<Item = u16> + '_ {
        self.name_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
    }
}

pub fn parse_file_name(v: &[u8]) -> Option<FileName<'_>> {
    let name_len = *v.get(64)? as usize;
    let namespace = *v.get(65)?;
    let name_bytes = v.get(66..66 + name_len * 2)?;
    Some(FileName {
        parent: u64_at(v, 0)? & 0x0000_FFFF_FFFF_FFFF,
        mtime: u64_at(v, 16)? as i64,
        attrs: u32_at(v, 56)?,
        namespace,
        name_bytes,
    })
}

/// Entrada de `$ATTRIBUTE_LIST`.
#[derive(Clone, Copy, Debug)]
pub struct ListEntry {
    pub ty: u32,
    pub name_len: u8,
    pub start_vcn: u64,
    pub record: u64,
}

pub fn parse_attribute_list(v: &[u8]) -> Vec<ListEntry> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 26 <= v.len() {
        let Some(len) = u16_at(v, off + 4).map(usize::from) else {
            break;
        };
        if len < 26 {
            break;
        }
        if let (Some(ty), Some(start_vcn), Some(fref)) =
            (u32_at(v, off), u64_at(v, off + 8), u64_at(v, off + 16))
        {
            out.push(ListEntry {
                ty,
                name_len: v[off + 6],
                start_vcn,
                record: fref & 0x0000_FFFF_FFFF_FFFF,
            });
        }
        off += len;
    }
    out
}

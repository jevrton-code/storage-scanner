//! Validação (assinatura + *fixups*) e interpretação de um registro FILE da MFT.

use ss_core::flags;

use super::attrs::{
    ATTR_FLAG_COMPRESSED, ATTR_FLAG_SPARSE, T_DATA, T_FILE_NAME, T_REPARSE_POINT, T_STANDARD_INFO,
    attrs, parse_file_name, parse_standard_info, u16_at, u64_at,
};

pub const NO_BASE: u32 = u32::MAX;
const SECTOR: usize = 512;
const REF_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// Atributos do DOS em $STANDARD_INFORMATION.
const DOS_HIDDEN: u32 = 0x2;
const DOS_SYSTEM: u32 = 0x4;
const DOS_SPARSE: u32 = 0x200;
const DOS_REPARSE: u32 = 0x400;
const DOS_COMPRESSED: u32 = 0x800;

/// Nome (hard link) de um arquivo: referência ao pai + posição na arena de nomes do *chunk*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkName {
    pub parent: u32,
    pub off: u32,
    pub len: u16,
}

/// Informações extraídas de um registro (base ou de extensão).
#[derive(Clone, Debug)]
pub struct Parsed {
    pub rec: u32,
    /// `NO_BASE` para registros base; senão, o nº do registro base.
    pub base: u32,
    /// Índice do chunk cuja arena de nomes contém `name`/`extra`.
    pub chunk: u32,
    pub is_dir: bool,
    pub flags: u16,
    pub mtime: i64,
    pub name: Option<LinkName>,
    /// Demais hard links.
    pub extra: Vec<LinkName>,
    pub has_data: bool,
    pub size: u64,
    pub alloc: u64,
    /// Alocado dos streams nomeados (ADS, `WofCompressedData`…).
    pub ads_alloc: u64,
}

/// Aplica a *update sequence array*. `false` se a assinatura/fixups forem inválidos.
pub fn apply_fixups(rec: &mut [u8]) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let (Some(usa_off), Some(usa_count)) = (u16_at(rec, 4), u16_at(rec, 6)) else {
        return false;
    };
    let (usa_off, usa_count) = (usa_off as usize, usa_count as usize);
    if usa_count < 1 || usa_off + usa_count * 2 > rec.len() || (usa_count - 1) * SECTOR > rec.len()
    {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..usa_count {
        let end = i * SECTOR;
        if end > rec.len() || rec[end - 2..end] != usn {
            return false;
        }
        let src = usa_off + i * 2;
        let (a, b) = (rec[src], rec[src + 1]);
        rec[end - 2] = a;
        rec[end - 1] = b;
    }
    true
}

fn push_utf16(units: impl Iterator<Item = u16>, names: &mut Vec<u8>) -> u16 {
    let start = names.len();
    let mut buf = [0u8; 4];
    for c in char::decode_utf16(units) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        names.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    (names.len() - start).min(u16::MAX as usize) as u16
}

/// Interpreta o registro `rec_no`. `buf` é modificado (fixups). Registros livres, inválidos ou
/// sem conteúdo útil devolvem `None`.
pub fn parse_record(
    rec_no: u32,
    buf: &mut [u8],
    chunk: u32,
    names: &mut Vec<u8>,
) -> Option<Parsed> {
    if buf.len() < 48 || &buf[0..4] != b"FILE" {
        return None; // zerado ou "BAAD"
    }
    let rflags = u16_at(buf, 0x16)?;
    if rflags & 0x01 == 0 || !apply_fixups(buf) {
        return None;
    }
    let base_ref = u64_at(buf, 0x20)? & REF_MASK;
    let mut p = Parsed {
        rec: rec_no,
        base: if base_ref == 0 {
            NO_BASE
        } else {
            base_ref as u32
        },
        chunk,
        is_dir: rflags & 0x02 != 0,
        flags: if rflags & 0x02 != 0 { flags::DIR } else { 0 },
        mtime: 0,
        name: None,
        extra: Vec::new(),
        has_data: false,
        size: 0,
        alloc: 0,
        ads_alloc: 0,
    };

    // (pai, namespace, nome) dos $FILE_NAME encontrados.
    let mut file_names: Vec<(u32, u8, Vec<u16>, i64)> = Vec::new();
    for a in attrs(buf) {
        match a.ty {
            T_STANDARD_INFO => {
                if let Some((m, dos)) = a.resident_value().and_then(parse_standard_info) {
                    p.mtime = m;
                    for (bit, f) in [
                        (DOS_HIDDEN, flags::HIDDEN),
                        (DOS_SYSTEM, flags::SYSTEM),
                        (DOS_SPARSE, flags::SPARSE),
                        (DOS_REPARSE, flags::REPARSE),
                        (DOS_COMPRESSED, flags::COMPRESSED),
                    ] {
                        if dos & bit != 0 {
                            p.flags |= f;
                        }
                    }
                }
            }
            T_FILE_NAME => {
                if let Some(fnm) = a.resident_value().and_then(parse_file_name) {
                    file_names.push((
                        fnm.parent as u32,
                        fnm.namespace,
                        fnm.name_units().collect(),
                        fnm.mtime,
                    ));
                }
            }
            T_REPARSE_POINT => p.flags |= flags::REPARSE,
            T_DATA => {
                let (size, alloc) = if a.non_resident {
                    let nr = a.non_resident()?;
                    if nr.start_vcn != 0 {
                        continue; // trecho subsequente: não carrega tamanhos
                    }
                    let packed = a.flags & (ATTR_FLAG_COMPRESSED | ATTR_FLAG_SPARSE) != 0;
                    let alloc = match (packed, nr.compressed) {
                        (true, Some(c)) => c,
                        _ => nr.allocated,
                    };
                    (nr.real_size, alloc)
                } else {
                    (a.resident_value().map_or(0, |v| v.len() as u64), 0)
                };
                if a.name_len == 0 {
                    p.has_data = true;
                    p.size = size;
                    p.alloc = alloc;
                    if a.flags & ATTR_FLAG_COMPRESSED != 0 {
                        p.flags |= flags::COMPRESSED;
                    }
                    if a.flags & ATTR_FLAG_SPARSE != 0 {
                        p.flags |= flags::SPARSE;
                    }
                } else {
                    p.ads_alloc += alloc;
                }
            }
            _ => {}
        }
    }

    // Escolha dos nomes: ignora DOS (8.3) se houver outro; Win32 antes de POSIX; sem duplicatas.
    let has_real = file_names.iter().any(|f| f.1 != 2);
    let mut chosen: Vec<LinkName> = Vec::new();
    let mut order: Vec<&(u32, u8, Vec<u16>, i64)> = file_names
        .iter()
        .filter(|f| !has_real || f.1 != 2)
        .collect();
    order.sort_by_key(|f| u8::from(!(f.1 == 1 || f.1 == 3)));
    for (parent, _, units, fm) in order {
        let off = names.len() as u32;
        let len = push_utf16(units.iter().copied(), names);
        let dup = chosen.iter().any(|c| {
            c.parent == *parent
                && c.len == len
                && names[c.off as usize..c.off as usize + len as usize]
                    == names[off as usize..off as usize + len as usize]
        });
        if dup {
            names.truncate(off as usize);
            continue;
        }
        if chosen.is_empty() && p.mtime == 0 {
            p.mtime = *fm;
        }
        chosen.push(LinkName {
            parent: *parent,
            off,
            len,
        });
    }
    let mut it = chosen.into_iter();
    p.name = it.next();
    p.extra = it.collect();
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::super::fixture::*;
    use super::*;

    fn parse(buf: &mut [u8]) -> (Option<Parsed>, Vec<u8>) {
        let mut names = Vec::new();
        let p = parse_record(30, buf, 0, &mut names);
        (p, names)
    }

    fn name_of(p: &Parsed, names: &[u8]) -> String {
        let n = p.name.unwrap();
        String::from_utf8(names[n.off as usize..n.off as usize + n.len as usize].to_vec()).unwrap()
    }

    #[test]
    fn fixups_roundtrip_and_detect_corruption() {
        let mut r = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "a.txt", 1))
            .build();
        assert!(apply_fixups(&mut r.clone()));
        r[510] ^= 0xFF; // sequência quebrada no fim do 1º setor
        assert!(!apply_fixups(&mut r));
    }

    #[test]
    fn plain_file_with_resident_and_nonresident_data() {
        let mut r = Rec::new(1024, 0x01)
            .attr(std_info_attr(1234, 0))
            .attr(file_name_attr(5, "docs.txt", 3))
            .attr(data_nonresident_attr(
                "",
                0,
                100_000,
                102_400,
                None,
                0,
                &[0x11, 0x19, 0x40, 0],
            ))
            .build();
        let (p, names) = parse(&mut r);
        let p = p.unwrap();
        assert_eq!(name_of(&p, &names), "docs.txt");
        assert_eq!(p.name.unwrap().parent, 5);
        assert_eq!((p.size, p.alloc), (100_000, 102_400));
        assert_eq!(p.mtime, 1234);
        assert!(!p.is_dir && p.has_data && p.base == NO_BASE);

        let mut r2 = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "tiny", 1))
            .attr(data_resident_attr("", b"hello"))
            .build();
        let (p2, _) = parse(&mut r2);
        let p2 = p2.unwrap();
        assert_eq!((p2.size, p2.alloc), (5, 0));
    }

    #[test]
    fn dos_name_ignored_and_win32_preferred() {
        let mut r = Rec::new(1024, 0x03)
            .attr(file_name_attr(5, "PROGRA~1", 2))
            .attr(file_name_attr(5, "Program Files", 1))
            .build();
        let (p, names) = parse(&mut r);
        let p = p.unwrap();
        assert!(p.is_dir);
        assert_eq!(name_of(&p, &names), "Program Files");
        assert!(p.extra.is_empty());
    }

    #[test]
    fn hard_links_become_extra_names() {
        let mut r = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "a.bin", 1))
            .attr(file_name_attr(40, "b.bin", 1))
            .attr(data_resident_attr("", b"x"))
            .build();
        let (p, _) = parse(&mut r);
        let p = p.unwrap();
        assert_eq!(p.extra.len(), 1);
        assert_eq!(p.extra[0].parent, 40);
    }

    #[test]
    fn compressed_and_ads_and_reparse() {
        // Comprimido: tamanho 1 MB, alocado de verdade 256 KB (campo "compressed size").
        let mut r = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "c.dat", 1))
            .attr(data_nonresident_attr(
                "",
                0,
                1_048_576,
                1_048_576,
                Some(262_144),
                ATTR_FLAG_COMPRESSED,
                &[0x11, 0x40, 0x20, 0],
            ))
            .attr(data_nonresident_attr(
                "WofCompressedData",
                0,
                500,
                8192,
                None,
                0,
                &[0x11, 0x02, 0x30, 0],
            ))
            .attr(reparse_attr())
            .build();
        let (p, _) = parse(&mut r);
        let p = p.unwrap();
        assert_eq!((p.size, p.alloc, p.ads_alloc), (1_048_576, 262_144, 8192));
        assert!(p.flags & flags::COMPRESSED != 0);
        assert!(p.flags & flags::REPARSE != 0);
    }

    #[test]
    fn extension_record_carries_base_reference() {
        let mut r = Rec::new(1024, 0x01)
            .base(77)
            .attr(data_nonresident_attr(
                "",
                0,
                9000,
                12288,
                None,
                0,
                &[0x11, 0x03, 0x50, 0],
            ))
            .build();
        let (p, _) = parse(&mut r);
        let p = p.unwrap();
        assert_eq!(p.base, 77);
        assert!(p.has_data && p.name.is_none());
    }

    #[test]
    fn continuation_extent_has_no_sizes_and_free_records_skipped() {
        let mut r = Rec::new(1024, 0x01)
            .base(77)
            .attr(data_nonresident_attr(
                "",
                16,
                0,
                0,
                None,
                0,
                &[0x11, 0x03, 0x50, 0],
            ))
            .build();
        let (p, _) = parse(&mut r);
        assert!(!p.unwrap().has_data);

        let mut free = Rec::new(1024, 0x00)
            .attr(file_name_attr(5, "gone", 1))
            .build();
        assert!(parse(&mut free).0.is_none());
        let mut zeros = vec![0u8; 1024];
        assert!(parse(&mut zeros).0.is_none());
    }
}

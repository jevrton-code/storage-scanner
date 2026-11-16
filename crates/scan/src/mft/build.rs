//! Montagem da [`Tree`] a partir dos registros interpretados da MFT.

use ss_core::{NONE, NodeInfo, Tree, TreeBuilder, flags};

use super::record::{LinkName, NO_BASE, Parsed};

/// Rótulos dos nós sintéticos (fornecidos pela camada de interface/i18n).
#[derive(Clone, Debug)]
pub struct Labels {
    pub system_files: String,
    pub orphans: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            system_files: "[Arquivos de sistema NTFS]".into(),
            orphans: "[Órfãos]".into(),
        }
    }
}

const ROOT_REC: u32 = 5;
/// Registros 0..24 são metarquivos ($MFT, $LogFile…), exceto a raiz (5).
const FIRST_USER_REC: u32 = 24;
const BAD_CLUS_REC: u32 = 8;
// Pais sintéticos.
const P_SYSTEM: u32 = u32::MAX - 1;
const P_ORPHAN: u32 = u32::MAX - 2;

/// Resultado do parse de um chunk, com sua arena de nomes.
pub struct ChunkOut {
    pub idx: u32,
    pub names: Vec<u8>,
    pub records: Vec<Parsed>,
}

fn link_name<'a>(names: &'a [Vec<u8>], chunk: u32, l: &LinkName) -> &'a str {
    let arena = &names[chunk as usize];
    std::str::from_utf8(&arena[l.off as usize..l.off as usize + l.len as usize]).unwrap_or("?")
}

pub fn build_tree(
    chunks: Vec<ChunkOut>,
    total_records: usize,
    root_name: &str,
    labels: &Labels,
) -> Result<Tree, &'static str> {
    // Arenas de nomes por índice de chunk.
    let n_chunks = chunks.iter().map(|c| c.idx as usize + 1).max().unwrap_or(0);
    let mut names: Vec<Vec<u8>> = vec![Vec::new(); n_chunks];
    let mut table: Vec<Option<Parsed>> = vec![None; total_records];
    let mut exts: Vec<Parsed> = Vec::new();
    for c in chunks {
        names[c.idx as usize] = c.names;
        for p in c.records {
            if p.base == NO_BASE {
                let r = p.rec as usize;
                if r < table.len() {
                    table[r] = Some(p);
                }
            } else {
                exts.push(p);
            }
        }
    }

    // Registros de extensão são fundidos ao registro base.
    for e in exts {
        let Some(Some(b)) = table.get_mut(e.base as usize) else {
            continue;
        };
        if e.has_data && !b.has_data {
            b.has_data = true;
            b.size = e.size;
            b.alloc = e.alloc;
        }
        b.ads_alloc += e.ads_alloc;
        b.flags |= e.flags & !flags::DIR;
        if let Some(n) = e.name {
            let mut all = Vec::new();
            all.push(n);
            all.extend(e.extra.iter().copied());
            for l in all {
                // Os nomes do registro de extensão vivem na arena do *seu* chunk; copiamos.
                let s = link_name(&names, e.chunk, &l).to_string();
                let arena = &mut names[b.chunk as usize];
                let off = arena.len() as u32;
                arena.extend_from_slice(s.as_bytes());
                let ln = LinkName {
                    parent: l.parent,
                    off,
                    len: s.len().min(u16::MAX as usize) as u16,
                };
                if b.name.is_none() {
                    b.name = Some(ln);
                } else {
                    b.extra.push(ln);
                }
            }
        }
    }

    let valid_dir = |t: &Vec<Option<Parsed>>, r: u32| -> bool {
        t.get(r as usize)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.is_dir && p.name.is_some())
    };
    if !valid_dir(&table, ROOT_REC) {
        return Err("registro raiz (5) ausente");
    }

    // Pai de cada registro (em número de registro ou sentinela).
    let mut parent: Vec<u32> = vec![NONE; total_records];
    let mut any_orphan = false;
    for r in 0..total_records as u32 {
        let Some(p) = table[r as usize].as_ref() else {
            continue;
        };
        let Some(n) = p.name else { continue };
        if r == ROOT_REC {
            continue;
        }
        parent[r as usize] = if r < FIRST_USER_REC {
            P_SYSTEM
        } else if valid_dir(&table, n.parent) && n.parent != r {
            n.parent
        } else {
            any_orphan = true;
            P_ORPHAN
        };
    }
    // Detecta ciclos / cadeias que não chegam à raiz: o ponto de quebra vira órfão.
    let mut state = vec![0u8; total_records]; // 0 = novo, 1 = ok, 2 = visitando
    state[ROOT_REC as usize] = 1;
    let mut path: Vec<u32> = Vec::new();
    for start in 0..total_records as u32 {
        if parent[start as usize] == NONE || state[start as usize] != 0 {
            continue;
        }
        path.clear();
        let mut cur = start;
        loop {
            match state[cur as usize] {
                1 => break,
                2 => {
                    parent[cur as usize] = P_ORPHAN;
                    any_orphan = true;
                    break;
                }
                _ => {}
            }
            state[cur as usize] = 2;
            path.push(cur);
            let p = parent[cur as usize];
            if p == P_SYSTEM || p == P_ORPHAN || p == ROOT_REC {
                break;
            }
            cur = p;
        }
        for &p in &path {
            state[p as usize] = 1;
        }
    }

    // Atribuição de ids: 0 raiz, 1 sistema, 2 órfãos (se houver), depois os registros.
    let system_id = 1u32;
    let orphan_id = 2u32;
    let mut next = if any_orphan { 3u32 } else { 2u32 };
    let mut rec_id: Vec<u32> = vec![NONE; total_records];
    rec_id[ROOT_REC as usize] = 0;
    for r in 0..total_records {
        if r as u32 != ROOT_REC && parent[r] != NONE {
            rec_id[r] = next;
            next += 1;
        }
    }
    let id_of_parent = |p: u32| match p {
        P_SYSTEM => system_id,
        P_ORPHAN => orphan_id,
        r => rec_id[r as usize],
    };
    // Hard links extras com pai válido.
    let mut link_count = 0u32;
    for r in 0..total_records {
        if let Some(p) = table[r]
            .as_ref()
            .filter(|_| parent[r] != NONE || r as u32 == ROOT_REC)
        {
            link_count += p
                .extra
                .iter()
                .filter(|l| valid_dir(&table, l.parent) && rec_id[l.parent as usize] != NONE)
                .count() as u32;
        }
    }

    let mut b = TreeBuilder::new();
    b.resize((next + link_count) as usize);
    let dir = NodeInfo {
        flags: flags::DIR,
        ..Default::default()
    };
    b.put(
        0,
        NONE,
        root_name,
        NodeInfo {
            mtime: table[ROOT_REC as usize].as_ref().map_or(0, |p| p.mtime),
            ..dir
        },
    );
    b.put(system_id, 0, &labels.system_files, dir);
    if any_orphan {
        b.put(orphan_id, 0, &labels.orphans, dir);
    }
    let mut link_next = next;
    for r in 0..total_records {
        let Some(p) = table[r].as_ref() else { continue };
        if r as u32 == ROOT_REC || parent[r] == NONE {
            continue;
        }
        let id = rec_id[r];
        let n = p.name.expect("registro com pai tem nome");
        let allocated = p.alloc + p.ads_alloc;
        // $BadClus é esparso e declara o volume inteiro como tamanho lógico.
        let size = if r as u32 == BAD_CLUS_REC {
            allocated
        } else {
            p.size
        };
        let info = NodeInfo {
            flags: p.flags,
            size,
            allocated,
            mtime: p.mtime,
        };
        b.put(
            id,
            id_of_parent(parent[r]),
            link_name(&names, p.chunk, &n),
            info,
        );
        for l in &p.extra {
            if valid_dir(&table, l.parent) && rec_id[l.parent as usize] != NONE {
                b.put(
                    link_next,
                    rec_id[l.parent as usize],
                    link_name(&names, p.chunk, l),
                    NodeInfo {
                        flags: (p.flags & !flags::DIR) | flags::HARDLINK,
                        size: 0,
                        allocated: 0,
                        mtime: p.mtime,
                    },
                );
                link_next += 1;
            }
        }
    }
    Ok(b.finish())
}

#[cfg(test)]
mod tests {
    use super::super::fixture::*;
    use super::super::record::parse_record;
    use super::*;

    /// Constrói o resultado de um chunk único a partir de registros (nº, bytes).
    fn chunk(recs: Vec<(u32, Vec<u8>)>) -> (Vec<ChunkOut>, usize) {
        let mut names = Vec::new();
        let mut out = Vec::new();
        let mut max = 0;
        for (n, mut bytes) in recs {
            max = max.max(n as usize + 1);
            if let Some(p) = parse_record(n, &mut bytes, 0, &mut names) {
                out.push(p);
            }
        }
        (
            vec![ChunkOut {
                idx: 0,
                names,
                records: out,
            }],
            max,
        )
    }

    fn dir_rec(parent: u64, name: &str) -> Vec<u8> {
        Rec::new(1024, 0x03)
            .attr(std_info_attr(10, 0))
            .attr(file_name_attr(parent, name, 1))
            .build()
    }
    fn file_rec(parent: u64, name: &str, size: u64, alloc: u64) -> Vec<u8> {
        Rec::new(1024, 0x01)
            .attr(std_info_attr(20, 0))
            .attr(file_name_attr(parent, name, 1))
            .attr(data_nonresident_attr(
                "",
                0,
                size,
                alloc,
                None,
                0,
                &[0x11, 0x01, 0x10, 0],
            ))
            .build()
    }

    #[test]
    fn builds_hierarchy_system_group_and_orphans() {
        let (chunks, total) = chunk(vec![
            (0, file_rec(5, "$MFT", 1_000_000, 1_000_448)),
            (5, dir_rec(5, ".")),
            (30, dir_rec(5, "Users")),
            (31, file_rec(30, "a.bin", 1000, 4096)),
            (32, dir_rec(30, "sub")),
            (33, file_rec(32, "b.bin", 50, 4096)),
            (34, file_rec(99, "lost.bin", 7, 4096)), // pai inexistente
        ]);
        let t = build_tree(chunks, total, "C:\\", &Labels::default()).unwrap();
        let root = t.node(0);
        assert_eq!(root.files, 4);
        assert_eq!(root.size, 1_000_000 + 1000 + 50 + 7);
        let top: Vec<&str> = t.children(0).iter().map(|&c| t.name(c)).collect();
        assert!(top.contains(&"Users"));
        assert!(top.contains(&"[Arquivos de sistema NTFS]"));
        assert!(top.contains(&"[Órfãos]"));
        let users = t.find_child_path(0, &["users"]).unwrap();
        assert_eq!(t.node(users).size, 1050);
        assert_eq!(
            t.path(t.find_child_path(0, &["Users", "sub", "b.bin"]).unwrap()),
            "C:\\Users\\sub\\b.bin"
        );
        let sys = t
            .find_child_path(0, &["[Arquivos de sistema NTFS]", "$MFT"])
            .unwrap();
        assert_eq!(t.node(sys).size, 1_000_000);
    }

    #[test]
    fn no_orphan_node_when_none() {
        let (chunks, total) = chunk(vec![(5, dir_rec(5, ".")), (30, file_rec(5, "x", 1, 1))]);
        let t = build_tree(chunks, total, "D:\\", &Labels::default()).unwrap();
        let top: Vec<&str> = t.children(0).iter().map(|&c| t.name(c)).collect();
        assert!(!top.contains(&"[Órfãos]"));
        assert_eq!(t.len(), 3); // raiz, sistema e x
    }

    #[test]
    fn cycles_are_broken() {
        let (chunks, total) = chunk(vec![
            (5, dir_rec(5, ".")),
            (30, dir_rec(31, "a")),
            (31, dir_rec(30, "b")),
            (32, file_rec(30, "f", 9, 9)),
        ]);
        let t = build_tree(chunks, total, "C:\\", &Labels::default()).unwrap();
        // Tudo alcançável a partir da raiz: tamanho agregado inclui o arquivo.
        assert_eq!(t.node(0).size, 9);
        assert!(t.find_child_path(0, &["[Órfãos]"]).is_some());
    }

    #[test]
    fn hard_links_count_once_and_extension_records_merge() {
        let hl = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "one", 1))
            .attr(file_name_attr(30, "two", 1))
            .attr(data_nonresident_attr(
                "",
                0,
                500,
                4096,
                None,
                0,
                &[0x11, 0x01, 0x10, 0],
            ))
            .build();
        // Registro 41: dados do arquivo 40 ficaram no registro de extensão.
        let base = Rec::new(1024, 0x01)
            .attr(file_name_attr(5, "big.dat", 1))
            .attr(attribute_list_attr(&[(0x80, 0, 41)]))
            .build();
        let ext = Rec::new(1024, 0x01)
            .base(40)
            .attr(data_nonresident_attr(
                "",
                0,
                70_000,
                73_728,
                None,
                0,
                &[0x11, 0x12, 0x10, 0],
            ))
            .build();
        let (chunks, total) = chunk(vec![
            (5, dir_rec(5, ".")),
            (30, dir_rec(5, "d")),
            (35, hl),
            (40, base),
            (41, ext),
        ]);
        let t = build_tree(chunks, total, "C:\\", &Labels::default()).unwrap();
        assert_eq!(t.node(0).size, 500 + 70_000);
        assert_eq!(t.node(0).files, 3); // one, two (hard link) e big.dat
        let two = t.find_child_path(0, &["d", "two"]).unwrap();
        assert!(t.node(two).has(flags::HARDLINK));
        assert_eq!(t.node(two).size, 0);
    }
}

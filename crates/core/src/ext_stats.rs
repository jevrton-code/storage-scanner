//! Extensões de arquivo e estatísticas por tipo.

use std::collections::HashMap;

use crate::metrics::Metric;
use crate::tree::{Tree, flags};

/// Totais de um tipo de arquivo (extensão em minúsculas; vazio = sem extensão).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtStat {
    pub ext: String,
    pub size: u64,
    pub allocated: u64,
    pub count: u64,
}

impl ExtStat {
    /// Valor usado para ordenar/proporcionar conforme a métrica ativa.
    pub fn key(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Size | Metric::Modified => self.size,
            Metric::Allocated => self.allocated,
            Metric::FileCount => self.count,
            Metric::Compression => self.size.saturating_sub(self.allocated),
        }
    }
}

/// Soma por extensão de todos os arquivos da árvore (hard links extras não são recontados).
pub fn compute(tree: &Tree) -> Vec<ExtStat> {
    let mut map: HashMap<String, ExtStat> = HashMap::new();
    for id in 0..tree.len() as u32 {
        let n = tree.node(id);
        if n.is_dir() || n.has(flags::HARDLINK) {
            continue;
        }
        let ext = extension_of(tree.name(id));
        let e = map.entry(ext.clone()).or_insert_with(|| ExtStat {
            ext,
            size: 0,
            allocated: 0,
            count: 0,
        });
        e.size += n.size;
        e.allocated += n.allocated;
        e.count += 1;
    }
    let mut v: Vec<ExtStat> = map.into_values().collect();
    sort_stats(&mut v, Metric::Size);
    v
}

/// Ordena do maior para o menor pela métrica (desempate pela extensão).
pub fn sort_stats(stats: &mut [ExtStat], metric: Metric) {
    stats.sort_by(|a, b| {
        b.key(metric)
            .cmp(&a.key(metric))
            .then_with(|| a.ext.cmp(&b.ext))
    });
}

/// Extensão em minúsculas sem o ponto; vazio se não houver.
/// Arquivos que começam com "." (ex.: `.gitignore`) não têm extensão.
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_by_extension() {
        use crate::tree::{NONE, NodeInfo, TreeBuilder};
        let mut b = TreeBuilder::new();
        let dir = NodeInfo {
            flags: flags::DIR,
            ..Default::default()
        };
        let root = b.push(NONE, "R", dir);
        let sub = b.push(root, "s", dir);
        let f = |size, alloc| NodeInfo {
            size,
            allocated: alloc,
            ..Default::default()
        };
        b.push(root, "a.MP4", f(100, 4096));
        b.push(sub, "b.mp4", f(50, 4096));
        b.push(sub, "c.txt", f(10, 4096));
        b.push(sub, "noext", f(1, 4096));
        b.push(
            sub,
            "link.mp4",
            NodeInfo {
                flags: flags::HARDLINK,
                ..Default::default()
            },
        );
        let stats = compute(&b.finish());
        assert_eq!(stats[0].ext, "mp4");
        assert_eq!(
            (stats[0].size, stats[0].allocated, stats[0].count),
            (150, 8192, 2)
        );
        assert_eq!(stats.len(), 3);
        let mut by_count = stats.clone();
        sort_stats(&mut by_count, Metric::FileCount);
        assert_eq!(by_count[0].ext, "mp4");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_of("a.TXT"), "txt");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("noext"), "");
        assert_eq!(extension_of("trailing."), "");
    }
}

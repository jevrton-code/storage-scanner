//! Ordenação sob demanda dos filhos de um nó.

use crate::metrics::Metric;
use crate::tree::{NodeId, Tree};

/// Filhos de `id` do maior para o menor pela métrica (desempate por nome).
pub fn sorted_children(tree: &Tree, id: NodeId, metric: Metric) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = tree.children(id).to_vec();
    v.sort_by(|&a, &b| {
        let (ka, kb) = (metric.key(tree.node(a)), metric.key(tree.node(b)));
        kb.cmp(&ka).then_with(|| {
            tree.name(a)
                .to_lowercase()
                .cmp(&tree.name(b).to_lowercase())
        })
    });
    v
}

/// Coluna de ordenação da árvore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortKey {
    Name,
    #[default]
    Size,
    Allocated,
    Files,
    Dirs,
    Modified,
    Compression,
}

impl SortKey {
    /// Ordenação padrão de uma métrica.
    pub fn from_metric(m: Metric) -> Self {
        match m {
            Metric::Size => SortKey::Size,
            Metric::Allocated => SortKey::Allocated,
            Metric::FileCount => SortKey::Files,
            Metric::Modified => SortKey::Modified,
            Metric::Compression => SortKey::Compression,
        }
    }

    fn value(self, tree: &Tree, id: NodeId) -> u64 {
        let n = tree.node(id);
        match self {
            SortKey::Name => 0,
            SortKey::Size => Metric::Size.key(n),
            SortKey::Allocated => Metric::Allocated.key(n),
            SortKey::Files => Metric::FileCount.key(n),
            SortKey::Dirs => u64::from(n.dirs),
            SortKey::Modified => Metric::Modified.key(n),
            SortKey::Compression => Metric::Compression.key(n),
        }
    }
}

/// Filhos de `id` ordenados por `key`. Empates (e a coluna Nome) usam o nome sem diferenciar
/// maiúsculas, sempre em ordem crescente.
pub fn sort_children_by(tree: &Tree, id: NodeId, key: SortKey, descending: bool) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = tree.children(id).to_vec();
    if key == SortKey::Name {
        v.sort_by_cached_key(|&c| tree.name(c).to_lowercase());
        if descending {
            v.reverse();
        }
    } else {
        v.sort_by_cached_key(|&c| {
            let val = key.value(tree, c);
            (
                if descending { u64::MAX - val } else { val },
                tree.name(c).to_lowercase(),
            )
        });
    }
    v
}

/// Os `n` maiores nós de toda a árvore pela métrica.
pub fn top_nodes(tree: &Tree, metric: Metric, n: usize, only_files: bool) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = (0..tree.len() as NodeId)
        .filter(|&i| !only_files || !tree.node(i).is_dir())
        .collect();
    let n = n.min(v.len());
    if n == 0 {
        return Vec::new();
    }
    v.select_nth_unstable_by(n - 1, |&a, &b| {
        metric.key(tree.node(b)).cmp(&metric.key(tree.node(a)))
    });
    v.truncate(n);
    v.sort_by_key(|&a| std::cmp::Reverse(metric.key(tree.node(a))));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{NONE, NodeInfo, TreeBuilder, flags};

    fn tree() -> Tree {
        let mut b = TreeBuilder::new();
        let r = b.push(
            NONE,
            "R",
            NodeInfo {
                flags: flags::DIR,
                ..Default::default()
            },
        );
        for (name, size) in [("b", 5u64), ("A", 50), ("c", 5), ("d", 500)] {
            b.push(
                r,
                name,
                NodeInfo {
                    size,
                    allocated: size,
                    ..Default::default()
                },
            );
        }
        b.finish()
    }

    #[test]
    fn sorts_desc_with_name_tiebreak() {
        let t = tree();
        let names: Vec<_> = sorted_children(&t, 0, Metric::Size)
            .iter()
            .map(|&i| t.name(i).to_string())
            .collect();
        assert_eq!(names, ["d", "A", "b", "c"]);
    }

    #[test]
    fn sort_by_key_asc_desc_and_name() {
        let t = tree();
        let names = |k, d| -> Vec<String> {
            sort_children_by(&t, 0, k, d)
                .iter()
                .map(|&i| t.name(i).to_string())
                .collect()
        };
        assert_eq!(names(SortKey::Size, true), ["d", "A", "b", "c"]);
        assert_eq!(names(SortKey::Size, false), ["b", "c", "A", "d"]);
        assert_eq!(names(SortKey::Name, false), ["A", "b", "c", "d"]);
        assert_eq!(names(SortKey::Name, true), ["d", "c", "b", "A"]);
    }

    #[test]
    fn top_n() {
        let t = tree();
        let top = top_nodes(&t, Metric::Size, 2, true);
        assert_eq!(t.name(top[0]), "d");
        assert_eq!(t.name(top[1]), "A");
    }
}

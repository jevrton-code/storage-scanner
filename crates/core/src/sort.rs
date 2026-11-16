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
    fn top_n() {
        let t = tree();
        let top = top_nodes(&t, Metric::Size, 2, true);
        assert_eq!(t.name(top[0]), "d");
        assert_eq!(t.name(top[1]), "A");
    }
}

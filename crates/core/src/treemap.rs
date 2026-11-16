//! Layout *squarified* de treemap (Bruls, Huizing, van Wijk).

use crate::metrics::Metric;
use crate::tree::{NodeId, Tree};

/// Retângulo posicionado de um nó. Os pais vêm antes dos filhos na saída.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TmRect {
    pub node: NodeId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub depth: u16,
    /// `true` = desenhado como bloco sólido (arquivo ou pasta sem filhos visíveis);
    /// `false` = contêiner cujos filhos estão dentro dele.
    pub leaf: bool,
}

impl TmRect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Margem interna de cada pasta (px), onde o fundo da pasta aparece.
    pub padding: f32,
    /// Retângulos com lado menor que isto (px) são descartados junto com seus filhos.
    pub min_size: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            padding: 1.0,
            min_size: 2.0,
        }
    }
}

#[derive(Clone, Copy)]
struct R {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Calcula o layout da subárvore de `root` dentro de `width × height`.
pub fn layout(
    tree: &Tree,
    root: NodeId,
    metric: Metric,
    width: f32,
    height: f32,
    params: Params,
) -> Vec<TmRect> {
    let mut out = Vec::new();
    if tree.is_empty() || width < 1.0 || height < 1.0 {
        return out;
    }
    let bounds = R {
        x: 0.0,
        y: 0.0,
        w: f64::from(width),
        h: f64::from(height),
    };
    let min = f64::from(params.min_size);
    let pad = f64::from(params.padding);
    // Pilha explícita: (nó, retângulo, profundidade).
    let mut stack = vec![(root, bounds, 0u16)];
    let mut items: Vec<(NodeId, f64)> = Vec::new();
    let mut placed: Vec<(NodeId, R)> = Vec::new();
    while let Some((id, r, depth)) = stack.pop() {
        let node = tree.node(id);
        let idx = out.len();
        out.push(TmRect {
            node: id,
            x: r.x as f32,
            y: r.y as f32,
            w: r.w as f32,
            h: r.h as f32,
            depth,
            leaf: true,
        });
        if !node.is_dir() || node.child_count == 0 {
            continue;
        }
        let inner = if depth > 0 || pad > 0.0 {
            R {
                x: r.x + pad,
                y: r.y + pad,
                w: r.w - 2.0 * pad,
                h: r.h - 2.0 * pad,
            }
        } else {
            r
        };
        if inner.w <= 0.0 || inner.h <= 0.0 {
            continue;
        }
        items.clear();
        let mut total = 0.0;
        for &c in tree.children(id) {
            let w = metric.weight(tree.node(c)) as f64;
            if w > 0.0 {
                items.push((c, w));
                total += w;
            }
        }
        if items.is_empty() {
            continue;
        }
        items.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let scale = inner.w * inner.h / total;
        for it in items.iter_mut() {
            it.1 *= scale; // peso → área
        }
        placed.clear();
        squarify(&items, inner, &mut placed);
        let mut any = false;
        // Empilha em ordem inversa para que a saída mantenha a ordem de tamanho.
        for &(c, cr) in placed.iter().rev() {
            if cr.w >= min && cr.h >= min {
                stack.push((c, cr, depth + 1));
                any = true;
            }
        }
        if any {
            out[idx].leaf = false;
        }
    }
    out
}

fn worst(short: f64, sum: f64, amax: f64, amin: f64) -> f64 {
    let s2 = short * short;
    let sum2 = sum * sum;
    (s2 * amax / sum2).max(sum2 / (s2 * amin))
}

fn squarify(items: &[(NodeId, f64)], mut rect: R, out: &mut Vec<(NodeId, R)>) {
    let mut i = 0;
    while i < items.len() {
        let short = rect.w.min(rect.h);
        if short <= 0.0 {
            break;
        }
        let mut j = i;
        let mut sum = 0.0;
        let mut best = f64::INFINITY;
        while j < items.len() {
            let s2 = sum + items[j].1;
            let w = worst(short, s2, items[i].1, items[j].1);
            if j > i && w > best {
                break;
            }
            sum = s2;
            best = w;
            j += 1;
        }
        let thickness = sum / short;
        if rect.w >= rect.h {
            // Coluna à esquerda; itens empilhados na vertical.
            let mut y = rect.y;
            for &(id, a) in &items[i..j] {
                let h = a / thickness;
                out.push((
                    id,
                    R {
                        x: rect.x,
                        y,
                        w: thickness,
                        h,
                    },
                ));
                y += h;
            }
            rect.x += thickness;
            rect.w -= thickness;
        } else {
            // Faixa no topo; itens lado a lado.
            let mut x = rect.x;
            for &(id, a) in &items[i..j] {
                let w = a / thickness;
                out.push((
                    id,
                    R {
                        x,
                        y: rect.y,
                        w,
                        h: thickness,
                    },
                ));
                x += w;
            }
            rect.y += thickness;
            rect.h -= thickness;
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{NONE, NodeInfo, TreeBuilder, flags};

    /// Árvore com `n` pastas de `m` arquivos de tamanhos pseudoaleatórios.
    fn random_tree(n: u32, m: u32) -> Tree {
        let mut b = TreeBuilder::new();
        let dir = NodeInfo {
            flags: flags::DIR,
            ..Default::default()
        };
        let root = b.push(NONE, "R", dir);
        let mut seed = 12345u64;
        let mut rnd = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        for d in 0..n {
            let dn = b.push(root, &format!("d{d}"), dir);
            for f in 0..m {
                let size = u64::from(rnd() % 100_000 + 1);
                b.push(
                    dn,
                    &format!("f{f}"),
                    NodeInfo {
                        size,
                        allocated: size,
                        ..Default::default()
                    },
                );
            }
        }
        b.finish()
    }

    fn leaves(l: &[TmRect]) -> Vec<&TmRect> {
        l.iter().filter(|r| r.leaf).collect()
    }

    #[test]
    fn leaf_areas_sum_to_bounds_without_padding() {
        let t = random_tree(12, 40);
        let p = Params {
            padding: 0.0,
            min_size: 0.0,
        };
        let l = layout(&t, 0, Metric::Size, 800.0, 500.0, p);
        let sum: f64 = leaves(&l)
            .iter()
            .map(|r| f64::from(r.w) * f64::from(r.h))
            .sum();
        assert!(
            (sum - 800.0 * 500.0).abs() < 800.0 * 500.0 * 1e-4,
            "soma = {sum}"
        );
        // Somente arquivos são folhas quando nada é descartado.
        assert!(leaves(&l).iter().all(|r| !t.node(r.node).is_dir()));
        assert_eq!(leaves(&l).len(), 12 * 40);
    }

    #[test]
    fn no_overlap_and_inside_bounds() {
        let t = random_tree(8, 25);
        let l = layout(&t, 0, Metric::Size, 640.0, 480.0, Params::default());
        let lv = leaves(&l);
        for r in &lv {
            assert!(r.x >= -1e-3 && r.y >= -1e-3);
            assert!(r.x + r.w <= 640.0 + 1e-2 && r.y + r.h <= 480.0 + 1e-2);
        }
        for (i, a) in lv.iter().enumerate() {
            for b in &lv[i + 1..] {
                let ox = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let oy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(ox <= 1e-2 || oy <= 1e-2, "{a:?} sobrepõe {b:?}");
            }
        }
    }

    #[test]
    fn children_are_inside_their_parent() {
        let t = random_tree(5, 10);
        let l = layout(&t, 0, Metric::Size, 400.0, 300.0, Params::default());
        for r in &l {
            if let Some(p) = t.parent(r.node)
                && let Some(pr) = l.iter().find(|x| x.node == p)
            {
                assert!(r.x >= pr.x - 1e-3 && r.y >= pr.y - 1e-3);
                assert!(r.x + r.w <= pr.x + pr.w + 1e-3);
                assert!(r.y + r.h <= pr.y + pr.h + 1e-3);
            }
        }
    }

    #[test]
    fn tiny_rects_are_discarded_and_area_tracks_weight() {
        let t = random_tree(3, 2000);
        let l = layout(&t, 0, Metric::Size, 300.0, 200.0, Params::default());
        assert!(l.iter().all(|r| r.w >= 2.0 && r.h >= 2.0 || r.depth == 0));
        assert!(leaves(&l).len() < 3 * 2000);

        // Dois arquivos 3:1 → áreas 3:1.
        let mut b = TreeBuilder::new();
        let root = b.push(
            NONE,
            "R",
            NodeInfo {
                flags: flags::DIR,
                ..Default::default()
            },
        );
        for (n, s) in [("a", 300u64), ("b", 100)] {
            b.push(
                root,
                n,
                NodeInfo {
                    size: s,
                    allocated: s,
                    ..Default::default()
                },
            );
        }
        let t2 = b.finish();
        let l2 = layout(
            &t2,
            0,
            Metric::Size,
            400.0,
            100.0,
            Params {
                padding: 0.0,
                min_size: 0.0,
            },
        );
        let area = |n: &str| {
            l2.iter()
                .find(|r| t2.name(r.node) == n)
                .map(|r| r.w * r.h)
                .unwrap()
        };
        assert!((area("a") / area("b") - 3.0).abs() < 1e-3);
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        let t = random_tree(1, 1);
        assert!(layout(&t, 0, Metric::Size, 0.5, 100.0, Params::default()).is_empty());
        let single = layout(&t, 1, Metric::Size, 100.0, 100.0, Params::default());
        assert!(!single.is_empty());
        assert_eq!(single[0].node, 1);
    }

    #[test]
    fn aspect_ratios_are_reasonable() {
        let t = random_tree(1, 60);
        let p = Params {
            padding: 0.0,
            min_size: 0.0,
        };
        let l = layout(&t, 1, Metric::Size, 600.0, 400.0, p);
        let ratios: Vec<f32> = leaves(&l)
            .iter()
            .map(|r| (r.w / r.h).max(r.h / r.w))
            .collect();
        let avg = ratios.iter().sum::<f32>() / ratios.len() as f32;
        assert!(avg < 3.0, "razão média {avg}");
    }
}

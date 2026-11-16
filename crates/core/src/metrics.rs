//! Métricas de exibição/ordenação.

use crate::tree::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Size,
    Allocated,
    FileCount,
    Modified,
    Compression,
}

impl Metric {
    pub const ALL: [Metric; 5] = [
        Metric::Size,
        Metric::Allocated,
        Metric::FileCount,
        Metric::Modified,
        Metric::Compression,
    ];

    /// Chave numérica (maior = mais relevante) usada em ordenação.
    pub fn key(self, n: &Node) -> u64 {
        match self {
            Metric::Size => n.size,
            Metric::Allocated => n.allocated,
            Metric::FileCount => u64::from(n.files),
            Metric::Modified => n.mtime.max(0) as u64,
            Metric::Compression => (compression_ratio(n) * 10_000.0) as u64,
        }
    }

    /// Peso (área no treemap / base da barra de %). Datas não têm peso próprio: usam o tamanho.
    pub fn weight(self, n: &Node) -> u64 {
        match self {
            Metric::Size | Metric::Modified => n.size,
            Metric::Allocated => n.allocated,
            Metric::FileCount => u64::from(n.files),
            Metric::Compression => n.size.saturating_sub(n.allocated),
        }
    }
}

/// `1 − alocado/tamanho`, limitada a `[0, 1]` (0 quando não há economia).
pub fn compression_ratio(n: &Node) -> f64 {
    if n.size == 0 {
        return 0.0;
    }
    (1.0 - n.allocated as f64 / n.size as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio() {
        let n = Node {
            size: 1000,
            allocated: 250,
            ..Default::default()
        };
        assert!((compression_ratio(&n) - 0.75).abs() < 1e-9);
        let m = Node {
            size: 100,
            allocated: 4096,
            ..Default::default()
        };
        assert_eq!(compression_ratio(&m), 0.0);
        assert_eq!(compression_ratio(&Node::default()), 0.0);
    }
}

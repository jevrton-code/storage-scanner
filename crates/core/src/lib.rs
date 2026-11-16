//! Modelo de dados e algoritmos puros (sem I/O) do Storage Scanner.

pub mod ext_stats;
pub mod format;
pub mod metrics;
pub mod sort;
pub mod tree;
pub mod treemap;

pub use metrics::Metric;
pub use tree::{NONE, Node, NodeId, NodeInfo, Tree, TreeBuilder, flags};

//! Backends de varredura: todos produzem uma [`ss_core::Tree`].

pub mod walk;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ss_core::Tree;

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("varredura cancelada")]
    Cancelled,
    #[error("o caminho não é uma pasta: {0}")]
    NotADirectory(String),
    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub enum ScanTarget {
    /// Volume, pasta local ou caminho UNC.
    Path(PathBuf),
}

/// Contadores atômicos lidos pela UI a cada frame.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    /// Pastas que não puderam ser lidas (acesso negado etc.).
    pub errors: AtomicU64,
}

impl Progress {
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.files.load(Ordering::Relaxed),
            self.dirs.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
        )
    }
}

pub trait Scanner {
    fn scan(
        &self,
        target: &ScanTarget,
        progress: &Progress,
        cancel: &AtomicBool,
    ) -> Result<Tree, ScanError>;
}

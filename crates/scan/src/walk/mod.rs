//! Backend Walker: varredura multithread por enumeração de diretórios.

pub mod dir_enum;
pub mod pool;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ss_core::{NONE, NodeInfo, Tree, TreeBuilder, flags};

use crate::{Progress, ScanError, ScanTarget, Scanner};

pub struct WalkScanner {
    pub threads: usize,
}

impl WalkScanner {
    /// Uma thread por núcleo (disco local).
    pub fn local() -> Self {
        Self {
            threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
        }
    }
    /// Mais threads para alvos de rede, onde a latência domina.
    pub fn network() -> Self {
        Self {
            threads: std::thread::available_parallelism().map_or(4, |n| n.get()) * 3,
        }
    }
}

/// Nome exibido para a raiz: remove barra final, exceto em raiz de unidade ("C:\").
fn root_display(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    let t = s.trim_end_matches('\\');
    if t.len() == 2 && t.ends_with(':') {
        format!("{t}\\")
    } else if t.is_empty() {
        s
    } else {
        t.to_string()
    }
}

impl Scanner for WalkScanner {
    fn scan(
        &self,
        target: &ScanTarget,
        progress: &Progress,
        cancel: &AtomicBool,
    ) -> Result<Tree, ScanError> {
        let ScanTarget::Path(path) = target;
        let abs: PathBuf = std::path::absolute(path)?;
        let meta = std::fs::metadata(&abs)?;
        if !meta.is_dir() {
            return Err(ScanError::NotADirectory(abs.display().to_string()));
        }

        let (outputs, total) = pool::run(abs.clone(), self.threads, progress, cancel);
        if cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }

        let mut b = TreeBuilder::new();
        b.resize(total as usize);
        b.put(
            0,
            NONE,
            &root_display(&abs),
            NodeInfo {
                flags: flags::DIR,
                ..Default::default()
            },
        );
        for out in &outputs {
            for n in &out.nodes {
                b.put(n.id, n.parent, &n.name, n.info);
            }
        }
        for out in &outputs {
            for &id in &out.failed_dirs {
                b.or_flags(id, flags::ACCESS_ERROR);
            }
        }
        Ok(b.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_fixture() {
        let dir = std::env::temp_dir().join("ss_walk_fixture");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::write(dir.join("r.bin"), vec![1u8; 1000]).unwrap();
        std::fs::write(dir.join("a/x.bin"), vec![1u8; 2000]).unwrap();
        std::fs::write(dir.join("a/b/y.bin"), vec![1u8; 30]).unwrap();

        let progress = Progress::default();
        let tree = WalkScanner { threads: 4 }
            .scan(
                &ScanTarget::Path(dir.clone()),
                &progress,
                &AtomicBool::new(false),
            )
            .unwrap();
        let root = tree.node(Tree::ROOT);
        assert_eq!(root.size, 3030);
        assert_eq!(root.files, 3);
        assert_eq!(root.dirs, 3);
        assert!(root.allocated >= 3030);
        assert_eq!(progress.snapshot().0, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops() {
        let progress = Progress::default();
        let r = WalkScanner { threads: 2 }.scan(
            &ScanTarget::Path(std::env::temp_dir()),
            &progress,
            &AtomicBool::new(true),
        );
        assert!(matches!(r, Err(ScanError::Cancelled)));
    }

    #[test]
    fn rejects_file() {
        let f = std::env::temp_dir().join("ss_walk_file.txt");
        std::fs::write(&f, b"x").unwrap();
        let r = WalkScanner { threads: 1 }.scan(
            &ScanTarget::Path(f.clone()),
            &Progress::default(),
            &AtomicBool::new(false),
        );
        assert!(matches!(r, Err(ScanError::NotADirectory(_))));
        let _ = std::fs::remove_file(f);
    }
}

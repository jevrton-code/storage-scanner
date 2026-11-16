//! Backends de varredura: todos produzem uma [`ss_core::Tree`].

pub mod elevation;
pub mod mft;
pub mod volumes;
pub mod walk;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ss_core::Tree;

pub use mft::Labels;
use volumes::{DriveKind, VolumeInfo};

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("varredura cancelada")]
    Cancelled,
    #[error("o caminho não é uma pasta: {0}")]
    NotADirectory(String),
    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("a leitura direta da MFT exige administrador")]
    NeedsElevation,
    #[error("o volume não é NTFS")]
    NotNtfs,
    #[error("MFT: {0}")]
    Mft(String),
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
    pub fn reset(&self) {
        for c in [&self.files, &self.dirs, &self.bytes, &self.errors] {
            c.store(0, Ordering::Relaxed);
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Auto,
    Mft,
    Walk,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "mft" => Some(Self::Mft),
            "walk" => Some(Self::Walk),
            _ => None,
        }
    }
}

pub struct ScanReport {
    pub tree: Tree,
    /// Backend efetivamente usado (`Mft` ou `Walk`).
    pub backend: Backend,
    /// Motivo de uma queda do MFT para o Walker, se houve.
    pub fallback_reason: Option<String>,
}

/// Ponto de entrada: escolhe o backend conforme o alvo e a elevação.
pub fn scan(
    target: &ScanTarget,
    backend: Backend,
    labels: &Labels,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<ScanReport, ScanError> {
    let ScanTarget::Path(path) = target;
    let abs = std::path::absolute(path)?;
    let vol = volumes::volume_root_of(&abs).map(|r| volumes::volume_info(&r));

    let is_remote = abs.to_string_lossy().starts_with(r"\\")
        || vol.as_ref().is_some_and(|v| v.kind == DriveKind::Remote);
    let walker = || {
        if is_remote {
            walk::WalkScanner::network()
        } else {
            walk::WalkScanner::local()
        }
    };

    let mft_possible = match (&vol, backend) {
        (_, Backend::Walk) => Err(None),
        (Some(v), _) if v.is_ntfs() && !is_remote => {
            if elevation::is_elevated() {
                Ok(v)
            } else {
                Err(Some(ScanError::NeedsElevation))
            }
        }
        _ => Err(Some(ScanError::NotNtfs)),
    };

    match mft_possible {
        Ok(v) => match scan_mft(&abs, v, labels, progress, cancel) {
            Ok(tree) => {
                return Ok(ScanReport {
                    tree,
                    backend: Backend::Mft,
                    fallback_reason: None,
                });
            }
            Err(ScanError::Cancelled) => return Err(ScanError::Cancelled),
            Err(e) if backend == Backend::Mft => return Err(e),
            Err(e) => {
                progress.reset();
                let tree = walker().scan(target, progress, cancel)?;
                return Ok(ScanReport {
                    tree,
                    backend: Backend::Walk,
                    fallback_reason: Some(e.to_string()),
                });
            }
        },
        // Pedido explícito de MFT que não pode ser atendido.
        Err(Some(e)) if backend == Backend::Mft => return Err(e),
        Err(_) => {}
    }

    let tree = walker().scan(target, progress, cancel)?;
    Ok(ScanReport {
        tree,
        backend: Backend::Walk,
        fallback_reason: None,
    })
}

/// Lê a MFT do volume inteiro e, se o alvo for uma subpasta, devolve só essa subárvore.
fn scan_mft(
    target: &Path,
    vol: &VolumeInfo,
    labels: &Labels,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<Tree, ScanError> {
    let drive = vol.root.trim_end_matches('\\');
    let root_name = vol.root.clone();
    let tree = mft::scan_volume(drive, &root_name, labels, progress, cancel)?;

    let target_s = target.to_string_lossy();
    let rel = target_s
        .get(vol.root.len().min(target_s.len())..)
        .unwrap_or("")
        .trim_matches('\\');
    if rel.is_empty() {
        return Ok(tree);
    }
    let comps: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
    let id = tree
        .find_child_path(Tree::ROOT, &comps)
        .ok_or_else(|| ScanError::Mft(format!("pasta não encontrada na MFT: {rel}")))?;
    if !tree.node(id).is_dir() {
        return Err(ScanError::NotADirectory(target_s.into_owned()));
    }
    Ok(tree.extract_subtree(id, target_s.trim_end_matches('\\')))
}

/// Varre um único item (arquivo ou pasta) e devolve uma árvore cuja raiz é o próprio item,
/// nomeado pelo último componente do caminho. Usado para atualizar a árvore após ações.
pub fn scan_single(path: &Path, cancel: &AtomicBool) -> Result<Tree, ScanError> {
    use ss_core::{NONE, NodeInfo, TreeBuilder};

    let meta = std::fs::symlink_metadata(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    if meta.is_dir() && !meta.file_type().is_symlink() {
        let tree = walk::WalkScanner::local().scan(
            &ScanTarget::Path(path.to_path_buf()),
            &Progress::default(),
            cancel,
        )?;
        // A raiz do walker leva o caminho completo; aqui só o nome.
        return Ok(tree.extract_subtree(Tree::ROOT, &name));
    }
    // Arquivo: pega tamanho alocado e atributos listando a pasta pai.
    let parent = path
        .parent()
        .ok_or_else(|| ScanError::NotADirectory(name.clone()))?;
    let mut entries = Vec::new();
    walk::dir_enum::read_dir(parent, &mut entries, &mut Vec::new())?;
    let e = entries
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| ScanError::Io(std::io::ErrorKind::NotFound.into()))?;
    let mut b = TreeBuilder::new();
    b.push(
        NONE,
        &e.name,
        NodeInfo {
            flags: e.flags,
            size: e.size,
            allocated: e.allocated,
            mtime: e.mtime,
        },
    );
    Ok(b.finish())
}

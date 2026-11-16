//! Ações sobre arquivos executadas em segundo plano, e atualização incremental da árvore.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Receiver;
use ss_core::{Tree, ext_stats};
use ss_ops::OpError;
use ss_ops::compression::{CompressProgress, set_compression_tree};
use ss_ops::file_op::{self, Op, Options};
use ss_ops::zip::{ZipProgress, unique_zip_path, zip_paths};

use crate::state::ScanResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    Reveal,
    CopyPath,
    Recycle,
    DeletePermanent,
    MoveTo,
    CopyTo,
    Zip,
    Compress(bool),
    NativeMenu,
}

pub enum JobProgress {
    /// Progresso exibido pelo próprio Windows (IFileOperation).
    Native,
    Zip(Arc<ZipProgress>),
    Compress(Arc<CompressProgress>),
}

/// Resultado de uma ação: o que sumiu e o que precisa ser (re)lido para a árvore.
pub struct Outcome {
    pub error: Option<String>,
    pub cancelled: bool,
    /// Caminhos de origem (removidos da árvore se não existirem mais).
    pub sources: Vec<PathBuf>,
    /// Caminhos de destino já relidos, prontos para enxertar.
    pub grafts: Vec<(PathBuf, Tree)>,
    /// Mensagem informativa (ex.: nome do zip criado).
    pub info: Option<String>,
}

pub struct ActionJob {
    pub label: String,
    pub cancel: Arc<AtomicBool>,
    pub progress: JobProgress,
    rx: Receiver<Outcome>,
}

impl ActionJob {
    fn spawn(
        ctx: &egui::Context,
        label: String,
        progress: JobProgress,
        cancel: Arc<AtomicBool>,
        work: impl FnOnce(&AtomicBool) -> Outcome + Send + 'static,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (ctx, c) = (ctx.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("action".into())
            .spawn(move || {
                let _ = tx.send(work(&c));
                ctx.request_repaint();
            })
            .expect("falha ao criar a thread da ação");
        Self {
            label,
            cancel,
            progress,
            rx,
        }
    }

    pub fn poll(&self) -> Option<Outcome> {
        self.rx.try_recv().ok()
    }

    /// Excluir / mover / copiar via `IFileOperation`.
    pub fn file_op(
        ctx: &egui::Context,
        label: String,
        op: Op,
        paths: Vec<PathBuf>,
        owner: Option<isize>,
    ) -> Self {
        Self::spawn(
            ctx,
            label,
            JobProgress::Native,
            Arc::new(AtomicBool::new(false)),
            move |cancel| {
                let res = file_op::run(
                    &op,
                    &paths,
                    Options {
                        owner,
                        silent: false,
                    },
                );
                let (sources, dests) = match &op {
                    Op::Delete { .. } => (paths.clone(), Vec::new()),
                    Op::Move { dest_dir } => (paths.clone(), joined(dest_dir, &paths)),
                    Op::Copy { dest_dir } => (Vec::new(), joined(dest_dir, &paths)),
                };
                finish(res, sources, dests, cancel, None)
            },
        )
    }

    /// Compacta em `.zip` ao lado do primeiro item.
    pub fn zip(ctx: &egui::Context, label: String, paths: Vec<PathBuf>) -> Self {
        let progress = Arc::new(ZipProgress::default());
        let p = progress.clone();
        Self::spawn(
            ctx,
            label,
            JobProgress::Zip(progress),
            Arc::new(AtomicBool::new(false)),
            move |cancel| {
                let Some(first) = paths.first() else {
                    return finish(Ok(()), vec![], vec![], cancel, None);
                };
                let dir = first.parent().unwrap_or(Path::new(".")).to_path_buf();
                let base = if paths.len() == 1 {
                    first
                        .file_stem()
                        .or_else(|| first.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "arquivo".into())
                } else {
                    dir.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "arquivos".into())
                };
                let zip = unique_zip_path(&dir, &base);
                let res = zip_paths(&paths, &zip, &p, cancel);
                let ok = res.is_ok();
                let info = ok.then(|| zip.display().to_string());
                finish(
                    res,
                    vec![],
                    if ok { vec![zip] } else { vec![] },
                    cancel,
                    info,
                )
            },
        )
    }

    /// Relê um item que pode ter mudado fora do app (ex.: pelo menu nativo do Windows).
    pub fn refresh(ctx: &egui::Context, label: String, path: PathBuf) -> Self {
        Self::spawn(
            ctx,
            label,
            JobProgress::Native,
            Arc::new(AtomicBool::new(false)),
            move |cancel| finish(Ok(()), vec![path.clone()], vec![path], cancel, None),
        )
    }

    /// Exclui um objeto de um dispositivo MTP; em caso de sucesso remove o nó da árvore.
    pub fn mtp_delete(
        ctx: &egui::Context,
        label: String,
        device: ss_scan::mtp::MtpDevice,
        object_id: String,
        tree_path: PathBuf,
    ) -> Self {
        Self::spawn(
            ctx,
            label,
            JobProgress::Native,
            Arc::new(AtomicBool::new(false)),
            move |_| {
                let res = ss_scan::mtp::transfer::delete_object(&device, &object_id);
                Outcome {
                    sources: if res.is_ok() { vec![tree_path] } else { vec![] },
                    error: res.err().map(|e| e.to_string()),
                    cancelled: false,
                    grafts: vec![],
                    info: None,
                }
            },
        )
    }

    /// Copia um objeto de um dispositivo MTP para uma pasta do PC.
    pub fn mtp_copy_to_pc(
        ctx: &egui::Context,
        label: String,
        device: ss_scan::mtp::MtpDevice,
        object_id: String,
        dest_dir: PathBuf,
    ) -> Self {
        Self::spawn(
            ctx,
            label,
            JobProgress::Native,
            Arc::new(AtomicBool::new(false)),
            move |_| {
                let res = ss_scan::mtp::transfer::copy_object_to_pc(&device, &object_id, &dest_dir);
                Outcome {
                    error: res.err().map(|e| e.to_string()),
                    cancelled: false,
                    sources: vec![],
                    grafts: vec![],
                    info: None,
                }
            },
        )
    }

    /// Liga/desliga a compressão NTFS recursivamente e relê o item.
    pub fn ntfs_compress(ctx: &egui::Context, label: String, path: PathBuf, enable: bool) -> Self {
        let progress = Arc::new(CompressProgress::default());
        let p = progress.clone();
        Self::spawn(
            ctx,
            label,
            JobProgress::Compress(progress),
            Arc::new(AtomicBool::new(false)),
            move |cancel| {
                let res = set_compression_tree(&path, enable, &p, cancel);
                // Mesmo cancelado, o que já foi comprimido muda os tamanhos: relê sempre.
                let sources = vec![path.clone()];
                let mut out = finish(res, sources, vec![path], cancel, None);
                let failed = p.failed.load(Ordering::Relaxed);
                if failed > 0 && out.error.is_none() {
                    out.info = Some(format!("{failed}"));
                }
                out
            },
        )
    }
}

fn joined(dest_dir: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| dest_dir.join(n)))
        .collect()
}

/// Monta o `Outcome`: relê (em segundo plano) cada destino existente para enxertar na árvore.
fn finish(
    res: Result<(), OpError>,
    sources: Vec<PathBuf>,
    dests: Vec<PathBuf>,
    cancel: &AtomicBool,
    info: Option<String>,
) -> Outcome {
    let (error, cancelled) = match res {
        Ok(()) => (None, false),
        Err(OpError::Cancelled) => (None, true),
        Err(e) => (Some(e.to_string()), false),
    };
    let never = AtomicBool::new(false);
    let _ = cancel;
    let grafts = dests
        .into_iter()
        .filter(|d| d.exists())
        .filter_map(|d| ss_scan::scan_single(&d, &never).ok().map(|t| (d, t)))
        .collect();
    Outcome {
        error,
        cancelled,
        sources,
        grafts,
        info,
    }
}

/// Aplica `outcome` à árvore carregada sem nova varredura. `true` se algo mudou.
pub fn apply_outcome(result: &mut ScanResult, outcome: &mut Outcome) -> bool {
    let tree = Arc::make_mut(&mut result.tree);
    let mut changed = false;
    for s in &outcome.sources {
        if s.exists() {
            continue; // não foi removido/movido
        }
        if let Some(id) = tree.find_by_path(&s.to_string_lossy())
            && tree.remove_subtree(id)
        {
            changed = true;
        }
    }
    for (dest, sub) in outcome.grafts.drain(..) {
        let Some(parent) = dest
            .parent()
            .and_then(|p| tree.find_by_path(&p.to_string_lossy()))
        else {
            continue; // destino fora da árvore analisada
        };
        // Substitui uma versão anterior do mesmo item (ex.: após compressão).
        if let Some(old) = tree.find_by_path(&dest.to_string_lossy()) {
            tree.remove_subtree(old);
        }
        if tree.graft(parent, &sub).is_some() {
            changed = true;
        }
    }
    if changed {
        result.generation += 1;
        result.ext_stats = ext_stats::compute(tree);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss_core::{Metric, flags};
    use ss_scan::walk::WalkScanner;
    use ss_scan::{Backend, Progress, ScanTarget, Scanner};
    use std::time::Duration;

    fn scan(root: &Path) -> Tree {
        WalkScanner { threads: 2 }
            .scan(
                &ScanTarget::Path(root.to_path_buf()),
                &Progress::default(),
                &AtomicBool::new(false),
            )
            .unwrap()
    }

    fn result_for(root: &Path) -> ScanResult {
        let tree = scan(root);
        ScanResult {
            ext_stats: ext_stats::compute(&tree),
            tree: Arc::new(tree),
            generation: 0,
            target: root.to_path_buf(),
            mtp: None,
            backend: Backend::Walk,
            fallback_reason: None,
            elapsed: Duration::ZERO,
            errors: 0,
        }
    }

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ss_incremental_{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a", "b", "d"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("a/x.bin"), vec![1u8; 70_000]).unwrap();
        std::fs::write(root.join("a/y.bin"), vec![1u8; 5_000]).unwrap();
        std::fs::write(root.join("b/z.bin"), vec![1u8; 123_456]).unwrap();
        std::fs::write(root.join("c.txt"), vec![1u8; 999]).unwrap();
        root
    }

    /// A árvore atualizada deve ter os mesmos totais de uma varredura nova.
    fn assert_matches_fresh_scan(r: &ScanResult, root: &Path) {
        let fresh = scan(root);
        let (a, b) = (r.tree.node(Tree::ROOT), fresh.node(Tree::ROOT));
        assert_eq!(
            (a.size, a.allocated, a.files, a.dirs),
            (b.size, b.allocated, b.files, b.dirs)
        );
        // Estatísticas por tipo também são recalculadas.
        let total: u64 = r.ext_stats.iter().map(|s| s.size).sum();
        assert_eq!(total, b.size);
        // Todos os nós alcançáveis existem no disco.
        let mut stack = vec![Tree::ROOT];
        while let Some(id) = stack.pop() {
            assert!(Path::new(&r.tree.path(id)).exists(), "{}", r.tree.path(id));
            stack.extend_from_slice(r.tree.children(id));
        }
    }

    fn silent(op: &Op, paths: &[PathBuf]) {
        file_op::run(
            op,
            paths,
            Options {
                owner: None,
                silent: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn delete_move_copy_zip_keep_tree_consistent() {
        let root = fixture("ops");
        let mut r = result_for(&root);
        let never = AtomicBool::new(false);
        let before_gen = r.generation;

        // Excluir (permanente) a pasta "a".
        let a = root.join("a");
        silent(&Op::Delete { permanent: true }, std::slice::from_ref(&a));
        let mut out = finish(Ok(()), vec![a.clone()], vec![], &never, None);
        assert!(apply_outcome(&mut r, &mut out));
        assert!(r.generation > before_gen);
        assert!(r.tree.find_by_path(&a.to_string_lossy()).is_none());
        assert_matches_fresh_scan(&r, &root);

        // Mover "b" para "d".
        let (b, d) = (root.join("b"), root.join("d"));
        silent(
            &Op::Move {
                dest_dir: d.clone(),
            },
            std::slice::from_ref(&b),
        );
        let mut out = finish(
            Ok(()),
            vec![b.clone()],
            joined(&d, std::slice::from_ref(&b)),
            &never,
            None,
        );
        assert!(apply_outcome(&mut r, &mut out));
        assert!(
            r.tree
                .find_by_path(&d.join("b").join("z.bin").to_string_lossy())
                .is_some()
        );
        assert!(r.tree.find_by_path(&b.to_string_lossy()).is_none());
        assert_matches_fresh_scan(&r, &root);

        // Copiar "c.txt" para "d".
        let c = root.join("c.txt");
        silent(
            &Op::Copy {
                dest_dir: d.clone(),
            },
            std::slice::from_ref(&c),
        );
        let mut out = finish(
            Ok(()),
            vec![],
            joined(&d, std::slice::from_ref(&c)),
            &never,
            None,
        );
        assert!(apply_outcome(&mut r, &mut out));
        assert_matches_fresh_scan(&r, &root);

        // Zip de "d" ao lado dele.
        let progress = ZipProgress::default();
        let zip = unique_zip_path(&root, "d");
        zip_paths(std::slice::from_ref(&d), &zip, &progress, &never).unwrap();
        let mut out = finish(Ok(()), vec![], vec![zip.clone()], &never, None);
        assert!(apply_outcome(&mut r, &mut out));
        let zid = r
            .tree
            .find_by_path(&zip.to_string_lossy())
            .expect("zip na árvore");
        assert!(r.tree.node(zid).size > 0 && !r.tree.node(zid).has(flags::DIR));
        assert_matches_fresh_scan(&r, &root);

        // Relê um item existente (substitui sem duplicar).
        let mut out = finish(Ok(()), vec![d.clone()], vec![d.clone()], &never, None);
        apply_outcome(&mut r, &mut out);
        assert_matches_fresh_scan(&r, &root);
        assert_eq!(
            ss_core::sort::top_nodes(&r.tree, Metric::Size, 100, true).len(),
            scan(&root).len() - 1 - r.tree.node(Tree::ROOT).dirs as usize
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ntfs_compression_refresh_updates_allocated() {
        let root = fixture("compress");
        std::fs::write(root.join("big.txt"), "abc".repeat(200_000)).unwrap();
        let mut r = result_for(&root);
        let never = AtomicBool::new(false);
        let p = CompressProgress::default();
        if ss_ops::compression::set_compression(&root, true).is_err() {
            let _ = std::fs::remove_dir_all(&root);
            return; // volume sem compressão NTFS
        }
        set_compression_tree(&root.join("big.txt"), true, &p, &never).unwrap();
        let mut out = finish(Ok(()), vec![], vec![root.join("big.txt")], &never, None);
        apply_outcome(&mut r, &mut out);
        assert_matches_fresh_scan(&r, &root);
        let _ = std::fs::remove_dir_all(&root);
    }
}

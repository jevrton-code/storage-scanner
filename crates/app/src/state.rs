//! Estado da varredura em segundo plano e resultado carregado.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use ss_core::Tree;
use ss_core::ext_stats::{self, ExtStat};
use ss_scan::mtp::MtpDevice;
use ss_scan::{Backend, Labels, Progress, ScanError, ScanReport, ScanTarget};

pub struct ScanJob {
    pub progress: Arc<Progress>,
    pub cancel: Arc<AtomicBool>,
    pub target: PathBuf,
    /// Dispositivo MTP em varredura (nesse caso `target` fica vazio).
    pub device: Option<MtpDevice>,
    pub started: Instant,
    rx: Receiver<Result<(ScanReport, Vec<ExtStat>), ScanError>>,
}

/// Dados necessários para agir sobre os itens de um dispositivo MTP.
pub struct MtpInfo {
    pub device: MtpDevice,
    /// Id do objeto de cada nó (mesma numeração da árvore).
    pub object_ids: Vec<String>,
}

pub struct ScanResult {
    pub tree: Arc<Tree>,
    /// Incrementa a cada alteração da árvore (invalida o layout do treemap).
    pub generation: u64,
    /// Totais por extensão (ordenados por tamanho).
    pub ext_stats: Vec<ExtStat>,
    pub target: PathBuf,
    pub mtp: Option<MtpInfo>,
    pub backend: Backend,
    pub fallback_reason: Option<String>,
    pub elapsed: Duration,
    /// Pastas sem acesso durante a varredura.
    pub errors: u64,
}

pub enum JobPoll {
    Running,
    Done(Box<ScanResult>),
    Failed(ScanError),
}

impl ScanJob {
    /// Inicia a varredura em uma thread própria; a UI só lê os contadores atômicos.
    pub fn start(
        ctx: &egui::Context,
        target: PathBuf,
        device: Option<MtpDevice>,
        labels: Labels,
    ) -> Self {
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, c, t, ctx) = (
            progress.clone(),
            cancel.clone(),
            target.clone(),
            ctx.clone(),
        );
        let dev = device.clone();
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                let scan_target = match dev {
                    Some(d) => ScanTarget::Mtp(d),
                    None => ScanTarget::Path(t),
                };
                let r = ss_scan::scan(&scan_target, Backend::Auto, &labels, &p, &c).map(|rep| {
                    let stats = ext_stats::compute(&rep.tree);
                    (rep, stats)
                });
                let _ = tx.send(r);
                ctx.request_repaint();
            })
            .expect("falha ao criar a thread de varredura");
        Self {
            progress,
            cancel,
            target,
            device,
            started: Instant::now(),
            rx,
        }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> JobPoll {
        match self.rx.try_recv() {
            Ok(Ok((mut report, ext_stats))) => JobPoll::Done(Box::new(ScanResult {
                errors: self.progress.errors.load(Ordering::Relaxed),
                tree: Arc::new(report.tree),
                generation: 0,
                ext_stats,
                target: self.target.clone(),
                mtp: self
                    .device
                    .clone()
                    .zip(report.object_ids.take())
                    .map(|(device, object_ids)| MtpInfo { device, object_ids }),
                backend: report.backend,
                fallback_reason: report.fallback_reason,
                elapsed: self.started.elapsed(),
            })),
            Ok(Err(e)) => JobPoll::Failed(e),
            Err(TryRecvError::Empty) => JobPoll::Running,
            Err(TryRecvError::Disconnected) => JobPoll::Failed(ScanError::Mft(
                "a thread de varredura terminou inesperadamente".into(),
            )),
        }
    }
}

impl ScanJob {
    /// Texto para o título da tela de progresso.
    pub fn title(&self) -> String {
        match &self.device {
            Some(d) => d.name.clone(),
            None => self.target.display().to_string(),
        }
    }
}

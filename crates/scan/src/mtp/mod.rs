//! Backend MTP (Windows Portable Devices): celulares, câmeras e players.
//!
//! Tudo roda em threads dedicadas com COM inicializado; a chamada principal só espera por
//! mensagens, com cancelamento e detecção de travamento (sem progresso por 90 s).

pub mod device;
pub mod enumerate;
pub mod transfer;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use device::{MtpDevice, list_devices};
pub use enumerate::Scanned;

use crate::{Progress, ScanError};

/// Tempo sem nenhum progresso após o qual a varredura é abandonada.
const STALL_TIMEOUT: Duration = Duration::from_secs(90);

/// Varre o dispositivo `dev`.
pub fn scan_device(
    dev: &MtpDevice,
    progress: &Arc<Progress>,
    cancel: &Arc<AtomicBool>,
) -> Result<Scanned, ScanError> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (id, name, p, c) = (
        dev.id.clone(),
        dev.name.clone(),
        progress.clone(),
        cancel.clone(),
    );
    std::thread::Builder::new()
        .name("mtp-scan".into())
        .spawn(move || {
            let _com = device::Com::init();
            let r = device::open(&id).and_then(|d| enumerate::scan_open_device(&d, &name, &p, &c));
            let _ = tx.send(r);
        })
        .map_err(ScanError::Io)?;

    let mut last_count = 0u64;
    let mut last_change = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => return r,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(ScanError::Mtp(
                    "a thread do dispositivo terminou inesperadamente".into(),
                ));
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }
        let (files, dirs, _, _) = progress.snapshot();
        if files + dirs != last_count {
            last_count = files + dirs;
            last_change = Instant::now();
        } else if last_change.elapsed() > STALL_TIMEOUT {
            // A thread (possivelmente presa numa chamada ao aparelho) vê o sinal ao voltar.
            cancel.store(true, Ordering::Relaxed);
            return Err(ScanError::Mtp(
                "o dispositivo parou de responder (tempo esgotado)".into(),
            ));
        }
    }
}

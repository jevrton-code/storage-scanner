//! Registro em arquivo (`%APPDATA%\StorageScanner\app.log`) e tratamento de falhas fatais.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Mutex;

use rust_i18n::t;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("StorageScanner").join("app.log"))
}

/// Liga o `tracing` ao arquivo de log (recomeça se passar de 1 MB); sem arquivo, usa o stderr.
pub fn init() {
    let file = log_path().and_then(|p| {
        std::fs::create_dir_all(p.parent()?).ok()?;
        let too_big = std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
        OpenOptions::new()
            .create(true)
            .append(!too_big)
            .write(true)
            .truncate(too_big)
            .open(p)
            .ok()
    });
    let builder = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO);
    match file {
        Some(f) => builder.with_ansi(false).with_writer(Mutex::new(f)).init(),
        None => builder.init(),
    }
}

/// Grava o pânico no log e, no modo gráfico, mostra uma mensagem ao usuário antes de encerrar.
pub fn install_panic_hook(gui: bool) {
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        tracing::error!("falha fatal: {msg}\n{bt}");
        if let Some(p) = log_path() {
            use std::io::Write;
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "PANIC: {msg}\n{bt}");
            }
        }
        if gui {
            let log = log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(t!("crash.title").to_string())
                .set_description(t!("crash.body", info = msg, log = log).to_string())
                .show();
        } else {
            eprintln!("erro fatal: {msg}");
        }
    }));
}

//! Ações sobre arquivos via shell do Windows.

pub mod compression;
pub mod context_menu;
pub mod explorer_integration;
pub mod file_op;
pub mod open;
pub mod protect;
pub mod zip;

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("operação do Windows falhou: {0}")]
    Windows(String),
    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("operação cancelada")]
    Cancelled,
    #[error("item protegido: {0}")]
    Protected(String),
}

impl From<windows::core::Error> for OpError {
    fn from(e: windows::core::Error) -> Self {
        OpError::Windows(e.message())
    }
}

/// UTF-16 terminado em NUL.
pub(crate) fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Mantém o COM inicializado (apartment STA) enquanto o guard existir na thread atual.
pub(crate) struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // S_FALSE (já inicializado) também exige um CoUninitialize correspondente;
        // RPC_E_CHANGED_MODE (outro modelo) não.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

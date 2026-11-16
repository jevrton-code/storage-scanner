//! Entrada "Analisar com Storage Scanner" no menu de contexto do Explorer (somente HKCU).

use std::ffi::OsStr;
use std::path::Path;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::{OpError, wide};

/// Chaves-base (relativas a HKCU) onde o item é registrado.
pub const TARGETS: [&str; 2] = [
    r"Software\Classes\Directory\shell\StorageScanner",
    r"Software\Classes\Drive\shell\StorageScanner",
];

fn w(s: &str) -> Vec<u16> {
    wide(OsStr::new(s))
}

fn set_default(key: HKEY, value: &str) -> Result<(), OpError> {
    let data: Vec<u8> = w(value).iter().flat_map(|u| u.to_le_bytes()).collect();
    let r = unsafe { RegSetValueExW(key, PCWSTR::null(), None, REG_SZ, Some(&data)) };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(OpError::Windows(format!("RegSetValueEx falhou ({})", r.0)))
    }
}

fn create_key(subkey: &str) -> Result<HKEY, OpError> {
    let mut key = HKEY::default();
    let name = w(subkey);
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if r == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(OpError::Windows(format!("RegCreateKeyEx falhou ({})", r.0)))
    }
}

/// Registra o item para o `exe` informado nas chaves `bases` (relativas a HKCU).
pub fn register_at(bases: &[&str], exe: &Path, label: &str) -> Result<(), OpError> {
    let cmd = format!("\"{}\" \"%1\"", exe.display());
    for base in bases {
        let key = create_key(base)?;
        let r1 = set_default(key, label);
        // Ícone: o próprio executável.
        let icon = format!("\"{}\",0", exe.display());
        let name = w("Icon");
        let data: Vec<u8> = w(&icon).iter().flat_map(|u| u.to_le_bytes()).collect();
        unsafe {
            let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data));
            let _ = RegCloseKey(key);
        }
        r1?;
        let ck = create_key(&format!(r"{base}\command"))?;
        let r2 = set_default(ck, &cmd);
        unsafe {
            let _ = RegCloseKey(ck);
        }
        r2?;
    }
    Ok(())
}

/// Remove o item (e subchaves) das chaves `bases`. Chaves inexistentes são ignoradas.
pub fn unregister_at(bases: &[&str]) -> Result<(), OpError> {
    for base in bases {
        let name = w(base);
        let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())) };
        // 2 = ERROR_FILE_NOT_FOUND: já não existia.
        if r != ERROR_SUCCESS && r.0 != 2 {
            return Err(OpError::Windows(format!("RegDeleteTree falhou ({})", r.0)));
        }
    }
    Ok(())
}

pub fn is_registered_at(base: &str) -> bool {
    let name = w(&format!(r"{base}\command"));
    let mut key = HKEY::default();
    let r = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if r == ERROR_SUCCESS {
        unsafe {
            let _ = RegCloseKey(key);
        }
        true
    } else {
        false
    }
}

pub fn register(exe: &Path, label: &str) -> Result<(), OpError> {
    register_at(&TARGETS, exe, label)
}

pub fn unregister() -> Result<(), OpError> {
    unregister_at(&TARGETS)
}

pub fn is_registered() -> bool {
    TARGETS.iter().all(|b| is_registered_at(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_unregister_roundtrip() {
        // Chaves de teste próprias: não tocam na integração real do usuário.
        const T: [&str; 1] = [r"Software\Classes\Directory\shell\StorageScannerTest"];
        let exe = Path::new(r"C:\Fake Dir\storage-scanner.exe");
        let _ = unregister_at(&T);
        assert!(!is_registered_at(T[0]));
        register_at(&T, exe, "Analisar (teste)").unwrap();
        assert!(is_registered_at(T[0]));
        // Lê de volta os valores gravados com o utilitário do sistema.
        let query = |sub: &str| {
            let out = std::process::Command::new("reg")
                .args(["query", &format!(r"HKCU\{}{sub}", T[0])])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert!(query("").contains("Analisar (teste)"));
        assert!(query(r"\command").contains(r#""C:\Fake Dir\storage-scanner.exe" "%1""#));
        unregister_at(&T).unwrap();
        assert!(!is_registered_at(T[0]));
        unregister_at(&T).unwrap(); // idempotente
    }
}

//! Listagem e identificação de volumes.

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    GetVolumePathNameW,
};
use windows::core::PCWSTR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Unknown,
    Removable,
    Fixed,
    Remote,
    Optical,
    RamDisk,
}

#[derive(Clone, Debug)]
pub struct VolumeInfo {
    /// Raiz com barra final, ex.: `C:\`.
    pub root: String,
    pub label: String,
    /// "NTFS", "exFAT", "FAT32"…; vazio se não legível (mídia ausente).
    pub fs: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
}

impl VolumeInfo {
    pub fn is_ntfs(&self) -> bool {
        self.fs.eq_ignore_ascii_case("NTFS")
    }
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

fn from_wide_nul(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    std::ffi::OsString::from_wide(&buf[..n])
        .to_string_lossy()
        .into_owned()
}

/// Informações de um volume a partir da raiz (`C:\` ou `\\srv\share\`).
pub fn volume_info(root: &str) -> VolumeInfo {
    let w = wide(root);
    let kind = match unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Remote,
        5 => DriveKind::Optical,
        6 => DriveKind::RamDisk,
        _ => DriveKind::Unknown,
    };
    let (mut label, mut fs) = ([0u16; 261], [0u16; 64]);
    let ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(w.as_ptr()),
            Some(&mut label),
            None,
            None,
            None,
            Some(&mut fs),
        )
    }
    .is_ok();
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        let _ = GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free));
    }
    VolumeInfo {
        root: root.to_string(),
        label: if ok {
            from_wide_nul(&label)
        } else {
            String::new()
        },
        fs: if ok {
            from_wide_nul(&fs)
        } else {
            String::new()
        },
        kind,
        total,
        free,
    }
}

/// Todas as unidades lógicas montadas.
pub fn list_volumes() -> Vec<VolumeInfo> {
    let mut buf = vec![0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return Vec::new();
    }
    buf[..n]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| volume_info(&String::from_utf16_lossy(s)))
        .collect()
}

/// Raiz do volume que contém `path` (`C:\`, `\\srv\share\`…).
pub fn volume_root_of(path: &Path) -> Option<String> {
    let abs = std::path::absolute(path).ok()?;
    let w = wide(&abs.to_string_lossy());
    let mut out = vec![0u16; 1024];
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut out) }.ok()?;
    let s = from_wide_nul(&out);
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_system_drive() {
        let vols = list_volumes();
        assert!(!vols.is_empty());
        let sysdrive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let v = vols
            .iter()
            .find(|v| v.root.to_uppercase().starts_with(&sysdrive.to_uppercase()))
            .expect("unidade do sistema");
        assert_eq!(v.kind, DriveKind::Fixed);
        assert!(v.total > 0);
    }

    #[test]
    fn root_of_temp() {
        let r = volume_root_of(&std::env::temp_dir()).unwrap();
        assert!(r.ends_with('\\'));
    }
}

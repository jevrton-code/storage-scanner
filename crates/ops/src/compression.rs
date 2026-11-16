//! Compressão NTFS (`FSCTL_SET_COMPRESSION`), recursiva e cancelável.

use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::FSCTL_SET_COMPRESSION;

const COMPRESSION_FORMAT_NONE: u16 = 0;
const COMPRESSION_FORMAT_DEFAULT: u16 = 1;

#[derive(Default)]
pub struct CompressProgress {
    pub done: AtomicU64,
    pub failed: AtomicU64,
}

/// Liga/desliga a compressão de um arquivo ou do atributo de uma pasta.
pub fn set_compression(path: &Path, enable: bool) -> io::Result<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    let format = if enable {
        COMPRESSION_FORMAT_DEFAULT
    } else {
        COMPRESSION_FORMAT_NONE
    };
    let mut ret = 0u32;
    unsafe {
        DeviceIoControl(
            HANDLE(file.as_raw_handle()),
            FSCTL_SET_COMPRESSION,
            Some((&format as *const u16).cast()),
            size_of::<u16>() as u32,
            None,
            0,
            Some(&mut ret),
            None,
        )
    }
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Aplica a todo o conteúdo de `path`. Arquivos que falham (em uso, sem permissão) são contados
/// em `progress.failed` e não interrompem o processo.
pub fn set_compression_tree(
    path: &Path,
    enable: bool,
    progress: &CompressProgress,
    cancel: &AtomicBool,
) -> Result<(), crate::OpError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::OpError::Cancelled);
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        progress.failed.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    match set_compression(path, enable) {
        Ok(()) => progress.done.fetch_add(1, Ordering::Relaxed),
        Err(_) => progress.failed.fetch_add(1, Ordering::Relaxed),
    };
    if meta.is_dir()
        && let Ok(rd) = std::fs::read_dir(path)
    {
        for e in rd.flatten() {
            set_compression_tree(&e.path(), enable, progress, cancel)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::MetadataExt;

    const COMPRESSED: u32 = 0x800;

    #[test]
    fn toggles_ntfs_compression_attribute() {
        let d = std::env::temp_dir().join("ss_compress_attr_test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("s")).unwrap();
        std::fs::write(d.join("s/a.txt"), "z".repeat(100_000)).unwrap();
        let p = CompressProgress::default();
        let cancel = AtomicBool::new(false);

        if set_compression(&d, true).is_err() {
            // Volume sem suporte a compressão NTFS (ReFS/exFAT): nada a verificar.
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        set_compression_tree(&d, true, &p, &cancel).unwrap();
        let attrs = |f: &Path| std::fs::metadata(f).unwrap().file_attributes();
        assert!(attrs(&d.join("s/a.txt")) & COMPRESSED != 0);
        assert_eq!(p.failed.load(Ordering::Relaxed), 0);

        set_compression_tree(&d, false, &p, &cancel).unwrap();
        assert!(attrs(&d.join("s/a.txt")) & COMPRESSED == 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}

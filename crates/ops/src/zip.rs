//! Compactação em `.zip` em segundo plano, com progresso e cancelamento.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::OpError;

#[derive(Default)]
pub struct ZipProgress {
    pub files_done: AtomicU64,
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    /// Arquivos ignorados por erro de leitura.
    pub skipped: AtomicU64,
}

/// Primeiro nome livre: `base.zip`, `base (2).zip`, …
pub fn unique_zip_path(dir: &Path, base: &str) -> PathBuf {
    let first = dir.join(format!("{base}.zip"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{base} ({n}).zip")))
        .find(|p| !p.exists())
        .expect("sequência infinita")
}

fn collect(path: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf, bool)>, total: &mut u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return; // não segue links/junções
    }
    if meta.is_dir() {
        out.push((path.to_path_buf(), rel.to_path_buf(), true));
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                collect(&e.path(), &rel.join(e.file_name()), out, total);
            }
        }
    } else {
        *total += meta.len();
        out.push((path.to_path_buf(), rel.to_path_buf(), false));
    }
}

fn entry_name(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compacta `sources` em `zip_path` (cada item entra com seu nome na raiz do zip).
/// Em caso de erro ou cancelamento o arquivo parcial é apagado.
pub fn zip_paths(
    sources: &[PathBuf],
    zip_path: &Path,
    progress: &ZipProgress,
    cancel: &AtomicBool,
) -> Result<(), OpError> {
    let mut items = Vec::new();
    let mut total = 0u64;
    for s in sources {
        let name = s.file_name().map(PathBuf::from).unwrap_or_default();
        collect(s, &name, &mut items, &mut total);
    }
    progress.bytes_total.store(total, Ordering::Relaxed);

    let result = write_zip(&items, zip_path, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(zip_path);
    }
    result
}

fn write_zip(
    items: &[(PathBuf, PathBuf, bool)],
    zip_path: &Path,
    progress: &ZipProgress,
    cancel: &AtomicBool,
) -> Result<(), OpError> {
    let file = File::create(zip_path)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    let mut buf = vec![0u8; 256 * 1024];
    for (path, rel, is_dir) in items {
        if cancel.load(Ordering::Relaxed) {
            return Err(OpError::Cancelled);
        }
        let name = entry_name(rel);
        if *is_dir {
            zw.add_directory(format!("{name}/"), opts)
                .map_err(|e| OpError::Windows(e.to_string()))?;
            continue;
        }
        let mut src = match File::open(path) {
            Ok(f) => f,
            Err(_) => {
                progress.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        zw.start_file(name, opts)
            .map_err(|e| OpError::Windows(e.to_string()))?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(OpError::Cancelled);
            }
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => return Err(e.into()),
            };
            zw.write_all(&buf[..n])?;
            progress.bytes_done.fetch_add(n as u64, Ordering::Relaxed);
        }
        progress.files_done.fetch_add(1, Ordering::Relaxed);
    }
    zw.finish().map_err(|e| OpError::Windows(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ss_zip_test_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("pasta/sub")).unwrap();
        std::fs::write(d.join("pasta/a.txt"), "a".repeat(10_000)).unwrap();
        std::fs::write(d.join("pasta/sub/b.txt"), "bb").unwrap();
        std::fs::write(d.join("solto.bin"), vec![7u8; 1234]).unwrap();
        d
    }

    #[test]
    fn zips_folder_and_file_readable_back() {
        let d = fixture("ok");
        let out = unique_zip_path(&d, "saida");
        let p = ZipProgress::default();
        zip_paths(
            &[d.join("pasta"), d.join("solto.bin")],
            &out,
            &p,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(p.files_done.load(Ordering::Relaxed), 3);
        assert_eq!(p.bytes_total.load(Ordering::Relaxed), 10_000 + 2 + 1234);
        assert_eq!(p.bytes_done.load(Ordering::Relaxed), 10_000 + 2 + 1234);

        let mut za = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<String> = (0..za.len())
            .map(|i| za.by_index(i).unwrap().name().unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "pasta/",
                "pasta/a.txt",
                "pasta/sub/",
                "pasta/sub/b.txt",
                "solto.bin"
            ]
        );
        let mut content = String::new();
        za.by_name("pasta/sub/b.txt")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "bb");
        assert!(
            unique_zip_path(&d, "saida")
                .to_string_lossy()
                .ends_with("saida (2).zip")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_removes_partial_zip() {
        let d = fixture("cancel");
        let out = d.join("cancelado.zip");
        let r = zip_paths(
            &[d.join("pasta")],
            &out,
            &ZipProgress::default(),
            &AtomicBool::new(true),
        );
        assert!(matches!(r, Err(OpError::Cancelled)));
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}

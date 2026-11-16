//! Backend MFT: lê a Master File Table do NTFS diretamente (somente leitura, exige elevação).

pub mod attrs;
pub mod build;
pub mod record;
pub mod runlist;
pub mod volume;

#[cfg(test)]
mod fixture;

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::iter::{ParallelBridge, ParallelIterator};
use ss_core::Tree;

pub use build::Labels;
use build::{ChunkOut, build_tree};
use record::{NO_BASE, parse_record};
use volume::{MftError, NtfsVolume, load_mft_map};

use crate::{Progress, ScanError};

/// Bytes lidos por bloco (múltiplo de qualquer tamanho de cluster/registro).
const CHUNK_BYTES: usize = 4 * 1024 * 1024;

impl From<MftError> for ScanError {
    fn from(e: MftError) -> Self {
        ScanError::Mft(e.to_string())
    }
}

/// Varre o volume inteiro (`drive` = `"C:"`) e devolve a árvore com raiz nomeada `root_name`.
pub fn scan_volume(
    drive: &str,
    root_name: &str,
    labels: &Labels,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<Tree, ScanError> {
    let vol = NtfsVolume::open(drive)?;
    let map = load_mft_map(&vol)?;
    let rs = vol.record_size;
    let total_records = (map.valid_len / rs as u64) as usize;
    if total_records == 0 || total_records > u32::MAX as usize {
        return Err(ScanError::Mft("quantidade de registros inválida".into()));
    }
    let chunk_bytes = (CHUNK_BYTES / rs).max(1) * rs;
    let n_chunks = (total_records * rs).div_ceil(chunk_bytes);

    let (tx, rx) = crossbeam_channel::bounded::<(u32, Vec<u8>)>(8);
    let mut chunks: Vec<ChunkOut> = Vec::with_capacity(n_chunks);
    let mut read_err: Option<ScanError> = None;

    std::thread::scope(|s| {
        let reader = s.spawn(|| -> Result<(), ScanError> {
            let end = (total_records * rs) as u64;
            for idx in 0..n_chunks {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ScanError::Cancelled);
                }
                let off = (idx * chunk_bytes) as u64;
                let len = ((end - off) as usize).min(chunk_bytes);
                let mut buf = vec![0u8; len];
                map.read_logical(&vol, off, &mut buf)
                    .map_err(MftError::from)?;
                if tx.send((idx as u32, buf)).is_err() {
                    break;
                }
            }
            Ok(())
        });

        chunks = rx
            .into_iter()
            .par_bridge()
            .map(|(idx, mut buf)| parse_chunk(idx, &mut buf, rs, chunk_bytes, progress))
            .collect();
        read_err = reader.join().ok().and_then(Result::err);
    });
    if let Some(e) = read_err {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    build_tree(chunks, total_records, root_name, labels).map_err(|e| ScanError::Mft(e.into()))
}

fn parse_chunk(
    idx: u32,
    buf: &mut [u8],
    rs: usize,
    chunk_bytes: usize,
    progress: &Progress,
) -> ChunkOut {
    let first = (idx as usize * chunk_bytes / rs) as u32;
    let mut names = Vec::with_capacity(buf.len() / 8);
    let mut records = Vec::with_capacity(buf.len() / rs / 2);
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    for (i, rec) in buf.chunks_exact_mut(rs).enumerate() {
        if let Some(p) = parse_record(first + i as u32, rec, idx, &mut names) {
            if p.base == NO_BASE {
                if p.is_dir {
                    dirs += 1;
                } else {
                    files += 1;
                    bytes += p.size;
                }
            }
            records.push(p);
        }
    }
    use std::sync::atomic::Ordering::Relaxed;
    progress.files.fetch_add(files, Relaxed);
    progress.dirs.fetch_add(dirs, Relaxed);
    progress.bytes.fetch_add(bytes, Relaxed);
    ChunkOut {
        idx,
        names,
        records,
    }
}

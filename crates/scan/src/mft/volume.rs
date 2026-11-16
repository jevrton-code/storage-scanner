//! Acesso de leitura ao volume NTFS bruto e localização dos extents da `$MFT`.

use std::fs::File;
use std::io;
use std::os::windows::fs::FileExt;
use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};

use super::attrs::{self, NonResident, T_ATTRIBUTE_LIST, T_DATA};
use super::record::apply_fixups;
use super::runlist::{self, Run};

#[derive(Debug, thiserror::Error)]
pub enum MftError {
    #[error("não foi possível abrir o volume: {0}")]
    Open(io::Error),
    #[error("falha de leitura do volume: {0}")]
    Read(#[from] io::Error),
    #[error("estrutura da MFT inválida: {0}")]
    Corrupt(&'static str),
}

pub struct NtfsVolume {
    file: File,
    pub bytes_per_cluster: u64,
    pub record_size: usize,
    pub mft_start_lcn: u64,
}

impl NtfsVolume {
    /// `drive` no formato `C:`.
    pub fn open(drive: &str) -> Result<Self, MftError> {
        let file = File::open(format!(r"\\.\{drive}")).map_err(MftError::Open)?;
        let mut data = NTFS_VOLUME_DATA_BUFFER::default();
        let mut ret = 0u32;
        unsafe {
            DeviceIoControl(
                HANDLE(file.as_raw_handle()),
                FSCTL_GET_NTFS_VOLUME_DATA,
                None,
                0,
                Some((&mut data as *mut NTFS_VOLUME_DATA_BUFFER).cast()),
                size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                Some(&mut ret),
                None,
            )
        }
        .map_err(|e| MftError::Read(io::Error::from_raw_os_error(e.code().0 & 0xFFFF)))?;
        if data.BytesPerFileRecordSegment < 512 || data.BytesPerCluster == 0 {
            return Err(MftError::Corrupt("parâmetros do volume"));
        }
        Ok(Self {
            file,
            bytes_per_cluster: u64::from(data.BytesPerCluster),
            record_size: data.BytesPerFileRecordSegment as usize,
            mft_start_lcn: data.MftStartLcn as u64,
        })
    }

    /// Leitura posicional completa (o offset e o tamanho devem ser múltiplos do setor).
    pub fn read_at(&self, mut off: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let n = self.file.seek_read(buf, off)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            off += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

/// Fonte de bytes lógicos da `$MFT` (mapeia offset lógico → extents no disco).
pub struct MftMap {
    /// (início lógico em bytes, início no disco em bytes ou `None` se esparso, comprimento)
    extents: Vec<(u64, Option<u64>, u64)>,
    /// Bytes válidos (inicializados) da MFT.
    pub valid_len: u64,
}

impl MftMap {
    fn from_runs(runs: &[Run], bpc: u64, valid_len: u64) -> Self {
        let mut extents = Vec::with_capacity(runs.len());
        let mut logical = 0u64;
        for r in runs {
            let len = r.len * bpc;
            extents.push((logical, r.lcn.map(|l| l * bpc), len));
            logical += len;
        }
        Self { extents, valid_len }
    }

    /// Lê `buf.len()` bytes a partir do offset lógico `off`, atravessando extents.
    pub fn read_logical(
        &self,
        vol: &NtfsVolume,
        mut off: u64,
        mut buf: &mut [u8],
    ) -> io::Result<()> {
        while !buf.is_empty() {
            let idx = self.extents.partition_point(|e| e.0 + e.2 <= off);
            let Some(&(start, disk, len)) = self.extents.get(idx) else {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "fora da MFT"));
            };
            let into = off - start;
            let n = ((len - into) as usize).min(buf.len());
            match disk {
                Some(d) => vol.read_at(d + into, &mut buf[..n])?,
                None => buf[..n].fill(0),
            }
            off += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

/// `$DATA` padrão (sem nome) de um registro: cabeçalho não residente + runs.
fn data_fragment(rec: &[u8]) -> Option<(NonResident, Vec<Run>)> {
    for a in attrs::attrs(rec) {
        if a.ty == T_DATA && a.name_len == 0 {
            let nr = a.non_resident()?;
            let runs = runlist::decode(a.runlist()?).ok()?;
            return Some((nr, runs));
        }
    }
    None
}

/// Lê o registro 0 (`$MFT`) e monta o mapa dos extents, inclusive quando a MFT é tão
/// fragmentada que usa `$ATTRIBUTE_LIST` e registros de extensão.
pub fn load_mft_map(vol: &NtfsVolume) -> Result<MftMap, MftError> {
    let rs = vol.record_size;
    let mut rec0 = vec![0u8; rs];
    vol.read_at(vol.mft_start_lcn * vol.bytes_per_cluster, &mut rec0)?;
    if !apply_fixups(&mut rec0) {
        return Err(MftError::Corrupt("registro 0 ($MFT)"));
    }
    let (nr0, runs0) =
        data_fragment(&rec0).ok_or(MftError::Corrupt("$MFT sem $DATA não residente"))?;
    let valid = if nr0.initialized != 0 {
        nr0.initialized
    } else {
        nr0.real_size
    };
    let mut fragments: Vec<(u64, Vec<Run>)> = vec![(nr0.start_vcn, runs0)];

    let list = attrs::attrs(&rec0)
        .find(|a| a.ty == T_ATTRIBUTE_LIST)
        .and_then(|a| a.resident_value().map(attrs::parse_attribute_list));
    if let Some(entries) = list {
        let mut wanted: Vec<u64> = entries
            .iter()
            .filter(|e| e.ty == T_DATA && e.name_len == 0 && e.record != 0)
            .map(|e| e.record)
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        // Os registros de extensão ficam nos extents já conhecidos (início da MFT).
        let mut flat: Vec<Run> = fragments[0].1.clone();
        let map = MftMap::from_runs(&flat, vol.bytes_per_cluster, u64::MAX);
        let mut extra = Vec::new();
        for r in wanted {
            let mut buf = vec![0u8; rs];
            map.read_logical(vol, r * rs as u64, &mut buf)?;
            if !apply_fixups(&mut buf) {
                return Err(MftError::Corrupt("registro de extensão da $MFT"));
            }
            if let Some((nr, runs)) = data_fragment(&buf) {
                extra.push((nr.start_vcn, runs));
            }
        }
        fragments.extend(extra);
        fragments.sort_by_key(|f| f.0);
        flat = fragments.into_iter().flat_map(|f| f.1).collect();
        return Ok(MftMap::from_runs(&flat, vol.bytes_per_cluster, valid));
    }
    Ok(MftMap::from_runs(
        &fragments[0].1,
        vol.bytes_per_cluster,
        valid,
    ))
}

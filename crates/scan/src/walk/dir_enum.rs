//! Enumeração de um diretório com poucas chamadas de sistema.
//!
//! Caminho rápido: `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` devolve nome, tamanho,
//! tamanho alocado, data e atributos de vários itens por chamada. Fallback: `FindFirstFileExW`.

use std::ffi::OsString;
use std::io;
use std::mem::offset_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use ss_core::flags;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, GetLastError,
    HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, FILE_ATTRIBUTE_SYSTEM,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FIND_FIRST_EX_LARGE_FETCH, FileIdBothDirectoryInfo,
    FileIdBothDirectoryRestartInfo, FindClose, FindExInfoBasic, FindExSearchNameMatch,
    FindFirstFileExW, FindNextFileW, GetFileInformationByHandleEx, OPEN_EXISTING, WIN32_FIND_DATAW,
};
use windows::core::PCWSTR;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub flags: u16,
    pub size: u64,
    pub allocated: u64,
    /// FILETIME da última gravação.
    pub mtime: i64,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIR != 0
    }
    /// Diretório que deve ser percorrido (não é reparse point).
    pub fn is_traversable(&self) -> bool {
        self.is_dir() && self.flags & flags::REPARSE == 0
    }
}

fn attrs_to_flags(attrs: u32) -> u16 {
    let mut f = 0;
    let map = [
        (FILE_ATTRIBUTE_DIRECTORY.0, flags::DIR),
        (FILE_ATTRIBUTE_REPARSE_POINT.0, flags::REPARSE),
        (FILE_ATTRIBUTE_COMPRESSED.0, flags::COMPRESSED),
        (FILE_ATTRIBUTE_SPARSE_FILE.0, flags::SPARSE),
        (FILE_ATTRIBUTE_HIDDEN.0, flags::HIDDEN),
        (FILE_ATTRIBUTE_SYSTEM.0, flags::SYSTEM),
    ];
    for (a, fl) in map {
        if attrs & a != 0 {
            f |= fl;
        }
    }
    f
}

/// Converte um caminho em wide string com prefixo `\\?\` (caminhos longos) e NUL final.
pub fn to_wide_extended(path: &Path) -> Vec<u16> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.as_os_str();
    let lossy = s.to_string_lossy();
    let prefixed: OsString = if lossy.starts_with(r"\\?\") {
        s.to_os_string()
    } else if let Some(rest) = lossy.strip_prefix(r"\\") {
        OsString::from(format!(r"\\?\UNC\{rest}"))
    } else {
        OsString::from(format!(r"\\?\{lossy}"))
    };
    let mut w: Vec<u16> = prefixed.encode_wide().collect();
    w.push(0);
    w
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() }.0 as i32)
}

/// Lê todas as entradas (sem `.` e `..`) de `dir`, acrescentando-as a `out`.
pub fn read_dir(dir: &Path, out: &mut Vec<Entry>, buf: &mut Vec<u64>) -> io::Result<()> {
    let wide = to_wide_extended(dir);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = match handle {
        Ok(h) => OwnedHandle(h),
        Err(e) => return Err(io::Error::from_raw_os_error(e.code().0 & 0xFFFF)),
    };
    match read_dir_handle(&handle, out, buf) {
        Err(e) if is_unsupported(&e) => {
            drop(handle);
            read_dir_find(&wide, out)
        }
        r => r,
    }
}

fn is_unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error().map(|c| c as u32),
        Some(c) if c == ERROR_INVALID_PARAMETER.0 || c == ERROR_NOT_SUPPORTED.0 || c == ERROR_INVALID_LEVEL.0
    )
}

fn read_dir_handle(h: &OwnedHandle, out: &mut Vec<Entry>, buf: &mut Vec<u64>) -> io::Result<()> {
    const BUF_BYTES: usize = 128 * 1024;
    if buf.len() * 8 < BUF_BYTES {
        buf.resize(BUF_BYTES / 8, 0);
    }
    let mut first = true;
    loop {
        let class = if first {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        first = false;
        let res = unsafe {
            GetFileInformationByHandleEx(h.0, class, buf.as_mut_ptr().cast(), BUF_BYTES as u32)
        };
        if res.is_err() {
            let err = last_error();
            // ERROR_NO_MORE_FILES (18) = fim da listagem.
            if err.raw_os_error() == Some(18) {
                return Ok(());
            }
            return Err(err);
        }
        let base = buf.as_ptr().cast::<u8>();
        let mut off = 0usize;
        loop {
            // SAFETY: o kernel preenche entradas válidas e alinhadas a 8 bytes dentro do buffer.
            let info = unsafe { &*(base.add(off).cast::<FILE_ID_BOTH_DIR_INFO>()) };
            let name_len = info.FileNameLength as usize / 2;
            let name_ptr = unsafe { base.add(off + offset_of!(FILE_ID_BOTH_DIR_INFO, FileName)) }
                .cast::<u16>();
            let name_u16 = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
            let is_dots = matches!(name_u16, [0x2E] | [0x2E, 0x2E]);
            if !is_dots {
                let name = OsString::from_wide(name_u16).to_string_lossy().into_owned();
                let attrs = info.FileAttributes;
                let f = attrs_to_flags(attrs);
                let is_dir = f & flags::DIR != 0;
                out.push(Entry {
                    name,
                    flags: f,
                    size: if is_dir {
                        0
                    } else {
                        info.EndOfFile.max(0) as u64
                    },
                    allocated: if is_dir {
                        0
                    } else {
                        info.AllocationSize.max(0) as u64
                    },
                    mtime: info.LastWriteTime,
                });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            off += info.NextEntryOffset as usize;
        }
    }
}

/// Fallback para sistemas de arquivos/servidores sem `FileIdBothDirectoryInfo`.
/// O tamanho alocado é estimado (arredondado para 4 KB), pois essa API não o informa.
fn read_dir_find(wide_dir: &[u16], out: &mut Vec<Entry>) -> io::Result<()> {
    let mut pattern: Vec<u16> = wide_dir[..wide_dir.len() - 1].to_vec();
    if pattern.last() != Some(&(b'\\' as u16)) {
        pattern.push(b'\\' as u16);
    }
    pattern.push(b'*' as u16);
    pattern.push(0);
    let mut data = WIN32_FIND_DATAW::default();
    let h = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let h = h.map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    loop {
        let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(0);
        let nm = &data.cFileName[..len];
        if !matches!(nm, [0x2E] | [0x2E, 0x2E]) {
            let f = attrs_to_flags(data.dwFileAttributes);
            let is_dir = f & flags::DIR != 0;
            let size = (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow);
            let mtime = (i64::from(data.ftLastWriteTime.dwHighDateTime) << 32)
                | i64::from(data.ftLastWriteTime.dwLowDateTime);
            out.push(Entry {
                name: OsString::from_wide(nm).to_string_lossy().into_owned(),
                flags: f,
                size: if is_dir { 0 } else { size },
                allocated: if is_dir {
                    0
                } else {
                    size.div_ceil(4096) * 4096
                },
                mtime,
            });
        }
        if unsafe { FindNextFileW(h, &mut data) }.is_err() {
            break;
        }
    }
    unsafe {
        let _ = FindClose(h);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_paths() {
        let w = |p: &str| {
            let v = to_wide_extended(Path::new(p));
            String::from_utf16_lossy(&v[..v.len() - 1])
        };
        assert_eq!(w(r"C:\Users"), r"\\?\C:\Users");
        assert_eq!(w(r"\\srv\share\x"), r"\\?\UNC\srv\share\x");
        assert_eq!(w(r"\\?\C:\a"), r"\\?\C:\a");
    }

    #[test]
    fn reads_temp_dir() {
        let dir = std::env::temp_dir().join("ss_dir_enum_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), vec![0u8; 5000]).unwrap();
        let mut out = Vec::new();
        read_dir(&dir, &mut out, &mut Vec::new()).unwrap();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "a.txt");
        assert_eq!(out[0].size, 5000);
        assert!(out[0].allocated >= 5000);
        assert!(out[1].is_dir());
        // O fallback produz os mesmos nomes e tamanhos.
        let mut f = Vec::new();
        read_dir_find(&to_wide_extended(&dir), &mut f).unwrap();
        f.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].size, 5000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Excluir (Lixeira/permanente), mover e copiar com `IFileOperation`
//! (diálogos nativos de progresso/conflito e desfazer).

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NO_UI, FOF_NOCONFIRMATION, FOFX_RECYCLEONDELETE, FileOperation,
    IFileOperation, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::PCWSTR;

use crate::protect::is_protected;
use crate::{ComGuard, OpError, wide};

#[derive(Clone, Debug)]
pub enum Op {
    /// `permanent = false` envia para a Lixeira (com desfazer).
    Delete {
        permanent: bool,
    },
    Move {
        dest_dir: PathBuf,
    },
    Copy {
        dest_dir: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Janela dona dos diálogos (HWND como inteiro).
    pub owner: Option<isize>,
    /// Sem nenhuma interface nem confirmação (usado em testes e automações).
    pub silent: bool,
}

fn shell_item(path: &Path) -> Result<IShellItem, OpError> {
    let w = wide(path.as_os_str());
    Ok(unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None)? })
}

/// Executa `op` sobre `paths`. Bloqueia até terminar (chame de uma thread de fundo).
/// Itens protegidos são recusados antes de qualquer alteração.
pub fn run(op: &Op, paths: &[PathBuf], opts: Options) -> Result<(), OpError> {
    let destructive = !matches!(op, Op::Copy { .. });
    if destructive && let Some(p) = paths.iter().find(|p| is_protected(p)) {
        return Err(OpError::Protected(p.display().to_string()));
    }
    let _com = ComGuard::new();
    let fo: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
    unsafe {
        if let Some(h) = opts.owner {
            fo.SetOwnerWindow(HWND(h as *mut _))?;
        }
        let mut flags = match op {
            Op::Delete { permanent: false } => FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE,
            _ => Default::default(),
        };
        if opts.silent {
            flags |= FOF_NO_UI | FOF_NOCONFIRMATION;
        }
        fo.SetOperationFlags(flags)?;
    }
    let dest = match op {
        Op::Move { dest_dir } | Op::Copy { dest_dir } => Some(shell_item(dest_dir)?),
        Op::Delete { .. } => None,
    };
    for p in paths {
        let item = shell_item(p)?;
        unsafe {
            match (&dest, op) {
                (_, Op::Delete { .. }) => fo.DeleteItem(&item, None)?,
                (Some(d), Op::Move { .. }) => fo.MoveItem(&item, d, PCWSTR::null(), None)?,
                (Some(d), Op::Copy { .. }) => fo.CopyItem(&item, d, PCWSTR::null(), None)?,
                _ => unreachable!("destino sempre existe para mover/copiar"),
            }
        }
    }
    unsafe {
        fo.PerformOperations()?;
        if fo.GetAnyOperationsAborted()?.as_bool() {
            return Err(OpError::Cancelled);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ss_ops_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const SILENT: Options = Options {
        owner: None,
        silent: true,
    };

    #[test]
    fn copy_move_and_permanent_delete() {
        let d = scratch("cmd");
        let (src, dst) = (d.join("src"), d.join("dst"));
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"world").unwrap();

        run(
            &Op::Copy {
                dest_dir: dst.clone(),
            },
            std::slice::from_ref(&src),
            SILENT,
        )
        .unwrap();
        assert!(dst.join("src/sub/b.txt").exists() && src.join("a.txt").exists());

        let moved = d.join("moved");
        std::fs::create_dir_all(&moved).unwrap();
        run(
            &Op::Move {
                dest_dir: moved.clone(),
            },
            &[dst.join("src")],
            SILENT,
        )
        .unwrap();
        assert!(!dst.join("src").exists());
        assert_eq!(std::fs::read(moved.join("src/a.txt")).unwrap(), b"hello");

        run(
            &Op::Delete { permanent: true },
            &[moved.join("src")],
            SILENT,
        )
        .unwrap();
        assert!(!moved.join("src").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Quantidade de itens na Lixeira (todas as unidades).
    fn recycle_bin_items() -> i64 {
        use windows::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
        let mut info = SHQUERYRBINFO {
            cbSize: size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info).unwrap() };
        info.i64NumItems
    }

    #[test]
    fn delete_to_recycle_bin_removes_original() {
        let d = scratch("recycle");
        let f = d.join("para_lixeira.txt");
        std::fs::write(&f, b"x").unwrap();
        let before = recycle_bin_items();
        run(
            &Op::Delete { permanent: false },
            std::slice::from_ref(&f),
            SILENT,
        )
        .unwrap();
        assert!(!f.exists());
        assert_eq!(
            recycle_bin_items(),
            before + 1,
            "o item deve estar na Lixeira"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_protected_items() {
        let r = run(
            &Op::Delete { permanent: true },
            &[PathBuf::from(r"C:\Windows")],
            SILENT,
        );
        assert!(matches!(r, Err(OpError::Protected(_))));
        let r = run(
            &Op::Move {
                dest_dir: std::env::temp_dir(),
            },
            &[PathBuf::from(r"C:\")],
            SILENT,
        );
        assert!(matches!(r, Err(OpError::Protected(_))));
    }
}

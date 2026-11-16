//! Abrir arquivos/pastas e revelar no Explorer.
//!
//! Se o app estiver elevado, o programa aberto herdaria privilégio de administrador. Para evitar
//! isso, a abertura é delegada ao shell do Explorer (`IShellDispatch2::ShellExecute` obtido da
//! janela da área de trabalho), que roda sem elevação.

use std::path::Path;

use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree, IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellWindows,
    SHOpenFolderAndSelectItems, SHParseDisplayName, SID_STopLevelBrowser, SVGIO_BACKGROUND,
    SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellExecuteW, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{BSTR, PCWSTR, w};

use crate::{ComGuard, OpError, wide};

/// Abre `path` com o programa padrão. `elevated`: o processo atual é administrador.
pub fn open(path: &Path, elevated: bool) -> Result<(), OpError> {
    if elevated {
        match open_via_explorer(path) {
            Ok(()) => return Ok(()),
            Err(e) => tracing::warn!("abertura via Explorer falhou ({e}); usando ShellExecute"),
        }
    }
    shell_execute(path)
}

fn shell_execute(path: &Path) -> Result<(), OpError> {
    let _com = ComGuard::new();
    let file = wide(path.as_os_str());
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(OpError::Windows(format!(
            "ShellExecute falhou ({})",
            r.0 as isize
        )))
    }
}

/// Executa a abertura dentro do Explorer (sem herdar a elevação deste processo).
pub fn open_via_explorer(path: &Path) -> Result<(), OpError> {
    let _com = ComGuard::new();
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let mut hwnd = 0i32;
        let loc = VARIANT::from(0i32); // CSIDL_DESKTOP
        let root = VARIANT::default();
        let disp: IDispatch =
            windows.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
        let sp: IServiceProvider = windows::core::Interface::cast(&disp)?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
        let view = browser.QueryActiveShellView()?;
        let bg: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
        let fv: IShellFolderViewDual = windows::core::Interface::cast(&bg)?;
        let app: IDispatch = fv.Application()?;
        let shell: IShellDispatch2 = windows::core::Interface::cast(&app)?;
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        shell.ShellExecute(
            &BSTR::from(path.to_string_lossy().as_ref()),
            &VARIANT::default(),
            &VARIANT::from(BSTR::from(dir.as_str())),
            &VARIANT::from(BSTR::from("open")),
            &VARIANT::from(SW_SHOWNORMAL.0),
        )?;
    }
    Ok(())
}

/// Abre o Explorer com o item selecionado.
pub fn reveal(path: &Path) -> Result<(), OpError> {
    let _com = ComGuard::new();
    let w_path = wide(path.as_os_str());
    unsafe {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(w_path.as_ptr()), None, &mut pidl, 0, None)?;
        let r = SHOpenFolderAndSelectItems(pidl, None, 0);
        CoTaskMemFree(Some(pidl as *const _));
        r?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre uma janela (Bloco de Notas): execute manualmente com `--ignored`.
    #[test]
    #[ignore = "abre um programa na tela"]
    fn open_txt_via_explorer() {
        let f = std::env::temp_dir().join("ss_open_test.txt");
        std::fs::write(&f, "teste").unwrap();
        open_via_explorer(&f).unwrap();
    }
}

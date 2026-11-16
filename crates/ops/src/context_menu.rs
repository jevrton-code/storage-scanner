//! Menu de contexto nativo do shell ("Mais opções do Windows").

use std::path::Path;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_NORMAL, CMINVOKECOMMANDINFO, IContextMenu, IShellFolder, SHBindToParent, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use windows::core::{PCSTR, PCWSTR};

use crate::{ComGuard, OpError, wide};

const ID_FIRST: u32 = 1;
const ID_LAST: u32 = 0x7FFF;

/// Mostra o menu nativo para `path` na posição de tela `(x, y)` e executa o item escolhido.
/// Deve ser chamado da thread da interface (usa o HWND da janela como dono do menu).
pub fn show(owner: isize, path: &Path, x: i32, y: i32) -> Result<(), OpError> {
    let _com = ComGuard::new();
    let hwnd = HWND(owner as *mut _);
    let w_path = wide(path.as_os_str());
    unsafe {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(w_path.as_ptr()), None, &mut pidl, 0, None)?;
        let result = show_for_pidl(hwnd, pidl, x, y);
        CoTaskMemFree(Some(pidl as *const _));
        result
    }
}

unsafe fn show_for_pidl(hwnd: HWND, pidl: *mut ITEMIDLIST, x: i32, y: i32) -> Result<(), OpError> {
    let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder: IShellFolder = unsafe { SHBindToParent(pidl, Some(&mut child))? };
    let menu: IContextMenu =
        unsafe { folder.GetUIObjectOf(hwnd, &[child as *const ITEMIDLIST], None)? };

    let hmenu = unsafe { CreatePopupMenu()? };
    let result = (|| -> Result<(), OpError> {
        unsafe {
            menu.QueryContextMenu(hmenu, 0, ID_FIRST, ID_LAST, CMF_NORMAL)
                .ok()?;
            let cmd =
                TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, hwnd, None);
            let id = cmd.0 as u32;
            if id == 0 {
                return Ok(()); // cancelado
            }
            let info = CMINVOKECOMMANDINFO {
                cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd,
                // MAKEINTRESOURCE(id - ID_FIRST): o "verbo" é um índice.
                lpVerb: PCSTR((id - ID_FIRST) as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            menu.InvokeCommand(&info)?;
        }
        Ok(())
    })();
    unsafe {
        let _ = DestroyMenu(hmenu);
    }
    result
}

/// Posição atual do cursor em coordenadas de tela (pixels físicos).
pub fn cursor_screen_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

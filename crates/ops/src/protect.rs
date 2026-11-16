//! Itens cujas ações destrutivas são bloqueadas.

use std::path::{Component, Path};

/// Diretório do Windows (`%SystemRoot%`), em minúsculas, sem barra final.
fn windows_dir() -> String {
    std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".into())
        .trim_end_matches('\\')
        .to_lowercase()
}

/// `true` se excluir/mover o item pode danificar o sistema: raiz de volume, tudo dentro do
/// diretório do Windows, `$Recycle.Bin`, `System Volume Information` e metarquivos NTFS (`$…`
/// na raiz).
pub fn is_protected(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    let lower = lower.trim_end_matches('\\');
    let win = windows_dir();
    if lower == win || lower.starts_with(&format!("{win}\\")) {
        return true;
    }
    let mut comps = path.components();
    let has_prefix = matches!(comps.next(), Some(Component::Prefix(_)));
    let rest: Vec<_> = comps
        .filter(|c| !matches!(c, Component::RootDir))
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if has_prefix && rest.is_empty() {
        return true; // raiz do volume
    }
    match rest.first() {
        Some(first) => first.starts_with('$') || first == "system volume information",
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_system_locations() {
        assert!(is_protected(Path::new(r"C:\")));
        assert!(is_protected(Path::new(r"C:\Windows")));
        assert!(is_protected(Path::new(r"C:\Windows\System32\kernel32.dll")));
        assert!(is_protected(Path::new(r"D:\$Recycle.Bin\S-1-5\x")));
        assert!(is_protected(Path::new(r"C:\System Volume Information")));
        assert!(is_protected(Path::new(r"C:\$MFT")));
    }

    #[test]
    fn allows_user_data() {
        assert!(!is_protected(Path::new(r"C:\Users\Fulano\Downloads\a.iso")));
        assert!(!is_protected(Path::new(r"D:\Projetos")));
        assert!(!is_protected(Path::new(r"C:\Windows.old\x"))); // prefixo parecido, não é o Windows
    }
}

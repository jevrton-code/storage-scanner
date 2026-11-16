//! Configurações persistentes em `%APPDATA%\StorageScanner\config.toml`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ss_core::Metric;

const MAX_RECENT: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
            Self::System => egui::ThemePreference::System,
        }
    }
}

/// Colunas opcionais da árvore (o nome é sempre exibido).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Columns {
    pub percent: bool,
    pub size: bool,
    pub allocated: bool,
    pub files: bool,
    pub dirs: bool,
    pub modified: bool,
    pub compression: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            percent: true,
            size: true,
            allocated: true,
            files: true,
            dirs: true,
            modified: true,
            compression: true,
        }
    }
}

impl Columns {
    /// Visibilidade na ordem das colunas (nome, %, tamanho, alocado, arquivos, pastas,
    /// modificado, compressão).
    pub fn as_array(&self) -> [bool; 8] {
        [
            true,
            self.percent,
            self.size,
            self.allocated,
            self.files,
            self.dirs,
            self.modified,
            self.compression,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeChoice,
    /// Métrica padrão: size | allocated | files | modified | compression.
    pub metric: String,
    pub columns: Columns,
    /// Último alvo analisado.
    pub last_path: String,
    /// Alvos recentes (mais novo primeiro).
    pub recent_paths: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            metric: "size".into(),
            columns: Columns::default(),
            last_path: String::new(),
            recent_paths: Vec::new(),
        }
    }
}

pub fn metric_from_str(s: &str) -> Metric {
    match s {
        "allocated" => Metric::Allocated,
        "files" => Metric::FileCount,
        "modified" => Metric::Modified,
        "compression" => Metric::Compression,
        _ => Metric::Size,
    }
}

pub fn metric_to_str(m: Metric) -> &'static str {
    match m {
        Metric::Size => "size",
        Metric::Allocated => "allocated",
        Metric::FileCount => "files",
        Metric::Modified => "modified",
        Metric::Compression => "compression",
    }
}

impl Config {
    pub fn metric(&self) -> Metric {
        metric_from_str(&self.metric)
    }

    /// Registra `path` como o alvo mais recente.
    pub fn remember(&mut self, path: &str) {
        self.last_path = path.to_string();
        self.recent_paths.retain(|p| !p.eq_ignore_ascii_case(path));
        self.recent_paths.insert(0, path.to_string());
        self.recent_paths.truncate(MAX_RECENT);
    }

    /// Caminho padrão do arquivo de configuração.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|a| PathBuf::from(a).join("StorageScanner").join("config.toml"))
    }

    /// Lê a configuração. Arquivo ausente = padrões; arquivo inválido é preservado como
    /// `config.toml.bad` e os padrões são usados (o `Option` traz o aviso).
    pub fn load_from(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => return (Self::default(), Some(e.to_string())),
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => (c, None),
            Err(e) => {
                let bad = path.with_extension("toml.bad");
                let _ = std::fs::rename(path, &bad);
                (
                    Self::default(),
                    Some(format!("{e} (cópia em {})", bad.display())),
                )
            }
        }
    }

    pub fn load() -> (Self, Option<String>) {
        match Self::default_path() {
            Some(p) => Self::load_from(&p),
            None => (Self::default(), None),
        }
    }

    /// Grava de forma atômica (arquivo temporário + renomeação).
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    pub fn save(&self) -> io::Result<()> {
        match Self::default_path() {
            Some(p) => self.save_to(&p),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ss_cfg_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        d.join("config.toml")
    }

    #[test]
    fn roundtrip_and_defaults() {
        let p = tmp("rt");
        let mut c = Config {
            theme: ThemeChoice::Light,
            metric: "compression".into(),
            ..Config::default()
        };
        c.columns.dirs = false;
        c.remember(r"C:\Users");
        c.save_to(&p).unwrap();
        let (loaded, warn) = Config::load_from(&p);
        assert!(warn.is_none());
        assert_eq!(loaded, c);
        assert_eq!(loaded.metric(), Metric::Compression);
        // Campos ausentes recebem o padrão.
        std::fs::write(&p, "theme = \"system\"\n").unwrap();
        let (partial, _) = Config::load_from(&p);
        assert_eq!(partial.theme, ThemeChoice::System);
        assert_eq!(partial.columns, Columns::default());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn missing_file_gives_defaults_and_bad_file_is_preserved() {
        let p = tmp("bad");
        let (c, w) = Config::load_from(&p);
        assert_eq!((c, w), (Config::default(), None));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "theme = [isto não é toml").unwrap();
        let (c, w) = Config::load_from(&p);
        assert_eq!(c, Config::default());
        assert!(w.is_some());
        assert!(p.with_extension("toml.bad").exists());
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn recent_paths_are_unique_and_bounded() {
        let mut c = Config::default();
        for i in 0..20 {
            c.remember(&format!(r"C:\p{i}"));
        }
        c.remember(r"c:\P19"); // mesma entrada, outra caixa
        assert_eq!(c.recent_paths.len(), MAX_RECENT);
        assert_eq!(c.recent_paths[0], r"c:\P19");
        assert_eq!(
            c.recent_paths
                .iter()
                .filter(|p| p.eq_ignore_ascii_case(r"C:\p19"))
                .count(),
            1
        );
    }

    #[test]
    fn metric_names_roundtrip() {
        for m in Metric::ALL {
            assert_eq!(metric_from_str(metric_to_str(m)), m);
        }
        assert_eq!(metric_from_str("desconhecida"), Metric::Size);
    }
}

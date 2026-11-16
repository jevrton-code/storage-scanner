//! Exportação da árvore para CSV (separador `;`, UTF-8 com BOM: abre direto no Excel pt-BR).

use std::io::{self, Write};

use crate::format::format_filetime_iso;
use crate::metrics::compression_ratio;
use crate::sort::{SortKey, sort_children_by};
use crate::tree::{Tree, flags};

/// Textos de cabeçalho e de tipo (fornecidos pela camada de interface/i18n).
pub struct CsvLabels {
    pub headers: [String; 8],
    pub dir: String,
    pub file: String,
}

fn field(s: &str, out: &mut Vec<u8>) {
    if s.contains([';', '"', '\n', '\r']) {
        out.push(b'"');
        out.extend_from_slice(s.replace('"', "\"\"").as_bytes());
        out.push(b'"');
    } else {
        out.extend_from_slice(s.as_bytes());
    }
}

/// Escreve todos os itens alcançáveis (pastas antes dos filhos, do maior para o menor).
/// Devolve o número de linhas de dados.
pub fn write_csv<W: Write>(tree: &Tree, mut w: W, labels: &CsvLabels) -> io::Result<u64> {
    w.write_all(b"\xEF\xBB\xBF")?;
    let mut line = Vec::with_capacity(256);
    for (i, h) in labels.headers.iter().enumerate() {
        if i > 0 {
            line.push(b';');
        }
        field(h, &mut line);
    }
    line.extend_from_slice(b"\r\n");
    w.write_all(&line)?;

    let mut rows = 0u64;
    if tree.is_empty() {
        return Ok(rows);
    }
    let mut stack = vec![Tree::ROOT];
    while let Some(id) = stack.pop() {
        let n = tree.node(id);
        line.clear();
        field(&tree.path(id), &mut line);
        line.push(b';');
        field(
            if n.is_dir() {
                &labels.dir
            } else {
                &labels.file
            },
            &mut line,
        );
        let ratio = format!("{:.1}", compression_ratio(n) * 100.0).replace('.', ",");
        let tail = format!(
            ";{};{};{};{};{};{}\r\n",
            n.size,
            n.allocated,
            if n.is_dir() {
                n.files.to_string()
            } else {
                String::new()
            },
            if n.is_dir() {
                n.dirs.to_string()
            } else {
                String::new()
            },
            format_filetime_iso(n.mtime),
            ratio,
        );
        line.extend_from_slice(tail.as_bytes());
        w.write_all(&line)?;
        rows += 1;
        if n.is_dir() && !n.has(flags::REMOVED) {
            let kids = sort_children_by(tree, id, SortKey::Size, true);
            stack.extend(kids.into_iter().rev());
        }
    }
    w.flush()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{NONE, NodeInfo, TreeBuilder};

    fn labels() -> CsvLabels {
        CsvLabels {
            headers: [
                "Caminho",
                "Tipo",
                "Tamanho",
                "Alocado",
                "Arquivos",
                "Pastas",
                "Modificado",
                "Compressão",
            ]
            .map(String::from),
            dir: "Pasta".into(),
            file: "Arquivo".into(),
        }
    }

    #[test]
    fn writes_bom_header_rows_and_escapes() {
        let mut b = TreeBuilder::new();
        let d = NodeInfo {
            flags: flags::DIR,
            ..Default::default()
        };
        let root = b.push(NONE, r"C:\dados", d);
        b.push(
            root,
            "a;b \"x\".txt",
            NodeInfo {
                size: 1000,
                allocated: 250,
                mtime: (1_704_164_640 + 11_644_473_600) * 10_000_000,
                ..Default::default()
            },
        );
        b.push(
            root,
            "grande.bin",
            NodeInfo {
                size: 5000,
                allocated: 5000,
                ..Default::default()
            },
        );
        let t = b.finish();
        let mut out = Vec::new();
        let rows = write_csv(&t, &mut out, &labels()).unwrap();
        assert_eq!(rows, 3);
        assert_eq!(&out[..3], b"\xEF\xBB\xBF");
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "Caminho;Tipo;Tamanho;Alocado;Arquivos;Pastas;Modificado;Compressão"
        );
        assert_eq!(
            lines[1],
            r"C:\dados;Pasta;6000;5250;2;0;2024-01-02 03:04:00;12,5"
        );
        // Maior primeiro; nome com ';' e aspas entre aspas duplicadas.
        assert!(lines[2].starts_with(r"C:\dados\grande.bin;Arquivo;5000;5000;;;;0,0"));
        assert_eq!(
            lines[3],
            "\"C:\\dados\\a;b \"\"x\"\".txt\";Arquivo;1000;250;;;2024-01-02 03:04:00;75,0"
        );
    }
}
